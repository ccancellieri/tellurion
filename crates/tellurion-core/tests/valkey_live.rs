//! Live round-trip test against a real Valkey (or Redis) instance, driving
//! the actual `ValkeyL2Cache` backend end to end rather than the in-crate
//! mock `cache.rs`'s unit tests use. `#[ignore]`d so `cargo test` never
//! needs a live server by default; run explicitly once
//! `TELLURION_TEST_VALKEY_URL` points at one:
//!
//!   TELLURION_TEST_VALKEY_URL=redis://127.0.0.1:6379 \
//!     cargo test -p tellurion-core --features valkey -- --ignored

#![cfg(feature = "valkey")]

use std::time::Duration;

use bytes::Bytes;

use tellurion_core::{Encoding, L2Cache, TileKey, TileMatrixSet, ValkeyL2Cache};

const URL_ENV_VAR: &str = "TELLURION_TEST_VALKEY_URL";

fn key() -> TileKey {
    TileKey {
        tenant: "public".to_string(),
        catalog: "default".to_string(),
        collection: "demo".to_string(),
        tms: TileMatrixSet::WebMercatorQuad,
        z: 5,
        x: 1,
        y: 1,
        encoding: Encoding::Mvt,
        policy_fingerprint: None,
        properties: Vec::new(),
        generation: 0,
    }
}

#[tokio::test]
#[ignore = "requires a real Valkey/Redis instance; set TELLURION_TEST_VALKEY_URL and pass --ignored"]
async fn valkey_l2_round_trip() {
    if std::env::var(URL_ENV_VAR).is_err() {
        eprintln!("skipping valkey_l2_round_trip: {URL_ENV_VAR} not set");
        return;
    }

    let backend = ValkeyL2Cache::connect(URL_ENV_VAR)
        .await
        .expect("connects to the live valkey instance");

    let target = key();
    let value = Bytes::from_static(b"live-round-trip");

    assert_eq!(
        backend.get(&target).await.expect("get before put"),
        None,
        "a fresh key should be a miss"
    );

    backend
        .put(target.clone(), value.clone(), Duration::from_secs(30))
        .await
        .expect("writes to valkey");

    assert_eq!(
        backend.get(&target).await.expect("get after put"),
        Some(value)
    );
}
