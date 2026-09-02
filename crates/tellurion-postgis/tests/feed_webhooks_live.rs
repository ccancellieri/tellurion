//! Live tests for the change feed and webhook delivery (`#115`) through the
//! actual `PostgisDriverFactory` entry point, against a real PostGIS
//! instance. Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is set,
//! matching every other live test in this workspace (`invalidation_live.rs`,
//! `index_live.rs`, ...).
//!
//! The webhook test drives a REAL HTTP delivery (`tellurion_core::
//! ReqwestDeliverer`) against a hand-rolled, dependency-free HTTP/1.1 mock
//! receiver bound to an ephemeral `127.0.0.1` port in-process — no external
//! network, no new crate dependency (`tokio`'s `net`/`io-util` are already
//! this crate's own `tokio = { workspace = true }`).

use std::env;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use tellurion_core::{
    feed, hmac_sha256_hex, run_webhook_consumer, CollectionDecl, DriverFactory, Mutation,
    MutationKind, ReqwestDeliverer, Sequence, StorageDecl, StorageDriver, WebhookConsumerSettings,
    WebhookDeliverer, WebhookRetryPolicy, WebhookSubscriptionRuntime,
};
use tellurion_postgis::test_harness;
use tellurion_postgis::PostgisDriverFactory;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const URL_ENV_VAR: &str = "TELLURION_POSTGIS_FEED_WEBHOOKS_LIVE_TEST_URL";

async fn connect(database_url: &str) -> tokio_postgres::Client {
    test_harness::connect(database_url).await
}

async fn seed_data_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "DROP TABLE IF EXISTS {table} CASCADE;
             DROP TABLE IF EXISTS {table}_outbox;
             CREATE TABLE {table} (
                 id bigserial PRIMARY KEY,
                 geom geometry(Point, 4326),
                 name text
             );"
        ),
    )
    .await
    .expect("seeds the data table");
}

/// Matches `tellurion-ingest::outbox::create_outbox_table_sql` exactly —
/// same convention every other live test in this crate follows.
async fn seed_outbox_table(database_url: &str, table: &str) {
    let client = connect(database_url).await;
    test_harness::apply_fixture_ddl(
        &client,
        table,
        &format!(
            "CREATE TABLE IF NOT EXISTS {table}_outbox (
                 sequence bigserial PRIMARY KEY,
                 feature_id text NOT NULL,
                 kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
                 payload jsonb,
                 committed_at timestamptz NOT NULL DEFAULT now(),
                 extent_crs84 jsonb
             );"
        ),
    )
    .await
    .expect("seeds the outbox table");
}

fn collection(table: &str) -> CollectionDecl {
    serde_yaml::from_str(&format!(
        "id: demo\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
    ))
    .expect("valid CollectionDecl yaml")
}

async fn build_driver(database_url: &str) -> Arc<dyn StorageDriver> {
    // Safety: this test binary sets this one env var exactly once per test
    // process before any connection pool spawns worker tasks, matching
    // every other live test's own documented safety argument for the
    // identical pattern.
    unsafe {
        env::set_var(URL_ENV_VAR, database_url);
    }
    let factory = PostgisDriverFactory::new(60);
    let decl = StorageDecl {
        id: "main".to_string(),
        driver: "postgis".to_string(),
        url_env: URL_ENV_VAR.to_string(),
        pool_size: None,
    };
    factory.build(&decl).expect("driver builds")
}

async fn upsert_point(
    sink: &dyn tellurion_core::WriteSink,
    collection: &CollectionDecl,
    id: &str,
    lon: f64,
    lat: f64,
) -> Sequence {
    sink.apply(
        collection,
        Mutation {
            feature_id: id.to_string(),
            kind: MutationKind::Upsert(json!({
                "type": "Feature",
                "geometry": {"type": "Point", "coordinates": [lon, lat]},
                "properties": {"name": id}
            })),
        },
    )
    .await
    .expect("upsert succeeds")
}

/// (1): a real committed-and-reread outbox row appears in the next feed
/// page, as a compact envelope carrying no payload, with a plausible
/// (real, recently-committed) timestamp — proving `Obligation::
/// committed_at`'s real `timestamptz -> SystemTime` round trip, not just
/// the in-memory fixture `feed.rs`'s own unit tests use.
#[tokio::test]
async fn a_real_write_appears_in_the_next_feed_page() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!("skipping a_real_write_appears_in_the_next_feed_page: TELLURION_TEST_DATABASE_URL not set");
        return;
    };
    let table = "tellurion_postgis_feed_live_test_page";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);

    let before_write = SystemTime::now();
    let seq_a = upsert_point(write.as_ref(), &collection, "1", 10.0, 45.0).await;
    let seq_b = upsert_point(write.as_ref(), &collection, "2", 11.0, 46.0).await;

    let obligations = outbox
        .read_after(&collection, Sequence(0), 100)
        .await
        .expect("read_after succeeds");
    assert_eq!(obligations.len(), 2);

    let page = feed::build_page("demo", &obligations, 100);
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].sequence, seq_a.0);
    assert_eq!(page.entries[1].sequence, seq_b.0);
    assert_eq!(page.entries[0].collection, "demo");
    assert_eq!(page.entries[0].item_id, "1");
    assert_eq!(page.entries[0].operation, feed::FeedOperation::Upsert);
    assert!(
        page.next.is_none(),
        "a page shorter than the requested limit must never carry a next token"
    );

    // The real `committed_at` (read back through Postgres, not a fixture)
    // parses as a plausible, recent RFC 3339 UTC timestamp — within a wide
    // tolerance window rather than an exact comparison, since server clock
    // skew/transaction commit timing is out of this test's control.
    let committed_at: SystemTime =
        parse_rfc3339_for_test(&page.entries[0].committed_at).expect("parses as RFC 3339");
    let elapsed = committed_at
        .duration_since(before_write)
        .unwrap_or(Duration::ZERO);
    assert!(
        elapsed < Duration::from_secs(30),
        "committed_at ({:?}) should be within 30s of the write, drifted {:?}",
        page.entries[0].committed_at,
        elapsed
    );
}

/// Hand-rolled RFC 3339 UTC parse (`YYYY-MM-DDTHH:MM:SS.sssZ`) — this test's
/// own sanity check that the real, round-tripped-through-Postgres
/// `committed_at` this crate's driver produced is a plausible timestamp
/// (`tellurion_core::parse_utc_datetime_text` is exercised directly by its
/// own unit tests; this local copy avoids reaching into a crate-private
/// path from an integration test just for one assertion).
fn parse_rfc3339_for_test(text: &str) -> Option<SystemTime> {
    let body = text.strip_suffix('Z')?;
    let (date, time) = body.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    let (hms, millis) = match time.split_once('.') {
        Some((hms, frac)) => (
            hms,
            frac.chars()
                .chain("000".chars())
                .take(3)
                .collect::<String>()
                .parse::<u64>()
                .ok()?,
        ),
        None => (time, 0),
    };
    let mut time_parts = hms.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;

    // Days since epoch via the same civil-calendar algorithm
    // `tellurion_core::timefmt` uses (public-domain, Howard Hinnant).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from((month + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    let total_secs = days * 86_400 + hour * 3600 + minute * 60 + second;
    if total_secs < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(total_secs as u64) + Duration::from_millis(millis))
}

// ---- webhook delivery against a local mock receiver --------------------

struct ReceivedRequest {
    body: Vec<u8>,
    signature: String,
}

/// A minimal, dependency-free HTTP/1.1 server: accepts one connection at a
/// time, reads exactly one request (headers + `Content-Length` body),
/// records it, and answers `500` for the first `fail_first_n` requests it
/// sees and `200` after that — simulating a receiver that is temporarily
/// down, then recovers, so a webhook delivery's own retry/backoff path has
/// something real to exercise. Always answers `Connection: close` and drops
/// the socket, so `reqwest` never tries to reuse a connection across the
/// simulated outage.
async fn spawn_mock_receiver(
    fail_first_n: u32,
) -> (
    u16,
    Arc<Mutex<Vec<ReceivedRequest>>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds an ephemeral port");
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(Mutex::new(Vec::new()));
    let remaining_failures = Arc::new(AtomicU32::new(fail_first_n));

    let received_for_task = Arc::clone(&received);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let (headers_end, content_length) = loop {
                let n = stream.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break (None, 0usize);
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                    let header_text = String::from_utf8_lossy(&buf[..pos]).to_string();
                    let content_length = header_text
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            (name.trim().eq_ignore_ascii_case("content-length"))
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    break (Some(pos + 4), content_length);
                }
            };
            let Some(headers_end) = headers_end else {
                continue;
            };
            while buf.len() < headers_end + content_length {
                let n = stream.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let header_text = String::from_utf8_lossy(&buf[..headers_end]).to_string();
            let signature = header_text
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim()
                        .eq_ignore_ascii_case(tellurion_core::SIGNATURE_HEADER)
                        .then(|| value.trim().to_string())
                })
                .unwrap_or_default();
            let body = buf[headers_end..(headers_end + content_length).min(buf.len())].to_vec();

            received_for_task
                .lock()
                .unwrap()
                .push(ReceivedRequest { body, signature });

            let remaining = remaining_failures.load(Ordering::SeqCst);
            let status_line = if remaining > 0 {
                remaining_failures.fetch_sub(1, Ordering::SeqCst);
                "HTTP/1.1 500 Internal Server Error\r\n"
            } else {
                "HTTP/1.1 200 OK\r\n"
            };
            let response = format!("{status_line}content-length: 0\r\nconnection: close\r\n\r\n");
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });

    (port, received, handle)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// (2): a real `WriteSink::apply` commits an obligation; `drain_once`
/// delivers it to a local mock receiver over a real HTTP connection,
/// HMAC-signed; the mock fails the first attempt (a simulated `500`) and
/// succeeds the second — proving the retry/backoff path redelivers the
/// SAME envelope (same `sequence`) rather than skipping or duplicating
/// content, and that the delivery cursor only advances once delivery
/// actually succeeds.
#[tokio::test]
async fn a_webhook_delivers_redelivers_after_a_simulated_500_and_dedupes_by_sequence() {
    let Ok(database_url) = env::var("TELLURION_TEST_DATABASE_URL") else {
        eprintln!(
            "skipping a_webhook_delivers_redelivers_after_a_simulated_500_and_dedupes_by_sequence: TELLURION_TEST_DATABASE_URL not set"
        );
        return;
    };
    let table = "tellurion_postgis_webhook_live_test";
    seed_data_table(&database_url, table).await;
    seed_outbox_table(&database_url, table).await;

    let driver = build_driver(&database_url).await;
    let write = driver.write_sink().expect("driver exposes WriteSink");
    let outbox = driver.outbox_source().expect("driver exposes OutboxSource");
    let collection = collection(table);

    let seq = upsert_point(write.as_ref(), &collection, "1", 1.0, 2.0).await;

    let (port, received, _server) = spawn_mock_receiver(1).await;
    let secret = b"top-secret".to_vec();
    let subscription = Arc::new(WebhookSubscriptionRuntime::new(
        "live-sub".to_string(),
        format!("http://127.0.0.1:{port}/hook"),
        secret.clone(),
        Vec::new(),
        [collection.id.clone()],
        10,
    ));
    let deliverer: Arc<dyn WebhookDeliverer> =
        Arc::new(ReqwestDeliverer::new(Duration::from_secs(5)));
    let retry = WebhookRetryPolicy {
        max_attempts: 3,
        base_backoff_ms: 10,
        max_backoff_ms: 50,
    };
    let settings = WebhookConsumerSettings {
        batch_size: 100,
        retry,
        poll_interval: Duration::from_secs(3600),
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let consumer = tokio::spawn(run_webhook_consumer(
        Arc::clone(&outbox),
        Arc::clone(&subscription),
        collection.clone(),
        deliverer,
        settings,
        shutdown_rx,
    ));

    // One pass is enough here: the consumer's own poll loop only sleeps
    // between passes once a pass finds nothing new, so give it a moment to
    // run its first pass (which drains the one obligation, retries through
    // the simulated 500, and succeeds) before asserting.
    tokio::time::sleep(Duration::from_millis(300)).await;
    shutdown_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), consumer)
        .await
        .expect("the webhook consumer should stop promptly on shutdown")
        .unwrap();

    assert_eq!(
        subscription.cursor(&collection.id),
        seq,
        "the cursor should advance past the obligation once delivery actually succeeded"
    );
    let (dead_letters, _) = subscription.dead_letters(None, 10).unwrap();
    assert!(
        dead_letters.is_empty(),
        "an eventually-successful delivery must never be dead-lettered"
    );

    let requests = received.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "expected exactly one failed attempt and one successful retry"
    );
    let first: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let second: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(
        first["sequence"], second["sequence"],
        "the retried delivery must carry the exact same sequence — a receiver dedupes on this"
    );
    assert_eq!(first["sequence"], seq.0);

    for request in requests.iter() {
        let expected_signature = hmac_sha256_hex(&secret, &request.body);
        assert_eq!(
            request.signature, expected_signature,
            "the HMAC signature header must verify against the shared secret"
        );
    }
}
