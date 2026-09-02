//! Test-only fixture support (`#[cfg(test)]`, see `lib.rs`): a tiny,
//! hand-rolled HTTP/1.1 loopback server good for exactly one thing — this
//! crate's own administrative compatibility tests of ranged reads. Understands
//! only a GET request with (or, for the
//! range-refusing fixture, deliberately ignoring) a `Range: bytes=
//! start-end` header; every other HTTP feature (`Host` validation,
//! keep-alive reuse, chunked encoding, ...) is out of scope. Hand-rolled on
//! `std::net::TcpListener` rather than pulling a second HTTP server crate
//! into this crate's own dev-dependencies — the full loopback-listener
//! acceptance proof (a real client hitting a real server through a real
//! `tellurion` binary) already lives at `tellurion-server`'s own
//! `cog_binary.rs`, which depends on `axum`/`tokio` for real; this module
//! only needs to prove the driver's own `Read`/`Seek` adapter against real
//! Range/206 wire semantics, in-process.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};

pub(crate) struct MockServer {
    addr: SocketAddr,
}

impl MockServer {
    /// Serves `body`, honoring `Range` requests with real 206/
    /// `Content-Range` semantics — the happy-path fixture.
    pub(crate) fn range_aware(body: Vec<u8>) -> Self {
        Self::spawn(body, RangeBehavior::Honor)
    }

    /// Serves `body` but always answers `200 OK` with the whole body,
    /// ignoring any `Range` header — the "refuses ranged reads" fixture.
    pub(crate) fn ignoring_range(body: Vec<u8>) -> Self {
        Self::spawn(body, RangeBehavior::Ignore)
    }

    fn spawn(body: Vec<u8>, behavior: RangeBehavior) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds a loopback port");
        let addr = listener
            .local_addr()
            .expect("bound listener has a local addr");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                handle_connection(stream, &body, behavior);
            }
        });
        Self { addr }
    }

    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

#[derive(Clone, Copy)]
enum RangeBehavior {
    Honor,
    Ignore,
}

fn handle_connection(mut stream: TcpStream, body: &[u8], behavior: RangeBehavior) {
    let mut reader = BufReader::new(stream.try_clone().expect("clones the loopback stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }

    let mut range: Option<(usize, usize)> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line
            .strip_prefix("Range: ")
            .or_else(|| line.strip_prefix("range: "))
        {
            range = parse_range_header(value, body.len());
        }
    }

    let (status_line, content_range, payload): (&str, Option<String>, &[u8]) =
        match (behavior, range) {
            (RangeBehavior::Honor, Some((start, end))) => (
                "HTTP/1.1 206 Partial Content",
                Some(format!("bytes {start}-{end}/{}", body.len())),
                &body[start..=end],
            ),
            _ => ("HTTP/1.1 200 OK", None, body),
        };

    let mut response = format!(
        "{status_line}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n",
        payload.len()
    );
    if let Some(content_range) = content_range {
        response.push_str(&format!("Content-Range: {content_range}\r\n"));
    }
    response.push_str("\r\n");

    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(payload);
}

/// Parses this fixture's own client's exact `Range: bytes=start-end` shape
/// (both bounds always present — `remote.rs`'s `send_range` never emits an
/// open-ended range) into an inclusive `(start, end)` pair, clamping `end`
/// to the real last byte the same way a real range-serving HTTP server
/// would.
fn parse_range_header(value: &str, total: usize) -> Option<(usize, usize)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    let start: usize = start.parse().ok()?;
    let end: usize = end.parse().ok().unwrap_or(total.saturating_sub(1));
    Some((start, end.min(total.saturating_sub(1))))
}
