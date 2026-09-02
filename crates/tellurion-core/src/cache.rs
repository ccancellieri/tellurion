//! Tile cache. One cache, byte-budgeted — never entry-count sized; Mvt, Png,
//! Glb and PngStyled variants of the same tile share one identity and one
//! budget (the weigher sizes by cached bytes only, so an extra encoding
//! variant costs nothing to add). `TileCache` is async so `LayeredCache` can
//! compose an L2 behind an unchanged L1 with no caller changes. A networked
//! L2 backend implements the separate `L2Cache` trait instead of `TileCache`
//! directly — it can fail and needs a TTL, neither of which `TileCache`
//! carries — and `L2CacheAdapter` bridges it into a `LayeredCache` layer,
//! degrading a backend error to a plain miss and writing through
//! fire-and-forget so an L2 outage never adds latency to, or fails, a
//! response.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;

use crate::error::Error;

/// The two CRSs the OGC API Maps lane's `crs`/`bbox-crs` parameters accept
/// (`#86`, first slice): the `WebMercatorQuad` tile matrix's own CRS
/// (EPSG:3857, the tile grid every covering tile is fetched from) and CRS84
/// (WGS84 longitude/latitude) — the same "CRS84 plus one other" shape
/// `crate::crs::RequestedCrs` gives the features lane, fixed here to the
/// tile grid's own CRS rather than a collection's storage SRID. Lives
/// alongside [`Encoding::Map`] (part of its own cache-key entry) rather than
/// in `tellurion-tiles` so the key type never depends on a protocol crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapCrs {
    WebMercator,
    Crs84,
}

/// Which render lane produced an [`Encoding::Map`] entry's bytes (`#37`).
///
/// A map window over one collection can be rendered two entirely different
/// ways — rasterized from the MVT pyramid (`TileSource`) or composited from
/// decoded raster windows (`RasterSource`) — and the two produce different
/// images from the same `(collection, crs, bbox, width, height)`. Without
/// this discriminator in the key they would be the SAME cache entry, so
/// whichever lane rendered first would answer for the other. That is a
/// correctness property, not a nicety: every tile-shaped entry in this
/// workspace shares one byte-budgeted cache.
///
/// [`MapLane::Raster`] additionally carries the collection's resolved
/// colormap fingerprint (`crate::ColormapConf::fingerprint`), `None` when no
/// colormap is configured — exactly the partitioning [`Encoding::PngRaster`]
/// already gives the raster TILE lane, and for exactly the same reason: the
/// tile cache is deliberately not part of `AppContext`'s atomically swapped
/// reload state, so a config reload that changes a colormap would otherwise
/// keep serving the previous colormap's bytes under an unchanged key.
///
/// No style id here: a MapLibre style document paints MVT layers, so it is
/// meaningful only on the vector lane, where [`Encoding::Map`]'s own `style`
/// field already carries it — the raster lane refuses a `style` parameter by
/// name rather than ignoring it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapLane {
    Vector,
    Raster(Option<u64>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Encoding {
    Mvt,
    Png,
    Glb,
    /// PNG rendered through a MapLibre style document; the style id lives in
    /// the key so two styles over the same tile never collide in the cache.
    PngStyled(String),
    /// PNG rendered from a raster (COG) source (`#37`), carrying its
    /// collection's resolved colormap fingerprint (`#92`,
    /// `ColormapConf::fingerprint`), or `None` when no colormap is
    /// configured. Distinct from the plain `Png` variant even though both
    /// serve `image/png`: a config reload can change a collection's
    /// colormap in place (`AppContext`'s own doc — the tile cache is not
    /// part of its atomically swapped reload state), and without the
    /// fingerprint in the key the previous colormap's cached bytes would
    /// keep answering under the same key indefinitely.
    PngRaster(Option<u64>),
    /// PNG rendered for an arbitrary bbox/width/height window rather than a
    /// fixed tile-pyramid coordinate (OGC API Maps Part 1, `#86`,
    /// `/collections/{cid}/map`) — this entry's `TileKey.z`/`.x`/`.y` are
    /// unused (always `0`) for this variant, since a map request has no
    /// tile-pyramid coordinate to carry there; every parameter that changes
    /// the rendered bytes lives on this variant instead, the same "an extra
    /// encoding variant costs nothing to add" reuse `PngRaster` already
    /// established for a differently-shaped render input. `bbox` is the
    /// four corner values, in `crs`'s own units, as `f64::to_bits` (plain
    /// `f64` doesn't derive `Eq`/`Hash`; this workspace already uses the
    /// same bit-pattern convention elsewhere, e.g. `Filter::fingerprint`).
    /// `style` mirrors `PngStyled`'s own style-id partitioning: `None` for
    /// the collection's default paint, `Some(style_id)` for a resolved
    /// MapLibre style document, so two styles (or a style vs. no style) over
    /// the same window never collide. `lane` (`#37`) is the same kind of
    /// partition one level up — which of the two render lanes produced these
    /// bytes, and (for the raster one) under which colormap; see
    /// [`MapLane`]'s own doc.
    Map {
        crs: MapCrs,
        bbox: [u64; 4],
        width: u32,
        height: u32,
        style: Option<String>,
        lane: MapLane,
    },
}

impl Encoding {
    /// Stable, low-cardinality label for `/metrics`: every `PngStyled`/
    /// `PngRaster`/`Map` variant collapses to one lane regardless of style
    /// id, colormap fingerprint, or window parameters, so neither an
    /// unbounded number of style documents nor an unbounded number of
    /// distinct windows turns into an unbounded number of Prometheus series.
    fn metric_label(&self) -> &'static str {
        match self {
            Encoding::Mvt => "mvt",
            Encoding::Png => "png",
            Encoding::Glb => "glb",
            Encoding::PngStyled(_) => "png_styled",
            Encoding::PngRaster(_) => "png_raster",
            Encoding::Map { .. } => "map",
        }
    }
}

/// Every field here is an internal id (`#39`) — external ids (whatever a
/// tenant/catalog/collection is renamed to in config) never enter a cache
/// key, which is exactly what makes a rename a cache HIT under the new name:
/// the internal ids, and so the key, never change.
///
/// `policy_fingerprint` (`#34`) partitions this same key by the requesting
/// subject's effective `#34` ABAC grant filter, when one applies:
/// `Some(Filter::fingerprint())` — a stable, process-local hash of the
/// resolved, post-claim-substitution filter (see [`Filter::fingerprint`](
/// crate::filter::Filter::fingerprint)'s own doc for how that hash is
/// computed and what it deliberately does and does not guarantee).
/// `None` for a subject whose access is unrestricted — the ordinary case for
/// public/anonymous traffic, and for every deployment with no `policy:`
/// configured at all — which keeps this exactly the pre-`#34` five-field key
/// (`tenant`/`catalog`/`collection`/`z`/`x`/`y`/`encoding`) byte-for-byte:
/// two unfiltered requests for the same tile still collide into one cache
/// entry regardless of which subject made them, so public traffic never pays
/// for a per-subject cache split it doesn't need. Two subjects who resolve
/// to *structurally the same* filter (same claim-substituted CQL2, whether
/// or not they hold the same role) share a fingerprint and so share a cache
/// entry; two subjects with different effective filters get different
/// entries, never each other's rows. Never carries a raw claim or token
/// value, or anything else identifying a specific subject beyond "which
/// filter did this request's access resolve to" — only the resolved filter's
/// own hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub tenant: String,
    pub catalog: String,
    pub collection: String,
    /// Which tile matrix set `z`/`x`/`y` below are indices INTO (`#190`) —
    /// a `WorldCRS84Quad` tile and a `WebMercatorQuad` tile at the same
    /// pyramid coordinate cover different ground and encode different
    /// bytes, so the grid is part of a tile's cache identity exactly the
    /// way `encoding` already is. `WebMercatorQuad` — the only grid that
    /// existed before `#190`, and what `context::mvt_key` still defaults to
    /// — keeps every pre-existing key equal to what it always was. Fixed at
    /// `WebMercatorQuad` for the `Encoding::Map` lane, the same way that
    /// variant's own doc already declares `z`/`x`/`y` unused: a map window
    /// carries its output CRS inside the variant itself and composes from
    /// the mercator pyramid.
    pub tms: crate::tms::TileMatrixSet,
    pub z: u8,
    pub x: u32,
    pub y: u32,
    pub encoding: Encoding,
    pub policy_fingerprint: Option<u64>,
    /// This collection's resolved vector-tile property allowlist (`#85`,
    /// `settings.tile_properties` through the platform -> tenant -> catalog
    /// -> collection chain), folded into the key so a config change to the
    /// allowlist never serves an MVT tile whose attribute shape belongs to
    /// the old configuration — the same "config change never serves stale
    /// content" guarantee `policy_fingerprint` gives the per-subject filter.
    /// Empty (pk-only, the default) for every collection that never sets
    /// `tile_properties`, which keeps this exactly the pre-`#85` key for
    /// every such collection. Set once, on the `Encoding::Mvt` entry
    /// `AppContext::fetch_mvt` builds (see `context::mvt_key`'s own doc) —
    /// every other encoding (Png/PngStyled/Glb) renders from that same MVT
    /// fetch, so busting the Mvt entry alone already forces a fresh render
    /// the next time a stale rendered-tile entry's own TTL expires, the same
    /// TTL-bounded propagation every other rendering-affecting config knob
    /// (fill/stroke/simplification/caps) already relies on rather than
    /// carrying its own copy into every encoding's key.
    pub properties: Vec<String>,
    /// Write-reactive invalidation (`#113`): the coarse spatial bucket's
    /// generation a write-reactive consumer has advanced this tile's
    /// coordinate to — see `crate::invalidation`'s own module doc for what a
    /// generation IS (the outbox `Sequence` of the last drained obligation
    /// whose bbox touched this tile's bucket) and why bumping it, rather
    /// than deleting keys, is what makes a write become visible before the
    /// entry's TTL would otherwise expire it. `0` (the default) for every
    /// collection that never opts into the consumer (`ServerConfig.
    /// tile_invalidation`/`CollectionDecl.tile_invalidation` both default
    /// off) — every such collection's tiles always resolve to generation
    /// `0`, which keeps this exactly the pre-`#113` key byte-for-byte: TTL
    /// alone still bounds freshness, unchanged. Threaded through by
    /// `AppContext::fetch_mvt`/the tiles and places handlers' own `*_key`
    /// helpers, never computed by `TileKey` itself.
    pub generation: u64,
}

/// A populate call already boxed into a lazy future — constructing one costs
/// nothing until it is polled, so building it unconditionally at a call site
/// and only actually awaiting it on a cache miss is free. Boxed (rather than
/// a generic `F: Future`) so the coalescing seam below stays callable through
/// `Arc<dyn TileCache>`.
pub type PopulateFuture = Pin<Box<dyn Future<Output = Result<Bytes, Error>> + Send>>;

#[async_trait::async_trait]
pub trait TileCache: Send + Sync {
    async fn get(&self, key: &TileKey) -> Option<Bytes>;
    async fn insert(&self, key: TileKey, value: Bytes);

    /// Coalesces concurrent misses on the same key into one evaluation of
    /// `populate`: N callers racing on a key that isn't cached yet share the
    /// single upstream fetch instead of each triggering their own (the tile
    /// stampede this seam exists to prevent). A `populate` that returns `Err`
    /// is never cached, so the key is never poisoned — the next caller gets
    /// a fresh attempt.
    ///
    /// The default implementation (get, then populate, then insert) gives
    /// every `TileCache` a correct fallback with no coalescing guarantee —
    /// the right behavior for a layer that cannot provide single-flight
    /// natively (a future networked L2). Override it where real coalescing
    /// is possible; `MokaTileCache` does, via moka's own `try_get_with`.
    async fn get_or_populate(
        &self,
        key: TileKey,
        populate: PopulateFuture,
    ) -> Result<Bytes, Arc<Error>> {
        if let Some(value) = self.get(&key).await {
            return Ok(value);
        }
        let value = populate.await.map_err(Arc::new)?;
        self.insert(key, value.clone()).await;
        Ok(value)
    }

    /// [`insert`](Self::insert)'s TTL-aware counterpart: `ttl` is the
    /// collection's effective `cache_ttl_s` (`settings.rs`, `#39`). The
    /// default ignores `ttl` and calls plain `insert` — correct for any
    /// layer with no per-entry expiry concept (the in-process L1: `moka`'s
    /// budget-only eviction has no TTL knob configured at all). Only a layer
    /// that can genuinely honor a TTL — today, `L2CacheAdapter` — overrides
    /// this.
    async fn insert_with_ttl(&self, key: TileKey, value: Bytes, ttl: Duration) {
        let _ = ttl;
        self.insert(key, value).await;
    }

    /// [`get_or_populate`](Self::get_or_populate)'s TTL-aware counterpart —
    /// same coalescing contract, but writes through
    /// [`insert_with_ttl`](Self::insert_with_ttl) instead of plain
    /// `insert` so a collection's effective `cache_ttl_s` reaches whichever
    /// layer can act on it. The default here mirrors `get_or_populate`'s
    /// default (no native single-flight guarantee); `MokaTileCache` already
    /// gets real coalescing from its `get_or_populate` override, and has no
    /// TTL concept to thread through regardless.
    async fn get_or_populate_with_ttl(
        &self,
        key: TileKey,
        populate: PopulateFuture,
        ttl: Duration,
    ) -> Result<Bytes, Arc<Error>> {
        if let Some(value) = self.get(&key).await {
            return Ok(value);
        }
        let value = populate.await.map_err(Arc::new)?;
        self.insert_with_ttl(key, value.clone(), ttl).await;
        Ok(value)
    }

    /// The optional L2 tier this cache composition carries (`#161`), when
    /// the operator configured one. Default `None`, the same `Option`-shaped
    /// "this implementation never claims this capability" convention every
    /// `StorageDriver` capability accessor uses (see `crate::router`'s
    /// `feature_source`/`tile_source`/`write_sink`/... accessors).
    ///
    /// `None` means exactly one thing: **no L2 tier was configured**. It is
    /// NOT "an L2 tier that happens to be down" — a configured tier whose
    /// backend never connected is still `Some`, carrying
    /// [`L2TierState::NeverConnected`], precisely so a readiness reporter
    /// can tell "the operator asked for no cache" apart from "the cache the
    /// operator asked for is missing". Collapsing those two states into one
    /// `None` is the untruth this accessor exists to make impossible.
    fn l2_tier(&self) -> Option<Arc<L2Tier>> {
        None
    }
}

/// An optional L2 tile-cache tier as the *process* can currently describe
/// it (`#161`) — the thing [`TileCache::l2_tier`] hands a readiness
/// reporter. Deliberately not the `L2Cache` backend itself: the two states
/// below are not both backed by a live backend, and a caller that only
/// wants to answer "is the configured tier usable, and if not, which one
/// isn't?" must not be handed a read/write handle to do it.
///
/// Constructing one is an assertion that an L2 tier IS configured. A
/// deployment with no `cache.l2` never builds one at all, so
/// [`TileCache::l2_tier`] stays `None` and nothing downstream has anything
/// to report — the "absence of an unconfigured optimization is not a
/// degradation" rule, enforced by construction rather than by a default.
pub struct L2Tier {
    backend: String,
    state: L2TierState,
}

/// Which of the two ways a configured L2 tier can exist in this process.
pub enum L2TierState {
    /// The backend connected at boot and is wired into the serving cache.
    /// [`L2Tier::probe`] asks it, live, whether it is still reachable.
    Connected(Arc<dyn L2Cache>),
    /// The backend was configured but never connected at boot, so this
    /// process has no handle to probe and is serving L1-only until it is
    /// restarted (`tellurion-server`'s `build_cache` — a cache tier being
    /// down must not be the reason the server cannot start). The string is
    /// the boot-time connect error, kept so the report can say *why* rather
    /// than a bare "degraded".
    ///
    /// This state never recovers on its own: unlike a post-boot outage
    /// (whose client reconnects underneath a `Connected` tier), there is no
    /// connection here to come back. That is a truthful report of this
    /// process's capability, not a stale one.
    NeverConnected(String),
}

impl L2Tier {
    /// A configured tier whose backend connected at boot.
    pub fn connected(backend: impl Into<String>, cache: Arc<dyn L2Cache>) -> Self {
        Self {
            backend: backend.into(),
            state: L2TierState::Connected(cache),
        }
    }

    /// A configured tier whose backend never connected at boot; `reason` is
    /// the connect error, reported verbatim through the operator-facing log
    /// (never through an HTTP body — see `tellurion-server`'s `readiness`).
    pub fn never_connected(backend: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            backend: backend.into(),
            state: L2TierState::NeverConnected(reason.into()),
        }
    }

    /// The configured backend's name (`"valkey"`), as an operator would
    /// recognize it from their own `cache.l2.backend` selection. This is
    /// what makes a report a NAMED one instead of a generic "degraded".
    pub fn backend(&self) -> &str {
        &self.backend
    }

    pub fn state(&self) -> &L2TierState {
        &self.state
    }

    /// One bounded reachability check against the configured backend.
    ///
    /// A `Connected` tier is probed by *reading* [`probe_key`] — reuse of
    /// the existing `L2Cache::get` seam rather than a second backend
    /// contract to implement and keep honest. A miss (`Ok(None)`) is a
    /// success: the question is "did the backend answer", not "is anything
    /// cached". Read-only on purpose — a probe must never write to, or
    /// evict from, an operator's shared cache instance.
    ///
    /// A `NeverConnected` tier answers with its recorded boot error without
    /// any I/O, so the boot-down case reports the same shape as an outage
    /// instead of silently looking like "no cache configured".
    ///
    /// Callers put their own deadline around this (readiness does); it
    /// deliberately owns no timeout policy of its own.
    pub async fn probe(&self) -> Result<(), Error> {
        match &self.state {
            L2TierState::Connected(cache) => cache.get(&probe_key()).await.map(|_| ()),
            L2TierState::NeverConnected(reason) => Err(Error::Config(reason.clone())),
        }
    }
}

/// The key [`L2Tier::probe`] reads. Nothing in this workspace ever writes
/// it, so it is expected to miss forever; it exists only to make the probe a
/// real round trip through the same code path a tile read takes. Its
/// tenant/catalog/collection components are named to be obviously reserved,
/// but nothing depends on that being enforced: the probe only ever READS, so
/// even a deployment that somehow used these exact internal ids would see a
/// probe that answers the reachability question and disturbs nothing.
fn probe_key() -> TileKey {
    TileKey {
        tenant: "__tellurion_readiness".to_string(),
        catalog: "__tellurion_readiness".to_string(),
        collection: "__tellurion_readiness".to_string(),
        tms: crate::tms::TileMatrixSet::WebMercatorQuad,
        z: 0,
        x: 0,
        y: 0,
        encoding: Encoding::Mvt,
        policy_fingerprint: None,
        properties: Vec::new(),
        generation: 0,
    }
}

pub struct MokaTileCache {
    inner: moka::future::Cache<TileKey, Bytes>,
}

impl MokaTileCache {
    pub fn with_byte_budget(max_capacity_bytes: u64) -> Self {
        let inner = moka::future::Cache::builder()
            .max_capacity(max_capacity_bytes)
            .weigher(|_key: &TileKey, value: &Bytes| -> u32 {
                value.len().try_into().unwrap_or(u32::MAX)
            })
            .build();
        Self { inner }
    }

    /// Budget = `memory_percent`% of the detected memory limit: cgroup v2,
    /// then cgroup v1, else total system RAM — see `crate::resources`,
    /// which owns the v1-vs-v2-vs-host precedence so this and every other
    /// cgroup-aware budget in the workspace share one detection path.
    pub fn from_memory_percent(memory_percent: f64) -> Self {
        let total = crate::resources::detect_memory_limit_bytes();
        let budget = (total as f64 * (memory_percent / 100.0)) as u64;
        Self::with_byte_budget(budget.max(1))
    }

    /// Forces the async eviction pass to run so tests can assert on
    /// post-eviction state deterministically.
    #[cfg(test)]
    async fn run_pending_tasks(&self) {
        self.inner.run_pending_tasks().await;
    }

    #[cfg(test)]
    fn weighted_size(&self) -> u64 {
        self.inner.weighted_size()
    }
}

#[async_trait::async_trait]
impl TileCache for MokaTileCache {
    async fn get(&self, key: &TileKey) -> Option<Bytes> {
        self.inner.get(key).await
    }

    async fn insert(&self, key: TileKey, value: Bytes) {
        self.inner.insert(key, value).await;
    }

    /// moka's `try_get_with` already guarantees single-flight coalescing on
    /// the key (concurrent callers on a not-yet-cached key share one
    /// evaluation of `populate`) and never inserts on `Err`, which is
    /// exactly this seam's contract — so there is nothing to add here.
    async fn get_or_populate(
        &self,
        key: TileKey,
        populate: PopulateFuture,
    ) -> Result<Bytes, Arc<Error>> {
        self.inner.try_get_with(key, populate).await
    }

    /// The in-process L1 has no per-entry TTL concept (`insert_with_ttl`'s
    /// default already documents this), so this simply drops `ttl` and
    /// delegates to [`get_or_populate`](Self::get_or_populate) — still
    /// through moka's `try_get_with`, so single-flight coalescing holds
    /// exactly as it does for the non-TTL entry point.
    async fn get_or_populate_with_ttl(
        &self,
        key: TileKey,
        populate: PopulateFuture,
        _ttl: Duration,
    ) -> Result<Bytes, Arc<Error>> {
        self.get_or_populate(key, populate).await
    }
}

/// Tries each layer in order on `get` (first hit wins); writes through to
/// every layer on `insert`. An L1-only deployment is `LayeredCache::new(vec![l1])`.
pub struct LayeredCache {
    layers: Vec<Arc<dyn TileCache>>,
    /// The configured L2 tier this composition reports through
    /// [`TileCache::l2_tier`] (`#161`), or `None` when the deployment
    /// configured no L2 at all. Declared by the wiring layer rather than
    /// inferred from `layers`, because the two are genuinely independent:
    /// a tier configured but unreachable at boot contributes NO layer (the
    /// serving path stays exactly L1-only, no extra hop, no per-write task)
    /// while still being a tier this process must report by name.
    tier: Option<Arc<L2Tier>>,
}

impl LayeredCache {
    /// Layers only, no L2 tier declared — `l2_tier()` stays `None`, i.e.
    /// "this deployment configured no L2 cache". Unchanged from before
    /// `#161` for every existing caller.
    pub fn new(layers: Vec<Arc<dyn TileCache>>) -> Self {
        Self { layers, tier: None }
    }

    /// Same composition, plus the declaration that an L2 tier IS configured
    /// (`#161`). `layers` still describes only what actually serves reads:
    /// see [`L2Tier`] for why a configured tier may legitimately contribute
    /// no layer at all.
    pub fn with_l2_tier(layers: Vec<Arc<dyn TileCache>>, tier: L2Tier) -> Self {
        Self {
            layers,
            tier: Some(Arc::new(tier)),
        }
    }
}

#[async_trait::async_trait]
impl TileCache for LayeredCache {
    async fn get(&self, key: &TileKey) -> Option<Bytes> {
        for layer in &self.layers {
            if let Some(value) = layer.get(key).await {
                return Some(value);
            }
        }
        None
    }

    async fn insert(&self, key: TileKey, value: Bytes) {
        for layer in &self.layers {
            layer.insert(key.clone(), value.clone()).await;
        }
    }

    /// The single-flight guarantee is delegated to the first layer's own
    /// `get_or_populate` — in every real deployment that is the in-process
    /// moka L1, which is where the real coalescing lives (see its override).
    /// The `populate` handed to that first layer first checks the remaining
    /// layers as plain reads, only falling through to the caller's
    /// `populate` if none of them have it either — so an L2 hit never
    /// triggers an upstream fetch, and an L2 that cannot coalesce natively
    /// still composes correctly. A hit found in a later layer is written
    /// back into the layers ahead of it (the first layer via its own
    /// `get_or_populate`, the rest explicitly below).
    async fn get_or_populate(
        &self,
        key: TileKey,
        populate: PopulateFuture,
    ) -> Result<Bytes, Arc<Error>> {
        let Some((first, rest)) = self.layers.split_first() else {
            return populate.await.map_err(Arc::new);
        };
        let rest = rest.to_vec();
        let fallback_key = key.clone();
        let fallback: PopulateFuture = Box::pin(async move {
            for layer in &rest {
                if let Some(value) = layer.get(&fallback_key).await {
                    return Ok(value);
                }
            }
            populate.await
        });

        let value = first.get_or_populate(key.clone(), fallback).await?;
        for layer in &self.layers[1..] {
            layer.insert(key.clone(), value.clone()).await;
        }
        Ok(value)
    }

    /// TTL-aware counterpart to [`get_or_populate`](Self::get_or_populate)
    /// above, same shape: the first layer stays the single-flight leader
    /// (through its own `get_or_populate_with_ttl`, so an L1 leader like
    /// `MokaTileCache`/`MetricsTileCache` keeps real coalescing and simply
    /// ignores `ttl`), and every later layer is written through
    /// `insert_with_ttl` instead of plain `insert` so the caller's TTL — a
    /// collection's effective `cache_ttl_s` (`settings.rs`, `#46`) — actually
    /// reaches an `L2CacheAdapter` layer.
    async fn get_or_populate_with_ttl(
        &self,
        key: TileKey,
        populate: PopulateFuture,
        ttl: Duration,
    ) -> Result<Bytes, Arc<Error>> {
        let Some((first, rest)) = self.layers.split_first() else {
            return populate.await.map_err(Arc::new);
        };
        let rest = rest.to_vec();
        let fallback_key = key.clone();
        let fallback: PopulateFuture = Box::pin(async move {
            for layer in &rest {
                if let Some(value) = layer.get(&fallback_key).await {
                    return Ok(value);
                }
            }
            populate.await
        });

        let value = first
            .get_or_populate_with_ttl(key.clone(), fallback, ttl)
            .await?;
        for layer in &self.layers[1..] {
            layer.insert_with_ttl(key.clone(), value.clone(), ttl).await;
        }
        Ok(value)
    }

    fn l2_tier(&self) -> Option<Arc<L2Tier>> {
        self.tier.clone()
    }
}

/// Backend for an L2 tile-cache tier: a byte-oriented, TTL-aware, fallible
/// key/value store consulted only after an L1 miss (see `LayeredCache`,
/// which reaches an L2 layer through `L2CacheAdapter` below rather than this
/// trait directly). Kept separate from `TileCache` because a real networked
/// cache needs both things that trait cannot express — a call that can fail,
/// and an entry TTL — and giving `TileCache` a fallible signature would
/// burden the in-process L1 and every existing layer with error handling
/// they never need.
#[async_trait::async_trait]
pub trait L2Cache: Send + Sync {
    /// `Err` here is a backend problem (connection, timeout, protocol), not
    /// "no value" — a real miss is `Ok(None)`. `L2CacheAdapter::get` treats
    /// both the same way (falls through as if this layer weren't there), but
    /// keeping them distinct lets a backend log or count the two separately.
    async fn get(&self, key: &TileKey) -> Result<Option<Bytes>, Error>;

    /// `ttl` is the entry's time-to-live in the backend (e.g. Valkey's own
    /// `EX` expiry) — the backend evicts on its own; nothing in this crate
    /// ever explicitly deletes an L2 entry.
    async fn put(&self, key: TileKey, value: Bytes, ttl: Duration) -> Result<(), Error>;
}

/// Bridges an `L2Cache` backend into the `TileCache` interface `LayeredCache`
/// composes over, so a networked L2 slots into the same `Vec<Arc<dyn
/// TileCache>>` as the in-process L1 with no change to `LayeredCache` itself.
pub struct L2CacheAdapter {
    backend: Arc<dyn L2Cache>,
    ttl: Duration,
}

impl L2CacheAdapter {
    pub fn new(backend: Arc<dyn L2Cache>, ttl: Duration) -> Self {
        Self { backend, ttl }
    }
}

#[async_trait::async_trait]
impl TileCache for L2CacheAdapter {
    /// A backend error is indistinguishable from a miss to every caller —
    /// `LayeredCache` falls through to the next layer (or `populate`)
    /// exactly as it would for an empty cache, so an L2 outage degrades
    /// silently to L1-only instead of failing the request.
    async fn get(&self, key: &TileKey) -> Option<Bytes> {
        match self.backend.get(key).await {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "L2 cache read failed, degrading to L1-only");
                None
            }
        }
    }

    /// Never awaits the backend: the write is spawned and only its outcome
    /// logged, so a slow or unreachable L2 adds no latency to — and can
    /// never fail — the response that triggered it. `TileCache::insert`
    /// already returns `()`, so there is nothing for a backend error to
    /// propagate into even if this awaited it directly; spawning is what
    /// keeps the *latency* off the response path too.
    async fn insert(&self, key: TileKey, value: Bytes) {
        self.insert_with_ttl(key, value, self.ttl).await;
    }

    /// Real TTL wiring (`#39`): unlike plain [`insert`](Self::insert), which
    /// always uses this adapter's own fixed `self.ttl`, this honors whatever
    /// TTL the caller passes — a collection's effective `cache_ttl_s`
    /// (`settings.rs`), when a caller resolves and threads one through.
    /// Still fire-and-forget for the same reason `insert` is: a slow or
    /// unreachable L2 must never add latency to, or fail, the response that
    /// triggered the write.
    async fn insert_with_ttl(&self, key: TileKey, value: Bytes, ttl: Duration) {
        let backend = Arc::clone(&self.backend);
        tokio::spawn(async move {
            if let Err(error) = backend.put(key, value, ttl).await {
                tracing::warn!(%error, "L2 cache write failed, tile stays L1-only");
            }
        });
    }
}

/// Wraps any `TileCache` to record hit/miss/insert counters, labeled by
/// encoding lane, on the process-global Prometheus recorder every `/metrics`
/// scrape reads from. Instrumented once here at `get_or_populate` — the one
/// seam every tile read goes through — instead of scattering `metrics::*!`
/// calls across each protocol handler's call site.
///
/// Single-flight coalescing means only the caller that actually becomes the
/// leader for a given key runs `populate`; callers who instead ride along on
/// an in-flight fetch are counted as hits here, since from their own vantage
/// point they triggered no new fetch. That is a deliberate simplification
/// for the current L1-only deployment, not a bug — a true per-request
/// breakdown would need cooperation from the underlying cache itself.
///
/// `#113`'s "invalidation-triggered miss rate" is `tile_cache_invalidation_
/// misses_total` / `tile_cache_misses_total`, both labeled by `encoding`: a
/// miss is counted on the former whenever `key.generation > 0` — meaning
/// this tile's bucket has been bumped at least once since the consumer
/// started, so the requested key's generation component could never have
/// matched an entry cached before that bump. This is a conservative upper
/// bound, not an exact attribution: a key can carry `generation > 0` and
/// still be a plain cold miss (nothing was ever cached at this coordinate,
/// bumped bucket or not) — telling the two apart would need a second,
/// per-coordinate history this crate declines to keep (the bounded-cache
/// discipline the tile cache itself already follows). Zero cost and zero
/// extra state either way: `key.generation` is already computed for the
/// lookup that just happened.
pub struct MetricsTileCache {
    inner: Arc<dyn TileCache>,
}

impl MetricsTileCache {
    pub fn new(inner: Arc<dyn TileCache>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl TileCache for MetricsTileCache {
    async fn get(&self, key: &TileKey) -> Option<Bytes> {
        self.inner.get(key).await
    }

    async fn insert(&self, key: TileKey, value: Bytes) {
        self.inner.insert(key, value).await;
    }

    async fn get_or_populate(
        &self,
        key: TileKey,
        populate: PopulateFuture,
    ) -> Result<Bytes, Arc<Error>> {
        let lane = key.encoding.metric_label();
        let generation = key.generation;

        // `populate` only actually runs on a miss (the underlying cache's
        // single-flight coalescing skips it entirely on a hit), so wrapping
        // it to flip this flag is how a hit is told apart from a miss
        // without an extra lookup ahead of `get_or_populate` that would race
        // the real one.
        let missed = Arc::new(AtomicBool::new(false));
        let missed_marker = Arc::clone(&missed);
        let tracked_populate: PopulateFuture = Box::pin(async move {
            missed_marker.store(true, Ordering::Relaxed);
            populate.await
        });

        let result = self.inner.get_or_populate(key, tracked_populate).await;

        if missed.load(Ordering::Relaxed) {
            metrics::counter!("tile_cache_misses_total", "encoding" => lane).increment(1);
            if generation > 0 {
                metrics::counter!("tile_cache_invalidation_misses_total", "encoding" => lane)
                    .increment(1);
            }
            if result.is_ok() {
                metrics::counter!("tile_cache_inserts_total", "encoding" => lane).increment(1);
            }
        } else {
            metrics::counter!("tile_cache_hits_total", "encoding" => lane).increment(1);
        }

        result
    }

    /// Same shape and rationale as [`get_or_populate`](Self::get_or_populate)
    /// above — the TTL-aware entry point needs its own override for exactly
    /// the same reason: without it, a caller routing through
    /// `get_or_populate_with_ttl` would silently fall back to the trait's
    /// default (no metrics, no coalescing beyond whatever `inner` itself
    /// provides through its own override).
    async fn get_or_populate_with_ttl(
        &self,
        key: TileKey,
        populate: PopulateFuture,
        ttl: Duration,
    ) -> Result<Bytes, Arc<Error>> {
        let lane = key.encoding.metric_label();
        let generation = key.generation;

        let missed = Arc::new(AtomicBool::new(false));
        let missed_marker = Arc::clone(&missed);
        let tracked_populate: PopulateFuture = Box::pin(async move {
            missed_marker.store(true, Ordering::Relaxed);
            populate.await
        });

        let result = self
            .inner
            .get_or_populate_with_ttl(key, tracked_populate, ttl)
            .await;

        if missed.load(Ordering::Relaxed) {
            metrics::counter!("tile_cache_misses_total", "encoding" => lane).increment(1);
            if generation > 0 {
                metrics::counter!("tile_cache_invalidation_misses_total", "encoding" => lane)
                    .increment(1);
            }
            if result.is_ok() {
                metrics::counter!("tile_cache_inserts_total", "encoding" => lane).increment(1);
            }
        } else {
            metrics::counter!("tile_cache_hits_total", "encoding" => lane).increment(1);
        }

        result
    }

    /// Pure delegation: this wrapper adds counters, never capabilities, and
    /// it is the outermost cache every real deployment hands to
    /// `AppContext`. Without this override an L2 tier declared underneath
    /// would be invisible to readiness — reported as "no cache configured"
    /// for a deployment that configured one, which is exactly the untruth
    /// `#161` is about.
    fn l2_tier(&self) -> Option<Arc<L2Tier>> {
        self.inner.l2_tier()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(z: u8, x: u32, y: u32) -> TileKey {
        key_with_encoding(z, x, y, Encoding::Mvt)
    }

    fn key_with_encoding(z: u8, x: u32, y: u32, encoding: Encoding) -> TileKey {
        TileKey {
            tenant: "public".to_string(),
            catalog: "default".to_string(),
            collection: "demo".to_string(),
            tms: crate::tms::TileMatrixSet::WebMercatorQuad,
            z,
            x,
            y,
            encoding,
            policy_fingerprint: None,
            properties: Vec::new(),
            generation: 0,
        }
    }

    /// `#190`: the tile matrix set partitions the cache exactly like
    /// `encoding`/`policy_fingerprint` already do — a `WorldCRS84Quad` tile
    /// at the same `z`/`x`/`y` as a `WebMercatorQuad` one covers different
    /// ground, so the two must never collide; and two keys on the same grid
    /// still do, keeping every pre-`#190` (WebMercatorQuad) key unchanged.
    #[test]
    fn keys_with_different_tile_matrix_sets_at_the_same_coordinate_are_distinct() {
        let mercator = key(5, 1, 1);
        let crs84 = TileKey {
            tms: crate::tms::TileMatrixSet::WorldCrs84Quad,
            ..key(5, 1, 1)
        };
        assert_eq!(mercator.tms, crate::tms::TileMatrixSet::WebMercatorQuad);
        assert_ne!(mercator, crs84);

        let crs84_again = TileKey {
            tms: crate::tms::TileMatrixSet::WorldCrs84Quad,
            ..key(5, 1, 1)
        };
        assert_eq!(crs84, crs84_again);
    }

    #[test]
    fn encoding_variants_over_the_same_coord_are_distinct_keys() {
        let variants = [
            Encoding::Mvt,
            Encoding::Png,
            Encoding::Glb,
            Encoding::PngStyled("basic".to_string()),
            Encoding::PngStyled("dark".to_string()),
            Encoding::PngRaster(None),
            Encoding::PngRaster(Some(7)),
        ];
        let keys: Vec<TileKey> = variants
            .into_iter()
            .map(|encoding| key_with_encoding(5, 1, 1, encoding))
            .collect();

        for (i, a) in keys.iter().enumerate() {
            for (j, b) in keys.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b, "key should equal itself");
                } else {
                    assert_ne!(a, b, "encoding variants at the same coord must not collide");
                }
            }
        }
    }

    /// `#34`: a key with no policy fingerprint (`None`, the unrestricted-
    /// access case) is exactly what every pre-`#34` key already was — this
    /// is the "byte-identical to before" guarantee `TileKey`'s own doc
    /// promises, expressed as key equality rather than a literal byte
    /// comparison (there is no earlier-version binary in this test to
    /// compare against): two keys built with `policy_fingerprint: None` from
    /// otherwise-identical coordinates are the same key, so public/anonymous
    /// traffic keeps sharing one cache entry exactly as before this field
    /// existed.
    #[test]
    fn unfiltered_keys_at_the_same_coordinate_are_still_equal() {
        let a = key(5, 1, 1);
        let b = key(5, 1, 1);
        assert_eq!(a.policy_fingerprint, None);
        assert_eq!(a, b);
    }

    /// Two subjects whose grants resolve to the same effective filter
    /// fingerprint share one cache entry — a `Some` fingerprint is just
    /// another key field, equal keys still collide.
    #[test]
    fn two_keys_with_the_same_fingerprint_are_equal() {
        let a = TileKey {
            policy_fingerprint: Some(42),
            ..key(5, 1, 1)
        };
        let b = TileKey {
            policy_fingerprint: Some(42),
            ..key(5, 1, 1)
        };
        assert_eq!(a, b);
    }

    /// A filtered request must never collide with an unfiltered one at the
    /// same coordinate, nor with a differently-filtered one — each
    /// fingerprint value partitions the cache into its own entry.
    #[test]
    fn keys_with_different_fingerprints_at_the_same_coordinate_are_distinct() {
        let unfiltered = key(5, 1, 1);
        let filtered_a = TileKey {
            policy_fingerprint: Some(1),
            ..key(5, 1, 1)
        };
        let filtered_b = TileKey {
            policy_fingerprint: Some(2),
            ..key(5, 1, 1)
        };
        assert_ne!(unfiltered, filtered_a);
        assert_ne!(filtered_a, filtered_b);
    }

    /// `#85`: a config change to the vector-tile property allowlist must
    /// never serve a tile cached under the old allowlist — each distinct
    /// `properties` list partitions the cache into its own entry, the same
    /// way each distinct `policy_fingerprint` already does.
    #[test]
    fn keys_with_different_tile_properties_at_the_same_coordinate_are_distinct() {
        let pk_only = key(5, 1, 1);
        let with_name = TileKey {
            properties: vec!["name".to_string()],
            ..key(5, 1, 1)
        };
        let with_name_and_pop = TileKey {
            properties: vec!["name".to_string(), "pop".to_string()],
            ..key(5, 1, 1)
        };
        assert_ne!(pk_only, with_name);
        assert_ne!(with_name, with_name_and_pop);
    }

    /// Two keys built with an empty `properties` list (the pk-only default)
    /// are still the same key — the "byte-identical to before" guarantee
    /// `unfiltered_keys_at_the_same_coordinate_are_still_equal` already
    /// proves for `policy_fingerprint`, expressed for `properties` instead.
    #[test]
    fn empty_tile_properties_at_the_same_coordinate_are_still_equal() {
        let a = key(5, 1, 1);
        let b = key(5, 1, 1);
        assert!(a.properties.is_empty());
        assert_eq!(a, b);
    }

    /// `#113`: a bucket generation bump must partition the cache exactly
    /// like `policy_fingerprint`/`properties` already do — two keys built at
    /// the same coordinate but a different `generation` never collide, and a
    /// generation of `0` (the default, what every collection gets while the
    /// write-reactive consumer is off) is still the pre-`#113` key
    /// byte-for-byte.
    #[test]
    fn keys_with_different_generations_at_the_same_coordinate_are_distinct() {
        let generation_zero = key(5, 1, 1);
        let generation_one = TileKey {
            generation: 1,
            ..key(5, 1, 1)
        };
        assert_eq!(generation_zero.generation, 0);
        assert_ne!(generation_zero, generation_one);

        let generation_one_again = TileKey {
            generation: 1,
            ..key(5, 1, 1)
        };
        assert_eq!(generation_one, generation_one_again);
    }

    #[tokio::test]
    async fn cache_stores_glb_and_png_styled_variants_independently() {
        let cache = MokaTileCache::with_byte_budget(1_000_000);
        let glb = key_with_encoding(4, 2, 2, Encoding::Glb);
        let styled_a = key_with_encoding(4, 2, 2, Encoding::PngStyled("basic".to_string()));
        let styled_b = key_with_encoding(4, 2, 2, Encoding::PngStyled("dark".to_string()));

        cache
            .insert(glb.clone(), Bytes::from_static(b"glb-bytes"))
            .await;
        cache
            .insert(styled_a.clone(), Bytes::from_static(b"styled-basic"))
            .await;
        cache
            .insert(styled_b.clone(), Bytes::from_static(b"styled-dark"))
            .await;

        assert_eq!(
            cache.get(&glb).await,
            Some(Bytes::from_static(b"glb-bytes"))
        );
        assert_eq!(
            cache.get(&styled_a).await,
            Some(Bytes::from_static(b"styled-basic"))
        );
        assert_eq!(
            cache.get(&styled_b).await,
            Some(Bytes::from_static(b"styled-dark"))
        );
    }

    #[tokio::test]
    async fn evicts_once_over_the_byte_budget() {
        let cache = MokaTileCache::with_byte_budget(1024);
        for i in 0..64u32 {
            cache
                .insert(key(0, i, 0), Bytes::from(vec![0u8; 100]))
                .await;
        }
        cache.run_pending_tasks().await;

        assert!(
            cache.weighted_size() <= 1024,
            "weighted size {} exceeds budget",
            cache.weighted_size()
        );
        // Only ~10 entries of 100 bytes fit in a 1024-byte budget; eviction
        // must have dropped most of the 64 inserted (order is the eviction
        // policy's choice, not asserted here — only that the budget holds).
        let mut present = 0;
        for i in 0..64u32 {
            if cache.get(&key(0, i, 0)).await.is_some() {
                present += 1;
            }
        }
        assert!(present < 64, "no eviction occurred: all entries present");
        assert!(
            present <= 10,
            "budget should admit at most ~10 entries, got {present}"
        );
    }

    #[tokio::test]
    async fn get_and_insert_round_trip_under_budget() {
        let cache = MokaTileCache::with_byte_budget(1_000_000);
        let tile = Bytes::from_static(b"mvt-bytes");
        cache.insert(key(5, 1, 1), tile.clone()).await;
        assert_eq!(cache.get(&key(5, 1, 1)).await, Some(tile));
    }

    #[tokio::test]
    async fn concurrent_misses_on_one_key_coalesce_into_a_single_upstream_fetch() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        let cache = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let calls = Arc::new(AtomicUsize::new(0));
        let target = key(1, 0, 0);

        let mut handles = Vec::new();
        for _ in 0..16 {
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            let target = target.clone();
            handles.push(tokio::spawn(async move {
                let calls = Arc::clone(&calls);
                let populate: PopulateFuture = Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    // Wide enough that every spawned task has issued its own
                    // get_or_populate call before the leader's fetch resolves,
                    // so this genuinely races rather than serializing.
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    Ok(Bytes::from_static(b"fetched"))
                });
                cache.get_or_populate(target, populate).await
            }));
        }

        for handle in handles {
            let result = handle.await.unwrap();
            assert_eq!(result.unwrap(), Bytes::from_static(b"fetched"));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "16 concurrent misses on one key must trigger exactly one upstream fetch"
        );
    }

    #[tokio::test]
    async fn a_failed_populate_does_not_poison_the_key() {
        let cache = MokaTileCache::with_byte_budget(1_000_000);
        let target = key(2, 0, 0);

        let failing: PopulateFuture = Box::pin(async { Err(Error::Timeout) });
        assert!(cache
            .get_or_populate(target.clone(), failing)
            .await
            .is_err());
        assert!(
            cache.get(&target).await.is_none(),
            "a failed fetch must not leave a poisoned cache entry"
        );

        let succeeding: PopulateFuture = Box::pin(async { Ok(Bytes::from_static(b"recovered")) });
        let result = cache.get_or_populate(target.clone(), succeeding).await;
        assert_eq!(result.unwrap(), Bytes::from_static(b"recovered"));
        assert_eq!(
            cache.get(&target).await,
            Some(Bytes::from_static(b"recovered"))
        );
    }

    struct FakeLayer {
        entries: std::sync::Mutex<std::collections::HashMap<TileKey, Bytes>>,
    }

    impl FakeLayer {
        fn empty() -> Arc<Self> {
            Arc::new(Self {
                entries: std::sync::Mutex::new(std::collections::HashMap::new()),
            })
        }

        fn with(key: TileKey, value: Bytes) -> Arc<Self> {
            let mut entries = std::collections::HashMap::new();
            entries.insert(key, value);
            Arc::new(Self {
                entries: std::sync::Mutex::new(entries),
            })
        }
    }

    #[async_trait::async_trait]
    impl TileCache for FakeLayer {
        async fn get(&self, key: &TileKey) -> Option<Bytes> {
            self.entries.lock().unwrap().get(key).cloned()
        }

        async fn insert(&self, key: TileKey, value: Bytes) {
            self.entries.lock().unwrap().insert(key, value);
        }
    }

    #[tokio::test]
    async fn layered_cache_falls_through_to_l2_on_l1_miss() {
        let l1 = FakeLayer::empty();
        let l2 = FakeLayer::with(key(2, 3, 4), Bytes::from_static(b"from-l2"));
        let layered = LayeredCache::new(vec![l1.clone(), l2.clone()]);

        let hit = layered.get(&key(2, 3, 4)).await;
        assert_eq!(hit, Some(Bytes::from_static(b"from-l2")));
        // l1 itself, queried directly, is still empty: fallthrough did not mutate it.
        assert!(l1.get(&key(2, 3, 4)).await.is_none());
    }

    #[tokio::test]
    async fn layered_cache_writes_through_every_layer() {
        let l1 = FakeLayer::empty();
        let l2 = FakeLayer::empty();
        let layered = LayeredCache::new(vec![l1.clone(), l2.clone()]);

        layered
            .insert(key(1, 0, 0), Bytes::from_static(b"tile"))
            .await;

        assert_eq!(
            l1.get(&key(1, 0, 0)).await,
            Some(Bytes::from_static(b"tile"))
        );
        assert_eq!(
            l2.get(&key(1, 0, 0)).await,
            Some(Bytes::from_static(b"tile"))
        );
    }

    #[tokio::test]
    async fn layered_get_or_populate_returns_an_l2_hit_without_calling_populate() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let l1 = FakeLayer::empty();
        let l2 = FakeLayer::with(key(3, 0, 0), Bytes::from_static(b"from-l2"));
        let layered = LayeredCache::new(vec![l1.clone(), l2.clone()]);

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in_populate = Arc::clone(&calls);
        let populate: PopulateFuture = Box::pin(async move {
            calls_in_populate.fetch_add(1, Ordering::SeqCst);
            Ok(Bytes::from_static(b"should-not-be-used"))
        });

        let result = layered.get_or_populate(key(3, 0, 0), populate).await;
        assert_eq!(result.unwrap(), Bytes::from_static(b"from-l2"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "an L2 hit must not invoke populate"
        );
        // Unlike plain `get`, `get_or_populate` promotes an L2 hit back into l1.
        assert_eq!(
            l1.get(&key(3, 0, 0)).await,
            Some(Bytes::from_static(b"from-l2"))
        );
    }

    #[tokio::test]
    async fn layered_get_or_populate_calls_populate_once_on_total_miss_and_writes_through() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let l1 = FakeLayer::empty();
        let l2 = FakeLayer::empty();
        let layered = LayeredCache::new(vec![l1.clone(), l2.clone()]);

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in_populate = Arc::clone(&calls);
        let populate: PopulateFuture = Box::pin(async move {
            calls_in_populate.fetch_add(1, Ordering::SeqCst);
            Ok(Bytes::from_static(b"fetched"))
        });

        let result = layered.get_or_populate(key(4, 0, 0), populate).await;
        assert_eq!(result.unwrap(), Bytes::from_static(b"fetched"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            l1.get(&key(4, 0, 0)).await,
            Some(Bytes::from_static(b"fetched"))
        );
        assert_eq!(
            l2.get(&key(4, 0, 0)).await,
            Some(Bytes::from_static(b"fetched"))
        );
    }

    #[tokio::test]
    async fn metrics_wrapper_records_a_miss_then_a_hit_per_encoding_lane() {
        use metrics_exporter_prometheus::PrometheusBuilder;

        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _guard = metrics::set_default_local_recorder(&recorder);

        let cache = MetricsTileCache::new(Arc::new(MokaTileCache::with_byte_budget(1_000_000)));
        let target = key(6, 1, 1);

        let populate: PopulateFuture = Box::pin(async { Ok(Bytes::from_static(b"tile")) });
        let miss = cache.get_or_populate(target.clone(), populate).await;
        assert_eq!(miss.unwrap(), Bytes::from_static(b"tile"));

        let populate_again: PopulateFuture =
            Box::pin(async { panic!("populate must not run again on a cache hit") });
        let hit = cache.get_or_populate(target.clone(), populate_again).await;
        assert_eq!(hit.unwrap(), Bytes::from_static(b"tile"));

        let rendered = handle.render();
        assert!(
            rendered.contains("tile_cache_misses_total{encoding=\"mvt\"} 1"),
            "missing miss counter in:\n{rendered}"
        );
        assert!(
            rendered.contains("tile_cache_hits_total{encoding=\"mvt\"} 1"),
            "missing hit counter in:\n{rendered}"
        );
        assert!(
            rendered.contains("tile_cache_inserts_total{encoding=\"mvt\"} 1"),
            "missing insert counter in:\n{rendered}"
        );
    }

    /// `#113`: a miss on a generation-partitioned key (`generation > 0`) is
    /// additionally counted as an invalidation-triggered miss; a miss on the
    /// default `generation: 0` key (every collection with the consumer off)
    /// is not — proving the metric stays silent for today's byte-identical
    /// TTL-only behavior and only engages once a real generation is in play.
    #[tokio::test]
    async fn metrics_wrapper_counts_a_miss_on_a_bumped_generation_as_invalidation_triggered() {
        use metrics_exporter_prometheus::PrometheusBuilder;

        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _guard = metrics::set_default_local_recorder(&recorder);

        let cache = MetricsTileCache::new(Arc::new(MokaTileCache::with_byte_budget(1_000_000)));

        // generation 0 (the off-by-default case): a miss here must NOT be
        // counted as invalidation-triggered.
        let unbumped = key(6, 2, 2);
        let populate: PopulateFuture = Box::pin(async { Ok(Bytes::from_static(b"tile")) });
        cache.get_or_populate(unbumped, populate).await.unwrap();

        // generation > 0: a miss here IS counted as invalidation-triggered.
        let bumped = TileKey {
            generation: 5,
            ..key(6, 3, 3)
        };
        let populate: PopulateFuture = Box::pin(async { Ok(Bytes::from_static(b"tile")) });
        cache.get_or_populate(bumped, populate).await.unwrap();

        let rendered = handle.render();
        assert!(
            rendered.contains("tile_cache_misses_total{encoding=\"mvt\"} 2"),
            "expected two plain misses in:\n{rendered}"
        );
        assert!(
            rendered.contains("tile_cache_invalidation_misses_total{encoding=\"mvt\"} 1"),
            "expected exactly one invalidation-triggered miss in:\n{rendered}"
        );
    }

    /// #23 routed the Png/PngStyled raster handlers through `get_or_populate`
    /// (previously direct `get`/`insert`, bypassing this wrapper entirely),
    /// which was the coverage caveat #22 left open. This proves the wrapper
    /// itself labels those two encodings correctly through the same seam —
    /// the handler-side routing change is what makes real raster requests
    /// reach it.
    #[tokio::test]
    async fn metrics_wrapper_labels_png_and_png_styled_lanes() {
        use metrics_exporter_prometheus::PrometheusBuilder;

        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _guard = metrics::set_default_local_recorder(&recorder);

        let cache = MetricsTileCache::new(Arc::new(MokaTileCache::with_byte_budget(1_000_000)));
        let png_target = key_with_encoding(7, 2, 2, Encoding::Png);
        let styled_target = key_with_encoding(7, 2, 2, Encoding::PngStyled("basic".to_string()));

        let populate_png: PopulateFuture = Box::pin(async { Ok(Bytes::from_static(b"png-tile")) });
        let png_miss = cache.get_or_populate(png_target, populate_png).await;
        assert_eq!(png_miss.unwrap(), Bytes::from_static(b"png-tile"));

        let populate_styled: PopulateFuture =
            Box::pin(async { Ok(Bytes::from_static(b"styled-tile")) });
        let styled_miss = cache.get_or_populate(styled_target, populate_styled).await;
        assert_eq!(styled_miss.unwrap(), Bytes::from_static(b"styled-tile"));

        let rendered = handle.render();
        assert!(
            rendered.contains("tile_cache_misses_total{encoding=\"png\"} 1"),
            "missing png miss counter in:\n{rendered}"
        );
        assert!(
            rendered.contains("tile_cache_inserts_total{encoding=\"png\"} 1"),
            "missing png insert counter in:\n{rendered}"
        );
        assert!(
            rendered.contains("tile_cache_misses_total{encoding=\"png_styled\"} 1"),
            "missing png_styled miss counter in:\n{rendered}"
        );
        assert!(
            rendered.contains("tile_cache_inserts_total{encoding=\"png_styled\"} 1"),
            "missing png_styled insert counter in:\n{rendered}"
        );
    }

    /// In-crate `L2Cache` double: an in-memory map with switches to force
    /// every call to either fail (simulating a backend outage) or hang until
    /// released (simulating a slow network round trip), plus call counters
    /// so a test can assert exactly what reached the backend.
    struct MockL2 {
        entries: std::sync::Mutex<std::collections::HashMap<TileKey, Bytes>>,
        fail: AtomicBool,
        get_calls: std::sync::atomic::AtomicUsize,
        put_calls: std::sync::atomic::AtomicUsize,
        /// The `ttl` argument of the most recent `put` call — lets a test
        /// assert which TTL actually reached the backend.
        last_put_ttl: std::sync::Mutex<Option<Duration>>,
        /// Notified once as soon as a `put` starts, before it does anything
        /// else — lets a test observe that the write began without racing
        /// its own assertions against the background task's scheduling.
        put_started: tokio::sync::Notify,
        /// A `put` awaits this before proceeding; a test holds a write open
        /// by simply not notifying it yet.
        put_gate: tokio::sync::Notify,
        gate_puts: AtomicBool,
    }

    impl MockL2 {
        fn empty() -> Arc<Self> {
            Arc::new(Self {
                entries: std::sync::Mutex::new(std::collections::HashMap::new()),
                fail: AtomicBool::new(false),
                get_calls: std::sync::atomic::AtomicUsize::new(0),
                put_calls: std::sync::atomic::AtomicUsize::new(0),
                last_put_ttl: std::sync::Mutex::new(None),
                put_started: tokio::sync::Notify::new(),
                put_gate: tokio::sync::Notify::new(),
                gate_puts: AtomicBool::new(false),
            })
        }

        fn with(key: TileKey, value: Bytes) -> Arc<Self> {
            let backend = Self::empty();
            backend.entries.lock().unwrap().insert(key, value);
            backend
        }

        fn failing() -> Arc<Self> {
            let backend = Self::empty();
            backend.fail.store(true, Ordering::SeqCst);
            backend
        }

        /// Every `put` blocks on `put_gate` until [`Self::release_puts`] is
        /// called — used to prove a write is fire-and-forget from the
        /// caller's perspective.
        fn gated() -> Arc<Self> {
            let backend = Self::empty();
            backend.gate_puts.store(true, Ordering::SeqCst);
            backend
        }

        fn release_puts(&self) {
            self.put_gate.notify_one();
        }
    }

    #[async_trait::async_trait]
    impl L2Cache for MockL2 {
        async fn get(&self, key: &TileKey) -> Result<Option<Bytes>, Error> {
            self.get_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err(Error::Timeout);
            }
            Ok(self.entries.lock().unwrap().get(key).cloned())
        }

        async fn put(&self, key: TileKey, value: Bytes, ttl: Duration) -> Result<(), Error> {
            *self.last_put_ttl.lock().unwrap() = Some(ttl);
            self.put_started.notify_one();
            if self.gate_puts.load(Ordering::SeqCst) {
                self.put_gate.notified().await;
            }
            self.put_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err(Error::Timeout);
            }
            self.entries.lock().unwrap().insert(key, value);
            Ok(())
        }
    }

    /// `#161`, the "no invented default" half: nothing in a deployment that
    /// never configured an L2 tier claims one, at any level of the cache
    /// composition a real deployment actually builds.
    #[tokio::test]
    async fn a_cache_with_no_declared_l2_tier_claims_none_anywhere() {
        let l1: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000));
        assert!(l1.l2_tier().is_none(), "the in-process L1 has no L2 tier");

        let layered = LayeredCache::new(vec![Arc::clone(&l1)]);
        assert!(
            layered.l2_tier().is_none(),
            "a LayeredCache built without a declared tier must not invent one"
        );

        let metrics = MetricsTileCache::new(Arc::new(layered));
        assert!(
            metrics.l2_tier().is_none(),
            "the metrics wrapper must not invent a tier either"
        );
    }

    /// `#161`, the "a declared tier survives to the top" half: the outermost
    /// cache every real deployment hands `AppContext` is a
    /// `MetricsTileCache`, so a tier that stops propagating anywhere below
    /// it would read as "no cache configured".
    #[tokio::test]
    async fn a_declared_l2_tier_is_visible_through_the_whole_composition() {
        let l1: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000));
        let backend = MockL2::empty();
        let layered = LayeredCache::with_l2_tier(
            vec![l1],
            L2Tier::connected("valkey", backend as Arc<dyn L2Cache>),
        );
        let metrics = MetricsTileCache::new(Arc::new(layered));

        let tier = metrics.l2_tier().expect("the declared tier must survive");
        assert_eq!(tier.backend(), "valkey");
        assert!(tier.probe().await.is_ok());
    }

    /// A probe is a reachability question, not a cache-content question: an
    /// empty backend that answers is available.
    #[tokio::test]
    async fn probing_a_reachable_but_empty_backend_succeeds() {
        let tier = L2Tier::connected("valkey", MockL2::empty() as Arc<dyn L2Cache>);

        assert!(tier.probe().await.is_ok());
    }

    #[tokio::test]
    async fn probing_a_failing_backend_reports_the_backend_error() {
        let tier = L2Tier::connected("valkey", MockL2::failing() as Arc<dyn L2Cache>);

        assert!(tier.probe().await.is_err());
    }

    /// The probe must not disturb the operator's shared cache instance: it
    /// reads, and never writes.
    #[tokio::test]
    async fn probing_never_writes_to_the_backend() {
        let backend = MockL2::empty();
        let tier = L2Tier::connected("valkey", Arc::clone(&backend) as Arc<dyn L2Cache>);

        tier.probe().await.unwrap();

        assert_eq!(backend.get_calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.put_calls.load(Ordering::SeqCst), 0);
        assert!(backend.entries.lock().unwrap().is_empty());
    }

    /// The boot-down case (`#161`): a tier configured but never connected is
    /// still a DECLARED tier — `Some`, not `None` — and every probe of it
    /// fails by name with the recorded boot reason. Reporting it as `None`
    /// would be indistinguishable from a deployment that configured no
    /// cache at all, which is exactly the untruth this slice removes.
    #[tokio::test]
    async fn a_never_connected_tier_is_declared_and_always_fails_its_probe() {
        let tier = L2Tier::never_connected("valkey", "connection refused");

        assert_eq!(tier.backend(), "valkey");
        assert!(
            matches!(tier.state(), L2TierState::NeverConnected(reason) if reason == "connection refused")
        );
        let error = tier.probe().await.unwrap_err();
        assert!(
            error.to_string().contains("connection refused"),
            "the boot reason must survive into the probe error: {error}"
        );
    }

    /// A tier declared without any serving layer behind it (the boot-down
    /// composition `build_cache` produces) must serve exactly like the
    /// L1-only cache it replaced — the declaration is metadata, never a hop.
    #[tokio::test]
    async fn declaring_a_tier_does_not_change_what_the_layers_serve() {
        let l1 = Arc::new(MokaTileCache::with_byte_budget(10_000));
        let layered = LayeredCache::with_l2_tier(
            vec![Arc::clone(&l1) as Arc<dyn TileCache>],
            L2Tier::never_connected("valkey", "connection refused"),
        );
        let target = key(3, 3, 3);

        layered
            .insert(target.clone(), Bytes::from_static(b"tile"))
            .await;

        assert_eq!(
            layered.get(&target).await,
            Some(Bytes::from_static(b"tile"))
        );
        assert_eq!(l1.get(&target).await, Some(Bytes::from_static(b"tile")));
    }

    #[tokio::test]
    async fn l2_adapter_get_returns_a_backend_hit() {
        let target = key(1, 1, 1);
        let backend = MockL2::with(target.clone(), Bytes::from_static(b"l2-value"));
        let adapter = L2CacheAdapter::new(backend, Duration::from_secs(60));

        assert_eq!(
            adapter.get(&target).await,
            Some(Bytes::from_static(b"l2-value"))
        );
    }

    #[tokio::test]
    async fn l2_adapter_get_degrades_a_backend_error_to_a_plain_miss() {
        let target = key(1, 1, 1);
        let backend = MockL2::failing();
        let adapter = L2CacheAdapter::new(backend, Duration::from_secs(60));

        assert_eq!(
            adapter.get(&target).await,
            None,
            "a backend error must look exactly like a miss to the caller"
        );
    }

    #[tokio::test]
    async fn l2_adapter_insert_returns_before_a_slow_backend_write_completes() {
        let target = key(2, 2, 2);
        let backend = MockL2::gated();
        let adapter = L2CacheAdapter::new(
            Arc::clone(&backend) as Arc<dyn L2Cache>,
            Duration::from_secs(60),
        );

        // The backend `put` is gated (blocks until released), so if
        // `insert` awaited it directly this would hang past the timeout.
        // Fire-and-forget means `insert` itself returns immediately.
        tokio::time::timeout(
            Duration::from_millis(200),
            adapter.insert(target.clone(), Bytes::from_static(b"v")),
        )
        .await
        .expect("insert must not block on the L2 write");

        // The spawned write has started (or will imminently) but cannot
        // have finished yet — it is parked on the gate.
        tokio::time::timeout(Duration::from_millis(200), backend.put_started.notified())
            .await
            .expect("the backend write should still have started in the background");
        assert_eq!(backend.put_calls.load(Ordering::SeqCst), 0);

        backend.release_puts();
        // Poll briefly for the background task to finish — bounded so a
        // regression (the write never landing) fails fast instead of
        // hanging.
        for _ in 0..100 {
            if backend.put_calls.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            backend.put_calls.load(Ordering::SeqCst),
            1,
            "the fire-and-forget write should eventually land once released"
        );
        assert_eq!(
            backend.entries.lock().unwrap().get(&target),
            Some(&Bytes::from_static(b"v"))
        );
    }

    #[tokio::test]
    async fn l2_adapter_insert_error_is_logged_and_never_propagates() {
        let target = key(3, 3, 3);
        let backend = MockL2::failing();
        let adapter = L2CacheAdapter::new(
            Arc::clone(&backend) as Arc<dyn L2Cache>,
            Duration::from_secs(60),
        );

        // `insert` returns `()` — there is nothing here to unwrap or match;
        // the assertion is that this compiles, runs, and does not panic.
        adapter
            .insert(target.clone(), Bytes::from_static(b"v"))
            .await;

        for _ in 0..100 {
            if backend.put_calls.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(backend.put_calls.load(Ordering::SeqCst), 1);
        assert!(
            backend.entries.lock().unwrap().get(&target).is_none(),
            "a failed backend write must not leave a stale entry"
        );
    }

    /// `#39`: `insert_with_ttl` honors a caller-supplied TTL instead of the
    /// adapter's own fixed one — the seam a collection's effective
    /// `cache_ttl_s` (`settings.rs`) would write through if a caller resolved
    /// and passed one. Plain `insert` still falls back to the adapter's own
    /// `self.ttl`, proven by the second assertion.
    #[tokio::test]
    async fn l2_adapter_insert_with_ttl_uses_the_passed_ttl_not_the_adapters_own() {
        let target = key(7, 7, 7);
        let backend = MockL2::empty();
        let adapter = L2CacheAdapter::new(
            Arc::clone(&backend) as Arc<dyn L2Cache>,
            Duration::from_secs(60),
        );

        adapter
            .insert_with_ttl(
                target.clone(),
                Bytes::from_static(b"v"),
                Duration::from_secs(5),
            )
            .await;
        for _ in 0..100 {
            if backend.put_calls.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            *backend.last_put_ttl.lock().unwrap(),
            Some(Duration::from_secs(5)),
            "insert_with_ttl must forward the caller's TTL, not the adapter's own 60s default"
        );

        adapter.insert(target, Bytes::from_static(b"w")).await;
        for _ in 0..100 {
            if backend.put_calls.load(Ordering::SeqCst) == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            *backend.last_put_ttl.lock().unwrap(),
            Some(Duration::from_secs(60)),
            "plain insert must still use the adapter's own configured TTL"
        );
    }

    #[tokio::test]
    async fn layered_l1_l2_read_path_backfills_l1_on_an_l2_hit() {
        let target = key(4, 4, 4);
        let l1 = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let l2_backend = MockL2::with(target.clone(), Bytes::from_static(b"from-l2"));
        let l2 = Arc::new(L2CacheAdapter::new(
            Arc::clone(&l2_backend) as Arc<dyn L2Cache>,
            Duration::from_secs(60),
        ));
        let layered = LayeredCache::new(vec![l1.clone() as Arc<dyn TileCache>, l2]);

        let populate: PopulateFuture =
            Box::pin(async { panic!("an L2 hit must never fall through to populate") });
        let result = layered.get_or_populate(target.clone(), populate).await;
        assert_eq!(result.unwrap(), Bytes::from_static(b"from-l2"));

        // L1 now has it directly, with no L2 round trip needed.
        assert_eq!(
            l1.get(&target).await,
            Some(Bytes::from_static(b"from-l2")),
            "an L2 hit must backfill L1"
        );
    }

    #[tokio::test]
    async fn layered_l1_l2_total_miss_coalesces_the_render_into_one_call() {
        use std::sync::atomic::AtomicUsize;
        use std::time::Duration as StdDuration;

        let target = key(5, 5, 5);
        let l1 = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let l2_backend = MockL2::empty();
        let l2 = Arc::new(L2CacheAdapter::new(
            l2_backend as Arc<dyn L2Cache>,
            Duration::from_secs(60),
        ));
        let layered = Arc::new(LayeredCache::new(vec![l1 as Arc<dyn TileCache>, l2]));

        let render_calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..16 {
            let layered = Arc::clone(&layered);
            let render_calls = Arc::clone(&render_calls);
            let target = target.clone();
            handles.push(tokio::spawn(async move {
                let render_calls = Arc::clone(&render_calls);
                let populate: PopulateFuture = Box::pin(async move {
                    render_calls.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(StdDuration::from_millis(30)).await;
                    Ok(Bytes::from_static(b"rendered"))
                });
                layered.get_or_populate(target, populate).await
            }));
        }

        for handle in handles {
            assert_eq!(
                handle.await.unwrap().unwrap(),
                Bytes::from_static(b"rendered")
            );
        }
        assert_eq!(
            render_calls.load(Ordering::SeqCst),
            1,
            "an L1-and-L2 miss stampede must still render exactly once"
        );
    }

    #[tokio::test]
    async fn layered_l1_l2_read_degrades_silently_when_l2_is_down() {
        let target = key(6, 6, 6);
        let l1 = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let l2_backend = MockL2::failing();
        let l2 = Arc::new(L2CacheAdapter::new(
            l2_backend as Arc<dyn L2Cache>,
            Duration::from_secs(60),
        ));
        let layered = LayeredCache::new(vec![l1 as Arc<dyn TileCache>, l2]);

        let populate: PopulateFuture = Box::pin(async { Ok(Bytes::from_static(b"rendered")) });
        // An L2 that errors on every call (both the read fallback and the
        // write-through) must still resolve the request successfully — the
        // outage is invisible to the caller, never a failed response.
        let result = layered.get_or_populate(target, populate).await;
        assert_eq!(result.unwrap(), Bytes::from_static(b"rendered"));
    }

    /// `#46`: the TTL-aware entry point must keep `MokaTileCache`'s real
    /// single-flight coalescing — mirrors
    /// `concurrent_misses_on_one_key_coalesce_into_a_single_upstream_fetch`
    /// above, but through `get_or_populate_with_ttl`.
    #[tokio::test]
    async fn moka_get_or_populate_with_ttl_still_coalesces_concurrent_misses() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let cache = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let calls = Arc::new(AtomicUsize::new(0));
        let target = key(8, 0, 0);

        let mut handles = Vec::new();
        for _ in 0..16 {
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            let target = target.clone();
            handles.push(tokio::spawn(async move {
                let calls = Arc::clone(&calls);
                let populate: PopulateFuture = Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    Ok(Bytes::from_static(b"fetched"))
                });
                cache
                    .get_or_populate_with_ttl(target, populate, Duration::from_secs(45))
                    .await
            }));
        }

        for handle in handles {
            let result = handle.await.unwrap();
            assert_eq!(result.unwrap(), Bytes::from_static(b"fetched"));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "16 concurrent misses on one key must still trigger exactly one upstream fetch with a TTL set"
        );
    }

    /// `#46`: the two things `get_or_populate_with_ttl` exists for, proven
    /// together on the realistic L1+L2 shape — mirrors
    /// `layered_l1_l2_total_miss_coalesces_the_render_into_one_call` above
    /// (coalescing) plus `l2_adapter_insert_with_ttl_uses_the_passed_ttl_not_the_adapters_own`
    /// (TTL reaches the backend), through the one seam a handler actually calls.
    #[tokio::test]
    async fn layered_get_or_populate_with_ttl_coalesces_and_writes_the_given_ttl_to_l2() {
        use std::sync::atomic::AtomicUsize;

        let target = key(9, 0, 0);
        let l1 = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let l2_backend = MockL2::empty();
        let l2 = Arc::new(L2CacheAdapter::new(
            Arc::clone(&l2_backend) as Arc<dyn L2Cache>,
            // Adapter's own fixed default, deliberately different from the
            // TTL threaded through below — proves the caller's TTL wins,
            // not this one.
            Duration::from_secs(60),
        ));
        let layered = Arc::new(LayeredCache::new(vec![l1 as Arc<dyn TileCache>, l2]));

        let render_calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..16 {
            let layered = Arc::clone(&layered);
            let render_calls = Arc::clone(&render_calls);
            let target = target.clone();
            handles.push(tokio::spawn(async move {
                let render_calls = Arc::clone(&render_calls);
                let populate: PopulateFuture = Box::pin(async move {
                    render_calls.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    Ok(Bytes::from_static(b"rendered"))
                });
                layered
                    .get_or_populate_with_ttl(target, populate, Duration::from_secs(45))
                    .await
            }));
        }

        for handle in handles {
            assert_eq!(
                handle.await.unwrap().unwrap(),
                Bytes::from_static(b"rendered")
            );
        }
        assert_eq!(
            render_calls.load(Ordering::SeqCst),
            1,
            "an L1-and-L2 miss stampede must still render exactly once with a TTL set"
        );

        for _ in 0..100 {
            if l2_backend.put_calls.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            *l2_backend.last_put_ttl.lock().unwrap(),
            Some(Duration::from_secs(45)),
            "the caller's TTL must reach the L2 write, not the adapter's own 60s default"
        );
    }

    /// `#46`: an L2 hit through `get_or_populate_with_ttl` must still skip
    /// `populate` and still backfill L1 — the with-ttl entry point cannot
    /// regress the plain one's L2-hit shortcut proven by
    /// `layered_get_or_populate_returns_an_l2_hit_without_calling_populate`.
    #[tokio::test]
    async fn layered_get_or_populate_with_ttl_returns_an_l2_hit_without_calling_populate() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let l1 = FakeLayer::empty();
        let l2 = FakeLayer::with(key(10, 0, 0), Bytes::from_static(b"from-l2"));
        let layered = LayeredCache::new(vec![l1.clone(), l2.clone()]);

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in_populate = Arc::clone(&calls);
        let populate: PopulateFuture = Box::pin(async move {
            calls_in_populate.fetch_add(1, Ordering::SeqCst);
            Ok(Bytes::from_static(b"should-not-be-used"))
        });

        let result = layered
            .get_or_populate_with_ttl(key(10, 0, 0), populate, Duration::from_secs(45))
            .await;
        assert_eq!(result.unwrap(), Bytes::from_static(b"from-l2"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "an L2 hit must not invoke populate"
        );
        assert_eq!(
            l1.get(&key(10, 0, 0)).await,
            Some(Bytes::from_static(b"from-l2")),
            "an L2 hit must still backfill L1"
        );
    }

    /// `#46`: the metrics wrapper's TTL-aware override must record the same
    /// counters as its plain one — mirrors
    /// `metrics_wrapper_records_a_miss_then_a_hit_per_encoding_lane` above.
    #[tokio::test]
    async fn metrics_wrapper_records_a_miss_then_a_hit_through_get_or_populate_with_ttl() {
        use metrics_exporter_prometheus::PrometheusBuilder;

        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _guard = metrics::set_default_local_recorder(&recorder);

        let cache = MetricsTileCache::new(Arc::new(MokaTileCache::with_byte_budget(1_000_000)));
        let target = key(11, 1, 1);

        let populate: PopulateFuture = Box::pin(async { Ok(Bytes::from_static(b"tile")) });
        let miss = cache
            .get_or_populate_with_ttl(target.clone(), populate, Duration::from_secs(45))
            .await;
        assert_eq!(miss.unwrap(), Bytes::from_static(b"tile"));

        let populate_again: PopulateFuture =
            Box::pin(async { panic!("populate must not run again on a cache hit") });
        let hit = cache
            .get_or_populate_with_ttl(target.clone(), populate_again, Duration::from_secs(45))
            .await;
        assert_eq!(hit.unwrap(), Bytes::from_static(b"tile"));

        let rendered = handle.render();
        assert!(
            rendered.contains("tile_cache_misses_total{encoding=\"mvt\"} 1"),
            "missing miss counter in:\n{rendered}"
        );
        assert!(
            rendered.contains("tile_cache_hits_total{encoding=\"mvt\"} 1"),
            "missing hit counter in:\n{rendered}"
        );
        assert!(
            rendered.contains("tile_cache_inserts_total{encoding=\"mvt\"} 1"),
            "missing insert counter in:\n{rendered}"
        );
    }
}
