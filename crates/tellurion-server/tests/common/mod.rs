//! Shared plumbing for the real-binary integration tests in this crate:
//! spawning the compiled `tellurion` binary, waiting for it to report its
//! listening address, and driving it over plain HTTP. Every `*_binary.rs`
//! test in this directory used to keep its own copy of these helpers,
//! including a startup wait that slept through one fixed wall-clock budget
//! instead of polling and never noticed the child exiting early — a defect
//! that had to be fixed by hand in each copy before this module existed.
//!
//! `tests/common/mod.rs` is a plain module, not its own test target: cargo
//! only auto-discovers `tests/*.rs` files directly under `tests/`, so this
//! file is compiled into whichever test binary declares `mod common;`,
//! never run as a test crate on its own.
//!
//! Each `*_binary.rs` file that pulls this module in uses a different
//! subset of it (a raster-tiles test never calls [`http_write_request`], a
//! read-only fixture never reads [`HttpResponse::location`]) — since this
//! module is compiled fresh into every one of those separate test-binary
//! crates, an item unused by a given binary would otherwise be a per-binary
//! dead-code warning despite being live in others, so the allow is on the
//! whole module rather than sprinkled per item.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Distinguishes two temp paths requested from different test threads in the
/// same process. A timestamp alone does NOT: `SystemTime::now()` is not
/// guaranteed to advance between two reads, and on macOS two threads
/// starting together routinely observe the same `as_nanos()` value, so
/// `{pid}-{nanos}` collides in practice rather than in theory.
static TEMP_PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Wraps a real-binary fixture in the explicitly selected legacy file mode.
/// Dynamic-store behavior has its own focused integration tests; driver
/// fixtures continue to exercise the established file/reload path.
pub fn legacy_config(yaml: &str) -> String {
    format!("control_store:\n  backend: legacy_file\n{yaml}")
}

/// A temp path unique across every thread of this test binary and every
/// concurrently running test binary.
///
/// The `{pid}-{nanos}` idiom this replaces produced a genuinely confusing
/// failure: two `#[test]` functions in one binary built the SAME fixture
/// directory, then the first to finish ran its `Drop` and recursively
/// deleted the directory the other test's server was still reading chunks
/// out of. The reader did what it is supposed to do with an absent chunk —
/// substituted the array's `fill_value` — so the symptom was not an I/O
/// error but a rendered tile whose pixels were quietly, plausibly wrong.
/// It reproduced about one run in three, and only ever under parallel
/// execution: `--test-threads=1` was always green, which is precisely what
/// makes this class of bug expensive to chase.
///
/// The process id keeps separate test binaries apart, the counter keeps
/// threads within one binary apart, and the timestamp is kept only so the
/// leftovers of a killed run stay humanly identifiable.
pub fn unique_temp_path(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = TEMP_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("{prefix}-{}-{nanos}-{seq}", std::process::id()));
    path
}

/// How often [`wait_for_listening_addr`] checks the channel and the child's
/// exit status. Small enough that a crashed or exited binary is reported in
/// a fraction of a second, never after waiting out the full startup
/// ceiling.
pub const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Startup readiness is an event to wait for, not a duration to guess: how
/// long the real binary takes to log its listening address depends on how
/// contended the host machine is (a concurrent `cargo build`, other tests
/// running in parallel) as much as on the binary itself.
/// [`wait_for_listening_addr`] polls for the event instead of sleeping
/// through one fixed wall-clock budget, and gives up immediately if the
/// child exits, so this ceiling only gets consumed in full by a process
/// that neither logs its address nor exits — the one case actually worth
/// waiting out. Override with `TELLURION_TEST_STARTUP_TIMEOUT_SECS` for a
/// slower CI runner or a heavily loaded machine.
pub fn startup_timeout() -> Duration {
    env_duration_secs("TELLURION_TEST_STARTUP_TIMEOUT_SECS").unwrap_or(Duration::from_secs(60))
}

/// How long a single HTTP request against the already-running server may
/// take before the raw socket read gives up. The server is already up by
/// the time this applies, but request handling is still subject to the
/// same host contention as everything else in this module. Override with
/// `TELLURION_TEST_REQUEST_TIMEOUT_SECS`.
pub fn request_timeout() -> Duration {
    env_duration_secs("TELLURION_TEST_REQUEST_TIMEOUT_SECS").unwrap_or(Duration::from_secs(5))
}

pub fn env_duration_secs(var: &str) -> Option<Duration> {
    std::env::var(var)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Kills the spawned server on drop so a failing assertion (panic mid-test)
/// can never leak a listening process past this test.
pub struct ServerProcess {
    pub child: Child,
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
}

/// Pulls the `addr` field out of the binary's JSON-formatted startup log
/// line (every config these tests write sets `log_json: true` so this is a
/// single parseable object rather than the plain-text format, which
/// interleaves ANSI color codes between a field's name and its value).
pub fn parse_listening_addr(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value["fields"]["message"].as_str()? != "tellurion listening" {
        return None;
    }
    value["fields"]["addr"].as_str().map(str::to_string)
}

/// Reads the child's stderr on a background thread, echoing each line to
/// this process's own stderr — matching what `Stdio::inherit()` used to
/// give for free — while also keeping a copy so a startup failure can
/// report what the binary actually printed, rather than just that the wait
/// gave up.
pub fn spawn_stderr_relay(stderr: ChildStderr) -> Arc<Mutex<Vec<String>>> {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_for_thread = Arc::clone(&captured);
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            eprintln!("{line}");
            captured_for_thread
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(line);
        }
    });
    captured
}

/// How long [`wait_for_exit_status`] keeps polling `try_wait` for a child
/// whose stdout has just closed before giving up and reporting it as still
/// running. Stdout closing and the child being reaped are not ordered
/// events — the descriptor going away says nothing about whether the OS has
/// already collected the exit status — so by the time this is called the
/// child has very often already exited, it just hasn't been waited on yet.
/// A child that is genuinely still running after this ceiling is worth
/// reporting as such, not waited on indefinitely.
const EXIT_STATUS_POLL_CEILING: Duration = Duration::from_secs(2);

/// Polls `child.try_wait()` on [`STARTUP_POLL_INTERVAL`] up to
/// [`EXIT_STATUS_POLL_CEILING`], returning `None` only once that ceiling
/// elapses with the child still unreaped. Replaces a single immediate
/// `try_wait()` call, which — called right after the child's stdout hits
/// EOF — frequently misses a child that has, in fact, already exited: the
/// two events are not ordered, so a single snapshot catches the process
/// mid-way between exiting and being reaped roughly one time in three.
fn wait_for_exit_status(child: &mut Child) -> Option<ExitStatus> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().ok().flatten() {
            return Some(status);
        }
        if started.elapsed() >= EXIT_STATUS_POLL_CEILING {
            return None;
        }
        std::thread::sleep(STARTUP_POLL_INTERVAL);
    }
}

/// Formats a startup-wait failure with everything needed to diagnose it
/// without a reproduction: how long the wait ran, the child's exit status
/// if it has one, and the last lines the binary actually wrote to stdout
/// and stderr before the wait gave up.
fn startup_failure_message(
    reason: &str,
    elapsed: Duration,
    status: Option<ExitStatus>,
    stdout_seen: &[String],
    stderr_log: &Mutex<Vec<String>>,
) -> String {
    let status_text = status
        .map(|status| status.to_string())
        .unwrap_or_else(|| "still running".to_string());
    let stdout_text = if stdout_seen.is_empty() {
        "(none)".to_string()
    } else {
        stdout_seen.join("\n")
    };
    let stderr_text = {
        let lines = stderr_log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if lines.is_empty() {
            "(none)".to_string()
        } else {
            lines.join("\n")
        }
    };
    format!(
        "{reason} (waited {elapsed:?}, exit status: {status_text})\n\
         --- stdout seen before giving up ---\n{stdout_text}\n\
         --- stderr seen before giving up ---\n{stderr_text}"
    )
}

/// Reads the child's stdout on a background thread until the listening line
/// appears, polling every [`STARTUP_POLL_INTERVAL`] rather than sleeping
/// through one fixed wall-clock budget: startup readiness is an event to
/// wait for, not a duration to guess, and the host machine's own load — a
/// concurrent `cargo build`, other tests running in parallel — affects how
/// long that event legitimately takes. Every poll tick also checks whether
/// the child has already exited, so a binary that crashes or refuses to
/// boot is reported within one poll interval rather than after waiting out
/// the whole [`startup_timeout`] ceiling; that ceiling only gets consumed
/// in full by a process that neither logs its address nor exits, which is
/// the one case actually worth waiting out.
pub fn wait_for_listening_addr(
    child: &mut Child,
    stdout: ChildStdout,
    stderr_log: &Mutex<Vec<String>>,
) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_for_thread = Arc::clone(&seen);
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Some(addr) = parse_listening_addr(&line) {
                let _ = tx.send(addr);
                return;
            }
            seen_for_thread
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(line);
        }
        // Stdout hit EOF without ever finding the listening line. Returning
        // here drops `tx`, so the `recv_timeout` below fails with
        // `Disconnected` on its very next poll instead of waiting out the
        // rest of the ceiling for a line that is never coming.
    });

    let started = Instant::now();
    let ceiling = startup_timeout();
    loop {
        match rx.recv_timeout(STARTUP_POLL_INTERVAL) {
            Ok(addr) => return addr,
            Err(RecvTimeoutError::Disconnected) => {
                let status = wait_for_exit_status(child);
                let seen = seen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                panic!(
                    "{}",
                    startup_failure_message(
                        "the binary's stdout closed before it logged its listening address",
                        started.elapsed(),
                        status,
                        &seen,
                        stderr_log,
                    )
                );
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(status) = child.try_wait().ok().flatten() {
                    let seen = seen
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    panic!(
                        "{}",
                        startup_failure_message(
                            "the binary exited before it logged its listening address",
                            started.elapsed(),
                            Some(status),
                            &seen,
                            stderr_log,
                        )
                    );
                }
                if started.elapsed() >= ceiling {
                    let seen = seen
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    panic!(
                        "{}",
                        startup_failure_message(
                            "the binary did not log its listening address within the startup timeout",
                            started.elapsed(),
                            None,
                            &seen,
                            stderr_log,
                        )
                    );
                }
            }
        }
    }
}

fn configure_test_server_command(command: &mut Command) {
    // The startup contract waits for the server's INFO-level listening event.
    // Make it independent of the developer or CI process that launched this
    // integration test.
    command.env("RUST_LOG", "info");
}

/// Spawns `command` with stdout/stderr piped, waits for the real listening
/// address via [`wait_for_listening_addr`], and hands back the process
/// guard (kills the child on drop), the address, and the captured stderr
/// lines in case a caller wants to fold them into its own failure message
/// later. `command` should already carry every `env`/config setting the
/// caller needs — this only takes over stdio and the startup wait.
pub fn spawn_server(mut command: Command) -> (ServerProcess, String, Arc<Mutex<Vec<String>>>) {
    configure_test_server_command(&mut command);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawns the tellurion binary");
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    // Constructed immediately after spawn (before anything that can panic)
    // so the process is always killed, even if the startup wait below fails.
    let mut process = ServerProcess { child };
    let stderr_log = spawn_stderr_relay(stderr);
    let addr = wait_for_listening_addr(&mut process.child, stdout, &stderr_log);
    (process, addr, stderr_log)
}

pub struct HttpResponse {
    pub status: u16,
    pub content_type: Option<String>,
    /// The `Location` response header, when present — a create response
    /// carries one pointing at the created item; most responses in these
    /// tests leave it `None`.
    pub location: Option<String>,
    /// The `ETag` response header (OGC API Features — Part 4 Optimistic
    /// Locking, ETags class, `#107`), when present — every single-feature
    /// `GET` response carries one; see `optimistic_locking_binary.rs`.
    pub etag: Option<String>,
    /// The `Last-Modified` response header (Optimistic Locking, Timestamps
    /// class, `#107`), when present — only a collection with a declared
    /// `modified_column` ever carries one.
    pub last_modified: Option<String>,
    pub body: Vec<u8>,
}

pub fn find_header_body_boundary(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

pub fn parse_http_response(raw: &[u8]) -> HttpResponse {
    let boundary = find_header_body_boundary(raw).expect("response has a header/body boundary");
    let header_text = String::from_utf8_lossy(&raw[..boundary]);
    let body = raw[boundary + 4..].to_vec();

    let mut lines = header_text.split("\r\n");
    let status_line = lines.next().expect("status line present");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("valid status code");
    let header_lines: Vec<&str> = lines.collect();
    let content_type = header_lines
        .iter()
        .find(|line| line.to_ascii_lowercase().starts_with("content-type:"))
        .map(|line| line.split_once(':').unwrap().1.trim().to_string());
    let location = header_lines
        .iter()
        .find(|line| line.to_ascii_lowercase().starts_with("location:"))
        .map(|line| line.split_once(':').unwrap().1.trim().to_string());
    let etag = header_lines
        .iter()
        .find(|line| line.to_ascii_lowercase().starts_with("etag:"))
        .map(|line| line.split_once(':').unwrap().1.trim().to_string());
    let last_modified = header_lines
        .iter()
        .find(|line| line.to_ascii_lowercase().starts_with("last-modified:"))
        .map(|line| line.split_once(':').unwrap().1.trim().to_string());

    HttpResponse {
        status,
        content_type,
        location,
        etag,
        last_modified,
        body,
    }
}

/// A raw HTTP/1.1 GET over a plain socket — no client dependency needed for
/// a handful of status/content-type assertions. `Connection: close` makes
/// the server end the connection after the response, so reading to EOF is
/// enough regardless of whether the body used `Content-Length` or chunking.
pub fn http_get(addr: &str, path: &str) -> HttpResponse {
    let mut stream = TcpStream::connect(addr).expect("connects to the running server");
    stream
        .set_read_timeout(Some(request_timeout()))
        .expect("sets a read timeout");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("writes the request");
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .expect("reads the full response");
    parse_http_response(&raw)
}

/// A raw HTTP/1.1 request over a plain socket carrying a `method`/`body` —
/// [`http_get`]'s counterpart for the write lane's `PUT`/`DELETE` routes. An
/// empty `body` omits both `Content-Type` and `Content-Length` rather than
/// sending a zero-length one, matching a `DELETE` request's own shape (no
/// entity body at all).
pub fn http_write_request(addr: &str, method: &str, path: &str, body: &[u8]) -> HttpResponse {
    http_request_with_headers(addr, method, path, body, &[])
}

/// [`http_write_request`]'s general form: same shape, plus arbitrary extra
/// request headers (`If-Match`/`If-Unmodified-Since`, `#107`) — added for
/// `optimistic_locking_binary.rs` without touching the many existing
/// `http_write_request` call sites across every other `*_binary.rs` file in
/// this crate, which never need one.
pub fn http_request_with_headers(
    addr: &str,
    method: &str,
    path: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> HttpResponse {
    let mut stream = TcpStream::connect(addr).expect("connects to the running server");
    stream
        .set_read_timeout(Some(request_timeout()))
        .expect("sets a read timeout");
    let mut request = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if !body.is_empty() {
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        {
            request.push_str("Content-Type: application/geo+json\r\n");
        }
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("writes the request line and headers");
    stream.write_all(body).expect("writes the request body");
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .expect("reads the full response");
    parse_http_response(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn server_command_forces_info_logs_when_the_parent_selected_warn() {
        let mut command = Command::new("tellurion");
        command.env("RUST_LOG", "warn");

        configure_test_server_command(&mut command);

        let rust_log = command
            .get_envs()
            .find(|(name, _)| *name == OsStr::new("RUST_LOG"))
            .and_then(|(_, value)| value)
            .expect("test server command sets RUST_LOG");
        assert_eq!(rust_log, OsStr::new("info"));
    }
}
