//! Write-reactive tile-cache invalidation (`#113`): a config-gated consumer
//! that drains the same outbox the write path already commits to (`#25`,
//! `crate::outbox`) and advances per-collection, per-bucket *generations* —
//! never key deletion, never an enumeration sweep over the tile pyramid.
//!
//! ## Why a generation IS an outbox sequence
//!
//! A bucket's generation is exactly the highest primary [`Sequence`] of any
//! drained obligation whose bbox touched it — not an independent counter.
//! This is what makes cross-instance/L2 staleness a declared number instead
//! of an invisible one: two server instances that have each independently
//! drained a collection's outbox up to sequence 100 agree on generation 100
//! for the same bucket with no coordination between them, because the
//! *value* is derived from the one shared, ordered log both are reading,
//! not from an arbitrary local counter. It also makes a restart harmless —
//! a fresh consumer starts back at sequence 0 and replays forward; any
//! leftover L2 entry keyed under a higher generation from before the
//! restart simply stays unreachable until the consumer earns its way back
//! up to that same sequence, at which point it is legitimately fresh again.
//!
//! ## Coalescing
//!
//! [`drain_once_for_generations`] applies one whole drained batch as a
//! single unit: every bucket a batch's obligations touch is bumped to that
//! batch's own highest sequence exactly once, never once per feature. A
//! bulk ingest of thousands of rows in one drained chunk still produces at
//! most `bucket_count` bumps, never `feature_count` ones.
//!
//! ## Coarse spatial bucketing
//!
//! [`BucketGrid`] is a fixed, shallow tile-matrix zoom
//! (`TileInvalidationConfig::bucket_zoom`, default `4` — `256` buckets,
//! fixed regardless of collection size or write volume). A write bumps only
//! the buckets its OLD and NEW extents intersect — both, so a feature that
//! moves invalidates the tile it left as well as the one it arrived in, and a
//! delete invalidates the tile it vanished from ([`GenerationStore::
//! apply_batch`]'s own doc has the exact rule). A pyramid tile's own
//! generation is its ancestor bucket's
//! generation (or the max over the covering range, for an overview tile
//! shallower than the grid itself) — see [`BucketGrid::buckets_for_tile`].
//!
//! ## Where a bucket-mapping bbox comes from — and where it must not
//!
//! Every bbox this module maps to buckets is one the STORAGE recorded, in
//! CRS84, inside the same transaction as the mutation:
//! [`Obligation::extent`](crate::outbox::Obligation::extent). Nothing here
//! reads the obligation's own `Upsert` payload geometry, and that is
//! deliberate (`#142`).
//!
//! A payload is the client-submitted feature verbatim, in whatever CRS that
//! client declared on `Content-Crs` — CRS84 by default, but equally the
//! collection's own storage CRS, which may be projected metres or merely
//! authority-ordered latitude-before-longitude (EPSG:4326). The payload
//! carries no record of which. Reading it as CRS84 is therefore a guess, and
//! this consumer used to make it: a write submitted in EPSG:3857 mapped
//! metres onto the lon/lat grid, clamped to the antimeridian, and bumped a
//! bucket on the far side of the world from the feature — leaving the tile
//! that actually renders it on its old generation, serving the pre-write
//! rendering with a `200`, indefinitely. Silence, not an error. So the guess
//! is gone: this module has no CRS reasoning left in it at all, and cannot
//! reacquire one without a producer changing what it records.
//!
//! [`ObligationExtent::Unrecorded`] — an outbox row written before the
//! extent column existed, or a driver that cannot express its storage CRS in
//! CRS84 — is treated as *unknown*, never as *empty*: the batch falls back to
//! a conservative whole-collection `floor` bump (a superset: every tile of
//! the collection re-renders once). That costs re-renders, bounded by the
//! drain cadence; the alternative, inventing a bbox, costs correctness with
//! no way for anyone to notice. The fallback is counted
//! (`tile_invalidation_unrecorded_extent_total`) so an operator reads the
//! degradation off a gauge rather than inferring it.
//!
//! ## The "old bbox" problem, and how `#141` retired it
//!
//! A `Delete` obligation's payload is a literal `NULL` — no geometry at all —
//! and by the time this consumer drains it the row is already gone from the
//! primary table, so there is nothing left to query. An `Upsert` payload
//! carries only the NEW geometry, never the prior one. Until `#141` this
//! consumer compensated with its own bounded per-collection cache of "this
//! feature's last known bbox", populated by observing the upserts it had
//! itself drained; a delete (or a move) on a feature it had no memory of — a
//! fresh restart, or eviction — fell back to the whole-grid bump.
//!
//! That memory is gone, because the write path now records the prior extent
//! at the source ([`ObligationExtent::Crs84`]'s `prior`), read in the same
//! transaction that performs the mutation. A restart no longer degrades
//! anything, an eviction cannot happen, and — the case the cache could never
//! have covered honestly — a remembered bbox is no longer a bbox this
//! consumer had to guess the CRS of on the way in.
//!
//! ## Layering note
//!
//! This crate has no geometry or projection dependency (`Cargo.toml`), and
//! cannot depend on `tellurion-tiles` (the reverse dependency direction).
//! The small amount of spherical Web Mercator math this module needs is
//! therefore a self-contained reimplementation of the same standard
//! slippy-map tiling formulas `tellurion-tiles::mercator` already has for
//! the rendering lane — duplicated, not shared, and it stays that way. The
//! CRS84 extents it consumes are produced by the storages, which DO have
//! projection machinery (PostGIS's `ST_Transform`, the GeoPackage driver's
//! own spherical Web Mercator inverse) — this module never acquires one.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::config::CollectionDecl;
use crate::error::Result;
use crate::outbox::{Obligation, ObligationExtent, OutboxSource, Sequence};

const EARTH_RADIUS_M: f64 = 6_378_137.0;
const WEB_MERCATOR_ORIGIN: f64 = EARTH_RADIUS_M * std::f64::consts::PI;
/// Standard Web Mercator latitude clamp (the projection is undefined at the
/// poles) — the same bound EPSG:3857 and every WebMercatorQuad-compatible
/// tile scheme use.
const MAX_LATITUDE_DEG: f64 = 85.051_128_78;

fn lon_to_mercator_x(lon_deg: f64) -> f64 {
    EARTH_RADIUS_M * lon_deg.to_radians()
}

fn lat_to_mercator_y(lat_deg: f64) -> f64 {
    let lat = lat_deg
        .clamp(-MAX_LATITUDE_DEG, MAX_LATITUDE_DEG)
        .to_radians();
    EARTH_RADIUS_M * (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln()
}

fn matrix_side(zoom: u8) -> u32 {
    1u32 << zoom
}

fn tile_size_m(zoom: u8) -> f64 {
    2.0 * WEB_MERCATOR_ORIGIN / f64::from(matrix_side(zoom))
}

fn clamp_index(raw: f64, max_index: u32) -> u32 {
    if !raw.is_finite() || raw <= 0.0 {
        0
    } else if raw >= f64::from(max_index) {
        max_index
    } else {
        raw as u32
    }
}

/// Inclusive tile column/row range, at `zoom`, whose tiles intersect
/// `bbox_m` (`[minx, miny, maxx, maxy]`, Web Mercator meters) — every index
/// clamped to `[0, matrixSide - 1]`, row 0 at the north edge (the same XYZ
/// convention every tile coordinate in this workspace uses).
fn covering_tile_range(bbox_m: [f64; 4], zoom: u8) -> (u32, u32, u32, u32) {
    let [minx, miny, maxx, maxy] = bbox_m;
    let max_index = matrix_side(zoom) - 1;
    let size = tile_size_m(zoom);

    let col_for = |x: f64| clamp_index(((x + WEB_MERCATOR_ORIGIN) / size).floor(), max_index);
    let row_for = |y: f64| clamp_index(((WEB_MERCATOR_ORIGIN - y) / size).floor(), max_index);

    let min_col = col_for(minx);
    let max_col = col_for(maxx);
    // Mercator meters increase northward; rows increase southward, so the
    // north (top, larger Y) edge maps to the SMALLER row index.
    let min_row = row_for(maxy);
    let max_row = row_for(miny);
    (min_col, max_col, min_row, max_row)
}

/// The fixed, shallow-zoom tile grid the coarse spatial bucketing refinement
/// bounds itself with — see the module doc.
struct BucketGrid {
    zoom: u8,
}

impl BucketGrid {
    fn new(zoom: u8) -> Self {
        Self { zoom }
    }

    fn buckets_for_bbox_mercator(&self, bbox_m: [f64; 4]) -> Vec<(u32, u32)> {
        let (min_col, max_col, min_row, max_row) = covering_tile_range(bbox_m, self.zoom);
        let mut buckets = Vec::new();
        for row in min_row..=max_row {
            for col in min_col..=max_col {
                buckets.push((col, row));
            }
        }
        buckets
    }

    fn buckets_for_bbox_lonlat(&self, bbox: [f64; 4]) -> Vec<(u32, u32)> {
        let [minx, miny, maxx, maxy] = bbox;
        let bbox_m = [
            lon_to_mercator_x(minx),
            lat_to_mercator_y(miny),
            lon_to_mercator_x(maxx),
            lat_to_mercator_y(maxy),
        ];
        self.buckets_for_bbox_mercator(bbox_m)
    }

    /// The bucket(s) a pyramid tile `(z, x, y)` maps to: exactly one
    /// ancestor bucket when `z >= self.zoom` (the common case for every
    /// real MVT/PNG/Glb request), or the full covering range of buckets
    /// when `z < self.zoom` (an overview tile shallower than the bucket
    /// grid itself) — bounded by `4^(self.zoom - z)`, at most `4^self.zoom`
    /// (the whole grid), never by anything data-dependent.
    fn buckets_for_tile(&self, z: u8, x: u32, y: u32) -> Vec<(u32, u32)> {
        if z >= self.zoom {
            let shift = z - self.zoom;
            vec![(x >> shift, y >> shift)]
        } else {
            let shift = self.zoom - z;
            let side = 1u32 << shift;
            let mut buckets = Vec::with_capacity((side * side) as usize);
            for dy in 0..side {
                for dx in 0..side {
                    buckets.push((x * side + dx, y * side + dy));
                }
            }
            buckets
        }
    }
}

/// One collection's write-reactive invalidation state.
struct CollectionGen {
    /// Drain cursor — the highest primary [`Sequence`] this consumer has
    /// itself processed for this collection. The resume point for
    /// `OutboxSource::read_after` and the input to the lag metric; distinct
    /// from `floor`/`buckets` below, which only change when a batch's
    /// obligations actually force them to (a batch that only touches
    /// already-current buckets still advances the cursor).
    cursor: AtomicU64,
    /// A whole-collection invalidation floor: bumped only when an
    /// obligation's bbox is unknown (see the module doc's "old bbox
    /// problem"). A bucket's effective generation is `max(floor, its own
    /// stored value)`, so a floor bump is one O(1) write that conservatively
    /// invalidates the entire grid without enumerating or touching every
    /// bucket entry — still bounded (one atomic, not `bucket_count` writes).
    floor: AtomicU64,
    /// Per-bucket generation: the highest `Sequence` of any obligation whose
    /// bbox intersected this bucket. A bucket absent here has never been
    /// individually touched — its own contribution is `0` (only `floor` can
    /// raise its effective generation above that).
    buckets: RwLock<HashMap<(u32, u32), u64>>,
}

impl CollectionGen {
    fn new() -> Self {
        Self {
            cursor: AtomicU64::new(0),
            floor: AtomicU64::new(0),
            buckets: RwLock::new(HashMap::new()),
        }
    }

    fn generation_for_bucket(&self, bucket: (u32, u32)) -> u64 {
        let floor = self.floor.load(Ordering::Relaxed);
        let stored = self
            .buckets
            .read()
            .expect("bucket generation lock is never held across a panic")
            .get(&bucket)
            .copied()
            .unwrap_or(0);
        floor.max(stored)
    }
}

/// Per-collection generation state fed by [`run_generation_consumer`]/
/// [`drain_once_for_generations`], and read on every tile fetch to build a
/// [`crate::cache::TileKey`]'s `generation` component. See the module doc
/// for the full design.
pub struct GenerationStore {
    grid: BucketGrid,
    collections: HashMap<String, Arc<CollectionGen>>,
}

impl GenerationStore {
    /// No collections tracked — every lookup answers generation `0`, byte
    /// for byte the pre-`#113` behavior. What `AppContext` holds before the
    /// `tellurion` server binary wires a real store in, and permanently what
    /// it holds whenever `ServerConfig.tile_invalidation.enabled` is
    /// `false` (the default).
    pub fn empty() -> Self {
        Self {
            grid: BucketGrid::new(0),
            collections: HashMap::new(),
        }
    }

    /// Pre-registers one [`CollectionGen`] per id in `collection_ids`. The
    /// tracked set is fixed at construction — mirrors `applier::spawn_all`'s
    /// own "the applier set is resolved once at boot, not respun on reload"
    /// precedent (see `tellurion-server`'s consumer-spawn wiring) — so no
    /// lock is needed around `collections` itself; only each entry's own
    /// interior state ever mutates after this returns.
    pub fn new(bucket_zoom: u8, collection_ids: impl IntoIterator<Item = String>) -> Self {
        let collections = collection_ids
            .into_iter()
            .map(|id| (id, Arc::new(CollectionGen::new())))
            .collect();
        Self {
            grid: BucketGrid::new(bucket_zoom),
            collections,
        }
    }

    /// The generation to embed in a pyramid-coordinate tile's cache key —
    /// `0` for a collection this store never registered (consumer off
    /// server-wide, or this collection never opted in), which is exactly
    /// what keeps such a tile's key byte-for-byte identical to before this
    /// field existed.
    pub fn generation_for_tile(&self, collection: &str, z: u8, x: u32, y: u32) -> u64 {
        let Some(state) = self.collections.get(collection) else {
            return 0;
        };
        self.grid
            .buckets_for_tile(z, x, y)
            .into_iter()
            .map(|bucket| state.generation_for_bucket(bucket))
            .max()
            .unwrap_or(0)
    }

    /// The generation to embed in an arbitrary-window `Encoding::Map`
    /// (`#86`) tile's cache key — same shape as
    /// [`generation_for_tile`](Self::generation_for_tile), over a Web
    /// Mercator meters bbox (the same normalization `tellurion-tiles::maps`
    /// already resolves a request's `bbox`/`bbox-crs` into) rather than a
    /// pyramid coordinate.
    pub fn generation_for_bbox_mercator(&self, collection: &str, bbox_m: [f64; 4]) -> u64 {
        let Some(state) = self.collections.get(collection) else {
            return 0;
        };
        self.grid
            .buckets_for_bbox_mercator(bbox_m)
            .into_iter()
            .map(|bucket| state.generation_for_bucket(bucket))
            .max()
            .unwrap_or(0)
    }

    /// The whole-collection fallback generation (`#190`): the highest
    /// drained write sequence for `collection`, regardless of where the
    /// write landed — `0` for an untracked collection, same as
    /// [`generation_for_tile`](Self::generation_for_tile). Serves the
    /// `WorldCRS84Quad` tile lane, whose grid indices are NOT mercator
    /// bucket indices: [`BucketGrid`] (and the `#142` write-bbox mapping
    /// feeding it) is WebMercator-based, so a per-bucket lookup keyed by
    /// CRS84 `z`/`x`/`y` would resolve the WRONG buckets and could miss a
    /// write that genuinely touched the tile. Correctness first: any
    /// drained write advances this value, so every WorldCRS84Quad tile of
    /// the collection re-renders after any write — deliberate whole-
    /// collection over-invalidation (bounded by the same drain cadence the
    /// buckets share), never a stale tile. Per-bucket precision for the
    /// CRS84 grid stays future work: `#142` made the *input* bbox
    /// trustworthy (it is now a storage-recorded CRS84 extent, not a guess),
    /// which is a precondition for that work but not the work itself — the
    /// `WorldCRS84Quad` grid's own `2^(z+1) x 2^z` indices
    /// ([`crate::tms::TileMatrixSet::matrix_width`]) still do not line up
    /// with this store's square Web Mercator buckets, and inventing a
    /// mapping between them is exactly the kind of open-coded arithmetic
    /// `#190` closed off.
    pub fn generation_for_collection(&self, collection: &str) -> u64 {
        self.collections
            .get(collection)
            .map(|state| state.cursor.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// This consumer's own drain cursor for `collection` — `Sequence(0)` for
    /// a collection this store never registered, or one that has not been
    /// drained yet.
    pub fn cursor(&self, collection: &str) -> Sequence {
        self.collections
            .get(collection)
            .map(|state| Sequence(state.cursor.load(Ordering::Relaxed)))
            .unwrap_or(Sequence(0))
    }

    /// `#113` box 4: this collection's staleness relative to the primary
    /// outbox's own `primary_high_water` — `None` for a collection this
    /// store never registered, matching the design doc's "staleness is
    /// declared, never faked" stance: an untracked collection's lag isn't
    /// zero, it is unmeasured.
    pub fn lag(&self, collection: &str, primary_high_water: Sequence) -> Option<u64> {
        self.collections.get(collection).map(|state| {
            primary_high_water
                .0
                .saturating_sub(state.cursor.load(Ordering::Relaxed))
        })
    }

    /// Applies one whole drained batch (`#113`'s coalescing requirement:
    /// the generation advances per batch, never per feature) to
    /// `collection`'s state. Returns whether any bucket or the collection's
    /// own floor actually changed value — used only to decide whether to
    /// count a bump event; irrelevant to correctness.
    ///
    /// Every obligation is read through
    /// [`Obligation::extent`](crate::outbox::Obligation::extent) alone —
    /// never through its payload geometry, never through a remembered bbox,
    /// and never through anything that would require knowing which CRS a
    /// write arrived in (see this module's own doc). An obligation's `prior`
    /// and `current` extents are BOTH bumped: `prior` is the tile the
    /// feature is disappearing from (a delete, or the far end of a move),
    /// `current` the tile it is appearing in. `Crs84 { prior: None, current:
    /// None }` — a feature with no geometry before or after — genuinely
    /// touches nothing, and correctly bumps nothing.
    ///
    /// A single [`ObligationExtent::Unrecorded`] anywhere in the batch makes
    /// the whole batch bump `floor` instead: the conservative superset. It
    /// does not suppress the precise bumps from the batch's other
    /// obligations — those are still correct, and `floor` only ever raises
    /// an effective generation.
    ///
    /// `obligations` MUST be the ascending-order slice
    /// `OutboxSource::read_after` returns; order does not affect the bucket
    /// SET (a batch coalesces to one bump at its own maximum sequence
    /// regardless), only the sequence value that lands there.
    fn apply_batch(&self, collection: &str, obligations: &[Obligation]) -> bool {
        let Some(state) = self.collections.get(collection) else {
            return false;
        };
        let Some(max_sequence) = obligations.iter().map(|o| o.sequence.0).max() else {
            return false;
        };

        let mut touched: HashSet<(u32, u32)> = HashSet::new();
        let mut whole_grid = false;
        let mut unrecorded = 0u64;

        for obligation in obligations {
            match obligation.extent {
                ObligationExtent::Unrecorded => {
                    whole_grid = true;
                    unrecorded += 1;
                }
                ObligationExtent::Crs84 { prior, current } => {
                    for bbox in [prior, current].into_iter().flatten() {
                        touched.extend(self.grid.buckets_for_bbox_lonlat(bbox));
                    }
                }
            }
        }

        if unrecorded > 0 {
            metrics::counter!(
                "tile_invalidation_unrecorded_extent_total",
                "collection" => collection.to_string()
            )
            .increment(unrecorded);
        }

        let mut changed = false;
        if whole_grid {
            let previous = state.floor.fetch_max(max_sequence, Ordering::Relaxed);
            changed |= previous < max_sequence;
        }
        if !touched.is_empty() {
            let mut buckets = state
                .buckets
                .write()
                .expect("bucket generation lock is never held across a panic");
            for bucket in touched {
                let entry = buckets.entry(bucket).or_insert(0);
                if *entry < max_sequence {
                    *entry = max_sequence;
                    changed = true;
                }
            }
        }
        state.cursor.store(max_sequence, Ordering::Relaxed);
        changed
    }
}

/// One drain pass: reads `store`'s own cursor for `collection` as the
/// resume point, pulls at most `batch_size` obligations strictly after it,
/// and applies the whole batch to `store` as one coalesced unit (see
/// [`GenerationStore::apply_batch`]). Returns how many obligations were
/// read (`0` means caught up — nothing new since the last pass). Mirrors
/// `crate::applier::drain_once`'s own shape, over `GenerationStore` instead
/// of an `IndexSink`.
pub async fn drain_once_for_generations(
    outbox: &dyn OutboxSource,
    store: &GenerationStore,
    collection: &CollectionDecl,
    batch_size: u32,
) -> Result<usize> {
    let cursor = store.cursor(&collection.id);
    let obligations = outbox.read_after(collection, cursor, batch_size).await?;
    if obligations.is_empty() {
        return Ok(0);
    }
    let changed = store.apply_batch(&collection.id, &obligations);
    if changed {
        metrics::counter!("tile_invalidation_bumps_total", "collection" => collection.id.clone())
            .increment(1);
    }
    Ok(obligations.len())
}

/// Runs [`drain_once_for_generations`] on a fixed `poll_interval` until
/// `shutdown` reports `true`, then returns — the background-task shape
/// `tellurion-server`'s config-gated consumer wiring spawns one of per
/// opted-in, outbox-resolvable collection. Also emits the `#113` box-4 lag
/// gauge (`tile_invalidation_generation_lag`, labeled `collection`) after
/// every pass that can resolve the primary's own high-water mark. Mirrors
/// `crate::applier::run_applier`'s own shape and failure handling: a failed
/// pass is logged and retried on the next tick, never a reason to stop.
pub async fn run_generation_consumer(
    outbox: Arc<dyn OutboxSource>,
    store: Arc<GenerationStore>,
    collection: CollectionDecl,
    batch_size: u32,
    poll_interval: Duration,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        match drain_once_for_generations(outbox.as_ref(), store.as_ref(), &collection, batch_size)
            .await
        {
            Ok(_) => {
                if let Ok(high_water) = outbox.primary_high_water(&collection).await {
                    if let Some(lag) = store.lag(&collection.id, high_water) {
                        metrics::gauge!(
                            "tile_invalidation_generation_lag",
                            "collection" => collection.id.clone()
                        )
                        .set(lag as f64);
                    }
                }
            }
            Err(error) => {
                tracing::error!(
                    collection = %collection.id,
                    %error,
                    "tile-generation invalidation pass failed; resuming from the last durable cursor on the next tick"
                );
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {}
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::error::Error;
    use crate::outbox::MutationKind;

    fn collection() -> CollectionDecl {
        serde_yaml::from_str(
            r#"
id: demo
catalog: default
storage: main
table: demo
geometry: geom
pk: id
"#,
        )
        .unwrap()
    }

    /// An upsert whose storage recorded a CRS84 extent: the feature was
    /// nowhere before (a fresh insert) and is at `(lon, lat)` now. The
    /// payload geometry is deliberately given the SAME coordinates, so that
    /// every assertion below still passes if a reader assumes the payload
    /// drives bucketing — `an_upsert_is_bucketed_by_its_recorded_extent_not_its_payload`
    /// is the one test that pulls the two apart on purpose.
    fn upsert_at(sequence: u64, feature_id: &str, lon: f64, lat: f64) -> Obligation {
        let mut obligation = payload_only_upsert_at(sequence, feature_id, lon, lat);
        obligation.extent = ObligationExtent::Crs84 {
            prior: None,
            current: Some([lon, lat, lon, lat]),
        };
        obligation
    }

    /// The same upsert as an outbox row written BEFORE the extent column
    /// existed carries it: a payload, and no recorded extent at all.
    fn payload_only_upsert_at(sequence: u64, feature_id: &str, lon: f64, lat: f64) -> Obligation {
        Obligation {
            sequence: Sequence(sequence),
            feature_id: feature_id.to_string(),
            kind: MutationKind::Upsert(json!({
                "type": "Feature",
                "geometry": {"type": "Point", "coordinates": [lon, lat]},
                "properties": {}
            })),
            version: Sequence(sequence),
            committed_at: std::time::SystemTime::UNIX_EPOCH,
            extent: ObligationExtent::Unrecorded,
        }
    }

    /// A delete whose storage recorded where the feature used to be
    /// (`#141`) — the whole point of that issue: the obligation itself
    /// still carries no geometry.
    fn delete_at(sequence: u64, feature_id: &str, lon: f64, lat: f64) -> Obligation {
        let mut obligation = payload_only_delete_at(sequence, feature_id);
        obligation.extent = ObligationExtent::Crs84 {
            prior: Some([lon, lat, lon, lat]),
            current: None,
        };
        obligation
    }

    /// A delete as a pre-`#141` outbox row carries it: no geometry, no
    /// extent, nothing at all to scope an invalidation by.
    fn payload_only_delete_at(sequence: u64, feature_id: &str) -> Obligation {
        Obligation {
            sequence: Sequence(sequence),
            feature_id: feature_id.to_string(),
            kind: MutationKind::Delete,
            version: Sequence(sequence),
            committed_at: std::time::SystemTime::UNIX_EPOCH,
            extent: ObligationExtent::Unrecorded,
        }
    }

    // ---- BucketGrid -----------------------------------------------------

    #[test]
    fn a_tile_at_or_below_the_bucket_zoom_maps_to_exactly_one_ancestor() {
        let grid = BucketGrid::new(4);
        // z == bucket_zoom: identity.
        assert_eq!(grid.buckets_for_tile(4, 3, 5), vec![(3, 5)]);
        // z > bucket_zoom: shifted down.
        assert_eq!(grid.buckets_for_tile(6, 12, 20), vec![(3, 5)]);
    }

    #[test]
    fn an_overview_tile_shallower_than_the_grid_covers_a_bounded_range() {
        let grid = BucketGrid::new(4);
        // z=0 is the whole world: every bucket in the 16x16 grid.
        let buckets = grid.buckets_for_tile(0, 0, 0);
        assert_eq!(buckets.len(), 256);
    }

    #[test]
    fn buckets_for_bbox_is_bounded_by_the_whole_grid() {
        let grid = BucketGrid::new(4);
        // The entire world in lon/lat must never yield more than the fixed
        // 16x16 grid, regardless of how large the bbox is.
        let buckets = grid.buckets_for_bbox_lonlat([-180.0, -85.0, 180.0, 85.0]);
        assert!(buckets.len() <= 256, "got {} buckets", buckets.len());
    }

    // ---- GenerationStore ------------------------------------------------

    #[test]
    fn an_untracked_collection_always_answers_generation_zero() {
        let store = GenerationStore::empty();
        assert_eq!(store.generation_for_tile("demo", 10, 0, 0), 0);
        assert_eq!(store.cursor("demo"), Sequence(0));
        assert_eq!(store.lag("demo", Sequence(100)), None);
    }

    /// (a) a write makes the next tile fetch re-render without waiting out
    /// the TTL: the bucket a fresh upsert's geometry falls in advances to a
    /// new, higher generation the moment the batch is applied — a
    /// subsequent tile fetch at that coordinate builds a DIFFERENT cache
    /// key (a fresh miss) rather than reusing the stale entry until its TTL
    /// expires.
    #[test]
    fn a_write_bumps_the_generation_for_the_tile_it_lands_in() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        assert_eq!(store.generation_for_tile("demo", 10, 0, 0), 0);

        let obligations = vec![upsert_at(1, "f1", 10.0, 45.0)];
        let changed = store.apply_batch("demo", &obligations);
        assert!(changed);

        // The tile pyramid coordinate at z=10 covering lon=10/lat=45 must
        // now report a non-zero generation.
        let (z, x, y) = lonlat_to_tile(10.0, 45.0, 10);
        assert!(store.generation_for_tile("demo", z, x, y) > 0);
    }

    /// (b) a delete invalidates the tile that contained the old geometry —
    /// `#141`: the delete obligation still carries no geometry of its own,
    /// but its storage recorded the prior CRS84 extent in the same
    /// transaction that removed the row, so the SAME bucket bumps again.
    #[test]
    fn a_delete_bumps_the_bucket_that_held_the_old_geometry() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        let (z, x, y) = lonlat_to_tile(10.0, 45.0, 10);

        store.apply_batch("demo", &[upsert_at(1, "f1", 10.0, 45.0)]);
        let after_upsert = store.generation_for_tile("demo", z, x, y);
        assert!(after_upsert > 0);

        let changed = store.apply_batch("demo", &[delete_at(2, "f1", 10.0, 45.0)]);
        assert!(changed, "the delete must bump the prior-extent bucket");
        let after_delete = store.generation_for_tile("demo", z, x, y);
        assert!(after_delete > after_upsert);
    }

    /// (c) steady writes to one region do not evict/bump the whole
    /// collection's cache: repeated upserts confined to one bucket must
    /// never raise the generation of an unrelated, far-away bucket.
    #[test]
    fn steady_writes_to_one_region_never_touch_a_distant_bucket() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        let (hot_z, hot_x, hot_y) = lonlat_to_tile(10.0, 45.0, 10);
        let (far_z, far_x, far_y) = lonlat_to_tile(-170.0, -80.0, 10);

        for sequence in 1..=20u64 {
            let mut obligation = upsert_at(sequence, "hot", 10.0 + 0.0001 * sequence as f64, 45.0);
            // An update in place: the feature was already where it is going.
            if let ObligationExtent::Crs84 { prior, current } = &mut obligation.extent {
                *prior = *current;
            }
            store.apply_batch("demo", &[obligation]);
        }

        assert!(store.generation_for_tile("demo", hot_z, hot_x, hot_y) > 0);
        assert_eq!(
            store.generation_for_tile("demo", far_z, far_x, far_y),
            0,
            "a distant bucket must stay untouched by writes confined to another region"
        );
    }

    /// (d) consumer off reproduces today's TTL behavior exactly: an empty
    /// store (what `AppContext` holds whenever `ServerConfig.
    /// tile_invalidation.enabled` is false, or a collection never opted in)
    /// answers generation `0` for every coordinate, unconditionally —
    /// proven directly above in
    /// `an_untracked_collection_always_answers_generation_zero`, and here
    /// again after obligations exist upstream but were never drained into
    /// an untracked store, showing the answer stays `0` regardless of
    /// primary activity.
    #[test]
    fn an_untracked_collection_stays_at_generation_zero_regardless_of_coordinate() {
        let store = GenerationStore::empty();
        for (z, x, y) in [(0, 0, 0), (10, 500, 500), (20, 1_000_000, 1_000_000)] {
            assert_eq!(store.generation_for_tile("demo", z, x, y), 0);
        }
    }

    /// (e) the generation-lag gauge moves: `lag` reflects the gap between a
    /// declared primary high-water and the store's own drained cursor, and
    /// changes as the cursor advances.
    #[test]
    fn lag_shrinks_as_the_cursor_advances_toward_the_primary_high_water() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        assert_eq!(store.lag("demo", Sequence(10)), Some(10));

        store.apply_batch(
            "demo",
            &[upsert_at(1, "f1", 0.0, 0.0), upsert_at(2, "f1", 0.0, 0.0)],
        );
        assert_eq!(store.lag("demo", Sequence(10)), Some(8));

        store.apply_batch("demo", &[upsert_at(10, "f1", 0.0, 0.0)]);
        assert_eq!(store.lag("demo", Sequence(10)), Some(0));
    }

    #[test]
    fn coalescing_a_batch_bumps_a_bucket_at_most_once_to_the_batch_max() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        let obligations = vec![
            upsert_at(1, "a", 10.0, 45.0),
            upsert_at(2, "b", 10.001, 45.001),
            upsert_at(3, "c", 10.002, 45.002),
        ];
        store.apply_batch("demo", &obligations);
        let (z, x, y) = lonlat_to_tile(10.0, 45.0, 10);
        // The whole batch coalesces to ONE bump at the batch's own max
        // sequence, not three separate ones.
        assert_eq!(store.generation_for_tile("demo", z, x, y), 3);
    }

    /// `#190`: the whole-collection fallback the WorldCRS84Quad tile lane
    /// keys by — ANY drained write advances it (even one confined to a
    /// single mercator bucket), and an untracked collection stays at `0`,
    /// the same byte-identical-when-off guarantee `generation_for_tile`
    /// gives.
    #[test]
    fn any_write_advances_the_whole_collection_generation() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        assert_eq!(store.generation_for_collection("demo"), 0);
        assert_eq!(store.generation_for_collection("untracked"), 0);

        store.apply_batch("demo", &[upsert_at(3, "f1", 10.0, 45.0)]);
        assert_eq!(
            store.generation_for_collection("demo"),
            3,
            "a write anywhere must advance the whole-collection generation"
        );
        assert_eq!(store.generation_for_collection("untracked"), 0);
    }

    /// `#141`/`#142`: an outbox row written before the extent column existed
    /// carries no answer at all — and this consumer must read that as
    /// UNKNOWN (conservative whole-grid bump), never as "nothing moved".
    #[test]
    fn a_delete_with_no_recorded_extent_falls_back_to_a_whole_grid_bump() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        let (far_z, far_x, far_y) = lonlat_to_tile(-170.0, -80.0, 10);

        store.apply_batch("demo", &[payload_only_delete_at(1, "unknown")]);

        assert!(
            store.generation_for_tile("demo", far_z, far_x, far_y) > 0,
            "an unrecorded-extent delete must conservatively bump the whole grid"
        );
    }

    /// `#142`, stated as a property rather than as an anecdote: an
    /// obligation is bucketed by the CRS84 extent its STORAGE recorded, and
    /// the payload geometry is not consulted at all.
    ///
    /// The payload here carries EPSG:3857 metres for a point near Rome —
    /// exactly what a `Content-Crs`-declared write against a projected
    /// collection puts in the outbox verbatim. Read as lon/lat those numbers
    /// clamp to the antimeridian and the Web Mercator latitude limit, so the
    /// pre-`#142` consumer bumped a bucket in the far south-east corner of
    /// the grid while leaving Rome's own bucket — the one whose tiles
    /// actually render this feature — untouched. Both halves are asserted:
    /// the right bucket moves, and the wrong one does not.
    #[test]
    fn an_upsert_is_bucketed_by_its_recorded_extent_not_its_payload() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        // 12.49E, 41.90N in EPSG:3857 metres.
        let (mx, my) = (1_390_331.0, 5_146_501.0);
        let mut obligation = payload_only_upsert_at(1, "f1", mx, my);
        obligation.extent = ObligationExtent::Crs84 {
            prior: None,
            current: Some([12.49, 41.90, 12.49, 41.90]),
        };

        store.apply_batch("demo", &[obligation]);

        let (rz, rx, ry) = lonlat_to_tile(12.49, 41.90, 10);
        assert_eq!(
            store.generation_for_tile("demo", rz, rx, ry),
            1,
            "the bucket that renders this feature must advance"
        );

        // Where reading the payload's metres as degrees would have landed:
        // clamped to the far corner of the world.
        let (wz, wx, wy) = lonlat_to_tile(180.0, -85.0, 10);
        assert_eq!(
            store.generation_for_tile("demo", wz, wx, wy),
            0,
            "the bucket a CRS-blind payload read would have bumped must stay untouched"
        );
    }

    /// `#141`'s "update to a feature it has no bbox memory of" half: a
    /// feature that MOVES must invalidate the bucket it left as well as the
    /// one it arrived in. Nothing in this store remembers where it was —
    /// the storage recorded it.
    #[test]
    fn a_move_bumps_both_the_bucket_left_and_the_bucket_arrived_in() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        let (fz, fx, fy) = lonlat_to_tile(12.49, 41.90, 10);
        let (tz, tx, ty) = lonlat_to_tile(-74.0, 40.7, 10);

        let mut obligation = payload_only_upsert_at(1, "f1", -74.0, 40.7);
        obligation.extent = ObligationExtent::Crs84 {
            prior: Some([12.49, 41.90, 12.49, 41.90]),
            current: Some([-74.0, 40.7, -74.0, 40.7]),
        };
        store.apply_batch("demo", &[obligation]);

        assert_eq!(
            store.generation_for_tile("demo", fz, fx, fy),
            1,
            "the bucket the feature LEFT must be invalidated"
        );
        assert_eq!(
            store.generation_for_tile("demo", tz, tx, ty),
            1,
            "the bucket the feature ARRIVED in must be invalidated"
        );
    }

    /// A recorded extent that is empty on both sides is an ANSWER ("this
    /// feature has no geometry, and had none"), not an unknown: it bumps
    /// nothing at all, including the floor.
    #[test]
    fn a_recorded_but_geometry_less_mutation_bumps_nothing() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        let mut obligation = payload_only_upsert_at(1, "f1", 0.0, 0.0);
        obligation.kind = MutationKind::Upsert(json!({
            "type": "Feature", "geometry": null, "properties": {}
        }));
        obligation.extent = ObligationExtent::Crs84 {
            prior: None,
            current: None,
        };

        assert!(!store.apply_batch("demo", &[obligation]));
        for (z, x, y) in [(0, 0, 0), (10, 500, 500)] {
            assert_eq!(store.generation_for_tile("demo", z, x, y), 0);
        }
        assert_eq!(
            store.cursor("demo"),
            Sequence(1),
            "the drain cursor still advances past it"
        );
    }

    /// One unrecorded obligation does not suppress the precise bumps its
    /// batch-mates earned: the floor rises AND the recorded buckets do.
    #[test]
    fn a_mixed_batch_bumps_both_the_floor_and_the_recorded_buckets() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        let (z, x, y) = lonlat_to_tile(10.0, 45.0, 10);
        let (fz, fx, fy) = lonlat_to_tile(-170.0, -80.0, 10);

        store.apply_batch(
            "demo",
            &[
                upsert_at(1, "known", 10.0, 45.0),
                payload_only_delete_at(2, "legacy"),
            ],
        );

        assert_eq!(store.generation_for_tile("demo", z, x, y), 2);
        assert_eq!(store.generation_for_tile("demo", fz, fx, fy), 2);
    }

    /// Small helper: the pyramid tile coordinate, at `zoom`, whose
    /// `WebMercatorQuad` cell covers `(lon, lat)` — used only to build test
    /// assertions against real coordinates, not part of the module's own
    /// public surface.
    fn lonlat_to_tile(lon: f64, lat: f64, zoom: u8) -> (u8, u32, u32) {
        let bbox_m = [
            lon_to_mercator_x(lon),
            lat_to_mercator_y(lat),
            lon_to_mercator_x(lon),
            lat_to_mercator_y(lat),
        ];
        let (col, _, row, _) = covering_tile_range(bbox_m, zoom);
        (zoom, col, row)
    }

    // ---- drain_once_for_generations / run_generation_consumer -----------

    struct FakeOutbox {
        obligations: Mutex<Vec<Obligation>>,
        high_water: Mutex<Sequence>,
        fail_next: Mutex<bool>,
    }

    impl FakeOutbox {
        fn new(obligations: Vec<Obligation>) -> Self {
            let high_water = obligations
                .last()
                .map(|o| o.sequence)
                .unwrap_or(Sequence(0));
            Self {
                obligations: Mutex::new(obligations),
                high_water: Mutex::new(high_water),
                fail_next: Mutex::new(false),
            }
        }
    }

    #[async_trait]
    impl OutboxSource for FakeOutbox {
        async fn read_after(
            &self,
            _collection: &CollectionDecl,
            after: Sequence,
            limit: u32,
        ) -> Result<Vec<Obligation>> {
            if std::mem::take(&mut *self.fail_next.lock().unwrap()) {
                return Err(Error::Storage(Box::new(std::io::Error::other("boom"))));
            }
            Ok(self
                .obligations
                .lock()
                .unwrap()
                .iter()
                .filter(|o| o.sequence > after)
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn primary_high_water(&self, _collection: &CollectionDecl) -> Result<Sequence> {
            Ok(*self.high_water.lock().unwrap())
        }
    }

    #[tokio::test]
    async fn drain_once_reads_nothing_new_when_already_caught_up() {
        let outbox = FakeOutbox::new(vec![upsert_at(1, "a", 0.0, 0.0)]);
        let store = GenerationStore::new(4, ["demo".to_string()]);
        let applied = drain_once_for_generations(&outbox, &store, &collection(), 100)
            .await
            .unwrap();
        assert_eq!(applied, 1);

        let applied_again = drain_once_for_generations(&outbox, &store, &collection(), 100)
            .await
            .unwrap();
        assert_eq!(applied_again, 0, "a fully caught-up pass reads nothing new");
    }

    #[tokio::test]
    async fn drain_once_resumes_from_the_stores_own_cursor_restart_safe() {
        let outbox = FakeOutbox::new(vec![
            upsert_at(1, "a", 0.0, 0.0),
            upsert_at(2, "b", 1.0, 1.0),
            upsert_at(3, "c", 2.0, 2.0),
        ]);
        let store = GenerationStore::new(4, ["demo".to_string()]);

        let applied = drain_once_for_generations(&outbox, &store, &collection(), 2)
            .await
            .unwrap();
        assert_eq!(applied, 2);
        assert_eq!(store.cursor("demo"), Sequence(2));

        // "Restart": a fresh drain against the same store resumes past what
        // was already applied.
        let applied = drain_once_for_generations(&outbox, &store, &collection(), 100)
            .await
            .unwrap();
        assert_eq!(applied, 1);
        assert_eq!(store.cursor("demo"), Sequence(3));
    }

    /// (d) consumer off reproduces today's TTL behavior exactly, at the
    /// applier-loop level: `run_generation_consumer` never runs for a
    /// collection the caller never spawned it for — this is proven at the
    /// `tellurion-server` wiring layer (`spawn_all` returns no handles when
    /// `ServerConfig.tile_invalidation.enabled` is false), and here we prove
    /// the complementary half: a `GenerationStore` no batch was ever
    /// applied to answers exactly like `GenerationStore::empty()` for every
    /// coordinate a real request could ask about.
    #[test]
    fn a_freshly_registered_but_never_drained_collection_answers_generation_zero() {
        let store = GenerationStore::new(4, ["demo".to_string()]);
        for (z, x, y) in [(0, 0, 0), (10, 500, 500)] {
            assert_eq!(store.generation_for_tile("demo", z, x, y), 0);
        }
    }

    /// (e) the generation-lag gauge moves, exercised through the real
    /// Prometheus recorder `run_generation_consumer` writes to — proves the
    /// end-to-end wiring, not just the `lag` computation in isolation
    /// (`lag_shrinks_as_the_cursor_advances_toward_the_primary_high_water`
    /// above already covers that).
    #[tokio::test]
    async fn run_generation_consumer_emits_a_moving_lag_gauge() {
        use metrics_exporter_prometheus::PrometheusBuilder;

        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _guard = metrics::set_default_local_recorder(&recorder);

        let outbox = Arc::new(FakeOutbox::new(vec![
            upsert_at(1, "a", 0.0, 0.0),
            upsert_at(2, "b", 1.0, 1.0),
        ]));
        let store = Arc::new(GenerationStore::new(4, ["demo".to_string()]));
        let (tx, rx) = tokio::sync::watch::channel(false);

        let task = tokio::spawn(run_generation_consumer(
            outbox,
            store,
            collection(),
            10,
            Duration::from_secs(3600),
            rx,
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("run_generation_consumer should stop promptly on shutdown")
            .unwrap();

        let rendered = handle.render();
        assert!(
            rendered.contains("tile_invalidation_generation_lag{collection=\"demo\"} 0"),
            "expected the lag gauge to reach zero once fully caught up, in:\n{rendered}"
        );
    }

    #[tokio::test]
    async fn a_failed_pass_is_retried_without_advancing_the_cursor() {
        let outbox = FakeOutbox::new(vec![upsert_at(1, "a", 0.0, 0.0)]);
        *outbox.fail_next.lock().unwrap() = true;
        let store = GenerationStore::new(4, ["demo".to_string()]);

        let error = drain_once_for_generations(&outbox, &store, &collection(), 100).await;
        assert!(error.is_err());
        assert_eq!(store.cursor("demo"), Sequence(0));

        let applied = drain_once_for_generations(&outbox, &store, &collection(), 100)
            .await
            .unwrap();
        assert_eq!(applied, 1);
        assert_eq!(store.cursor("demo"), Sequence(1));
    }
}
