//! Regression test for `common::wait_for_listening_addr`'s EOF diagnostic.
//!
//! Stdout closing and the child process being reaped are not ordered
//! events, so a single `try_wait()` called the instant the reader thread
//! sees EOF frequently misses a child that has, in fact, already exited —
//! the diagnostic then claims the process is "still running" when it just
//! isn't. This spawns a child that writes nothing to stdout and exits
//! non-zero, and asserts the panic message names the real exit status
//! rather than claiming the child is still running.
//!
//! No real `tellurion` binary involved and no feature gate needed: this
//! only exercises the generic exit-status wait, not anything driver- or
//! config-shaped.

mod common;

use std::panic::AssertUnwindSafe;
use std::process::{Command, Stdio};

#[test]
fn eof_diagnostic_reports_the_actual_exit_status_not_still_running() {
    let mut child = Command::new("sh")
        .args(["-c", "exit 7"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawns a trivial child process");
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let stderr_log = common::spawn_stderr_relay(stderr);

    let panic_payload = std::panic::catch_unwind(AssertUnwindSafe(|| {
        common::wait_for_listening_addr(&mut child, stdout, &stderr_log)
    }))
    .expect_err("a child that never logs a listening address must panic");

    let message = panic_payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic_payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("panic payload is a string message");

    assert!(
        !message.contains("still running"),
        "diagnostic wrongly claimed the child is still running: {message}"
    );
    assert!(
        message.contains("exit status: 7"),
        "diagnostic did not name the actual exit status: {message}"
    );
}
