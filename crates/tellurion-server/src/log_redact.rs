//! Secret redaction at the log-formatting boundary (`#189`).
//!
//! Call-site care is not enough: a failed connect can format the whole DSN —
//! password included — into an error's `Display` chain, and field-level
//! filtering never sees that rendered text. This module is the second layer:
//! a `MakeWriter` that scrubs URL userinfo passwords from every formatted
//! event, plain or JSON, before a byte reaches stdout.
//!
//! Scope is deliberately the one shape known to carry credentials end-to-end
//! today: `scheme://user:password@host` (`DATABASE_URL`, Valkey URLs). Broader
//! shapes (bearer tokens, presign signatures) extend `redact_line` when a
//! surface that logs them appears.

use std::borrow::Cow;
use std::io::{self, Write};

use tracing_subscriber::fmt::MakeWriter;

const MASK: &str = "***";

/// Replaces the password span of every `scheme://user:password@…` authority
/// in `line` with [`MASK`]. Authorities without userinfo (`host:port`) have
/// no `@` and pass through untouched, so ordinary URLs are never mangled.
fn redact_line(line: &str) -> Cow<'_, str> {
    let mut masked: Vec<(usize, usize)> = Vec::new();
    let mut from = 0;
    while let Some(found) = line[from..].find("://") {
        let auth_start = from + found + 3;
        let auth_end = line[auth_start..]
            .find(|c: char| {
                matches!(c, '/' | '?' | '#' | '"' | '\'' | '`' | '\\') || c.is_whitespace()
            })
            .map(|offset| auth_start + offset)
            .unwrap_or(line.len());
        let authority = &line[auth_start..auth_end];
        if let Some(at) = authority.rfind('@') {
            if let Some(colon) = authority[..at].find(':') {
                let password = (auth_start + colon + 1, auth_start + at);
                if password.1 > password.0 {
                    masked.push(password);
                }
            }
        }
        from = auth_end.max(auth_start + 1);
        if from >= line.len() {
            break;
        }
    }
    if masked.is_empty() {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0;
    for (start, end) in masked {
        out.push_str(&line[cursor..start]);
        out.push_str(MASK);
        cursor = end;
    }
    out.push_str(&line[cursor..]);
    Cow::Owned(out)
}

fn write_redacted_event(destination: &mut impl Write, event: &[u8]) -> io::Result<()> {
    let text = String::from_utf8_lossy(event);
    let scrubbed = redact_line(&text);
    destination.write_all(scrubbed.as_bytes())?;
    destination.flush()
}

/// `MakeWriter` handed to `tracing_subscriber::fmt`: yields one buffering
/// writer per formatted event, scrubbing and emitting the whole event as a
/// single stdout write on drop — so redaction always sees the complete
/// rendered line, never a fragment split across `write` calls.
pub struct RedactingStdout;

impl<'a> MakeWriter<'a> for RedactingStdout {
    type Writer = RedactingWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter { buf: Vec::new() }
    }
}

pub struct RedactingWriter {
    buf: Vec<u8>,
}

impl Write for RedactingWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for RedactingWriter {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let mut stdout = io::stdout().lock();
        let _ = write_redacted_event(&mut stdout, &self.buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_a_dsn_password_in_a_plain_line() {
        let line = "connect failed: postgres://tellurion:s3cr3t@db.internal:5432/gis timed out";
        assert_eq!(
            redact_line(line),
            "connect failed: postgres://tellurion:***@db.internal:5432/gis timed out"
        );
    }

    #[test]
    fn masks_inside_a_json_encoded_field() {
        let line = r#"{"level":"ERROR","error":"invalid dsn redis://user:hunter2@cache:6379"}"#;
        assert_eq!(
            redact_line(line),
            r#"{"level":"ERROR","error":"invalid dsn redis://user:***@cache:6379"}"#
        );
    }

    #[test]
    fn masks_every_occurrence() {
        let line = "a=postgres://u:one@h/db b=valkey://u:two@h:6379";
        assert_eq!(
            redact_line(line),
            "a=postgres://u:***@h/db b=valkey://u:***@h:6379"
        );
    }

    #[test]
    fn leaves_urls_without_userinfo_alone() {
        for line in [
            "listening on http://0.0.0.0:8080/healthz",
            "backend postgres://db.internal:5432/gis unreachable",
            "no url here at all",
        ] {
            assert!(matches!(redact_line(line), Cow::Borrowed(_)), "{line}");
        }
    }

    #[test]
    fn a_connection_failure_line_never_contains_the_password() {
        // The `#189` acceptance case: the rendered error chain of a failed
        // connect embeds the full DSN; the formatted line must not.
        let rendered = format!(
            "error connecting to database: invalid connection: {}",
            "postgresql://app:correct-horse-battery@10.0.0.7:5432/tellurion?sslmode=require"
        );
        let scrubbed = redact_line(&rendered);
        assert!(!scrubbed.contains("correct-horse-battery"));
        assert!(scrubbed.contains("postgresql://app:***@10.0.0.7:5432/tellurion"));
    }

    #[test]
    fn writes_the_redacted_event_and_flushes_its_destination() {
        struct Capture {
            bytes: Vec<u8>,
            flushed: bool,
        }

        impl Write for Capture {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                self.bytes.extend_from_slice(data);
                Ok(data.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                self.flushed = true;
                Ok(())
            }
        }

        let mut capture = Capture {
            bytes: Vec::new(),
            flushed: false,
        };

        write_redacted_event(
            &mut capture,
            b"database postgres://tellurion:secret@db.internal/gis failed\n",
        )
        .expect("writes and flushes the destination");

        assert_eq!(
            capture.bytes,
            b"database postgres://tellurion:***@db.internal/gis failed\n"
        );
        assert!(capture.flushed);
    }
}
