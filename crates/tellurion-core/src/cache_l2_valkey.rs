//! Valkey-backed `L2Cache` implementation. Gated behind the `valkey` cargo
//! feature (default off) — see `cache.rs` for the `L2Cache` trait this
//! implements and `L2CacheAdapter`, which is what actually plugs an instance
//! of this into a `LayeredCache`.
//!
//! Talks RESP over the `redis` crate, which speaks the same wire protocol
//! Valkey forked from and needs no Valkey-specific client. Connects through
//! `redis::aio::ConnectionManager`, which reconnects and retries on its own
//! after a transient outage — this backend never implements its own retry
//! loop, and `L2CacheAdapter` already treats every `Err` from here as a
//! plain cache miss (reads) or a logged, swallowed failure (writes), so a
//! Valkey outage degrades the server to L1-only rather than failing
//! requests.

use std::time::Duration;

use bytes::Bytes;
use redis::AsyncCommands;

use crate::cache::{Encoding, L2Cache, MapCrs, TileKey};
use crate::error::Error;

/// Real Valkey/Redis-backed L2 cache. Construct with [`ValkeyL2Cache::connect`].
pub struct ValkeyL2Cache {
    conn: redis::aio::ConnectionManager,
}

impl ValkeyL2Cache {
    /// Connects to the URL held by the environment variable named
    /// `url_env` — mirrors `StorageDecl.url_env`, so the URL itself never
    /// lives in config. Fails fast (same as an unresolvable storage
    /// `url_env`) rather than deferring the error to the first request.
    pub async fn connect(url_env: &str) -> Result<Self, Error> {
        let url = std::env::var(url_env).map_err(|_| {
            Error::Config(format!(
                "cache.l2: environment variable '{url_env}' is not set"
            ))
        })?;
        let client = redis::Client::open(url)
            .map_err(|err| Error::Config(format!("cache.l2: invalid valkey url: {err}")))?;
        let conn = client.get_connection_manager().await.map_err(|err| {
            Error::Config(format!("cache.l2: failed to connect to valkey: {err}"))
        })?;
        Ok(Self { conn })
    }
}

/// Deterministic string key for a `TileKey`, namespaced so this backend can
/// share a Valkey instance with unrelated keys without collision. Mirrors
/// the key's own field order; every `Encoding` field that partitions the
/// in-process L1 (style id, colormap fingerprint, the whole `Map` window —
/// see the `TileKey` hash impl and each variant's doc in `cache.rs`) is
/// embedded here too, so two entries that don't collide in L1 can't collide
/// in Valkey either. `generation` (`#113`) is embedded the same way: a
/// bucket bump changes the suffix, so an L2 entry written under an older
/// generation is simply never looked up again — no remote purge sweep ever
/// runs against this backend, exactly the design doc's own stance.
fn redis_key(key: &TileKey) -> String {
    let encoding = match &key.encoding {
        Encoding::Mvt => "mvt".to_string(),
        Encoding::Png => "png".to_string(),
        Encoding::Glb => "glb".to_string(),
        Encoding::PngStyled(style) => format!("png_styled:{style}"),
        Encoding::PngRaster(colormap) => match colormap {
            Some(fingerprint) => format!("png_raster:{fingerprint}"),
            None => "png_raster:none".to_string(),
        },
        Encoding::Map {
            crs,
            bbox,
            width,
            height,
            style,
            lane,
        } => {
            let crs = match crs {
                MapCrs::WebMercator => "web_mercator",
                MapCrs::Crs84 => "crs84",
            };
            let style = style.as_deref().unwrap_or("none");
            // `#37`: the render lane partitions Valkey exactly like it
            // partitions L1 (`Encoding::Map`'s own `lane`) — but the VECTOR
            // lane (the only one that existed before this slice)
            // deliberately contributes NO segment, so every map entry a
            // previous release wrote stays addressable under its original
            // key after a rolling deploy; only the new lane pays for a new
            // segment. Same convention, and the same reason, as `tms` below.
            let lane = match lane {
                crate::cache::MapLane::Vector => String::new(),
                crate::cache::MapLane::Raster(None) => ":raster:none".to_string(),
                crate::cache::MapLane::Raster(Some(fingerprint)) => {
                    format!(":raster:{fingerprint}")
                }
            };
            format!(
                "map:{crs}:{}:{}:{}:{}:{width}x{height}:{style}{lane}",
                bbox[0], bbox[1], bbox[2], bbox[3]
            )
        }
    };
    // `#190`: the tile matrix set partitions Valkey exactly like it
    // partitions L1 (`TileKey::tms`) — but WebMercatorQuad (the only grid
    // that existed before `#190`) deliberately contributes NO segment, so
    // every pre-`#190` L2 entry stays addressable under its original key
    // after a rolling deploy; only the new grid pays for a new segment.
    let tms = match key.tms {
        crate::tms::TileMatrixSet::WebMercatorQuad => "",
        crate::tms::TileMatrixSet::WorldCrs84Quad => "WorldCRS84Quad:",
    };
    format!(
        "tellurion:tile:{}:{}:{}:{tms}{}/{}/{}:{encoding}:g{}",
        key.tenant, key.catalog, key.collection, key.z, key.x, key.y, key.generation
    )
}

#[async_trait::async_trait]
impl L2Cache for ValkeyL2Cache {
    async fn get(&self, key: &TileKey) -> Result<Option<Bytes>, Error> {
        let mut conn = self.conn.clone();
        let raw: Option<Vec<u8>> = conn
            .get(redis_key(key))
            .await
            .map_err(|err| Error::Storage(Box::new(err)))?;
        Ok(raw.map(Bytes::from))
    }

    async fn put(&self, key: TileKey, value: Bytes, ttl: Duration) -> Result<(), Error> {
        let mut conn = self.conn.clone();
        // Valkey's `SETEX` requires a strictly positive second count;
        // `AppConfig::validate` already rejects `ttl_s == 0`, but `.max(1)`
        // keeps this call correct even if it's ever driven by an
        // unvalidated `Duration` in a future caller.
        let ttl_s = ttl.as_secs().max(1);
        let _: () = conn
            .set_ex(redis_key(&key), value.to_vec(), ttl_s)
            .await
            .map_err(|err| Error::Storage(Box::new(err)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_with_encoding(encoding: Encoding) -> TileKey {
        TileKey {
            tenant: "public".to_string(),
            catalog: "default".to_string(),
            collection: "demo".to_string(),
            tms: crate::tms::TileMatrixSet::WebMercatorQuad,
            z: 5,
            x: 1,
            y: 1,
            encoding,
            policy_fingerprint: None,
            properties: Vec::new(),
            generation: 0,
        }
    }

    /// `#190`: a WorldCRS84Quad entry gets its own explicit grid segment,
    /// while the WebMercatorQuad key stays byte-for-byte the pre-`#190`
    /// string (proven by `redis_key_is_namespaced_and_includes_every_coordinate`
    /// above) — so the two grids can never collide in Valkey and no
    /// existing L2 entry is orphaned by the new field.
    #[test]
    fn redis_key_separates_the_two_tile_matrix_sets() {
        let mercator = key_with_encoding(Encoding::Mvt);
        let crs84 = TileKey {
            tms: crate::tms::TileMatrixSet::WorldCrs84Quad,
            ..key_with_encoding(Encoding::Mvt)
        };
        assert_ne!(redis_key(&mercator), redis_key(&crs84));
        assert_eq!(
            redis_key(&crs84),
            "tellurion:tile:public:default:demo:WorldCRS84Quad:5/1/1:mvt:g0"
        );
    }

    #[test]
    fn redis_key_is_namespaced_and_includes_every_coordinate() {
        let key = redis_key(&key_with_encoding(Encoding::Mvt));
        assert_eq!(key, "tellurion:tile:public:default:demo:5/1/1:mvt:g0");
    }

    /// `#113`: a generation bump changes the Valkey key exactly like it
    /// changes the in-process one — the L2 counterpart of `cache.rs`'s own
    /// `keys_with_different_generations_at_the_same_coordinate_are_distinct`.
    #[test]
    fn redis_key_changes_when_the_generation_changes() {
        let mut key = key_with_encoding(Encoding::Mvt);
        let at_zero = redis_key(&key);
        key.generation = 7;
        let at_seven = redis_key(&key);
        assert_ne!(at_zero, at_seven);
        assert_eq!(at_seven, "tellurion:tile:public:default:demo:5/1/1:mvt:g7");
    }

    #[test]
    fn redis_key_distinguishes_every_encoding_variant_at_the_same_coord() {
        let variants = [
            Encoding::Mvt,
            Encoding::Png,
            Encoding::Glb,
            Encoding::PngStyled("basic".to_string()),
            Encoding::PngStyled("dark".to_string()),
        ];
        let keys: Vec<String> = variants
            .into_iter()
            .map(|encoding| redis_key(&key_with_encoding(encoding)))
            .collect();

        for (i, a) in keys.iter().enumerate() {
            for (j, b) in keys.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "encoding variants at the same coord must not collide");
                }
            }
        }
    }
}
