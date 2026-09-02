//! End-to-end proof for the `demo` subcommand: runs the real
//! `tellurion-ingest demo` composition against a temp `.gpkg`, waits for the
//! `tellurion` server it hands off to to come up, drives it over real HTTP
//! for collections/items/tiles, then shuts it down and confirms both the
//! wrapper and the server it spawned are gone — no orphaned listener left
//! behind.
//!
//! Needs the `tellurion` binary already built alongside `tellurion-ingest`
//! (the same `cargo build -p tellurion -p tellurion-ingest` the README's own
//! Quickstart already asks for) — see `demo.rs`'s own doc for why this
//! stays a runtime sibling-binary lookup rather than a Cargo dependency
//! edge between the two crates, the same call `tellurion-server`'s own
//! `geopackage_binary.rs` test already makes for the reverse direction.
//!
//! This is a different crate from `tellurion-server`, whose own
//! `*_binary.rs` tests share a `tests/common/mod.rs` — this file has no
//! Cargo dependency edge to that crate to reach it across (the doc above
//! already explains why one is never added just to resolve a path), and its
//! own readiness wait is a different shape besides: polling `/healthz` over
//! real HTTP rather than parsing a JSON line off `tellurion`'s stdout, since
//! the process this test spawns and waits on is the `tellurion-ingest`
//! wrapper, not `tellurion` itself. It keeps its own copy of the
//! poll-and-fail-fast pattern rather than sharing one.

use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often [`wait_for_healthy`] polls `/healthz` and checks the child's
/// exit status. Small enough that a crashed or refused-to-boot wrapper is
/// reported in a fraction of a second, never after waiting out the full
/// startup ceiling.
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Startup readiness is an event to wait for, not a duration to guess: how
/// long `tellurion-ingest demo` takes to provision, seed, and hand off to a
/// healthy `tellurion` server depends on host load (a concurrent `cargo
/// build`, other tests running in parallel) as much as on the work itself.
/// [`wait_for_healthy`] already polls for the event rather than sleeping
/// through one fixed budget, and now gives up immediately if the wrapper
/// exits, so this ceiling only gets consumed in full by a process that
/// neither answers healthy nor exits. Override with
/// `TELLURION_TEST_STARTUP_TIMEOUT_SECS` for a slower CI runner or a
/// heavily loaded machine.
fn startup_timeout() -> Duration {
    env_duration_secs("TELLURION_TEST_STARTUP_TIMEOUT_SECS").unwrap_or(Duration::from_secs(15))
}

/// How long [`DemoProcess::drop`] waits for the wrapper to actually exit
/// once it has been signaled. Same load sensitivity as [`startup_timeout`],
/// on the way out instead of the way in. Override with
/// `TELLURION_TEST_SHUTDOWN_TIMEOUT_SECS`.
fn shutdown_timeout() -> Duration {
    env_duration_secs("TELLURION_TEST_SHUTDOWN_TIMEOUT_SECS").unwrap_or(Duration::from_secs(8))
}

/// How long a single HTTP request against the already-running server may
/// take before the raw socket read gives up. Override with
/// `TELLURION_TEST_REQUEST_TIMEOUT_SECS`.
fn request_timeout() -> Duration {
    env_duration_secs("TELLURION_TEST_REQUEST_TIMEOUT_SECS").unwrap_or(Duration::from_secs(5))
}

fn env_duration_secs(var: &str) -> Option<Duration> {
    std::env::var(var)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn temp_path(suffix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "tellurion-ingest-demo-binary-test-{}-{suffix}",
        std::process::id(),
    ));
    path
}

fn cleanup(gpkg_path: &PathBuf) {
    let _ = std::fs::remove_file(gpkg_path);
    let _ = std::fs::remove_file(format!("{}-wal", gpkg_path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", gpkg_path.display()));
}

/// An OS-assigned ephemeral port: binds `127.0.0.1:0`, reads back whichever
/// free port the kernel picked, then releases the listener so the spawned
/// `tellurion-ingest demo` process can bind it in turn. Replaces a former
/// `39000 + pid % 4000` scheme — two different processes whose PIDs happen
/// to agree modulo 4000 would have computed the same "random" port despite
/// never sharing one, and the fixed 39000-43000 range could also collide
/// with anything else already listening on the host in that band. Asking
/// the kernel for a free port removes both failure modes; it hands back one
/// that was actually free at the moment of the bind.
fn pick_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binds an ephemeral port");
    listener
        .local_addr()
        .expect("bound listener has a local addr")
        .port()
}

/// Sends SIGTERM to the wrapper (`demo.rs`'s own `run_until_child_exits_or_
/// signaled` forwards it to the `tellurion` child it spawned and waits on
/// that before returning), then waits for the wrapper itself to exit;
/// falls back to a hard kill so a failing assertion earlier in the test can
/// never leak a listening process past this test, matching every other
/// `*_binary.rs` test's own `ServerProcess` convention in this workspace.
struct DemoProcess {
    child: Child,
}

impl Drop for DemoProcess {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(self.child.id().to_string())
                .status();
        }
        let deadline = Instant::now() + shutdown_timeout();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

fn find_header_body_boundary(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

fn http_get(addr: &str, path: &str) -> Option<HttpResponse> {
    let mut stream = TcpStream::connect(addr).ok()?;
    stream.set_read_timeout(Some(request_timeout())).ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    std::io::Write::write_all(&mut stream, request.as_bytes()).ok()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    let boundary = find_header_body_boundary(&raw)?;
    let header_text = String::from_utf8_lossy(&raw[..boundary]);
    let status = header_text
        .split("\r\n")
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(HttpResponse {
        status,
        body: raw[boundary + 4..].to_vec(),
    })
}

/// Reads the wrapper's stderr on a background thread, echoing each line to
/// this process's own stderr — matching what `Stdio::inherit()` used to
/// give for free — while also keeping a copy so a startup failure can
/// report what it actually printed, rather than just that the wait gave up.
fn spawn_stderr_relay(stderr: ChildStderr) -> Arc<Mutex<Vec<String>>> {
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

fn startup_failure_message(
    reason: &str,
    elapsed: Duration,
    status: Option<ExitStatus>,
    stderr_log: &Mutex<Vec<String>>,
) -> String {
    let status_text = status
        .map(|status| status.to_string())
        .unwrap_or_else(|| "still running".to_string());
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
         --- stderr seen before giving up ---\n{stderr_text}"
    )
}

/// Polls `/healthz` (always live, no dependency probe) every
/// [`STARTUP_POLL_INTERVAL`] until it answers or [`startup_timeout`]
/// elapses. Also checks the wrapper's own exit status on every tick, so a
/// `tellurion-ingest demo` that crashes or refuses to boot is reported
/// within one poll interval instead of only surfacing as a misleading
/// connection failure after the full startup ceiling.
fn wait_for_healthy(child: &mut Child, addr: &str, stderr_log: &Mutex<Vec<String>>) {
    let started = Instant::now();
    let ceiling = startup_timeout();
    loop {
        if let Some(response) = http_get(addr, "/healthz") {
            if response.status == 200 {
                return;
            }
        }
        if let Some(status) = child.try_wait().ok().flatten() {
            panic!(
                "{}",
                startup_failure_message(
                    "tellurion-ingest demo exited before the server became healthy",
                    started.elapsed(),
                    Some(status),
                    stderr_log,
                )
            );
        }
        if started.elapsed() >= ceiling {
            panic!(
                "{}",
                startup_failure_message(
                    "the demo server did not become healthy within the startup timeout",
                    started.elapsed(),
                    None,
                    stderr_log,
                )
            );
        }
        std::thread::sleep(STARTUP_POLL_INTERVAL);
    }
}

/// The proof, end to end: one command (`tellurion-ingest demo`) provisions
/// and seeds a fresh `.gpkg` and hands off to the real `tellurion` binary,
/// which serves the collections list, the full 500-row synthetic grid, and
/// a real MVT tile — then a clean shutdown leaves nothing listening.
#[test]
fn demo_composes_provisioning_seeding_and_serving_over_http() {
    let gpkg_path = temp_path("demo.gpkg");
    cleanup(&gpkg_path);
    let port = pick_port();
    let addr = format!("127.0.0.1:{port}");

    let mut command = Command::new(env!("CARGO_BIN_EXE_tellurion-ingest"));
    command
        .arg("demo")
        .arg("--path")
        .arg(&gpkg_path)
        .arg("--port")
        .arg(port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = command.spawn().expect("spawns tellurion-ingest demo");
    let stderr = child.stderr.take().expect("stderr was piped");
    // Constructed immediately after spawn (before anything that can panic)
    // so the process is always killed, even if the startup wait below fails.
    let mut process = DemoProcess { child };
    let stderr_log = spawn_stderr_relay(stderr);

    wait_for_healthy(&mut process.child, &addr, &stderr_log);

    let collections = http_get(&addr, "/public/features/catalogs/default/collections")
        .expect("collections request succeeds");
    assert_eq!(collections.status, 200);
    let body: serde_json::Value =
        serde_json::from_slice(&collections.body).expect("valid JSON body");
    let list = body["collections"].as_array().expect("collections array");
    assert_eq!(list.len(), 1, "exactly the seeded demo collection");
    assert_eq!(list[0]["id"], "demo");

    let items = http_get(
        &addr,
        "/public/features/catalogs/default/collections/demo/items?limit=1000",
    )
    .expect("items request succeeds");
    assert_eq!(items.status, 200);
    let items_body: serde_json::Value =
        serde_json::from_slice(&items.body).expect("valid JSON body");
    assert_eq!(
        items_body["features"]
            .as_array()
            .expect("features array")
            .len(),
        500,
        "the full deterministic synthetic grid the demo composition seeds"
    );

    let tile = http_get(
        &addr,
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.mvt",
    )
    .expect("tile request succeeds");
    assert_eq!(
        tile.status, 200,
        "z0 tile covering the world should return 200"
    );
    assert!(!tile.body.is_empty(), "the tile body should not be empty");

    // Clean shutdown: SIGTERM the wrapper, expect both it and the
    // `tellurion` child it spawned to be gone, and the port freed.
    drop(process);
    assert!(
        http_get(&addr, "/healthz").is_none(),
        "nothing should still be listening on the demo port after shutdown"
    );

    cleanup(&gpkg_path);
}
