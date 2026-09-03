//! Configuration model. Mirrors the YAML shape in the design doc:
//! `server`, `cache`, `storages`, `tenants`, `catalogs`, `collections`,
//! `styles`, `settings`, `auth`.
//! `AppConfig::validate` enforces referential integrity (storage/tenant/
//! catalog refs resolve, ids unique at their proper scope, zoom ranges sane,
//! cache percentage bounded) so a bad file fails fast at startup, not
//! mid-request.
//!
//! Routing shape (`#39`): a tenant owns catalogs by reference (never
//! nesting), a catalog owns collections by reference the same way. Every
//! declaration carries an internal `id` (used everywhere below the HTTP
//! boundary — cache keys, driver lookups, metrics labels) and an
//! `external_id` (defaults to `id`, used only in URLs and response bodies).
//! Internal ids are globally unique; external ids are unique only at their
//! own scope — tenant external ids globally, catalog external ids per
//! tenant, collection external ids per catalog — so two tenants may both
//! declare a `default` catalog, or two catalogs may both declare a `demo`
//! collection, without colliding.
//!
//! Naming convention for the types below: `*Decl` is a per-item declaration
//! the operator writes one of per entity (`TenantDecl`, `CatalogDecl`,
//! `CollectionDecl`, `PropertyDecl`, `StorageDecl`, `StyleRef`) — the thing
//! that has an `id`. `*Conf` is a leaf option block nested inside a `*Decl`
//! or `SettingsDecl` (`TilesConf`, `StyleConf`, `Places3dConf`, `StacConf`)
//! — no `id` of its own, just a bundle of settings. `*Config` is a
//! mode/backend-selecting type (`ServerConfig`, `CacheConfig`, `AuthConfig`,
//! `RegistryConfig`, `L2CacheConfig`) — either one of `AppConfig`'s own
//! top-level sections, or (`L2CacheConfig`) a nested backend switch with the
//! same tagged-enum shape. This is a documentation convention only — no
//! renaming here, since these are public types crates.io consumers already
//! depend on. `AuthConfig` is the one exception to the tagged-enum shape:
//! its two credential sources (`bearer_tokens`, `oidc`) are independently
//! optional rather than mutually exclusive, so an operator can run both at
//! once — see `AuthConfig`'s own doc.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::admission::AdmissionDecl;
use crate::batch::BatchDecl;
use crate::catalog::{AttributeColumn, GeometryProfile, ProjectionFacts};
use crate::error::{Error, Result};

const MAX_ZOOM: u8 = 24;

/// URL segments that stay top-level (`/metrics`, `/ui`, ...) and can never
/// be claimed as a tenant `external_id`. Not a routing hazard by itself —
/// axum matches a literal segment ahead of a `{tenant}` capture regardless —
/// but a tenant external id equal to one of these would make that tenant's
/// entire route tree permanently unreachable behind the literal route, so
/// `AppConfig::validate` refuses it at boot instead of silently shadowing it
/// at request time.
pub const RESERVED_TENANT_SEGMENTS: &[&str] = &["metrics", "ui", "api", "healthz", "readyz"];

/// Default TTL (seconds) for a derived collection descriptor before
/// `Router` re-derives it from the backend on next access — see
/// `descriptor.rs` and the driver-contract design doc (`#19`). Lives in the
/// same `server:` home as the other engine-timing knobs
/// (`request_timeout_s`, `max_concurrency`).
pub const DEFAULT_DESCRIPTOR_TTL_S: u64 = 300;

/// Default count-based cap on `Router`'s derived-descriptor cache
/// (registry scale-out, `#42`). Generous for any file-backed deployment —
/// thousands of collections fit comfortably below this — the ceiling
/// starts mattering once a routed registry driver (a later slice) can serve
/// far more collections than fit in memory as fully-derived descriptors at
/// once. See `router.rs`'s module doc.
pub const DEFAULT_DESCRIPTOR_CACHE_CAPACITY: u64 = 100_000;

/// Public collection identifier allowed to become a bounded metrics label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetricCollectionRef {
    pub tenant: String,
    pub catalog: String,
    pub collection: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub port: u16,
    pub request_timeout_s: u64,
    /// Optional canonical public URL used for links returned by landing and
    /// directory documents. When absent, Tellurion emits relative links so a
    /// deployment can remain portable behind arbitrary reverse proxies.
    ///
    /// Set this explicitly when clients need copy-and-pasteable links (for
    /// example a public demo). Tellurion never derives it from `Host` or
    /// forwarded request headers.
    #[serde(default)]
    pub public_base_url: Option<String>,
    /// Structured JSON log lines instead of the default human-readable
    /// format. Behavior, so it lives here rather than an env var.
    pub log_json: bool,
    /// Overrides the CPU-derived concurrency-limit ceiling ahead of the
    /// storage pool (see `derive_max_concurrency` in the `tellurion` server crate).
    /// `None` (the default) keeps the derived value; set this to pin the
    /// ceiling low for deliberately exercising the load-shed 503 path
    /// without needing hundreds of real concurrent connections.
    #[serde(default)]
    pub max_concurrency: Option<usize>,
    /// How long a derived `CollectionDescriptor` (table/geometry/pk/extent
    /// resolved from the backend) stays cached before `Router` re-derives it
    /// on next access. See `descriptor.rs`.
    pub descriptor_ttl_s: u64,
    /// Count-based eviction cap on `Router`'s derived-descriptor cache
    /// (`#42`, registry scale-out): once the cache holds this many entries,
    /// a cold one is evicted to make room for one just touched — the same
    /// "bounded, never unlimited" discipline `cache.memory_percent` already
    /// applies to the tile cache, sized by entry count here rather than
    /// bytes since a descriptor is a small, roughly fixed-size struct
    /// rather than a variable-sized tile.
    pub descriptor_cache_capacity: u64,
    /// The index applier's background-task wiring (`#67`) — see
    /// `IndexApplierConfig`'s own doc.
    pub index_applier: IndexApplierConfig,
    /// The Processes lane's durable job ledger (`#182`) — see
    /// [`ProcessesConfig`]'s own doc. `None`/absent, the default and the
    /// shape of every config written before this field existed, means the
    /// deployment has no ledger: no job can be recorded, so no Processes root
    /// is served anywhere and no runner loop is spawned. Declaring the block
    /// IS the opt-in, the same shape `IndexApplierConfig::lease` already
    /// uses, so there is deliberately no `enabled` flag inside it.
    #[serde(default)]
    pub processes: Option<ProcessesConfig>,
    /// The write-reactive tile-cache invalidation consumer's background-task
    /// wiring (`#113`) — see `TileInvalidationConfig`'s own doc.
    pub tile_invalidation: TileInvalidationConfig,
    /// The change-feed lane's page-size bounds (`#115`) — see
    /// `ChangeFeedConfig`'s own doc.
    pub change_feed: ChangeFeedConfig,
    /// The webhook-delivery consumer's background-task wiring (`#115`) —
    /// see `WebhookDeliveryConfig`'s own doc.
    pub webhook_delivery: WebhookDeliveryConfig,
    /// The consumer-aware outbox retention floor's background-task wiring
    /// (`#115`) — see `OutboxRetentionConfig`'s own doc.
    pub outbox_retention: OutboxRetentionConfig,
    /// Maximum time to wait for in-flight requests during graceful shutdown.
    pub drain_timeout_s: u64,
    /// Interval between readiness dependency checks.
    pub readiness_probe_interval_s: u64,
    /// Deadline for one readiness dependency check.
    pub readiness_probe_timeout_s: u64,
    /// Public tenant identifiers allowed to become metrics labels.
    pub metrics_tenant_allowlist: Vec<String>,
    /// Fully-qualified public collections allowed to become metrics labels.
    pub metrics_collection_allowlist: Vec<MetricCollectionRef>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            // Matches the 60s hard request ceiling from the operational rules.
            request_timeout_s: 60,
            public_base_url: None,
            log_json: false,
            max_concurrency: None,
            descriptor_ttl_s: DEFAULT_DESCRIPTOR_TTL_S,
            descriptor_cache_capacity: DEFAULT_DESCRIPTOR_CACHE_CAPACITY,
            index_applier: IndexApplierConfig::default(),
            processes: None,
            tile_invalidation: TileInvalidationConfig::default(),
            change_feed: ChangeFeedConfig::default(),
            webhook_delivery: WebhookDeliveryConfig::default(),
            outbox_retention: OutboxRetentionConfig::default(),
            drain_timeout_s: 10,
            readiness_probe_interval_s: 5,
            readiness_probe_timeout_s: 2,
            metrics_tenant_allowlist: Vec::new(),
            metrics_collection_allowlist: Vec::new(),
        }
    }
}

impl ServerConfig {
    /// Returns an external link for an application-relative `path`.
    ///
    /// A configured base may include a path prefix for a reverse proxy. The
    /// input path, including any query string, is appended without URL
    /// resolution so a leading slash cannot discard that prefix.
    pub fn public_href(&self, path: &str) -> String {
        match self.public_base_url.as_deref() {
            Some(base) => format!("{}{}", base.trim_end_matches('/'), path),
            None => path.to_string(),
        }
    }
}

/// Config-gated background task wiring for `crate::applier::run_applier`
/// (`#67`): default OFF, so a deployment that never declared a
/// `routing.index` lane on any collection sees zero behavior change from
/// this existing. When `enabled`, the `tellurion` server binary spawns one
/// applier task per collection whose `routing.index` is configured,
/// draining its `OutboxSource` into its `IndexSink` in bounded batches of
/// `batch_size`, polling every `poll_interval_ms` while idle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexApplierConfig {
    pub enabled: bool,
    /// Bounded per-pass fetch size (`OutboxSource::read_after`'s `limit`) —
    /// keeps one applier pass's memory and lock footprint fixed regardless
    /// of how far behind the index has fallen.
    pub batch_size: u32,
    /// How long an applier task sleeps after a pass that found nothing new
    /// (or failed) before trying again.
    pub poll_interval_ms: u64,
    /// Single-leader coordination for a multi-replica deployment (`#193`).
    /// Absent — the default, and the shape of every config written before
    /// this field existed — means no lease at all: one process drains, the
    /// applier behaves exactly as it always has, and no coordinator is ever
    /// contacted. There is deliberately no default `LeaseDecl` and no
    /// `enabled` flag inside it: declaring it IS the opt-in, so a
    /// single-binary-plus-PostgreSQL deployment cannot accidentally pay for
    /// clustering it does not run.
    pub lease: Option<LeaseDecl>,
}

impl Default for IndexApplierConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            batch_size: 200,
            poll_interval_ms: 1_000,
            lease: None,
        }
    }
}

/// The Processes lane's durable job ledger and in-process runner loop
/// (`#182`). Reached only through `ServerConfig::processes`, which is
/// `Option`-shaped: declaring this block IS the opt-in, so a deployment that
/// never wrote one has no ledger, no runner loop, and — because a job it
/// could not durably record is a job it must not accept — no Processes root
/// at all.
///
/// Two independent switches govern the lane, and conflating them would be a
/// bug: this block says whether the deployment *can* run jobs at all, while
/// `settings.protocols.processes` (`#185`) says whether a given catalog
/// *exposes* the HTTP root. Both must say yes. That mirrors the relationship
/// `routing.index` (can this collection be indexed) already has with
/// `index_applier.enabled` (does this deployment drain it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessesConfig {
    /// Which declared `storages[].id` holds the ledger — the storage whose
    /// driver must advertise `StorageDriver::job_store`. Required, with no
    /// default: guessing "the first storage" would put a deployment's job
    /// ledger somewhere the operator never chose, and there is no such thing
    /// as a sensible default location for durable state.
    ///
    /// Refused at config load when it names no declared storage
    /// (`validate_processes`), and refused by name at boot when the storage
    /// exists but its driver has no job ledger
    /// (`Router::resolve_job_store`) — in which case no Processes root is
    /// served rather than one whose submissions would evaporate.
    pub storage: String,
    /// How long a runner sleeps after a claim pass that found no job.
    ///
    /// This slice polls; `#182`'s `pg_notify`/`LISTEN` wake-up is deferred,
    /// so this interval is the whole latency budget between a submission and
    /// its execution on an idle deployment. The default matches
    /// `IndexApplierConfig::poll_interval_ms` — the same "one second is cheap
    /// against one connection" tradeoff that consumer already made.
    #[serde(default = "default_processes_poll_interval_ms")]
    pub poll_interval_ms: u64,
    /// How long a claimed job stays reserved for its claimant before another
    /// may take it.
    ///
    /// This is the at-least-once knob: too short and a slow-but-healthy job is
    /// executed twice concurrently; too long and a job whose claimant was
    /// SIGKILLed sits idle for that long. Five minutes is a deliberately
    /// conservative default for a first slice with no heartbeat — a runner
    /// that outlives it re-claims nothing, it simply loses the right to
    /// record its own outcome (`JobStore::finish` answers `Ok(None)`), and
    /// the job runs again.
    #[serde(default = "default_processes_visibility_timeout_s")]
    pub visibility_timeout_s: u64,
}

fn default_processes_poll_interval_ms() -> u64 {
    1_000
}

fn default_processes_visibility_timeout_s() -> u64 {
    300
}

/// Opt-in clustered-applier lease (`#193`, closing the transactional-outbox
/// design doc's own deferred item in section 9): with this declared, each
/// applier task drains only while it holds its collection's lease
/// (`crate::lease::Lease`), so 2+ replicas keep that doc's "single ordered
/// consumer per collection" invariant. The lease is resolved from the
/// collection's own write-lane storage — the database that already holds
/// the obligations — so clustering adds no component the deployment did not
/// already run.
///
/// A collection whose lease cannot be resolved is skipped rather than
/// started unleased: an operator who declared a lease asked for exactly one
/// drainer, and quietly starting a second one is the failure they were
/// preventing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LeaseDecl {
    /// Separates deployments that share one physical database. Two stacks
    /// (a staging and a preview, say) pointed at the same PostgreSQL would
    /// otherwise derive identical lease keys for their identically-named
    /// collections and contend over leadership of state they do not
    /// actually share. Absent when there is one deployment per database,
    /// which is the ordinary case.
    pub namespace: Option<String>,
}

/// Config-gated background task wiring for
/// `crate::invalidation::run_generation_consumer` (`#113`): default OFF,
/// same posture as `IndexApplierConfig` above — a deployment that never sets
/// `enabled: true` (and never opts a collection in via `CollectionDecl.
/// tile_invalidation`) sees zero behavior change, byte-for-byte the TTL-only
/// tile cache this crate always had. When enabled, the `tellurion` server
/// binary spawns one consumer task per collection that both opts in AND has
/// a resolvable `routing.write` outbox (`Router::resolve_outbox`) — the same
/// obligation stream the write path already commits to, never a second one.
///
/// `bucket_zoom` is the fixed, shallow tile-matrix zoom the coarse spatial
/// bucket grid is anchored at (`crate::invalidation`'s own module doc): a
/// low zoom keeps the grid's size fixed and small (`4^bucket_zoom` buckets
/// per collection, `256` at the default `4`) regardless of collection size
/// or write volume — the design doc's own "bounded by construction, never
/// proportional to write volume" requirement for the bucketing refinement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TileInvalidationConfig {
    pub enabled: bool,
    /// Bounded per-pass fetch size (`OutboxSource::read_after`'s `limit`) —
    /// same rationale as `IndexApplierConfig::batch_size`.
    pub batch_size: u32,
    /// How long a consumer task sleeps after a pass that found nothing new
    /// (or failed) before trying again.
    pub poll_interval_ms: u64,
    /// Shallow tile-matrix zoom the bucket grid is anchored at. See this
    /// struct's own doc for why a low value is the right default.
    pub bucket_zoom: u8,
}

impl Default for TileInvalidationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            batch_size: 200,
            poll_interval_ms: 1_000,
            bucket_zoom: 4,
        }
    }
}

/// The change-feed lane's page-size bounds (`#115`) — `GET
/// /collections/{cid}/changes`'s own `limit`/`max_page_size` policy, the
/// same "default small, hard-capped" shape `ItemsQueryParams`'s
/// `DEFAULT_LIMIT`/`MAX_LIMIT` already apply to `/items`, lifted to
/// config since this lane has no per-collection opt-in of its own (every
/// collection with a resolvable outbox — `Router::resolve_outbox` — serves
/// this endpoint, unconditionally, the same way `/items` needs no opt-in
/// beyond a resolvable features lane).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChangeFeedConfig {
    pub default_page_size: u32,
    pub max_page_size: u32,
}

impl Default for ChangeFeedConfig {
    fn default() -> Self {
        Self {
            default_page_size: 100,
            max_page_size: 1_000,
        }
    }
}

/// Config-gated background task wiring for `crate::webhooks::
/// run_webhook_consumer` (`#115`): default OFF, the same "a deployment that
/// never turns this on sees no behavior change" posture `IndexApplierConfig`/
/// `TileInvalidationConfig` already establish. When `enabled`, the
/// `tellurion` server binary spawns one delivery task per (subscription,
/// collection) pair whose scopes match and whose outbox resolves — see
/// `AppConfig.webhooks`'s own doc for the declarative subscription list this
/// gates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookDeliveryConfig {
    pub enabled: bool,
    /// Bounded per-pass fetch size (`OutboxSource::read_after`'s `limit`) —
    /// same rationale as `IndexApplierConfig::batch_size`.
    pub batch_size: u32,
    /// How long a delivery task sleeps after a pass that found nothing new
    /// (or failed) before trying again.
    pub poll_interval_ms: u64,
    /// Deadline for one HTTP delivery attempt.
    pub request_timeout_ms: u64,
    /// Bounded retry budget for one obligation's delivery — see
    /// `crate::webhooks::backoff_delay`'s own doc for the exponential
    /// schedule these bound.
    pub max_attempts: u32,
    pub base_backoff_ms: u64,
    pub max_backoff_ms: u64,
    /// Per-subscription bounded dead-letter ring capacity — see
    /// `crate::webhooks::WebhookSubscriptionRuntime`'s own doc.
    pub dead_letter_capacity: usize,
    /// The paged dead-letter inspection surface's own `limit`/`max_page_size`
    /// policy (`#115`) — same "default small, hard-capped" shape
    /// `ChangeFeedConfig` already applies to the change feed itself, kept as
    /// its own pair here (rather than reusing `ChangeFeedConfig`) since the
    /// two lanes page a different, independently-sized resource: a
    /// collection's outbox versus one subscription's bounded dead-letter
    /// ring.
    pub dead_letter_default_page_size: u32,
    pub dead_letter_max_page_size: u32,
}

impl Default for WebhookDeliveryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            batch_size: 200,
            poll_interval_ms: 1_000,
            request_timeout_ms: 10_000,
            max_attempts: 5,
            base_backoff_ms: 500,
            max_backoff_ms: 60_000,
            dead_letter_capacity: 1_000,
            dead_letter_default_page_size: 100,
            dead_letter_max_page_size: 1_000,
        }
    }
}

/// Config-gated background task wiring for the consumer-aware outbox
/// retention floor (`#115`, `crate::retention::compute_floor`): default OFF
/// — a deployment that never turns this on keeps every outbox growing
/// forever, byte-for-byte today's behavior. When `enabled`, the `tellurion`
/// server binary spawns one retention task per collection with a
/// resolvable outbox, computing the floor from every currently-registered
/// consumer (the index applier, the tile-generation consumer, and every
/// webhook subscription's own cursor — whichever of those are actually
/// running for that collection) and pruning at most `prune_batch_size` rows
/// per tick.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutboxRetentionConfig {
    pub enabled: bool,
    pub poll_interval_ms: u64,
    /// Bounded per-tick prune size (`OutboxSource::prune_before`'s
    /// `batch_size`) — one tick removes at most this many rows, regardless
    /// of how far the floor has advanced since the last tick.
    pub prune_batch_size: u32,
}

impl Default for OutboxRetentionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            poll_interval_ms: 60_000,
            prune_batch_size: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// Percentage (0.0..=100.0) of the detected memory limit reserved for the
    /// in-process tile cache.
    pub memory_percent: f64,
    /// L2 tile-cache backend, consulted only after an L1 (in-process) miss.
    /// Defaults to `L2CacheConfig::None` — no L2 layer at all, byte-identical
    /// behavior to a deployment with no `cache.l2` section.
    pub l2: L2CacheConfig,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            memory_percent: 25.0,
            l2: L2CacheConfig::None,
        }
    }
}

/// Selects the L2 tile-cache backend. `None` (the default) is L1-only.
/// `Valkey` names the environment variable holding the connection URL —
/// mirrors `StorageDecl.url_env`, so the URL itself never lives in config —
/// plus the TTL applied to every entry the backend stores. Selecting
/// `valkey` in a binary built without the crate's `valkey` cargo feature
/// fails fast at boot, the same way an unregistered storage `driver` does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "lowercase")]
pub enum L2CacheConfig {
    None,
    Valkey {
        url_env: String,
        #[serde(default = "default_l2_ttl_s")]
        ttl_s: u64,
    },
}

fn default_l2_ttl_s() -> u64 {
    3600
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StorageDecl {
    pub id: String,
    /// Driver name resolved against the `Registry` at startup.
    pub driver: String,
    /// Name of the environment variable holding the connection secret; the
    /// value itself never lives in config.
    pub url_env: String,
    /// Explicit connection-pool size override (read by the `postgis`
    /// driver; a driver with no pool concept of its own ignores it). `None`
    /// (the default) derives the size instead — cgroup-aware effective CPU
    /// count doubled for connection headroom, clamped to a sane range; see
    /// `tellurion-postgis`'s `pool::derive_pool_size` and
    /// `tellurion_core::resources::effective_cpu_count`. Precedence is
    /// explicit override > cgroup-derived value > hardcoded clamp bounds —
    /// the same "override wins outright" rule `CollectionDecl::table`/
    /// `.geometry`/`.pk` already follow for a collection's physical shape.
    #[serde(default)]
    pub pool_size: Option<usize>,
}

/// Where a managed asset's bytes live (assets-and-object-storage proposal,
/// first slice) — a first-class, sibling config concept to `storages`
/// above, deliberately never the same word: `storages` declares *data*
/// backends (where features live); `object_stores` declares *blob* backends
/// (where uploaded asset bytes live). The two never touch — a collection
/// opts into an object store by id (`CollectionDecl::object_store`)
/// entirely independently of its `storage`/`routing`. See `objectstore.rs`
/// for the port trait and the `fs` adapter this declares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectStoreDecl {
    pub id: String,
    #[serde(flatten)]
    pub profile: ObjectStoreProfile,
}

/// The profile-specific section `ObjectStoreDecl` carries — neutral core
/// (`id`, this tag) plus exactly what one backend needs, never a schema
/// shaped like one vendor's API (the proposal's own design rule). `fs` and
/// `s3` exist as of this slice; declaring any other `profile:` value
/// (`gcs`, `azure` — later slices per the proposal's own scoping) fails
/// config load with serde's own "unknown variant" error, the same "refuse
/// by name, at load, not at request time" idiom `AppConfig::validate` uses
/// everywhere else in this file — no separate check needed here for that
/// case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "profile", rename_all = "lowercase")]
pub enum ObjectStoreProfile {
    Fs {
        /// Absolute filesystem path this store writes objects under. Must
        /// already exist as a directory — checked at
        /// `objectstore::build_object_store` time (server boot), a named
        /// startup failure rather than a confusing first-upload I/O error.
        root: String,
    },
    /// The plain HTTP protocol any S3-compatible object store speaks
    /// (MinIO, Ceph RGW, Cloudflare R2, AWS S3 itself) — hand-rolled AWS
    /// Signature Version 4 over the canonical request (`sigv4.rs`), never a
    /// vendor SDK. Always path-style addressing
    /// (`{endpoint}/{bucket}/{key_prefix}{key}`), never virtual-hosted, so
    /// one `endpoint` works against a store with no DNS wildcarding of its
    /// own — see `objectstore::S3ObjectStore`'s own doc.
    S3 {
        /// This deployment's s3-compatible endpoint, e.g.
        /// `https://s3.us-east-1.amazonaws.com` or `http://localhost:9000`
        /// (MinIO). Must parse as an absolute `http(s)` URL — checked by
        /// `AppConfig::validate`.
        endpoint: String,
        bucket: String,
        region: String,
        /// Prepended to every object key this store writes under `bucket`
        /// — lets several collections/deployments share one bucket without
        /// colliding. Empty (the default) writes objects at the bucket
        /// root.
        #[serde(default)]
        key_prefix: String,
        /// Name of the environment variable holding the access-key id —
        /// mirrors `StorageDecl.url_env`/`L2CacheConfig::Valkey.url_env`:
        /// the secret itself never lives in config, only checked to be a
        /// non-empty variable NAME here; its value is read once at
        /// `objectstore::build_object_store` time (server boot), the same
        /// "named startup failure, not a confusing first-request error"
        /// rule `FsObjectStore::new`'s missing-root check already follows.
        access_key_env: String,
        /// Name of the environment variable holding the secret access key.
        secret_key_env: String,
        /// Presigned-URL lifetime in seconds (the `presigned-upload`
        /// conformance class) — `AppConfig::validate` requires it inside
        /// `1..=604_800` (SigV4's own maximum lifetime for long-term
        /// credentials). Absent from the YAML document defaults to
        /// [`default_presign_expiry_s`].
        #[serde(default = "default_presign_expiry_s")]
        presign_expiry_s: u64,
    },
}

/// 900 seconds (15 minutes) — long enough that a human operating a client
/// through a presigned upload/download rarely races the expiry, short
/// enough that a leaked URL doesn't stay valid for long.
fn default_presign_expiry_s() -> u64 {
    900
}

/// SigV4's own maximum presigned-URL lifetime for long-term (access-key)
/// credentials — a presigned URL requesting longer than this is refused by
/// every real S3-compatible endpoint at request time regardless of what
/// this deployment configures, so `AppConfig::validate` catches it at load
/// instead.
const MAX_PRESIGN_EXPIRY_S: u64 = 604_800;

/// Registry read/validation configuration (`#42`, registry scale-out).
/// `validation` governs `Router::validate_catalog`'s boot-time sweep (see
/// `RegistryValidationMode`) and applies identically no matter which
/// `backend` is selected — it is a property of *when* a collection gets
/// cross-checked against its storage, orthogonal to *where* the
/// catalog/collection declarations themselves are read from.
///
/// `backend` selects where `RegistryReader` (`registry.rs`) reads catalog
/// and collection declarations from: `file` (the default) is today's
/// unchanged behavior — `FileRegistryReader` indexes `AppConfig.catalogs`/
/// `.collections` in memory, no I/O, ever. `relational` reads them from a
/// database instead (`registry.rs`'s `RelationalRegistryFactory`).
///
/// `RegistryReader`'s own interface never carries a `TenantDecl` — only
/// `CatalogDecl`/`CollectionDecl`, each scoped by a tenant/catalog *internal
/// id* the caller already has. Tenant *declarations* read for routing
/// purposes go through the sibling `TenantReader` seam instead
/// (`tenant.rs`, `#143`), dispatched by [`build_tenant_reader`](crate::tenant::build_tenant_reader)
/// off this SAME `backend`/`storage` pair — a deployment does not get a
/// second knob to move its tenant declarations to a database independently
/// of its catalogs/collections. The normalized tenant snapshot is stored in
/// `ContextState` alongside the router/resolver and is authoritative for
/// runtime tenant settings, admission, readiness, and metrics. Auth and
/// policy declarations remain operator configuration and refer to tenant
/// internal ids. See `RelationalRegistryFactory`'s own doc for how a
/// `relational` backend connects.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RegistryConfig {
    pub validation: RegistryValidationMode,
    pub backend: RegistryBackend,
    /// The storage (an internal id from `AppConfig.storages`) a `relational`
    /// `backend` connects through — reuses that storage's own `url_env`
    /// rather than a second connection-config shape, the same way a
    /// collection's `storage`/`routing` fields already reference one.
    /// Ignored when `backend` is `file`; required, and checked by
    /// `AppConfig::validate` to resolve against a declared storage, when
    /// `backend` is `relational`. See `RelationalRegistryFactory`.
    #[serde(default)]
    pub storage: Option<String>,
    /// Which registered relational implementation a `relational` `backend`
    /// connects through (`#162`) — the stable name a driver crate's
    /// `RelationalRegistryFactory::name`/`RelationalTenantFactory::name`
    /// declares and that the binary registered at boot (`tellurion-postgis`
    /// registers `postgis`). One name selects both halves of this knob: the
    /// catalog/collection `RegistryReader` and the sibling `TenantReader`
    /// always come from the same driver crate.
    ///
    /// Ignored when `backend` is `file`, which is the direct built-in backend
    /// and has no factory to name.
    ///
    /// `None` — the default, and what every config written before `#162`
    /// says — means "the sole relational implementation this binary
    /// contains," which is exactly what `backend: relational` already meant
    /// when only one could ever be compiled in. It is deliberately NOT a
    /// baked-in `"postgis"`: a binary built without the postgis feature must
    /// keep failing with the same "no driver providing a relational registry
    /// factory" error it always did, not with a confusing "unknown
    /// implementation 'postgis'". Once a second relational implementation
    /// exists, leaving this unset is refused by name rather than resolved by
    /// registration or alphabetical order — see
    /// `registry::select_relational_implementation`, which owns the whole
    /// rule for both halves.
    ///
    /// `AppConfig::validate` only checks its shape (non-empty when present).
    /// Which implementations exist is a property of the binary, not of the
    /// document, and `validate` runs in contexts — config linting, tests,
    /// `tellurion-ingest` — with no registry in hand; boot and reload do the
    /// resolution and produce the named refusal.
    #[serde(default)]
    pub implementation: Option<String>,
}

/// Which store `RegistryReader` reads catalog/collection declarations from
/// (`#42`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistryBackend {
    /// `FileRegistryReader`, built from `AppConfig.catalogs`/`.collections`
    /// at boot or reload — no I/O, byte-for-byte the behavior every
    /// deployment already has.
    #[default]
    File,
    /// A relational `RegistryReader`, connected at boot/reload against
    /// `RegistryConfig::storage`. See `RelationalRegistryFactory`.
    Relational,
}

/// How `Router::validate_catalog`'s boot-time sweep runs (`#42`).
///
/// `Eager` (the default) is today's unchanged behavior: every collection is
/// cross-checked against its storage's `CatalogSource` once, at boot, so a
/// misconfigured collection fails startup with a named error before the
/// first request ever arrives — the right tradeoff for a file-backed
/// registry sized to a config file, where the full sweep is cheap and a bad
/// deploy is worth catching immediately.
///
/// `Lazy` skips that boot sweep entirely: a collection is validated the
/// first time a request resolves it, and the verdict — success, or an
/// `Error::Config` misconfiguration — is cached by `Router` (see
/// `descriptor.rs`'s `CachedDescriptor`) so a broken collection costs one
/// clear per-request error, never a slow boot, and a repeat request against
/// the same broken collection costs no extra backend round trip either.
/// The shape a registry sized past what a boot-time full sweep can afford
/// needs; see the registry-scale-out design note in `router.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistryValidationMode {
    #[default]
    Eager,
    Lazy,
}

/// Whitelisted, inheritable settings (`#39`): the only keys that flow down
/// the platform -> tenant -> catalog -> collection chain. Every field is
/// `Option` — `None` means "this level says nothing," letting a lower level
/// (or the platform default) show through. Resolution is per key, nearest
/// level wins, scalars replace outright and maps replace whole (`tile_caps`
/// is never merged entry-by-entry across levels) — see `settings.rs`.
/// Routing and storage declarations are deliberately absent from this type:
/// they are never inheritable, always explicit per collection.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsDecl {
    pub tile_caps: Option<ZoomCaps>,
    pub cache_ttl_s: Option<u64>,
    /// Slow-request logging threshold inherited through the settings chain.
    pub slow_request_ms: Option<u64>,
    /// Static STAC Collection metadata (`#36`) — license, keywords, and
    /// providers a STAC Collection response can't derive from the physical
    /// descriptor alone. Whitelisted the same way as the two settings
    /// above: nearest level wins, the whole subtree replaces (never merged
    /// field-by-field across levels).
    pub stac: Option<StacConf>,
    /// Vector-tile feature property allowlist (`#85`): the non-geometry
    /// column names a `TileSource` projects into an MVT feature's attribute
    /// table, beyond the primary key it always carries under the reserved
    /// `id` property. Whitelisted the same way as `stac` above — nearest
    /// level wins, the whole list replaces (never merged entry-by-entry
    /// across levels). `None`/absent (the default) means pk-only: no
    /// behavior change for a collection that never sets this. Reconciled
    /// against the collection's own derived attribute schema at
    /// boot-or-first-touch, the same seam `SchemaDecl` uses (see
    /// `descriptor::reconcile_tile_properties`) — an unknown column name is
    /// a named config error, never a silent drop at render time.
    pub tile_properties: Option<Vec<String>>,
    /// Single-band raster colormap (`#92`) for the COG PNG tile lane — a
    /// named built-in ramp or an explicit value -> RGBA stop list.
    /// Whitelisted the same way as `stac` above: nearest level wins, the
    /// whole value replaces (never merged stop-by-stop across levels).
    /// Meaningless for a multi-band (RGB/RGBA) raster; `tellurion-cog`
    /// refuses a tile request outright rather than ignore a configured
    /// colormap it can't honor (see that crate's `driver` module doc).
    pub colormap: Option<ColormapConf>,
    /// Item write-lane request body cap in bytes (`#91`) — enforced against
    /// the streamed length before a `PUT`/`POST` body is buffered into
    /// memory. Whitelisted the same way as the scalar keys above: nearest
    /// level wins. `None`/absent falls back to `settings::
    /// DEFAULT_MAX_REQUEST_BODY_BYTES`, sized for a single-feature write.
    pub max_request_body_bytes: Option<u64>,
    /// Per-tile vertex budget (`#90`): the total vertex count a single MVT
    /// tile's features may sum to, on top of (composing with, never
    /// replacing) the existing per-zoom feature cap and simplification
    /// tolerance. A dense single geometry can cost more to fetch, encode,
    /// and cache than thousands of simple ones — feature count alone never
    /// catches that; this bounds the actual geometry-complexity cost
    /// instead. A tile whose candidate rows sum past this budget has the
    /// marginal geometry dropped rather than served unbounded — see
    /// `tellurion-postgis::sql::build_mvt_budgeted_plan` and
    /// `tellurion-geopackage::driver`'s own vertex-counting encode loop for
    /// the two backends' enforcement. Whitelisted the same way as the
    /// scalar keys above: nearest level wins. `None`/absent falls back to
    /// `settings::DEFAULT_TILE_VERTEX_BUDGET`, generous enough that no
    /// currently-served tile should ever observe it.
    pub tile_vertex_budget: Option<u64>,
    /// Cumulative source-geometry vertex budget for one exact items page or
    /// single-item response. Unlike the tile budget, crossing this limit
    /// refuses the response rather than simplifying or dropping geometry.
    /// `None` falls back to `settings::DEFAULT_ITEMS_VERTEX_BUDGET`.
    pub items_vertex_budget: Option<u64>,
    /// GET-items page byte budget (`#184`): the cumulative serialized size
    /// in bytes one items page's features may sum to. Unlike
    /// `items_vertex_budget` above — which refuses an over-budget response
    /// outright — crossing this budget trims the page's tail instead: the
    /// longest front-to-back feature prefix that fits is served (never
    /// fewer than one feature, so paging always advances past an oversized
    /// row) and `next_token` is re-minted from the last kept feature's id
    /// so the dropped tail is re-served on the next page. Whitelisted the
    /// same way as the scalar keys above: nearest level wins. `None`/absent
    /// (the default) turns the lane off entirely — there is deliberately NO
    /// built-in default constant, so a deployment that never sets this
    /// observes exactly the pre-`#184` behavior. Applied in the features
    /// handler (`tellurion-features::handlers::list_items` via
    /// `crate::page_bytes`), never inside a `FeatureSource` decorator:
    /// trimming is response-shaping policy, not part of the source
    /// contract.
    pub page_max_bytes: Option<u64>,
    /// Direct-upload asset size cap in bytes (assets-and-object-storage
    /// proposal, first slice) — the same "declared size checked against a
    /// configured cap, at registration, before any storage I/O" rule
    /// `max_request_body_bytes` documents for the write lane, ridden down
    /// this identical platform -> tenant -> catalog -> collection chain.
    /// `None`/absent falls back to `settings::DEFAULT_MAX_ASSET_BYTES`.
    pub max_asset_bytes: Option<u64>,
    /// Media-type allow-list for both declared-upload (`type` on the Asset
    /// Object) and remote-asset registration. Whitelisted the same way as
    /// `tile_properties` above: nearest level wins, the whole list replaces
    /// (never merged entry-by-entry). `None`/absent (the default) means
    /// unrestricted — every declared media type is accepted, no behavior
    /// change for a collection that never sets this.
    pub asset_media_types: Option<Vec<String>>,
    /// Protocol exposure matrix (`#185`): which protocol roots this
    /// deployment actually serves at (and below) this level, plus the
    /// Features write lane as a key of its own. Whitelisted the same way as
    /// `stac`/`colormap` above — nearest level wins, the whole value
    /// replaces (never merged key-by-key across levels) — and governable by
    /// `final:` like any other settings key, so a platform operator can pin
    /// a protocol off for a tenant that cannot turn it back on.
    /// `None`/absent (the default) means no level in the chain ever
    /// expressed an opinion: every root is served, byte-for-byte the
    /// pre-`#185` behavior. There is deliberately NO built-in default
    /// constant to fall back to — the `page_max_bytes` (`#184`) precedent,
    /// see [`ProtocolsConf`] and `settings::EffectiveSettings::protocols`.
    pub protocols: Option<ProtocolsConf>,
    /// Named profile reference (`#111`): at most one per level. Expands as
    /// if the named `ProfileDecl`'s own keys were declared inline at this
    /// same level — an explicitly declared key here still wins over the
    /// profile's value for that key (see `settings::resolve_field`'s own
    /// doc for the exact per-level order). `None`/absent (the default)
    /// means this level names no profile. A list here is refused at parse
    /// (this field is a single id, never a sequence); a value naming an
    /// unknown profile, or a profile whose own `SettingsDecl.profile` is
    /// itself set (profile-of-profiles), is refused at config load — see
    /// `validate_profiles`/`validate_settings`.
    pub profile: Option<String>,
    /// Per-tenant admission control override (`#66`) — bounded queue depth,
    /// queue deadline, and fair-share weight. Whitelisted the same way as
    /// `stac`/`colormap` above: nearest level wins, the whole value
    /// replaces. See `admission::AdmissionDecl`'s own doc for why only the
    /// platform and tenant levels are ever actually consulted, even though
    /// this field (like every other `SettingsDecl` key) is technically
    /// settable at any of the four levels.
    ///
    /// `#156`: governable by `final:` like any other key — but because
    /// `validate_settings` already refuses a catalog- or collection-level
    /// `admission` outright, declaring `admission` final at the platform
    /// level governs exactly one level: the TENANT level, the only one
    /// below the platform that may legally carry an `admission` override.
    /// That is the protective budget the issue asks for: a platform that
    /// pins queue capacity, queue deadline, and fair-share weight so no
    /// tenant can raise its own. Finality's own walk does consult a level's
    /// `profile:` reference (`settings_provides_key`), but no profile can
    /// ever supply `admission` — `validate_profiles` runs `validate_settings`
    /// with `admission_allowed: false` — so for this key that branch is
    /// unreachable rather than a second way in.
    pub admission: Option<AdmissionDecl>,
    /// Batch-ingest byte/item budget and chunk size (`#114`) — grouped the
    /// same way as `admission` above, but (unlike `admission`) rides the
    /// FULL platform -> tenant -> catalog -> collection chain, the same one
    /// `max_request_body_bytes` uses: nearest level wins, the whole value
    /// replaces. See `batch::BatchDecl`'s own doc.
    pub batch: Option<BatchDecl>,
    /// Settings keys this level declares `final` (`#110`), by their wire
    /// name (see [`SETTINGS_KEY_NAMES`] for the fixed vocabulary) — a
    /// strictly lower level in the platform -> tenant -> catalog ->
    /// collection chain may not declare (override) any of them, whether
    /// directly or through its own `profile:` reference. Refused by name at
    /// both load (`AppConfig::validate`) and write (the same validation
    /// path the mutation endpoint and boot both run) — see
    /// `validate_settings_finality`. Serialized as `final` (a YAML/JSON
    /// string key, not a Rust identifier, hence `final_keys` here — `final`
    /// is a reserved word). Declaring `final_keys` at the collection level
    /// (the bottom of the chain) is accepted but has no effect: nothing
    /// sits below a collection to enforce it against. A profile's own
    /// `settings.final_keys` must stay empty — see `validate_profiles`.
    /// The same accepted-but-inert rule reaches one level higher for
    /// `admission` (`#156`), whose own chain bottoms out at the tenant
    /// level — see [`SETTINGS_KEY_NAMES`].
    #[serde(rename = "final")]
    pub final_keys: Vec<String>,
}

/// The fixed, whitelisted vocabulary of settings key names a level's own
/// `final:` list (`SettingsDecl::final_keys`, `#110`) may name — exactly
/// the field names `SettingsDecl` itself carries, `profile` excepted
/// (finality governs settings *values*, not the profile-reference
/// mechanism). Declaring a name outside this list is refused at load, the
/// same way an unknown `profile:` reference is.
///
/// `#156`: `admission` is in this vocabulary like every other key, but the
/// chain it governs is shorter than the other keys'. A catalog- or
/// collection-level `admission` is ALREADY refused unconditionally by
/// [`validate_settings`] ("only honored at the platform or tenant level" —
/// admission runs before routing resolves a catalog or collection), so
/// declaring `admission` final adds exactly one thing over that standing
/// refusal: it closes the **tenant** level, the only level below the
/// platform that may legally carry an `admission` override at all. That is
/// the whole point of the key here — a platform operator pinning queue
/// capacities and fair-share weights so no tenant can raise its own budget.
/// Declaring `admission` final at the tenant level is therefore
/// accepted-but-inert, exactly as `final:` at the collection level (the
/// bottom of the four-level chain) already is: the tenant level is the
/// bottom of `admission`'s own chain, and nothing below it could carry a
/// legal `admission` override for the declaration to bite on.
pub const SETTINGS_KEY_NAMES: &[&str] = &[
    "tile_caps",
    "cache_ttl_s",
    "slow_request_ms",
    "stac",
    "tile_properties",
    "colormap",
    "max_request_body_bytes",
    "tile_vertex_budget",
    "items_vertex_budget",
    "page_max_bytes",
    "max_asset_bytes",
    "asset_media_types",
    "admission",
    "batch",
    "protocols",
];

/// Whether one protocol surface is served (`#185`). A plain two-state enum
/// rather than a `bool` because it is written by operators in YAML, where
/// `stac: disabled` says what `stac: false` only implies, and because the
/// wire vocabulary the issue specifies (`enabled`/`disabled`) is the one an
/// operator reads back out of `/config/effective`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolExposure {
    #[default]
    Enabled,
    Disabled,
}

impl ProtocolExposure {
    pub fn is_enabled(self) -> bool {
        matches!(self, ProtocolExposure::Enabled)
    }
}

/// The `settings.protocols` value (`#185`): one exposure decision per
/// protocol root this server mounts, plus `features_write` for the Features
/// write lane.
///
/// Every field defaults to [`ProtocolExposure::Enabled`], and the whole
/// value replaces down the chain like every other settings group — so a
/// level that declares `protocols: { tiles: disabled }` is declaring "tiles
/// off, everything else on," never "tiles off, everything else whatever my
/// parent said." That is the same whole-value-replacement rule `tile_caps`
/// and `stac` already follow, and it is why an operator turning one
/// protocol off at the catalog level sees the catalog's block, not a merge,
/// in `/config/effective`.
///
/// Key names are the URL path segments themselves (`crate::` has no
/// `Protocol` type — that lives in `tellurion-server::protocol`, which owns
/// the segment vocabulary and maps each variant onto the field here), so
/// what an operator disables reads exactly like the prefix that stops
/// answering.
///
/// `features_write` is deliberately not "features, but narrower": read and
/// write exposure are different decisions, and they get different HTTP
/// answers. `features: disabled` removes the whole root — nothing at that
/// prefix answers, so `404`. `features_write: disabled` leaves every read
/// serving and refuses only the write methods, which live on the *same*
/// URIs as those reads; a `404` there would claim a resource does not exist
/// while the `GET` on that exact URI keeps returning it. OGC API Features
/// Part 4 defines `405` for precisely that shape ("the resource only
/// supports GET requests") and explicitly allows a server to implement only
/// a subset of the write methods, so a disabled write lane answers `405`
/// with a truthful `Allow` instead.
/// `serde` default for [`ProtocolsConf::records`] and
/// [`ProtocolsConf::processes`] — see those fields' docs for why these two
/// keys default to `disabled` while every other defaults to `enabled`.
fn opt_in_exposure_default() -> ProtocolExposure {
    ProtocolExposure::Disabled
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProtocolsConf {
    pub features: ProtocolExposure,
    /// The OGC API Features Part 4 write lane (`POST`/`PUT`/`PATCH`/
    /// `DELETE` under the `features` root, batch ingest included). Only
    /// meaningful while `features` itself is enabled: a disabled `features`
    /// root takes the write methods down with it, since the paths they live
    /// on are gone.
    pub features_write: ProtocolExposure,
    pub tiles: ProtocolExposure,
    pub styles: ProtocolExposure,
    /// The 3D Tiles root — named for its URL segment (`/{tenant}/3dtiles/
    /// ...`), not for the crate that serves it (`tellurion-places`), so the
    /// key an operator writes matches the prefix that stops answering.
    #[serde(rename = "3dtiles")]
    pub three_d_tiles: ProtocolExposure,
    pub stac: ProtocolExposure,
    /// The OGC API — Records root (`#192`). The one key here that defaults
    /// to [`ProtocolExposure::Disabled`], and the asymmetry is deliberate:
    /// the other five roots default to `enabled` because they were already
    /// being served when `#185` gave operators a key to turn them off, so
    /// `enabled` *is* "what this deployment already did". For `records`,
    /// "what this deployment already did" is *nothing at all* — no
    /// `/records` prefix answered, no `records` link appeared in the tenant
    /// directory, and no Records classes were in any conformance list. A
    /// default of `enabled` would change every existing deployment's tenant
    /// directory the moment this field shipped. So the lane is opt-in:
    /// `protocols: { records: enabled }` is the operator asking for it, and
    /// until they do, the root answers exactly the `404` an unmounted prefix
    /// answers (`app::enforce_protocol_exposure`).
    ///
    /// Independent of [`CollectionKind::Record`](crate::CollectionKind):
    /// this key says whether the *root* is served, the kind says which
    /// collections appear under it. Enabling the root in a catalog with no
    /// record collection yields an empty, honest `/collections` listing, the
    /// same way an empty catalog's Features root does.
    #[serde(default = "opt_in_exposure_default")]
    pub records: ProtocolExposure,
    /// The OGC API — Processes root (`#182`). Defaults to
    /// [`ProtocolExposure::Disabled`], for exactly the reason
    /// [`records`](Self::records) above does: "what this deployment already
    /// did" is *nothing at all* — no `/processes` prefix answered, no
    /// `processes` link appeared in the tenant directory, and no Processes
    /// classes were in any conformance list — so `enabled` would change every
    /// existing deployment's tenant directory the moment this field shipped.
    ///
    /// Enabling it is necessary but NOT sufficient. The root also needs a
    /// deployment-wide durable job ledger and at least one runner compiled
    /// into this binary ([`ProcessesConfig`],
    /// `crate::process::ProcessRegistry`); with either missing, the prefix
    /// answers the same `404` an unmounted one answers even with this key set
    /// to `enabled`. That asymmetry is deliberate: a Processes root that
    /// accepts a job it cannot durably record, or advertises processes
    /// nothing can execute, is the half-working surface `#182` exists to
    /// avoid — better no root than a root that lies.
    #[serde(default = "opt_in_exposure_default")]
    pub processes: ProtocolExposure,
}

/// Hand-written rather than derived because [`ProtocolsConf::records`] and
/// [`ProtocolsConf::processes`] do not share the other five fields' `enabled`
/// default — see those fields' own docs. Deriving `Default` here would
/// silently expose the Records and Processes roots to every deployment that
/// never asked for them.
impl Default for ProtocolsConf {
    fn default() -> Self {
        Self {
            features: ProtocolExposure::Enabled,
            features_write: ProtocolExposure::Enabled,
            tiles: ProtocolExposure::Enabled,
            styles: ProtocolExposure::Enabled,
            three_d_tiles: ProtocolExposure::Enabled,
            stac: ProtocolExposure::Enabled,
            records: opt_in_exposure_default(),
            processes: opt_in_exposure_default(),
        }
    }
}

/// One named settings profile (`#111`): a reusable fragment of the exact
/// same whitelisted settings surface `SettingsDecl` carries — routing,
/// storages, identity, and auth are structurally absent, not merely
/// forbidden by a runtime check, because `SettingsDecl` itself has no
/// fields for any of them. Referenced by id from any single level of the
/// platform -> tenant -> catalog -> collection chain (`SettingsDecl::
/// profile`); never a list, never another profile (a profile's own
/// `settings.profile` is refused at load — see `validate_profiles`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileDecl {
    pub id: String,
    #[serde(flatten)]
    pub settings: SettingsDecl,
}

/// Static declared collection metadata (`#36`, slice A): license, keywords,
/// and providers for the `stac:` config subtree, plus the responsible-party
/// `contacts` list (`#187`, first slice). Every field left unset falls
/// back to `tellurion-stac`'s own defaults (see that crate's `mapping` and
/// `iso19139` modules) — this struct only carries what an operator actually
/// declared.
///
/// Deliberately *not* joined by a second settings key: `stac` is already in
/// the closed [`SETTINGS_KEY_NAMES`] vocabulary and already resolves as a
/// whole-value replacement down the platform -> tenant -> catalog ->
/// collection chain, so declared descriptor metadata with no key of its own
/// (`contacts`) extends this block instead of introducing another governed
/// key with its own finality, provenance, and effective-view plumbing. The
/// name `stac` is historical: this is the descriptor's declared-metadata
/// block, and not every field in it projects into STAC (see `contacts`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StacConf {
    /// SPDX license identifier/expression, or `other` — see the STAC
    /// Collection spec's `license` field. `None` lets `tellurion-stac` apply
    /// its own default rather than fabricating one here.
    pub license: Option<String>,
    pub keywords: Vec<String>,
    pub providers: Vec<StacProvider>,
    /// Declared STAC Collection `assets` (`#36` slice 1, "a real,
    /// driver-neutral assets model"): a link this deployment doesn't derive
    /// from live routing capabilities at all — documentation, a thumbnail,
    /// an external download — keyed by asset id. The operator-declared
    /// counterpart to `tellurion_stac::assets::collection_assets`'s
    /// capability-derived assets (`#48`); both end up in the same STAC
    /// Collection `assets` object, with a declared entry winning outright
    /// over a capability-derived one sharing the same id (see
    /// `tellurion_stac::mapping::to_stac_collection`). Keyed by a
    /// `BTreeMap`, not a list, so a duplicate asset id is impossible by
    /// construction — the same shape `ZoomCaps` already uses for the same
    /// reason.
    pub assets: BTreeMap<String, AssetDecl>,
    /// Responsible-party contacts for this collection (`#187`, first
    /// slice). Empty by default, and an empty list is meaningfully
    /// different from a declared one: every projection that consumes this
    /// keeps its pre-`#187` output byte-for-byte when nobody declares a
    /// contact (`tellurion_stac::iso19139` keeps emitting `gmd:contact`
    /// with `gco:nilReason="unknown"`), so this field costs an existing
    /// deployment nothing.
    ///
    /// Distinct from `providers`, which is the STAC Collection spec's own
    /// Provider Object and stays the only thing projected into STAC. A
    /// contact is a *person or role to reach*, which STAC has no slot for
    /// and ISO 19115 requires (`MD_Metadata/contact` is `1..*`) — see
    /// `tellurion_stac::iso19139`'s module doc.
    pub contacts: Vec<ContactDecl>,
    /// Declared lineage/provenance for this collection (`#50`, lineage
    /// slice) — see [`LineageDecl`] for the model and for why the operator's
    /// declaration is the only honest source this workspace has. `None`
    /// (the default) emits nothing: the ISO 19139 projection's
    /// `gmd:dataQualityInfo` element simply never appears, and every
    /// pre-existing document is byte-for-byte unchanged. Skipped on
    /// serialize when absent so a published registry `decl` and every other
    /// serialized `StacConf` keep their prior bytes too — the same
    /// keep-the-wire-stable shape `service_assets` below uses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lineage: Option<LineageDecl>,
    /// Whether this collection's STAC documents still carry the
    /// capability-derived *service* asset templates (`#220`). Defaults to
    /// [`ServiceAssetsMode::Templated`] — the pre-`#220` document,
    /// byte-for-byte — and is the operator's switch, not this server's.
    /// See [`ServiceAssetsMode`].
    #[serde(default, skip_serializing_if = "ServiceAssetsMode::is_templated")]
    pub service_assets: ServiceAssetsMode,
}

/// How a collection's STAC `assets` map represents the *service* surfaces
/// this deployment derives from live routing capabilities — the MVT, PNG,
/// styled-PNG and glTF-binary tile templates
/// `tellurion_stac::assets::collection_assets` materializes (`#48`).
///
/// `#220`: a STAC client treats an Asset Object's `href` as a retrievable
/// representation, while those four entries are RFC 6570-shaped API
/// templates carrying a `templated` flag the STAC Asset Object does not
/// define. The truthful expression of "this collection is also served over
/// tiles/maps/3D" is a rel-typed *link* to the resource that describes
/// those surfaces, which is what `tellurion-server`'s link contributors
/// emit. Retiring the templates is nonetheless a client-visible change —
/// somebody is reading `assets.mvt.href` today — so it is an operator's
/// decision, taken per level of the ordinary settings chain, never a
/// default this server invents.
///
/// Only the capability-derived service entries are in scope either way.
/// Operator-declared `stac.assets` (a thumbnail, a licence document) and
/// the per-item asset records (`#221`: a scene's own COG or Zarr store)
/// are genuine Asset Objects with literal hrefs and are untouched by this
/// setting — the issue's own "source COG/Zarr/download/thumbnail objects
/// remain STAC assets" rule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAssetsMode {
    /// Keep materializing the templated service assets (the default, and
    /// exactly what every deployment written before `#220` emits).
    #[default]
    Templated,
    /// Leave the service surfaces to the typed capability links alone; the
    /// `assets` map then carries only literal, directly retrievable Asset
    /// Objects.
    Links,
}

impl ServiceAssetsMode {
    /// The `skip_serializing_if` hook on [`StacConf::service_assets`]: an
    /// operator who never wrote this key reads back an effective-config
    /// view (`/config/effective`) byte-for-byte what it was before the key
    /// existed.
    pub fn is_templated(&self) -> bool {
        matches!(self, ServiceAssetsMode::Templated)
    }
}

/// One STAC Collection `providers[]` entry (the STAC Collection spec's
/// Provider Object). `roles` is free-form here — the spec's suggested
/// values (`licensor`/`producer`/`processor`/`host`) are not enforced by
/// this config model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StacProvider {
    pub name: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// One `stac.contacts` entry (`#187`, first slice): a responsible party for
/// the collection, in the shape ISO 19115's `CI_ResponsibleParty` actually
/// needs — an individual name, an organization, an electronic address, a
/// role, and a link. `name` is the only required field, the same
/// "require exactly what the target vocabulary requires" rule
/// [`AssetDecl`]/[`StacProvider`] already follow; everything else stays
/// `Option` all the way to the wire, where an absent field means an omitted
/// XML element rather than an empty string.
///
/// `role` is free-form, like [`StacProvider::roles`]: ISO's
/// `CI_RoleCode` codelist (`pointOfContact`, `custodian`, `owner`,
/// `distributor`, …) is the vocabulary a projection will use it under, but
/// this config model does not enforce membership in it — an operator
/// mirroring a foreign catalog's role string is not a boot failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContactDecl {
    pub name: String,
    #[serde(default)]
    pub organization: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// Operator-declared lineage/provenance for one collection (`#50`, lineage
/// slice — the decision `#187` deferred: "what can tellurion honestly assert
/// about the provenance of data it does not produce?"). The answer this
/// models: nothing of its own — only what the operator asserts. None of this
/// workspace's machinery persists a collection-level provenance fact the
/// server could read back at request time (the `#191` harvester's source URL
/// lives in its CLI-side resume bookmark and NDJSON report only; the `#202`
/// sidecar is a per-item document channel; a file-backed driver's source
/// path lives in a `url_env` environment variable the config layer treats
/// as a connection secret), so a declaration down the ordinary settings
/// chain is the one honest source, exactly like [`StacConf::contacts`].
///
/// Consumed by the ISO 19139 projection only
/// (`gmd:dataQualityInfo/gmd:DQ_DataQuality/gmd:lineage/gmd:LI_Lineage` —
/// see `tellurion_stac::iso19139`); the STAC Collection projection ignores
/// it, the same split `contacts` already draws (STAC has no collection-level
/// lineage slot). Undeclared (`None` on [`StacConf::lineage`]) means the
/// element is never emitted at all — the projections' pre-existing output is
/// byte-for-byte unchanged, the same compatibility bar `contacts` set.
///
/// A declared block must carry at least one member, and no member may be
/// blank — `validate_settings` refuses the empty shapes by name at load,
/// because the only things a projection could do with them (fabricate an
/// empty `gmd:LI_Lineage`, or silently drop the declaration) are both
/// unacceptable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LineageDecl {
    /// `LI_Lineage/statement`: the general, free-text explanation of the
    /// data's provenance ("Digitised from the 1:25000 IGM series", …).
    pub statement: Option<String>,
    /// `LI_Lineage/source/LI_Source` entries — the datasets this
    /// collection's content was derived from, one per source.
    pub sources: Vec<LineageSourceDecl>,
    /// `LI_Lineage/processStep/LI_ProcessStep` entries — the events that
    /// produced this collection's content, one per step, in declaration
    /// order.
    pub process_steps: Vec<LineageProcessStepDecl>,
}

impl LineageDecl {
    /// Whether this declaration carries no fact at all — the shape
    /// `validate_settings` refuses by name (`lineage: {}` is an operator
    /// mistake, not an assertion).
    pub fn is_empty(&self) -> bool {
        self.statement.is_none() && self.sources.is_empty() && self.process_steps.is_empty()
    }
}

/// One `LI_Source` in a [`LineageDecl`]: `description` is its only field
/// because that is the only `LI_Source` property a free-text operator
/// declaration can honestly fill — a source citation would demand a
/// title-plus-date `CI_Citation` this model has no facts for, and the
/// description alone already satisfies the element's own "at least a
/// description or a sourceExtent" constraint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageSourceDecl {
    pub description: String,
}

/// One `LI_ProcessStep` in a [`LineageDecl`]: `description` is required
/// here for the same reason it is mandatory (`1`) on the ISO element itself
/// — a process step that describes nothing asserts nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageProcessStepDecl {
    pub description: String,
}

/// One `stac.assets` entry (`#36` slice 1) — the STAC Asset Object's own
/// shape: `href` is the only field the spec requires, so it's the only
/// required field here too; `type`/`title`/`roles` are genuinely optional
/// and stay that way all the way to the wire (`tellurion_stac::model::
/// StacAsset` omits each one when absent rather than fabricating an empty
/// string or an empty array — see that type's own doc). The map key this
/// lives under (`StacConf::assets`) is the asset id; there is no separate
/// `id` field here, the same convention `ZoomCaps`'s zoom-keyed map uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetDecl {
    pub href: String,
    /// STAC Asset Object `type` — a media type. `None` when the operator
    /// didn't declare one.
    #[serde(rename = "type", default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantDecl {
    pub id: String,
    /// Public URL segment (`/{external_id}/...`); defaults to `id` when
    /// omitted. `id` never appears in a URL or a response body.
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub settings: SettingsDecl,
}

impl TenantDecl {
    pub fn external_id(&self) -> &str {
        self.external_id.as_deref().unwrap_or(&self.id)
    }
}

/// Cross-tenant visibility for a catalog or collection (`#34` policy layer,
/// authorization directive 3). Absent (this type's own `Default`, both
/// fields at their zero value) is fully private: visible only to a subject
/// with membership in the resource's own owning tenant — see `policy.rs`'s
/// module doc for the isolation rule this feeds. Deny-by-default across
/// tenants: nothing here ever widens access on its own, only
/// [`policy::authorize_resource`](crate::policy::authorize_resource)
/// consults it, and only as one of several conditions that must hold.
///
/// A collection's *effective* visibility is resolved against its owning
/// catalog's the same "nearest level wins, whole value replaces" way
/// `SettingsDecl` resolves each key (`settings.rs`): a collection that
/// declares any non-default visibility of its own (`public: true`, or a
/// non-empty `shared_with`) wins outright over its catalog's; a collection
/// that declares neither inherits its catalog's value unchanged. See
/// `Router::effective_visibility`, which materializes this once per
/// collection at build time, the same way `Router::effective_settings`
/// already does for the platform -> tenant -> catalog -> collection chain.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VisibilityDecl {
    /// Visible to every tenant (and to an anonymous subject, when `auth` is
    /// configured but no credential was presented) — the widest setting.
    pub public: bool,
    /// Tenant *internal* ids (the same convention `CatalogDecl::tenant`
    /// uses) this resource is additionally visible to, beyond its own
    /// owning tenant. A subject with membership in any listed tenant clears
    /// the isolation check for this resource even without membership in the
    /// resource's own tenant.
    #[serde(default)]
    pub shared_with: Vec<String>,
}

impl VisibilityDecl {
    /// Whether this is the zero value (`public: false`, `shared_with`
    /// empty) — the signal `Router::effective_visibility` uses to decide
    /// whether a collection's own declaration should win over its catalog's,
    /// or fall through to it. See this type's own doc.
    pub(crate) fn is_default(&self) -> bool {
        !self.public && self.shared_with.is_empty()
    }
}

/// A catalog belongs to exactly one tenant (`tenant`, the tenant's internal
/// `id`) but is never nested inside it in config — ownership is by
/// reference, the same shape `CollectionDecl::catalog` uses one level down.
/// Every `/{tenant_external_id}/{protocol}/catalogs/{catalog_external_id}`
/// prefix is a full OGC API root for that catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogDecl {
    pub id: String,
    #[serde(default)]
    pub external_id: Option<String>,
    /// The owning tenant's internal id.
    pub tenant: String,
    #[serde(default)]
    pub settings: SettingsDecl,
    /// This catalog's own cross-tenant visibility (`#34`). Absent (the
    /// default) is fully private — see `VisibilityDecl`'s own doc.
    #[serde(default)]
    pub visibility: VisibilityDecl,
}

impl CatalogDecl {
    pub fn external_id(&self) -> &str {
        self.external_id.as_deref().unwrap_or(&self.id)
    }
}

/// Fallback feature cap for a zoom with no explicit or nearest-lower-zoom cap
/// configured (e.g. an empty `caps` table). Defined once here so the tiles
/// handlers and every storage driver apply the same rule.
pub const DEFAULT_TILE_CAP: u64 = 5_000;

/// Sparse per-zoom feature caps, serialized as a `{ "z0": 2000, "z10": 20000 }`
/// map. Only zooms with an explicit cap are populated; [`ZoomCaps::get`] is
/// exact-match, [`ZoomCaps::effective`] applies the documented fallback.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ZoomCaps(pub BTreeMap<u8, u64>);

impl ZoomCaps {
    pub fn get(&self, zoom: u8) -> Option<u64> {
        self.0.get(&zoom).copied()
    }

    /// The explicit cap for `zoom`: an exact match if configured, else the
    /// cap of the nearest lower zoom that has one, else `None` when nothing
    /// at or below `zoom` is configured at all. Distinguishes "the operator
    /// tuned nothing here" from "the operator tuned a lower zoom and this
    /// one inherits it" — both are [`effective`](Self::effective)'s job to
    /// treat identically; only `descriptor::heuristics::effective_feature_cap`
    /// needs the distinction, to know when it may fill the gap with a
    /// derived value instead of [`DEFAULT_TILE_CAP`].
    pub fn explicit(&self, zoom: u8) -> Option<u64> {
        self.0.range(..=zoom).next_back().map(|(_, cap)| *cap)
    }

    /// The cap to apply at `zoom`: [`explicit`](Self::explicit) if any, else
    /// [`DEFAULT_TILE_CAP`]. See `descriptor::heuristics::effective_feature_cap`
    /// for the row-estimate-aware version that fills the gap `explicit`
    /// leaves with a heuristic instead of jumping straight to this flat
    /// default.
    pub fn effective(&self, zoom: u8) -> u64 {
        self.explicit(zoom).unwrap_or(DEFAULT_TILE_CAP)
    }
}

impl Serialize for ZoomCaps {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let map: BTreeMap<String, u64> = self
            .0
            .iter()
            .map(|(zoom, cap)| (format!("z{zoom}"), *cap))
            .collect();
        map.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ZoomCaps {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw: BTreeMap<String, u64> = BTreeMap::deserialize(deserializer)?;
        let mut caps = BTreeMap::new();
        for (key, cap) in raw {
            let zoom = key
                .strip_prefix('z')
                .and_then(|s| s.parse::<u8>().ok())
                .ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "invalid zoom cap key '{key}', expected 'zN' (e.g. 'z10')"
                    ))
                })?;
            caps.insert(zoom, cap);
        }
        Ok(ZoomCaps(caps))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TilesConf {
    pub minzoom: u8,
    pub maxzoom: u8,
    pub caps: ZoomCaps,
}

impl Default for TilesConf {
    fn default() -> Self {
        Self {
            minzoom: 0,
            maxzoom: 14,
            caps: ZoomCaps::default(),
        }
    }
}

impl TilesConf {
    fn validate(&self, collection_id: &str) -> Result<()> {
        if self.minzoom > self.maxzoom {
            return Err(Error::Config(format!(
                "collection '{collection_id}': minzoom ({}) > maxzoom ({})",
                self.minzoom, self.maxzoom
            )));
        }
        if self.maxzoom > MAX_ZOOM {
            return Err(Error::Config(format!(
                "collection '{collection_id}': maxzoom ({}) exceeds {MAX_ZOOM}",
                self.maxzoom
            )));
        }
        for (zoom, cap) in &self.caps.0 {
            if *zoom < self.minzoom || *zoom > self.maxzoom {
                return Err(Error::Config(format!(
                    "collection '{collection_id}': cap for zoom {zoom} outside [{}, {}]",
                    self.minzoom, self.maxzoom
                )));
            }
            if *cap == 0 {
                return Err(Error::Config(format!(
                    "collection '{collection_id}': cap for zoom {zoom} must be > 0"
                )));
            }
        }
        Ok(())
    }
}

/// One additional pre-generalized geometry column a collection's tiles lane
/// reads instead of the base `geometry` column, for the zoom range
/// `[minzoom, maxzoom]` (`#104`). Entirely declarative: the operator
/// produces the column (however they like -- a materialized simplification,
/// a generalization pass, anything that lands a real geometry column on the
/// same table), and tellurion only ever reads whichever one
/// `CollectionDecl::resolved_geometry_for_zoom` selects -- it never generates
/// or refreshes one itself. See `Router::validate_catalog` for the
/// boot-time check that the column actually exists and shares the base
/// column's SRID and geometry type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeometryVariantDecl {
    /// The physical column name this variant reads, exactly as the operator
    /// produced it -- no naming convention enforced.
    pub column: String,
    pub minzoom: u8,
    pub maxzoom: u8,
}

/// Eager, shape-only validation for `geometry_variants` (`#104`): a variant's
/// column name must be non-empty and declared at most once, its own zoom
/// range must be well-formed (`minzoom <= maxzoom`) and fall within this
/// collection's own `tiles` range, and no two variants' zoom ranges may
/// overlap -- an overlap would leave "which variant serves this zoom"
/// undefined, the same "an explicit startup failure beats an arbitrary pick"
/// rule `#104`'s ambiguous-geometry-column fix already applies one level up.
/// Whether each declared column actually exists on the backend, and shares
/// the base column's SRID/geometry type, needs a built `Router` and is
/// checked in `Router::validate_catalog` instead -- the same "shape here,
/// backend reality there" split every other lane/capability check in this
/// file already follows.
fn validate_geometry_variants(
    collection_id: &str,
    tiles: &TilesConf,
    variants: &[GeometryVariantDecl],
) -> Result<()> {
    let mut seen_columns = HashSet::new();
    let mut seen_ranges: Vec<(u8, u8)> = Vec::new();
    for variant in variants {
        if variant.column.is_empty() {
            return Err(Error::Config(format!(
                "collection '{collection_id}': geometry_variants declares an empty column name"
            )));
        }
        if !seen_columns.insert(variant.column.as_str()) {
            return Err(Error::Config(format!(
                "collection '{collection_id}': geometry_variants declares column '{}' more than once",
                variant.column
            )));
        }
        if variant.minzoom > variant.maxzoom {
            return Err(Error::Config(format!(
                "collection '{collection_id}': geometry_variants entry '{}' minzoom ({}) > maxzoom ({})",
                variant.column, variant.minzoom, variant.maxzoom
            )));
        }
        if variant.minzoom < tiles.minzoom || variant.maxzoom > tiles.maxzoom {
            return Err(Error::Config(format!(
                "collection '{collection_id}': geometry_variants entry '{}' zoom range [{}, {}] outside this collection's tiles range [{}, {}]",
                variant.column, variant.minzoom, variant.maxzoom, tiles.minzoom, tiles.maxzoom
            )));
        }
        for (existing_min, existing_max) in &seen_ranges {
            if variant.minzoom <= *existing_max && *existing_min <= variant.maxzoom {
                return Err(Error::Config(format!(
                    "collection '{collection_id}': geometry_variants entry '{}' zoom range [{}, {}] overlaps another variant's range [{existing_min}, {existing_max}]",
                    variant.column, variant.minzoom, variant.maxzoom
                )));
            }
        }
        seen_ranges.push((variant.minzoom, variant.maxzoom));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleConf {
    pub fill: String,
    pub stroke: String,
    pub stroke_width: f64,
}

impl Default for StyleConf {
    fn default() -> Self {
        Self {
            fill: "#3388ff66".to_string(),
            stroke: "#3366cc".to_string(),
            stroke_width: 1.0,
        }
    }
}

/// Built-in single-band colormap ramps (`#92`) — deliberately small (this
/// slice's own scope is "a couple of built-in ramps," not a palette
/// library). An operator who needs anything else declares an explicit
/// `ColormapConf::Stops` list instead. The actual color science for each
/// ramp lives in `tellurion-cog::colormap`, not here — this is only the
/// declared choice of ramp, the same split `StyleConf`'s hex-string fields
/// draw against `tellurion-render::style::RenderStyle`'s parsed RGBA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorRamp {
    Grayscale,
    Viridis,
}

/// One explicit value -> RGBA stop (`#92`, `ColormapConf::Stops`). `rgba`'s
/// own alpha channel is an operator's only lever for making a specific
/// sample value (a nodata sentinel, say) render transparent — this config
/// model has no separate nodata concept of its own.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColormapStop {
    pub value: f64,
    pub rgba: [u8; 4],
}

/// A single-band COG colormap declaration (`#92`, first slice): either a
/// named built-in ramp linearly interpolated across `[min, max]`, or an
/// explicit value -> RGBA stop list linearly interpolated between
/// consecutive stops (and clamped to the nearest end stop outside the
/// declared range). Resolved through the same settings-inheritance chain as
/// `stac`/`tile_caps` (`SettingsDecl`, `settings.rs`) — see
/// `Router::apply_inherited_settings` for how the resolved value reaches a
/// driver, and `tellurion-cog::colormap` for how it's actually applied to a
/// raw sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ColormapConf {
    Ramp { ramp: ColorRamp, min: f64, max: f64 },
    Stops { stops: Vec<ColormapStop> },
}

impl ColormapConf {
    /// Eager, fail-boot validation (`#92`) — the same "bad shape fails at
    /// load time, not mid-request" contract `TilesConf::validate`/
    /// `StacConf::validate` already give their own blocks: a `Ramp` needs a
    /// non-empty domain; a `Stops` list needs at least one stop and
    /// strictly ascending values (a duplicate or out-of-order value would
    /// make the bracket search `tellurion-cog::colormap` does over them
    /// ill-defined).
    fn validate(&self, context: &str) -> Result<()> {
        match self {
            ColormapConf::Ramp { min, max, .. } => {
                if *min >= *max {
                    return Err(Error::Config(format!(
                        "{context}.colormap: min ({min}) must be less than max ({max})"
                    )));
                }
            }
            ColormapConf::Stops { stops } => {
                if stops.is_empty() {
                    return Err(Error::Config(format!(
                        "{context}.colormap: stops must not be empty"
                    )));
                }
                for pair in stops.windows(2) {
                    if pair[1].value <= pair[0].value {
                        return Err(Error::Config(format!(
                            "{context}.colormap: stops must be strictly ascending by value ({} then {})",
                            pair[0].value, pair[1].value
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// A stable, process-local hash of this colormap's own content (`#92`):
    /// folded into the raster tile cache key
    /// (`tellurion-tiles::handlers::raster_tile_response`) so a config
    /// reload that changes a collection's colormap never serves the
    /// previous colormap's cached PNG bytes for the same tile — the tile
    /// cache is deliberately NOT part of `AppContext`'s atomically swapped
    /// reload state (see that module's own doc), so without this a stale
    /// render would otherwise sit under an unchanged key indefinitely. Same
    /// `DefaultHasher` choice and same "process-local only, not a stored
    /// identity" reasoning as `Filter::fingerprint`.
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        match self {
            ColormapConf::Ramp { ramp, min, max } => {
                0u8.hash(&mut hasher);
                ramp.hash(&mut hasher);
                min.to_bits().hash(&mut hasher);
                max.to_bits().hash(&mut hasher);
            }
            ColormapConf::Stops { stops } => {
                1u8.hash(&mut hasher);
                for stop in stops {
                    stop.value.to_bits().hash(&mut hasher);
                    stop.rgba.hash(&mut hasher);
                }
            }
        }
        hasher.finish()
    }
}

/// 3D-places extrusion config: which MVT feature properties carry height, so
/// the Glb lane (`extrude_mvt_to_glb`) can turn a footprint polygon into a
/// prism without any new SQL — the driver already ships these as ordinary
/// tile properties.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Places3dConf {
    /// MVT feature property carrying the extrusion height. Required — there
    /// is no sensible default for "which property is height".
    pub height_property: String,
    #[serde(default)]
    pub min_height_property: Option<String>,
    #[serde(default = "default_places3d_height")]
    pub default_height: f64,
    #[serde(default = "default_places3d_exaggeration")]
    pub exaggeration: f64,
    /// Per-zoom vertex-count budget for the `VolumeSource` lane (`#41`):
    /// same `ZoomCaps` shape and override-wins-else-heuristic precedence
    /// `TilesConf.caps` uses for MVT feature counts — see
    /// `descriptor::heuristics::effective_volume_vertex_cap`. A solid
    /// exceeding the effective budget for the tile's zoom is dropped
    /// (counted, logged), never the whole tile. Meaningless for the
    /// footprint+height extrusion fallback (extrusion has no comparable
    /// vertex-count risk profile — a footprint's vertex count is already
    /// bounded by the ordinary MVT feature cap), so an empty table here
    /// (the default) is the ordinary case for every collection that never
    /// routes to a `VolumeSource`.
    #[serde(default)]
    pub vertex_caps: ZoomCaps,
}

fn default_places3d_height() -> f64 {
    0.0
}

fn default_places3d_exaggeration() -> f64 {
    1.0
}

impl Places3dConf {
    /// `tiles` supplies the zoom range `vertex_caps` entries must fall
    /// within — the same bound `TilesConf::validate` enforces for its own
    /// `caps` — since `Places3dConf` carries no zoom range of its own.
    fn validate(&self, collection_id: &str, tiles: &TilesConf) -> Result<()> {
        if self.exaggeration <= 0.0 {
            return Err(Error::Config(format!(
                "collection '{collection_id}': places3d.exaggeration ({}) must be > 0",
                self.exaggeration
            )));
        }
        for (zoom, cap) in &self.vertex_caps.0 {
            if *zoom < tiles.minzoom || *zoom > tiles.maxzoom {
                return Err(Error::Config(format!(
                    "collection '{collection_id}': places3d.vertex_caps entry for zoom {zoom} outside [{}, {}]",
                    tiles.minzoom, tiles.maxzoom
                )));
            }
            if *cap == 0 {
                return Err(Error::Config(format!(
                    "collection '{collection_id}': places3d.vertex_caps entry for zoom {zoom} must be > 0"
                )));
            }
        }
        Ok(())
    }
}

/// One protocol lane's ordered storage chain: a bare id (`tiles: main`) or
/// an explicit list (`tiles: [main, mirror]`). Deserializes either shape into
/// the same non-empty `Vec` — first entry is primary, later entries are a
/// read-only fallback tail that `Router` consults only when an earlier
/// entry's driver call errors (never on an empty result). See the
/// driver-contract design doc, section 3.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LaneRouting(pub Vec<String>);

impl<'de> Deserialize<'de> for LaneRouting {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            One(String),
            Many(Vec<String>),
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::One(id) => LaneRouting(vec![id]),
            Raw::Many(ids) => LaneRouting(ids),
        })
    }
}

/// Per-lane storage routing (`#21`): which storage(s) serve which protocol
/// lane for a collection. `features`/`tiles` left `None` default to the
/// collection's single `storage` — that default IS the design's
/// "unambiguous single storage" case, not a legacy fallback.
///
/// `write` (`#25`, the transactional-outbox design) has no such default: a
/// collection is only writable when it explicitly names exactly one storage
/// here — never a fallback tail (a write has nowhere sensible to fall
/// through to; see the design doc, section 3.1). `AppConfig::validate`
/// rejects an empty or multi-entry `write` lane before a `Router` is ever
/// built.
///
/// `index` (`#67`, the derived-index half of the same design) has the
/// identical shape: opt-in per collection, exactly one storage, no fallback
/// tail — an obligation apply target has nowhere sensible to fall through
/// to either.
///
/// `search` (`#67`, freshness-gated search routing) is opt-in per collection
/// like `write`/`index` — no "defaults to the single storage" fallback,
/// since most collections have nothing search-specific to route — but,
/// unlike `write`/`index`, it MAY name more than one storage: an ordered
/// chain where an entry that can't confirm the routed index is fresh enough
/// (or errors) falls through to the next, e.g. `search: [index, main]`. See
/// `Router::resolve_search` and `SearchConf::freshness_bound` for the gate
/// itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutingDecl {
    pub features: Option<LaneRouting>,
    pub tiles: Option<LaneRouting>,
    /// The OGC API Maps Part 1 `/collections/{cid}/map` lane (`#86`) — same
    /// "defaults to the single `storage`" shape as `features`/`tiles`, not
    /// the opt-in-with-no-fallback shape `write`/`index`/`search` have. A
    /// collection that never declares this routes `maps` to the same
    /// storage its `tiles` lane defaults to; naming a different storage here
    /// lets the two diverge, same as `tiles` can already diverge from
    /// `features`. `tellurion-tiles::maps`' own `map` handler resolves this
    /// one lane through EITHER of two capabilities, in this order:
    /// `TileSource` (a vector collection, rasterized from cached MVT —
    /// `Router::resolve_maps`) or, when nothing in the lane advertises a
    /// `TileSource` at all, `RasterSource` (`#37`: a COG- or Zarr-backed
    /// collection, composited from decoded raster windows —
    /// `Router::resolve_maps_raster`). A storage named here that advertises
    /// neither is refused by name.
    pub maps: Option<LaneRouting>,
    pub write: Option<LaneRouting>,
    pub index: Option<LaneRouting>,
    pub search: Option<LaneRouting>,
}

/// Per-collection behavior for the search lane (`#67`) — kept alongside
/// `TilesConf`/`StyleConf` rather than folded into `RoutingDecl` itself,
/// matching this file's "a routing decl carries wiring, a `*Conf` carries
/// behavior" split (`tiles`/`routing.tiles` is the precedent). Meaningless,
/// never consulted, unless `routing.search` is also declared.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConf {
    /// The freshness gate's tolerance, in sequence-lag units (design doc
    /// section 4): `lag = write lane's OutboxSource::primary_high_water(c) -
    /// SearchSource::applied_high_water(c)`; the search lane serves from the
    /// routed index only while `lag <= freshness_bound`, else it prefers its
    /// fallback tail. Default `0` — the strictest bound, requiring the index
    /// to be fully caught up — so a collection that routes `search` to an
    /// index without setting this can never silently tolerate arbitrary
    /// staleness.
    pub freshness_bound: u64,
}

/// Closed set of scalar types a declared schema property may claim (`#44`):
/// deliberately small and flat — not a JSON-Schema engine — so every value
/// maps unambiguously onto both a JSON Schema `type`/`format` pair (for
/// `tellurion-features`' queryables document) and a backend SQL type class
/// (for reconciling a declaration against what the storage actually
/// reports). Geometry columns have no `PropertyType`: they are never part
/// of a declared schema's flat property model, the same way `filter::
/// validate` gives them their own dedicated predicate (`S_INTERSECTS`)
/// rather than treating them as an ordinary scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PropertyType {
    String,
    Integer,
    Number,
    Boolean,
    Date,
    DateTime,
}

impl PropertyType {
    /// Classifies a backend's broad SQL type name (PostGIS:
    /// `information_schema.columns.data_type`) into this closed set — the
    /// single mapping both `descriptor::reconcile_schema` (declaration vs.
    /// backend reality) and `tellurion-features`' queryables document (SQL
    /// type vs. JSON Schema shape) compare against, so the two can never
    /// drift apart the way two independent copies of this match arm would.
    /// Unrecognized types classify as `String` — safe because a query
    /// against them still round-trips through the driver's own filter
    /// compiler, which casts the column to the literal's CQL2 type rather
    /// than trusting this classification (see `tellurion-postgis`'s
    /// `sql::compile_filter`).
    pub fn from_sql_type(sql_type: &str) -> Self {
        match sql_type {
            "boolean" | "bool" => Self::Boolean,
            "smallint" | "integer" | "bigint" | "int2" | "int4" | "int8" | "serial"
            | "bigserial" | "smallserial" => Self::Integer,
            "real" | "double precision" | "numeric" | "decimal" | "float4" | "float8" => {
                Self::Number
            }
            "date" => Self::Date,
            "timestamp without time zone"
            | "timestamp with time zone"
            | "timestamptz"
            | "timestamp" => Self::DateTime,
            _ => Self::String,
        }
    }

    /// `(json_schema_type, format)` for a queryables `PropertySchema`
    /// (`tellurion-features`) — the reverse direction of
    /// [`from_sql_type`](Self::from_sql_type), kept alongside it so the two
    /// stay obviously in sync.
    pub fn json_schema_shape(&self) -> (&'static str, Option<&'static str>) {
        match self {
            Self::String => ("string", None),
            Self::Integer => ("integer", None),
            Self::Number => ("number", None),
            Self::Boolean => ("boolean", None),
            Self::Date => ("string", Some("date")),
            Self::DateTime => ("string", Some("date-time")),
        }
    }

    /// Whether `value`'s own JSON shape matches this declared type — the
    /// write-side counterpart of [`from_sql_type`](Self::from_sql_type),
    /// used by [`SchemaDecl::validate_feature_properties`] (`#44`) to check
    /// an inbound feature's property values before any outbox obligation
    /// commits. `Integer` requires a JSON number with no fractional part
    /// (`serde_json::Number::is_i64`/`is_u64`) — a bare `Number` accepts any
    /// JSON number, integer or not. `Date`/`DateTime` only check that the
    /// value is a JSON string; this is a flat, typed model, not a full
    /// JSON-Schema engine (`#44`'s own non-goal), so neither actually
    /// parses the string as a calendar date.
    pub fn matches_json_value(&self, value: &serde_json::Value) -> bool {
        match self {
            Self::String | Self::Date | Self::DateTime => value.is_string(),
            Self::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
        }
    }

    /// Lowercase name matching this type's own YAML/JSON spelling — used in
    /// `descriptor::reconcile_schema`'s error messages so a mismatch names
    /// both sides in the same vocabulary the config file uses.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::DateTime => "datetime",
        }
    }
}

/// One property in a collection's declared schema (`#44`). Deliberately
/// flat — a name, a [`PropertyType`], and whether it is required — not a
/// JSON-Schema engine; see `SchemaDecl`'s doc comment for why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertyDecl {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: PropertyType,
    #[serde(default)]
    pub required: bool,
}

/// A collection's optional declared property schema (`#44`). Absent
/// (`CollectionDecl::schema` is `None`) is the default and stays
/// byte-for-byte identical to a collection with no schema story at all —
/// the derived `CollectionDescriptor` is the only shape, exactly as before
/// this type existed. When declared, it refines the read-side surface
/// (queryables types/required-ness) and narrows filtering when
/// `additional_properties` is `false`; it is reconciled against the derived
/// descriptor at boot-or-first-touch (`descriptor::reconcile_schema`, wired
/// into `Router`'s existing `merge_and_enforce` step — no separate
/// validation phase). A per-collection declaration, never inherited down
/// the platform -> tenant -> catalog -> collection chain `SettingsDecl`
/// uses — every collection states its own schema, or none.
///
/// Deliberately NOT a field on [`SettingsDecl`] and never added to the
/// platform -> tenant -> catalog -> collection whitelist that type resolves
/// (see `settings::resolve_effective_settings`): a property schema describes
/// one collection's own physical shape, not a piece of shared operator
/// policy several collections might reasonably want the same value for the
/// way `tile_caps`/`cache_ttl_s`/`stac` do. Keep it that way — every
/// collection states its own schema, or none, full stop.
///
/// Non-goal (matching the design issue): a full JSON-Schema engine.
/// Precise errors over a small model. Write-side (inbound feature)
/// validation against this same declaration is a later slice — nothing
/// here validates request bodies yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SchemaDecl {
    pub properties: Vec<PropertyDecl>,
    /// Whether a property outside `properties` may still be served/filtered.
    /// Defaults to `true` (JSON Schema's own `additionalProperties`
    /// default): declaring a schema narrows the *known* shape without, by
    /// itself, closing the collection to whatever else the backend reports.
    /// Set `false` to make the declaration exhaustive — see
    /// `filter::validate_attribute_property` and
    /// `tellurion-features`'s `queryables` module for where this actually
    /// bites.
    pub additional_properties: bool,
}

impl Default for SchemaDecl {
    fn default() -> Self {
        Self {
            properties: Vec::new(),
            additional_properties: true,
        }
    }
}

impl SchemaDecl {
    /// Referential check within the declaration itself: no property name
    /// repeated. Cross-checking a declared property against the backend
    /// (missing column, type mismatch) is `descriptor::reconcile_schema`'s
    /// job — that needs a derived `CollectionDescriptor`, which doesn't
    /// exist yet at `AppConfig::validate` time.
    fn validate(&self, collection_id: &str) -> Result<()> {
        let mut seen = HashSet::new();
        for property in &self.properties {
            if !seen.insert(property.name.as_str()) {
                return Err(Error::Config(format!(
                    "collection '{collection_id}': schema declares property '{}' more than once",
                    property.name
                )));
            }
        }
        Ok(())
    }

    /// Write-side input validation (`#44`): checks `properties` (a GeoJSON
    /// Feature's `properties` object) against this declared schema before
    /// any outbox obligation is committed for it — a malformed feature must
    /// never become a committed obligation. Every violation is collected
    /// rather than stopping at the first, so a rejection names every
    /// offending property at once; the caller (the write handler) never
    /// calls this at all for a collection with no declared schema — a
    /// free-form collection accepts features as-is, no validation in the
    /// way.
    ///
    /// A property is checked in three ways: a `required` property missing
    /// or explicitly `null` fails; a present, non-null property whose JSON
    /// value doesn't match its declared [`PropertyType`] fails, naming both
    /// the expected and actual shape; and, when `additional_properties` is
    /// `false`, any property present that this schema doesn't declare
    /// fails. Not a full JSON-Schema engine by design (`#44`'s own
    /// non-goal) — a flat, typed model with precise errors over an
    /// expressive one with vague ones.
    pub fn validate_feature_properties(
        &self,
        properties: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        let mut problems = Vec::new();

        for property in &self.properties {
            match properties.get(property.name.as_str()) {
                None | Some(serde_json::Value::Null) => {
                    if property.required {
                        problems.push(format!("property '{}' is required", property.name));
                    }
                }
                Some(value) => {
                    if !property.type_.matches_json_value(value) {
                        problems.push(format!(
                            "property '{}' expected type '{}' but got {}",
                            property.name,
                            property.type_.as_str(),
                            json_value_kind(value)
                        ));
                    }
                }
            }
        }

        if !self.additional_properties {
            let declared: HashSet<&str> = self.properties.iter().map(|p| p.name.as_str()).collect();
            for key in properties.keys() {
                if !declared.contains(key.as_str()) {
                    problems.push(format!(
                        "property '{key}' is not declared and additional properties are not allowed for this collection"
                    ));
                }
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "feature validation failed: {}",
                problems.join("; ")
            )))
        }
    }
}

/// The JSON type-name a validation message names a mismatched value by —
/// deliberately the JSON vocabulary (`"number"`, `"array"`, ...), not a Rust
/// or SQL one, since the caller reading this message is a write-API client
/// looking at the request body it sent, not this codebase's own SQL layer.
fn json_value_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// A collection's primary-key value-space (`#88`, widened to real UUID
/// support by `#87`, then caller-supplied `Text` ids by `#94`): decides how
/// `POST /collections/{cid}/items` (create) can mint a new item's id, since a
/// server-assigned create — unlike `PUT`, whose id is always caller-supplied
/// via the URL — has no id of its own to start from, and how every id-bearing
/// request (`GET`/`PUT`/`DELETE /items/{fid}`, keyset paging tokens) parses
/// `feature_id` at the boundary. `Integer` (the default) means a
/// `bigserial`-backed pk column: `WriteSink::create`'s PostGIS implementation
/// omits the pk column from its `INSERT` and lets the column's own `DEFAULT
/// nextval(...)` mint it, reading the value back via `RETURNING` in the same
/// statement; every id parses/casts as `i64`/`bigint`. `Uuid` means a real
/// `uuid`-typed pk column with a server-side default (typically `DEFAULT
/// gen_random_uuid()`): the same omit-from-INSERT/RETURNING create path
/// mints it server-side, and every id parses/casts as `uuid`/`uuid` instead.
/// `Text` means a real `text`/`varchar`-typed pk column with deliberately NO
/// server default expected: the pk is always caller-supplied, so `POST`
/// create requires a top-level `id` in the feature body (a named refusal
/// when it's absent) and binds that id directly rather than omitting the pk
/// column, still reading it back via the same `RETURNING` clause so the
/// returned id is exactly what the database stored; an id already claimed by
/// another row is a named `409`, never a raw constraint-violation error.
/// Keyset paging over a `Text` pk pins an explicit `COLLATE "C"` in its
/// `ORDER BY`/`WHERE` comparisons rather than trusting the database's own
/// default collation, so paging stays stable and complete across deployments
/// regardless of locale. The PostGIS driver refuses, by name, a `Uuid`/`Text`
/// collection whose physical pk column doesn't match the declared type
/// (checked live, at create time), and a `Uuid` pk with no server default
/// (the pk column's own `NOT NULL` violation on a deliberately pk-less
/// `INSERT`) — see `tellurion-postgis`'s own docs. Single-column pk only in
/// every case; a composite pk is out of scope, same as it always was. Every
/// collection that predates this field deserializes to `Integer` (the
/// `#[serde(default)]` below), so existing `PUT`/`DELETE`/keyset-paging
/// behavior for every collection that never declares `id_type` is unchanged
/// byte-for-byte.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdType {
    #[default]
    Integer,
    Uuid,
    Text,
}

/// What a collection *is*, independent of which storage happens to back it
/// (`#192`): a vector feature collection, a raster coverage, or a
/// geometry-less record collection (a thesaurus, a document registry,
/// dataset-level metadata that has no features of its own).
///
/// Owned by the data model — a [`CollectionDecl`] field, never a
/// [`StorageDecl`] one. A kind stored per backend would silently misclassify
/// a collection the moment its `routing` changed: the same physical driver
/// serves vector features for one collection and records for another, so
/// "what kind of thing is this" is a property of the collection, not of the
/// box its rows live in.
///
/// The kind is what each protocol root filters its `/collections` listing
/// by — Features and Tiles skip [`Record`](Self::Record), the Records root
/// serves only it, and STAC serves every kind (a STAC Collection describes
/// metadata regardless of whether the thing described has geometry). See
/// `tellurion-server`'s `Protocol::serves_kind`.
///
/// [`Vector`](Self::Vector) is the default, and it is deliberately *not* a
/// guess about the data: it is what every collection in every deployment
/// written before this field existed already behaved as, so an unconfigured
/// deployment's `/collections` listings, links, and conformance responses
/// stay byte-for-byte what they were. [`Raster`](Self::Raster) exists so an
/// operator can label a coverage collection honestly; it is served by
/// exactly the lanes `Vector` is (this workspace's raster collections
/// already reach the tiles/maps lanes through `RasterSource`), so labelling
/// one changes no routing today — the distinction is descriptive, and the
/// two fold together wherever only "has geometry / has none" matters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollectionKind {
    #[default]
    Vector,
    Raster,
    Record,
}

impl CollectionKind {
    /// Whether this collection has a geometry story at all — `true` for
    /// `vector`/`raster`, `false` for `record`. The one predicate every
    /// geometry-serving lane (Features Part 1 items, Tiles, Maps, 3D Tiles)
    /// gates on, and the one that relaxes
    /// [`descriptor::require_feature_capable`](crate::descriptor::require_feature_capable)'s
    /// geometry requirement.
    pub fn has_geometry(self) -> bool {
        !matches!(self, CollectionKind::Record)
    }

    /// Whether this is a record collection — the OGC API — Records root's
    /// own filter. Spelled out rather than left as `!has_geometry()` so a
    /// future fourth geometry-less kind does not silently become a record.
    pub fn is_record(self) -> bool {
        matches!(self, CollectionKind::Record)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionDecl {
    pub id: String,
    /// What this collection *is* (`#192`) — see [`CollectionKind`].
    /// `vector` (the default) for every collection written before this key
    /// existed, which is exactly how each of them already behaved.
    #[serde(default)]
    pub kind: CollectionKind,
    /// Public URL segment (`/collections/{external_id}`); defaults to `id`.
    #[serde(default)]
    pub external_id: Option<String>,
    /// The owning catalog's internal id (renamed from `tenant` in `#39` —
    /// collections belong to a catalog, not directly to a tenant; the
    /// catalog's own `tenant` field supplies that).
    pub catalog: String,
    pub storage: String,
    /// Per-lane storage overrides (`#21`). Omitted lanes default to
    /// `storage` above; see `RoutingDecl`.
    #[serde(default)]
    pub routing: RoutingDecl,
    /// Physical target name override; driver-interpreted (a table for
    /// PostGIS). `None` derives it by convention as this collection's `id` —
    /// see `descriptor::target_table`. Precedence is override > derived; an
    /// override that names a table the backend doesn't report still fails
    /// fast at boot, exactly as a required `table` did before `#19`.
    #[serde(default)]
    pub table: Option<String>,
    /// Geometry column override. `None` derives it from the storage's
    /// `CatalogSource` (see `descriptor::merge_descriptor`).
    #[serde(default)]
    pub geometry: Option<String>,
    /// Primary key column override. `None` derives it from the storage's
    /// `CatalogSource` (see `descriptor::merge_descriptor`).
    #[serde(default)]
    pub pk: Option<String>,
    /// This collection's primary-key value-space (`#88`) — see [`IdType`]'s
    /// own doc. `Integer` (the default) for every collection that doesn't
    /// declare otherwise.
    #[serde(default)]
    pub id_type: IdType,
    /// Datetime column override. `None` derives it from the storage's
    /// `CatalogSource` when the backend reports exactly one candidate column
    /// (see `descriptor::merge_descriptor`); zero or multiple candidates
    /// also leave this `None`.
    #[serde(default)]
    pub datetime: Option<String>,
    /// Declared modification-timestamp column (OGC API Features — Part 4,
    /// 20-002r1 draft, Optimistic Locking: Timestamps class, `#107`) — the
    /// source a `Last-Modified` response header and `If-Unmodified-Since`
    /// write-side evaluation read from. Unlike `datetime` above (an item's
    /// own domain timestamp, used for `datetime`-parameter filtering), this
    /// names the column this collection's storage updates on every write —
    /// no naming convention enforced, and no derivation attempted: absence
    /// (`None`, the default) is this collection's honest answer whenever no
    /// such column exists or the operator simply never declared one, never
    /// a value this crate invents. Following the same "operator declares a
    /// real physical fact, the router validates it against the backend at
    /// boot" shape `geometry_variants` uses (`#104`) rather than the
    /// override-with-derived-fallback shape `table`/`geometry`/`pk`/
    /// `datetime` follow: there is no honest default to derive a
    /// modification timestamp from, so this is declare-or-absent, not
    /// declare-or-derive. `Router::validate_catalog` (via
    /// `descriptor::reconcile_modified_column`) requires a declared column
    /// to actually exist on the backend, classify as `PropertyType::DateTime`,
    /// and — for a collection with a *closed* schema
    /// (`SchemaDecl::additional_properties: false`) — also appear among its
    /// declared properties, since a column a closed schema excludes from
    /// `properties` would never reach `FeatureSource::item`'s own response
    /// body for this module to read a value out of.
    #[serde(default)]
    pub modified_column: Option<String>,
    /// Cheap row-count estimate, filled in by `Router::resolve_tiles`/
    /// `resolve_features` from the derived descriptor when one was computed
    /// for this resolve. Never operator-configured — skipped on both
    /// serialize and deserialize. Consumed by
    /// `descriptor::heuristics::effective_feature_cap` to size a per-zoom
    /// feature cap when `tiles.caps` says nothing about a zoom; `None` when
    /// the descriptor was never derived for this resolve (e.g. `table`/
    /// `geometry`/`pk` are all explicitly overridden — see
    /// `Router::effective_decl`) or the backend couldn't estimate.
    #[serde(skip)]
    pub row_estimate: Option<u64>,
    /// This collection's native storage SRID, filled in by `Router::
    /// resolve_tiles`/`resolve_features` from the derived descriptor exactly
    /// like `row_estimate` above (same skip-on-boot/`None`-on-the-fully-
    /// overridden-fast-path rules) — never operator-configured. Feeds
    /// `tellurion-postgis::sql`'s `crs`/`bbox-crs` reprojection (OGC API
    /// Features Part 2 CRS by Reference) and this collection's
    /// `storageCrs`/`crs` metadata (`tellurion_core::crs`).
    #[serde(skip)]
    pub srid: Option<i32>,
    /// This collection's backend-known projection facts
    /// (`CatalogSource::projection`, `#36` — STAC `projection` extension),
    /// filled in from the derived descriptor exactly like `row_estimate`/
    /// `srid` above (same skip-on-boot/`None`-on-the-fully-overridden-fast-
    /// path rules) — never operator-configured: derivation needs no
    /// configuration, and an operator with a correction to make supplies it
    /// through the per-item STAC metadata sidecar (`#202`), which is a
    /// per-item override channel rather than a second collection-level
    /// source of truth. `None` for every driver that never overrides the
    /// accessor (every vector backend — their SRID travels as `srid` above).
    #[serde(skip)]
    pub projection: Option<ProjectionFacts>,
    /// This collection's geometry statistics profile (`#101`), filled in by
    /// `Router::resolve_tiles`/`resolve_maps` only — deliberately narrower
    /// than `row_estimate`/`srid` above, which every `effective_decl` call
    /// fills regardless of lane. A geometry profile samples real table rows
    /// (`CatalogSource::geometry_profile`'s own doc: bounded, but not free),
    /// so only the two lanes that actually render MVT and consult it
    /// (`descriptor::heuristics::simplify_tolerance_meters_for_profile`,
    /// `#102`) ever pay for it — `resolve_features`/`resolve_write`/
    /// `resolve_raster` leave this `None` unconditionally. `None` also for a
    /// collection whose driver never overrides `CatalogSource::
    /// geometry_profile`, or whose profile computation failed (see
    /// `Router::effective_tile_decl`'s own doc for the never-fail-the-
    /// request handling). Never operator-configured — same `#[serde(skip)]`
    /// shape as `row_estimate`/`srid`.
    #[serde(skip)]
    pub geometry_profile: Option<GeometryProfile>,
    /// Every non-geometry column this collection's backend reports, name
    /// plus broad type (`CollectionDescriptor::attributes`, `#19`) — filled
    /// in by `Router::resolve_tiles`/`resolve_features` from the derived
    /// descriptor exactly like `row_estimate`/`srid` above, and never
    /// operator-configured (same `#[serde(skip)]` shape: it can't be
    /// authored in YAML at all, so it adds no configuration surface).
    ///
    /// This is the same backend derivation `CanonicalSchema` is built from
    /// (`descriptor::canonical::build_schema`), carried on the decl rather
    /// than read off that merged view because the merged view is not a
    /// column list: it drops backend columns when a declared schema says
    /// `additional_properties: false`, and it adds declared properties the
    /// backend never reported. A GeoJSON `properties` projection needs the
    /// physical columns exactly as they are, so it reads this instead
    /// (`#278`).
    ///
    /// `None` — never `Some(vec![])`, which legitimately means "this table
    /// has no non-geometry columns" — when the descriptor was never derived
    /// for this resolve (`table`/`geometry`/`pk` all explicitly overridden,
    /// see `Router::effective_decl`'s fully-pinned fast path) or the backend
    /// couldn't introspect columns at all. Every consumer must keep its
    /// prior behavior byte for byte in that case.
    #[serde(skip)]
    pub attribute_columns: Option<Vec<AttributeColumn>>,
    #[serde(default)]
    pub tiles: TilesConf,
    /// Additional pre-generalized geometry column variants this collection's
    /// tiles lane may read instead of `geometry`, each serving one declared
    /// zoom range (`#104`). Empty (the default) is today's behavior exactly:
    /// every zoom reads the base `geometry` column. See
    /// `resolved_geometry_for_zoom` for the selection rule and
    /// `Router::validate_catalog` for the boot-time existence/SRID/type
    /// check every declared variant must pass.
    #[serde(default)]
    pub geometry_variants: Vec<GeometryVariantDecl>,
    #[serde(default)]
    pub style: StyleConf,
    /// Search-lane freshness-gate behavior (`#67`). See `SearchConf`'s own
    /// doc; meaningless unless `routing.search` is also declared.
    #[serde(default)]
    pub search: SearchConf,
    /// Per-collection opt-in for the write-reactive tile-cache invalidation
    /// consumer (`#113`). `false` (the default) is today's TTL-only
    /// behavior, byte for byte — same "opt-in, not a routing lane" shape as
    /// `search` above, since this consumer needs nothing routing itself can
    /// express beyond the write lane's already-resolvable outbox
    /// (`Router::resolve_outbox`). Meaningless (the server logs and skips
    /// spawning a consumer task for this collection, the same "best-effort
    /// per collection, never a reason to fail boot" treatment
    /// `IndexApplierConfig`'s own wiring gives an unresolvable index lane)
    /// unless `routing.write` is also declared. Also gated by the
    /// server-wide `ServerConfig.tile_invalidation.enabled` switch — both
    /// must be on for this collection to actually get write-reactive
    /// invalidation.
    #[serde(default)]
    pub tile_invalidation: bool,
    /// Enables the Glb tile lane for this collection. Absent means "no 3D
    /// places" — the `/3dtiles` routes refuse the collection at resolve
    /// time, same as any other missing capability.
    #[serde(default)]
    pub places3d: Option<Places3dConf>,
    /// This collection's optional declared property schema (`#44`). `None`
    /// (the default) is free-form — the derived descriptor is the only
    /// shape, exactly like a collection with no schema story at all. See
    /// `SchemaDecl`.
    #[serde(default)]
    pub schema: Option<SchemaDecl>,
    /// This collection's own settings overrides (`#39`) — the nearest link
    /// in the platform -> tenant -> catalog -> collection chain. See
    /// `settings.rs`.
    #[serde(default)]
    pub settings: SettingsDecl,
    /// This collection's effective vector-tile property allowlist (`#85`),
    /// resolved from `settings.tile_properties` through the platform ->
    /// tenant -> catalog -> collection chain by `Router::
    /// apply_inherited_tile_properties` — mirrors `tiles.caps`'s own
    /// settings-overlay shape (`Router::apply_inherited_tile_caps`). Empty
    /// (the default) means pk-only: nothing beyond the primary key is
    /// projected into an MVT feature's attribute table, exactly the
    /// behavior every collection had before `#85`. Never operator-
    /// configured directly on this field — see `SettingsDecl::
    /// tile_properties` for the config-facing knob. Same `#[serde(skip)]`
    /// shape as `row_estimate`/`srid`: never round-trips through YAML.
    #[serde(skip)]
    pub tile_properties: Vec<String>,
    /// This collection's own cross-tenant visibility override (`#34`).
    /// Absent (the default) inherits the owning catalog's own visibility —
    /// see `VisibilityDecl`'s own doc and `Router::effective_visibility`.
    #[serde(default)]
    pub visibility: VisibilityDecl,
    /// The object store (an internal id from `AppConfig.object_stores`)
    /// this collection's managed assets live in (assets-and-object-storage
    /// proposal, first slice). `None` (the default): this collection has no
    /// managed-storage lane at all — `Router::resolve_object_store` refuses
    /// as a plain capability unsupported ("managed-storage"), the same
    /// shape every other opt-in capability in this file uses. Declared
    /// assets and remote-asset registration (the `core` conformance class)
    /// never need this — only a managed asset's byte lifecycle does.
    #[serde(default)]
    pub object_store: Option<String>,
    /// Per-collection opt-in for the per-item STAC metadata sidecar
    /// (`#202`): the `"<table>_stac"` table `tellurion-ingest stac
    /// create-tables` provisions, read — and merged into an Item — by the
    /// STAC lane alone. `false` (the default) is today's behavior byte for
    /// byte: `Router::resolve_stac_metadata` answers `Ok(None)` without
    /// probing anything, and this collection's STAC Items are exactly the
    /// documents it served before this field existed.
    ///
    /// Same "opt-in flag, not a routing lane" shape `tile_invalidation`
    /// above uses, and for the same reason: the sidecar lives in the
    /// collection's own canonical storage (the anchor driver — see
    /// `Router::resolve_stac_metadata`), so there is nothing for
    /// `RoutingDecl` to express and STAC keeps resolving `features`
    /// unchanged. Deliberately NOT a `settings.stac` key: that block is
    /// collection-level declared descriptor metadata (license, keywords,
    /// providers, contacts) resolved down the settings chain, whereas this
    /// is a physical-provisioning fact about one collection's own storage,
    /// the same kind of fact `geometry_variants`/`object_store` are.
    ///
    /// Declaring it against a driver that advertises no
    /// `stac_metadata_source` is a named request-time
    /// `CapabilityUnsupported("stac-metadata")` refusal, and declaring it
    /// without provisioning the table is the driver's own named
    /// `StacTableMissing` — never a silent empty merge, which an operator
    /// could not tell apart from a correctly provisioned but empty sidecar.
    #[serde(default)]
    pub stac_metadata: bool,
    /// Per-collection opt-in for projecting this collection's *item-scoped*
    /// asset records (`#221`) into its STAC Items' `assets` object — the
    /// same `"<table>_assets"` table `tellurion-ingest assets create-tables`
    /// already provisions for the assets API (`crate::asset`), read on the
    /// STAC lane through the existing `AssetRecordStore` capability rather
    /// than a new one. `false` (the default) is today's behavior byte for
    /// byte: `Router::resolve_item_assets` answers `Ok(None)` without
    /// probing anything, and every Item carries exactly the
    /// capability-derived asset map it carried before this field existed.
    ///
    /// Same "opt-in flag, not a routing lane" shape `stac_metadata` above
    /// uses, and for the same reasons: the assets table is a sidecar of the
    /// collection's own physical table (so it lives on the anchor driver,
    /// see `Router::resolve_assets`), and whether it was provisioned is a
    /// physical fact about one collection's storage, not a settings-chain
    /// value.
    ///
    /// The flag is required rather than inferred from "this driver happens
    /// to be `AssetRecordStore`-capable": inferring it would make every
    /// PostGIS-backed collection that never ran `assets create-tables`
    /// start failing its STAC Items with the driver's named
    /// `AssetsTableMissing`, which is exactly the silent behavior change an
    /// unconfigured deployment must never see. Declaring it against a
    /// driver advertising no `asset_record_store` is a named request-time
    /// `CapabilityUnsupported("assets")` refusal, and declaring it without
    /// provisioning the table is the driver's own named
    /// `AssetsTableMissing` — never a silently empty asset map, which an
    /// operator could not tell apart from a collection whose items simply
    /// have no asset records yet.
    #[serde(default)]
    pub stac_item_assets: bool,
}

impl CollectionDecl {
    pub fn external_id(&self) -> &str {
        self.external_id.as_deref().unwrap_or(&self.id)
    }

    /// The resolved physical table name. Panics if this collection was never
    /// resolved by `Router` — every `CollectionDecl` a driver receives via
    /// `FeatureSource`/`TileSource` has already passed through
    /// `Router::resolve_features`/`resolve_tiles`, which guarantee `table`
    /// is `Some` before handing the decl to a driver. See `descriptor.rs`.
    pub fn resolved_table(&self) -> &str {
        self.table
            .as_deref()
            .expect("CollectionDecl.table must be resolved by Router before reaching a driver")
    }

    /// The resolved geometry column. See [`resolved_table`](Self::resolved_table).
    pub fn resolved_geometry(&self) -> &str {
        self.geometry
            .as_deref()
            .expect("CollectionDecl.geometry must be resolved by Router before reaching a driver")
    }

    /// The resolved primary key column. See [`resolved_table`](Self::resolved_table).
    pub fn resolved_pk(&self) -> &str {
        self.pk
            .as_deref()
            .expect("CollectionDecl.pk must be resolved by Router before reaching a driver")
    }

    /// The geometry column to read for `zoom` (`#104`): the first declared
    /// `geometry_variants` entry whose `[minzoom, maxzoom]` range covers it,
    /// else the base [`resolved_geometry`](Self::resolved_geometry) column.
    /// Declaration order only matters when ranges overlap, which
    /// `AppConfig::validate` refuses at boot (`validate_geometry_variants`),
    /// so in practice at most one variant ever matches a given zoom. Every
    /// declared variant has already been confirmed to exist and to share the
    /// base column's SRID and geometry type by the time a driver ever calls
    /// this — see `Router::validate_catalog`.
    pub fn resolved_geometry_for_zoom(&self, zoom: u8) -> &str {
        self.geometry_variants
            .iter()
            .find(|variant| zoom >= variant.minzoom && zoom <= variant.maxzoom)
            .map(|variant| variant.column.as_str())
            .unwrap_or_else(|| self.resolved_geometry())
    }
}

/// Tenant authentication/authorization (`#17`, OIDC half `#34`). An absent
/// `auth:` section in the YAML document deserializes to this type's own
/// `Default` (both fields empty) via `AppConfig`'s container-level
/// `#[serde(default)]`, and every request may act as any tenant — identical
/// to every deployment before `#17` existed; see `is_configured`, which
/// `auth::build_authorizer` consults to decide between that permissive case
/// and building a real authorizer.
///
/// Unlike `L2CacheConfig`'s tagged-enum "pick one backend" shape, the two
/// credential sources here are independently optional and compose: an
/// operator can hand service accounts fixed `bearer_tokens` while humans
/// authenticate through `trusted_issuers` (or the compatible singular
/// `oidc` form), all checked against the same per-request
/// tenant, by the same `TenantAuthorizer` (`auth::authorize`'s decision
/// order: a static-token match wins outright — cheap, no token parsing —
/// before an `oidc` block is ever consulted). See `tellurion_core::auth`
/// for the trait/decision logic this selects an implementation of.
///
/// Top-level only, by design: `AppConfig::auth` is the single source for
/// this and no `TenantDecl`/`CatalogDecl`/`CollectionDecl` carries its own
/// override. `BearerTokenDecl::tenants` and `OidcConfig::claims` already let
/// one credential authorize a subset of tenants, which covers the
/// per-tenant access-control need without a second, per-level `auth:`
/// section to keep in sync with this one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub bearer_tokens: Vec<BearerTokenDecl>,
    /// Backward-compatible single issuer. New control-plane deployments use
    /// `trusted_issuers`, while both shapes share the same validation path.
    pub oidc: Option<OidcConfig>,
    /// Platform-approved identity providers. A token's unverified `iss` is
    /// used only to select one entry from this list before full validation.
    pub trusted_issuers: Vec<OidcConfig>,
    /// Optional browser-based OIDC client for the control workspace. Secret
    /// material is named here but resolved only by the server at boot.
    pub browser: Option<ControlBrowserAuthConfig>,
}

impl AuthConfig {
    /// Whether any credential source is set. `false` (all empty) is the
    /// permissive default this type's own `Default` produces — see this
    /// type's own doc for why `auth::build_authorizer` treats that as "build
    /// no authorizer at all," not as "build one with nothing in it."
    pub fn is_configured(&self) -> bool {
        !self.bearer_tokens.is_empty()
            || self.oidc.is_some()
            || !self.trusted_issuers.is_empty()
            || self.browser.is_some()
    }
}

const MAX_CONTROL_BROWSER_SESSION_TTL_S: u64 = 86_400;
const MAX_CONTROL_BROWSER_LOGIN_TTL_S: u64 = 600;
const MAX_CONTROL_BROWSER_SESSIONS: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlBrowserAuthConfig {
    pub issuer: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret_env: Option<String>,
    pub public_origin: String,
    #[serde(default = "default_control_browser_scopes")]
    pub scopes: Vec<String>,
    #[serde(default = "default_control_browser_session_ttl_s")]
    pub session_ttl_s: u64,
    #[serde(default = "default_control_browser_login_ttl_s")]
    pub login_ttl_s: u64,
    #[serde(default = "default_control_browser_max_sessions")]
    pub max_sessions: usize,
}

impl ControlBrowserAuthConfig {
    pub fn callback_url(&self) -> String {
        format!(
            "{}/_auth/control/callback",
            self.public_origin.trim_end_matches('/')
        )
    }
}

fn default_control_browser_scopes() -> Vec<String> {
    vec!["openid".to_string(), "profile".to_string()]
}

fn default_control_browser_session_ttl_s() -> u64 {
    3_600
}

fn default_control_browser_login_ttl_s() -> u64 {
    300
}

fn default_control_browser_max_sessions() -> usize {
    1_024
}

/// One bearer principal's authorization (`#17`): its token value authorizes
/// every tenant in `tenants` (each a tenant *internal* id — the same
/// convention `CatalogDecl::tenant` uses to reference a tenant from
/// elsewhere in the config document). The token value itself is never
/// logged or echoed anywhere on the request path — see
/// `tellurion_core::auth`'s module doc.
///
/// Where that value *lives* is `token_env` vs `token` (`#144`): exactly one
/// of the two, checked by `AppConfig::validate`. `token_env` names an
/// environment variable and is the shape this project already uses for
/// every other credential (`StorageDecl::url_env`,
/// `ControlStoreLocator::Postgres::url_env`, `L2CacheConfig::Valkey::
/// url_env`) — behavior in the document, secrets in the environment.
/// Inline `token` is the pre-`#144` arrangement, still accepted and still
/// working exactly as it did, and reported once per boot/reload by name;
/// see `auth::resolve_bearer_credentials`.
///
/// `roles`/`claims` (`#34` policy layer) are additive, optional refinements
/// consulted only by the RBAC/ABAC policy checkpoint (`policy.rs`), never by
/// the coarse `tenants`-based tenant gate above, which stays exactly as
/// `#17` built it. A token with both left at their empty default carries
/// membership (via `tenants`) but no role in any tenant and no claim for
/// ABAC substitution — the same "member, but nothing a policy grant can
/// match" starting point every subject has until an operator configures
/// otherwise.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BearerTokenDecl {
    /// The token value written straight into the configuration document —
    /// the pre-`#144` arrangement. `#[serde(default)]` so an entry that
    /// declares `token_env` instead need not carry an empty `token:` line;
    /// an entry declaring NEITHER is refused by name at validation, so the
    /// default is a shape this type accepts, never a credential it invents.
    ///
    /// Deprecated, not removed: every deployment written before `#144`
    /// carries this field, and refusing it would stop all of them booting —
    /// the same reasoning `ControlStoreLocator::LegacyFile` is the
    /// `Default` for. It is instead named, once per boot and per reload,
    /// by `auth::resolve_bearer_credentials`.
    #[serde(default)]
    pub token: String,
    /// Names the environment variable this principal's token value is read
    /// from (`#144`) — the value never appears in the configuration
    /// document, so it cannot reach a control-store snapshot, a `GET
    /// /config` response, or a config file committed by mistake.
    ///
    /// Per-principal rather than one variable naming the whole set: the
    /// *list* is behavior (which principals exist, which tenants and roles
    /// each holds), and only each principal's single token value is a
    /// secret. Packing the list into one variable would move the behavior
    /// into the environment too, where it is neither reviewable nor
    /// diffable — the opposite of what `url_env` established.
    ///
    /// An unset variable is a named refusal at boot/reload, never a
    /// principal that silently stops authorizing.
    #[serde(default)]
    pub token_env: Option<String>,
    pub tenants: Vec<String>,
    /// Tenant internal id -> role names this token holds in that tenant.
    /// Every key must also appear in `tenants` above — a token cannot hold a
    /// role in a tenant it isn't even a member of; see
    /// `AppConfig::validate`.
    #[serde(default)]
    pub roles: HashMap<String, Vec<String>>,
    /// Arbitrary claims available for ABAC filter-template substitution
    /// (e.g. `org: acme`) — the static-token equivalent of an OIDC token's
    /// JWT claims. See `policy.rs`'s claim-substitution doc.
    #[serde(default)]
    pub claims: HashMap<String, serde_json::Value>,
    /// Platform-level administrative authority (`#110`): whether this token
    /// may act against the config-mutation control lane
    /// (`tellurion-server::config_mutation`), independent of `tenants`
    /// above — a platform mutation touches the whole document, not any one
    /// tenant's own slice of it, so it needs a gate orthogonal to the
    /// tenant-membership one `tenants` already provides. `false` (the
    /// default) means this token authorizes tenants as usual but never
    /// configuration mutations.
    #[serde(default)]
    pub platform_admin: bool,
    /// A human-readable identifier for this token, used only in the
    /// config-mutation audit trail (`tellurion_core::audit`) — never the
    /// token value itself (see `tellurion_core::auth`'s "never logs or
    /// echoes" rule). `None` (the default) falls back to a short,
    /// non-reversible fingerprint of the token at audit time.
    #[serde(default)]
    pub principal: Option<String>,
}

/// OIDC bearer-token validation (`#34`): a presented `Authorization: Bearer
/// <jwt>` that doesn't match a static token instead gets verified as a JWT
/// issued by `issuer` for `audience` — signature checked against that
/// issuer's published JWKS (RS256/ES256), plus `iss`/`aud`/`exp`/`nbf`
/// (`auth::OidcValidator` does the checking; this type only carries the
/// operator-facing knobs). `claims.tenants` names the JWT claim tenant
/// memberships come from — the values that feed the exact same
/// tenant-membership check `BearerTokenDecl::tenants` does, so a 403 from a
/// wrong-tenant OIDC token looks identical to a 403 from a wrong-tenant
/// static one.
///
/// No secret material lives here: this is the *relying party* side of OIDC
/// (verify someone else's token), never the *client* side (this server
/// never holds a client secret or requests a token of its own).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OidcConfig {
    /// The token issuer, e.g. `https://accounts.example.com`. Must equal
    /// the token's `iss` claim exactly (no trailing-slash normalization) and
    /// doubles as the base URL `auth::OidcValidator` resolves
    /// `/.well-known/openid-configuration` against to discover `jwks_uri` —
    /// see that type's own doc for the discovery/caching behavior.
    pub issuer: String,
    /// The expected `aud` claim value.
    pub audience: String,
    /// Which JWT claim carries tenant memberships, and how to read it. See
    /// `OidcClaimsConfig`.
    #[serde(default)]
    pub claims: OidcClaimsConfig,
    /// Whether configured tenant/role claim names may directly create
    /// memberships. Disabled by default for entries under
    /// `auth.trusted_issuers`; durable `(issuer, sub)` bindings remain
    /// authoritative. The legacy singular `auth.oidc` form preserves its
    /// historical claim-mapping behavior regardless of this flag.
    #[serde(default)]
    pub claims_authoritative: bool,
    /// Clock skew tolerance (seconds) for `exp`/`nbf` validation — the same
    /// role `jsonwebtoken::Validation::leeway` plays, just named for this
    /// config document. Small on purpose: this absorbs clock drift between
    /// this server and the token issuer, not a grace period for actually
    /// expired tokens.
    #[serde(default = "default_oidc_clock_skew_s")]
    pub clock_skew_s: u64,
    /// How long a fetched JWKS document is trusted before `OidcValidator`
    /// attempts to refresh it — see that type's own doc for the
    /// single-flight refresh this bounds.
    #[serde(default = "default_oidc_jwks_ttl_s")]
    pub jwks_ttl_s: u64,
}

fn default_oidc_clock_skew_s() -> u64 {
    60
}

fn default_oidc_jwks_ttl_s() -> u64 {
    300
}

pub(crate) fn oidc_endpoint_url_is_allowed(url: &url::Url) -> bool {
    if url.scheme() == "https" {
        return true;
    }
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Claim-mapping section of `OidcConfig`: today this only names the tenant
/// claim, but it's its own struct (rather than a bare `String` field on
/// `OidcConfig`) so a future claim mapping (roles, scopes) has somewhere to
/// live without another top-level `OidcConfig` field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OidcClaimsConfig {
    /// The claim name tenant memberships are read from. Accepts either a
    /// JSON array of strings (`["tenant-a", "tenant-b"]`) or a single
    /// space-separated string (`"tenant-a tenant-b"`, the OAuth2 `scope`
    /// convention many IdPs already emit tenant-like claims in) — see
    /// `auth::tenant_memberships_from_claim`.
    pub tenants: String,
    /// The claim carrying role names, for the RBAC/ABAC policy layer
    /// (`#34`). `None` (the default) means this token carries no roles at
    /// all — membership (via `claims.tenants`) without any role a policy
    /// grant can match, same starting point a `bearer_tokens` entry with an
    /// empty `roles` map has. When set, read with the same array-or-
    /// space-separated-string convention as `tenants`. Deliberately flat,
    /// not per-tenant structured (e.g. Keycloak's nested
    /// `resource_access`): every role this claim names applies uniformly
    /// across every tenant the subject holds membership in — see
    /// `auth::OidcValidator::subject`'s own doc for the exact mechanics and
    /// this slice's documented follow-up for per-tenant role claims.
    #[serde(default)]
    pub roles: Option<String>,
}

impl Default for OidcClaimsConfig {
    fn default() -> Self {
        Self {
            tenants: "tenants".to_string(),
            roles: None,
        }
    }
}

/// Which protocol lane a [`GrantDecl`] covers (`#34` policy layer,
/// authorization directive 4; `#68` added `Write`). Mirrors this workspace's
/// own protocol-crate split (`tellurion-features`, `tellurion-tiles`,
/// `tellurion-places`, `tellurion-stac`) rather than a finer per-endpoint
/// grain — a role that should read a collection's features but not its tiles
/// states two separate grants (or a `lanes` list on one grant naming both),
/// never a single implicit "read everything" toggle. `tellurion-styles` has
/// no variant here: style documents are global, not tenant/catalog-scoped
/// (see `tellurion-server::app`'s own module doc), so there is no resource
/// for a grant to scope against.
///
/// `Write` is never implied by any read lane, and no read lane is ever
/// implied by `Write` — a role that should both read and write a collection
/// states both in `lanes`. It also can never appear on a filtered grant
/// (`GrantDecl::filter`): `validate_grant` rejects that combination at boot
/// (row-level write conditions are out of scope until a real caller needs
/// them — see that function's own doc), so `policy::authorize_resource`
/// never has to reason about a filtered write in practice, even though
/// nothing in its evaluation loop is lane-specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyLane {
    Features,
    Tiles,
    Places3d,
    Stac,
    Write,
    /// The change-feed lane (`#115`): a compact envelope of ids/sequences,
    /// never a feature's payload. Like `Write`, this can never appear on a
    /// filtered grant — `validate_grant` rejects that combination at boot,
    /// the same treatment `Write`'s own doc explains, for the same reason:
    /// a filter narrows which ROWS a subject may see, but this lane never
    /// evaluates a filter against anything (there is no payload to test one
    /// against), so a grant that named one would either be silently ignored
    /// (which this codebase never does for a stated filter) or would have to
    /// deny outright regardless of the filter's content — refusing the
    /// combination by name at config load is the honest version of that
    /// same "enforced or refused, never dropped" rule.
    Feed,
}

/// Which collections a [`GrantDecl`] covers. Both lists empty (the default)
/// matches every collection in scope — the common case for a platform-wide
/// read role. `catalogs`/`collections` each hold internal ids (the same
/// convention `CatalogDecl::tenant` uses to reference an entity elsewhere in
/// the config document) and are additive: a collection matches if either
/// list names it — directly via `collections`, or via its owning catalog
/// via `catalogs` — or both lists are empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GrantScope {
    pub catalogs: Vec<String>,
    pub collections: Vec<String>,
}

impl GrantScope {
    /// Whether this scope matches a collection with internal id
    /// `collection_id` under catalog `catalog_id`. Both lists empty ("all")
    /// always matches.
    pub fn matches(&self, catalog_id: &str, collection_id: &str) -> bool {
        (self.catalogs.is_empty() && self.collections.is_empty())
            || self.catalogs.iter().any(|c| c == catalog_id)
            || self.collections.iter().any(|c| c == collection_id)
    }
}

/// One RBAC grant (`#34` policy layer, authorization directive 4): a role
/// that matches this grant's `lanes`/`scope` may read the resource outright
/// (`filter: None`) or with `filter` AND-merged into the query (ABAC,
/// directive 5) — see `policy::authorize_resource` for the exact
/// evaluation, and `policy.rs`'s module doc for which lanes can actually
/// push a filter down (only features'/STAC's items-list lanes can; every
/// other lane treats a filtered grant as "deny," never as "serve
/// unfiltered," since silently widening past what the filter says is unsafe
/// — see that module's doc for the full reasoning per lane).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GrantDecl {
    pub scope: GrantScope,
    pub lanes: Vec<PolicyLane>,
    /// CQL2-text expression, with `{{claims.NAME}}` placeholders substituted
    /// from the subject's claims at evaluation time (`#34` ABAC, directive
    /// 5). `None` means this grant allows outright, no row-level narrowing.
    /// A placeholder whose claim is absent from the subject makes this
    /// specific grant unsatisfied for that subject (excluded from the
    /// evaluation, not an error, and not a fallback to "unfiltered") — see
    /// `policy::authorize_resource`'s own doc for exactly where that rule is
    /// applied.
    #[serde(default)]
    pub filter: Option<String>,
    /// `#188`: a fixed-window request-rate ceiling charged whenever this
    /// grant authorizes a served request — see
    /// [`RateLimitDecl`](crate::rate_limit::RateLimitDecl) for the shape and
    /// `policy::enforce_rate_limits` for how several matching grants'
    /// ceilings compose. `None`/absent declares no ceiling, which is what
    /// every grant did before `#188`.
    ///
    /// Not accepted on a grant naming the `tiles` or `places3d` lane:
    /// [`validate_grant`] refuses that combination at boot, because those
    /// lanes' checkpoints are not wired to the rate seam in this slice and
    /// accepting a ceiling nothing would ever charge is exactly the silent
    /// no-op this codebase refuses everywhere else (the same treatment
    /// `AdmissionDecl` gets at a settings level admission never reads).
    #[serde(default)]
    pub rate: Option<crate::rate_limit::RateLimitDecl>,
}

/// One role's grants — a named bundle of [`GrantDecl`]s (`#34`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoleDecl {
    pub name: String,
    #[serde(default)]
    pub grants: Vec<GrantDecl>,
}

/// One tenant-custom policy document (`#34` policy layer, authorization
/// directive 6): `roles` declared here are visible only to `tenant` (an
/// internal id). A role name declared both here and in
/// `PolicyConfig::roles` (platform-shared) has this document's own grants
/// win outright for that name — nearest-level-wins, whole-role replacement,
/// the same "maps replace whole, never merged across levels" convention
/// `SettingsDecl` already uses (`settings.rs`) — not the platform grants
/// plus these, and not a per-grant merge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantPolicyDecl {
    /// The tenant internal id this document is scoped to.
    pub tenant: String,
    #[serde(default)]
    pub roles: Vec<RoleDecl>,
}

/// Authorization policy configuration (`#34`, the policy layer built on top
/// of `#17`/`#34`'s OIDC/membership authentication). Absent from the YAML
/// document (`PolicyConfig::default()`, both lists empty) means RBAC never
/// activates for any tenant — see [`is_configured`](Self::is_configured) and
/// `policy.rs`'s module doc for the exact per-tenant activation rule this
/// feeds, and this crate's authorization design doc
/// (`docs/design/2026-07-18-authorization-policy-layer.md`) for the full
/// picture. Two levels, nearest-wins (directive 6): `roles` is
/// platform-shared (available to every tenant); `tenant_policies` is
/// tenant-custom (visible to one tenant only, declared once per tenant —
/// see `AppConfig::validate` for the one-document-per-tenant check).
///
/// Deliberately independent of [`VisibilityDecl`] (isolation/cross-tenant
/// sharing): a resource with default (private) visibility and NO policy
/// configured for its tenant behaves exactly as it always has — full,
/// unfiltered access to every tenant member, RBAC inactive — see
/// `policy::authorize_resource`'s own doc for the precise per-tenant
/// activation threshold this and `VisibilityDecl` each independently gate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyConfig {
    pub roles: Vec<RoleDecl>,
    pub tenant_policies: Vec<TenantPolicyDecl>,
}

/// One declarative webhook subscription (`#115`): naming a URL, an event
/// filter (`scope`/`operations`), and a shared secret. The authenticated
/// config control lane exposes these as resources at `GET /config/webhooks`;
/// create/edit/disable uses the same versioned, audited `PUT /config`
/// compare-and-swap contract as every other config section. The server's
/// webhook manager rebinds on the resulting atomic config-generation swap
/// while preserving cursor/dead-letter state for an unchanged declaration.
///
/// `scope` reuses [`GrantScope`] verbatim (same "empty matches everything,
/// additive across `catalogs`/`collections`" rule) rather than inventing a
/// second scoping shape — a subscription's own event filter is "which
/// collections," the identical question a policy grant already answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookSubscriptionDecl {
    pub id: String,
    /// Must be an absolute `http(s)` URL — checked by `AppConfig::validate`.
    pub url: String,
    #[serde(default)]
    pub scope: GrantScope,
    /// Which operations this subscription delivers — empty (the default)
    /// means every operation, the same "empty list matches everything"
    /// convention `scope` above already uses.
    #[serde(default)]
    pub operations: Vec<crate::feed::FeedOperation>,
    /// Name of the environment variable holding the HMAC signing secret —
    /// the secret itself never lives in config, the same `url_env`/
    /// `secret_env` convention `StorageDecl.url_env`/`L2CacheConfig::Valkey.
    /// url_env` already use.
    pub secret_env: String,
    /// `false` skips spawning delivery for this subscription entirely
    /// (config stays declared, e.g. while an integration is being set up) —
    /// default `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// One MapLibre Style JSON document known to the file-backed `StyleStore`.
/// Richer metadata (name, layers, sources) lives in the document itself —
/// this is only enough to resolve an id to a path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StyleRef {
    pub id: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub cache: CacheConfig,
    pub storages: Vec<StorageDecl>,
    /// Managed-asset byte backends (assets-and-object-storage proposal,
    /// first slice) — a sibling list to `storages` above, never the same
    /// concept (see `ObjectStoreDecl`'s own doc). Optional: an empty list
    /// is a normal deployment with no managed-storage lane at all, exactly
    /// the remote-assets-only, metadata-only `core` conformance class.
    pub object_stores: Vec<ObjectStoreDecl>,
    pub tenants: Vec<TenantDecl>,
    /// Catalogs, each owned by exactly one tenant (`CatalogDecl::tenant`).
    pub catalogs: Vec<CatalogDecl>,
    pub collections: Vec<CollectionDecl>,
    /// Style documents known to the file-backed `StyleStore`. Optional —
    /// an empty list is a normal deployment with no styled-tile lane.
    /// Style documents are global (not tenant/catalog-scoped) in this wave —
    /// every `/{tenant}/styles/catalogs/{catalog}/styles/...` root serves
    /// the same registry regardless of which tenant/catalog reached it.
    pub styles: Vec<StyleRef>,
    /// Named settings profiles (`#111`) — reusable fragments any single
    /// level of the settings chain may reference by id (`SettingsDecl::
    /// profile`). Optional: an empty list is a normal deployment with no
    /// reuse across siblings at all, byte-for-byte today's behavior.
    /// Plain config data, gated by no cargo feature and no driver
    /// dependency — see `settings.rs`'s own doc for how expansion works.
    pub profiles: Vec<ProfileDecl>,
    /// Platform-level settings — the root of the inheritance chain
    /// (`#39`). See `SettingsDecl` and `settings.rs`.
    pub settings: SettingsDecl,
    /// Tenant authentication/authorization (`#17`, OIDC half `#34`). Absent
    /// from the YAML document defaults to `AuthConfig::default()`
    /// (permissive) — see that type's own doc.
    pub auth: AuthConfig,
    /// Registry read/validation configuration (`#42`). Absent from the YAML
    /// document defaults to `RegistryConfig::default()` — eager boot-time
    /// validation, byte-for-byte today's behavior.
    pub registry: RegistryConfig,
    /// Authorization policy (`#34`, the RBAC/ABAC layer built on top of
    /// `#17`/`#34`'s authentication). Absent from the YAML document defaults
    /// to `PolicyConfig::default()` — RBAC never activates for any tenant;
    /// see that type's own doc.
    pub policy: PolicyConfig,
    /// Declarative webhook subscriptions (`#115`). Optional — an empty list
    /// (the default) is a deployment with no push lane at all, and
    /// `ServerConfig.webhook_delivery.enabled` staying `false` (also the
    /// default) means even a non-empty list here spawns nothing. See
    /// `WebhookSubscriptionDecl`'s own doc.
    pub webhooks: Vec<WebhookSubscriptionDecl>,
}

impl AppConfig {
    /// Referential integrity across the whole document: unique ids, resolvable
    /// storage/tenant references, sane zoom ranges. Run once at load time so
    /// the router can assume a valid `AppConfig` forever after.
    pub fn validate(&self) -> Result<()> {
        if !(0.0..=100.0).contains(&self.cache.memory_percent) {
            return Err(Error::Config(format!(
                "cache.memory_percent ({}) must be within [0, 100]",
                self.cache.memory_percent
            )));
        }

        if let L2CacheConfig::Valkey { ttl_s, .. } = &self.cache.l2 {
            if *ttl_s == 0 {
                return Err(Error::Config(
                    "cache.l2.ttl_s must be greater than 0".to_string(),
                ));
            }
        }

        if self.server.max_concurrency == Some(0) {
            return Err(Error::Config(
                "server.max_concurrency must be at least 1".to_string(),
            ));
        }

        if let Some(public_base_url) = &self.server.public_base_url {
            let parsed = url::Url::parse(public_base_url).map_err(|_| {
                Error::Config("server.public_base_url must be an absolute http(s) URL".to_string())
            })?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.host().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
            {
                return Err(Error::Config(
                    "server.public_base_url must be an http(s) URL with a host and optional path prefix, without credentials, query, or fragment"
                        .to_string(),
                ));
            }
        }

        let webhook_delivery = self.server.webhook_delivery;
        if webhook_delivery.dead_letter_default_page_size == 0
            || webhook_delivery.dead_letter_default_page_size
                > webhook_delivery.dead_letter_max_page_size
        {
            return Err(Error::Config(format!(
                "server.webhook_delivery.dead_letter_default_page_size ({}) must be within [1, dead_letter_max_page_size ({})]",
                webhook_delivery.dead_letter_default_page_size,
                webhook_delivery.dead_letter_max_page_size
            )));
        }

        for (name, value) in [
            ("server.drain_timeout_s", self.server.drain_timeout_s),
            (
                "server.readiness_probe_interval_s",
                self.server.readiness_probe_interval_s,
            ),
            (
                "server.readiness_probe_timeout_s",
                self.server.readiness_probe_timeout_s,
            ),
        ] {
            if value == 0 {
                return Err(Error::Config(format!("{name} must be greater than 0")));
            }
        }

        let mut metric_tenants = HashSet::new();
        for tenant in &self.server.metrics_tenant_allowlist {
            if tenant.is_empty() {
                return Err(Error::Config(
                    "server.metrics_tenant_allowlist: tenant must not be empty".to_string(),
                ));
            }
            if !metric_tenants.insert(tenant.as_str()) {
                return Err(Error::Config(
                    "server.metrics_tenant_allowlist: duplicate tenant entry".to_string(),
                ));
            }
        }

        let mut metric_collections = HashSet::new();
        for collection in &self.server.metrics_collection_allowlist {
            for (name, value) in [
                ("tenant", collection.tenant.as_str()),
                ("catalog", collection.catalog.as_str()),
                ("collection", collection.collection.as_str()),
            ] {
                if value.is_empty() {
                    return Err(Error::Config(format!(
                        "server.metrics_collection_allowlist: {name} must not be empty"
                    )));
                }
            }
            if !metric_collections.insert((
                collection.tenant.as_str(),
                collection.catalog.as_str(),
                collection.collection.as_str(),
            )) {
                return Err(Error::Config(
                    "server.metrics_collection_allowlist: duplicate collection entry".to_string(),
                ));
            }
        }

        let known_profile_ids = collect_profile_ids(&self.profiles)?;
        validate_profiles(&self.profiles, &known_profile_ids)?;
        validate_settings("settings", &self.settings, &known_profile_ids, true)?;
        let profiles_by_id: HashMap<&str, &SettingsDecl> = self
            .profiles
            .iter()
            .map(|profile| (profile.id.as_str(), &profile.settings))
            .collect();

        let mut storage_ids = HashSet::new();
        for storage in &self.storages {
            if !storage_ids.insert(storage.id.as_str()) {
                return Err(Error::Config(format!(
                    "duplicate storage id '{}'",
                    storage.id
                )));
            }
            if storage.pool_size == Some(0) {
                return Err(Error::Config(format!(
                    "storages[{}].pool_size must be at least 1",
                    storage.id
                )));
            }
        }

        let mut object_store_ids = HashSet::new();
        for object_store in &self.object_stores {
            if !object_store_ids.insert(object_store.id.as_str()) {
                return Err(Error::Config(format!(
                    "duplicate object_store id '{}'",
                    object_store.id
                )));
            }
            match &object_store.profile {
                ObjectStoreProfile::Fs { root } => {
                    if root.trim().is_empty() {
                        return Err(Error::Config(format!(
                            "object_store '{}': fs.root must not be empty",
                            object_store.id
                        )));
                    }
                }
                ObjectStoreProfile::S3 {
                    endpoint,
                    bucket,
                    region,
                    access_key_env,
                    secret_key_env,
                    presign_expiry_s,
                    key_prefix: _,
                } => {
                    for (field, value) in [
                        ("bucket", bucket.as_str()),
                        ("region", region.as_str()),
                        ("access_key_env", access_key_env.as_str()),
                        ("secret_key_env", secret_key_env.as_str()),
                    ] {
                        if value.trim().is_empty() {
                            return Err(Error::Config(format!(
                                "object_store '{}': s3.{field} must not be empty",
                                object_store.id
                            )));
                        }
                    }
                    let parsed = url::Url::parse(endpoint).map_err(|source| {
                        Error::Config(format!(
                            "object_store '{}': s3.endpoint '{endpoint}' is not a valid URL: {source}",
                            object_store.id
                        ))
                    })?;
                    if parsed.scheme() != "http" && parsed.scheme() != "https" {
                        return Err(Error::Config(format!(
                            "object_store '{}': s3.endpoint must be an http(s) URL",
                            object_store.id
                        )));
                    }
                    if *presign_expiry_s == 0 || *presign_expiry_s > MAX_PRESIGN_EXPIRY_S {
                        return Err(Error::Config(format!(
                            "object_store '{}': s3.presign_expiry_s ({presign_expiry_s}) must be within [1, {MAX_PRESIGN_EXPIRY_S}]",
                            object_store.id
                        )));
                    }
                }
            }
        }

        if self.registry.backend == RegistryBackend::Relational {
            match &self.registry.storage {
                Some(storage) if storage_ids.contains(storage.as_str()) => {}
                Some(storage) => {
                    return Err(Error::Config(format!(
                        "registry.storage '{storage}' does not reference a declared storage"
                    )));
                }
                None => {
                    return Err(Error::Config(
                        "registry.backend is 'relational' but registry.storage is not set"
                            .to_string(),
                    ));
                }
            }
            // `#162`: shape only. An empty `implementation` names nothing and
            // could never resolve, so it is worth catching in the document
            // rather than at boot; whether a non-empty name is one this
            // binary actually registered is deliberately not checked here
            // (see `RegistryConfig::implementation`'s own doc).
            if self
                .registry
                .implementation
                .as_deref()
                .is_some_and(str::is_empty)
            {
                return Err(Error::Config(
                    "registry.implementation must not be empty".to_string(),
                ));
            }
            // Double-source is refused outright rather than merged with any
            // precedence rule (`#42`, third slice): a `relational` backend
            // means catalogs/collections are published into the registry
            // tables (`tellurion-ingest registry publish-*`), never declared
            // here too — an operator who forgets to remove a leftover
            // `catalogs:`/`collections:` section after switching backends
            // gets a named boot error instead of silently routing only one
            // of the two sources (whichever precedence would have picked)
            // and wondering why the other's collections 404.
            if !self.tenants.is_empty() || !self.catalogs.is_empty() || !self.collections.is_empty()
            {
                return Err(Error::Config(
                    "registry.backend is 'relational', but this config file also declares \
                     tenants/catalogs/collections; publish them to the registry tables instead \
                     (tellurion-ingest registry publish-tenant / publish-catalog / \
                     publish-collection) and \
                     remove them from this file, or switch registry.backend back to 'file'"
                        .to_string(),
                ));
            }
        }

        // Internal ids are globally unique across every declaration kind —
        // cache keys, driver lookups, and the resolver's forward index all
        // assume no two entities (tenant, catalog, or collection) ever share
        // one, even across kinds.
        let mut internal_ids = HashSet::new();

        validate_tenant_declarations(&self.tenants, &known_profile_ids)?;
        let tenant_ids: HashSet<&str> = self
            .tenants
            .iter()
            .map(|tenant| tenant.id.as_str())
            .collect();
        internal_ids.extend(tenant_ids.iter().copied());

        validate_catalogs_and_collections(
            &tenant_ids,
            &storage_ids,
            &object_store_ids,
            &self.catalogs,
            &self.collections,
            &mut internal_ids,
            &known_profile_ids,
        )?;

        // `#182`: the job ledger's storage, checked here with every other
        // referential-integrity check rather than at boot, so a typo is a
        // config-load refusal naming the key — the same treatment
        // `routing.<lane>` references already get. Whether that storage's
        // DRIVER can actually hold a ledger is a capability question no
        // config-only pass can answer; `Router::resolve_job_store` refuses
        // that one by name at boot.
        if let Some(processes) = &self.server.processes {
            if !storage_ids.contains(processes.storage.as_str()) {
                return Err(Error::Config(format!(
                    "server.processes.storage references unknown storage '{}'",
                    processes.storage
                )));
            }
            if processes.poll_interval_ms == 0 {
                return Err(Error::Config(
                    "server.processes.poll_interval_ms must be at least 1".to_string(),
                ));
            }
            if processes.visibility_timeout_s == 0 {
                return Err(Error::Config(
                    "server.processes.visibility_timeout_s must be at least 1".to_string(),
                ));
            }
        }

        let mut style_ids = HashSet::new();
        for style in &self.styles {
            if !style_ids.insert(style.id.as_str()) {
                return Err(Error::Config(format!("duplicate style id '{}'", style.id)));
            }
        }

        // Unconditional now (`#34`): `bearer_tokens` is a plain `Vec`, not one
        // arm of a backend-selecting enum, since it composes with `oidc`
        // rather than excluding it — see `AuthConfig`'s own doc.
        let mut seen_tokens = HashSet::new();
        let mut seen_token_envs = HashSet::new();
        for entry in &self.auth.bearer_tokens {
            // `#144`: exactly one of the two credential *locations*. Both at
            // once is ambiguous about which one is live, and neither leaves
            // the principal with no credential at all; both are refused by
            // name rather than resolved by a precedence rule nobody asked
            // for. Shape only — whether the named variable is actually set
            // is `auth::resolve_bearer_credentials`'s question, deliberately
            // not asked here so `validate` stays a pure function of the
            // document (a `PUT /config` dry run must not depend on the
            // environment of whichever instance served it).
            match (entry.token.is_empty(), entry.token_env.as_deref()) {
                (true, None) => {
                    return Err(Error::Config(
                        "auth.bearer_tokens: an entry declares neither 'token_env' nor an inline 'token'; declare exactly one".to_string(),
                    ));
                }
                (false, Some(_)) => {
                    return Err(Error::Config(
                        "auth.bearer_tokens: an entry declares both 'token_env' and an inline 'token'; declare exactly one".to_string(),
                    ));
                }
                (true, Some(name)) => {
                    if name.trim().is_empty() {
                        return Err(Error::Config(
                            "auth.bearer_tokens: token_env must not be empty".to_string(),
                        ));
                    }
                    // Two principals reading one variable are one credential
                    // wearing two hats: the second entry could never be
                    // reached, since the authorizer is keyed by token value.
                    // The variable NAME is not a secret, so it is safe to
                    // name here — the value it holds is never read at all on
                    // this path.
                    if !seen_token_envs.insert(name) {
                        return Err(Error::Config(format!(
                            "auth.bearer_tokens: two entries both read token_env '{name}'"
                        )));
                    }
                }
                (false, None) => {
                    // Never interpolate the token value itself into an error
                    // message — see `tellurion_core::auth`'s "never logs or
                    // echoes" rule.
                    if !seen_tokens.insert(entry.token.as_str()) {
                        return Err(Error::Config(
                            "auth.bearer_tokens: duplicate token entry".to_string(),
                        ));
                    }
                }
            }
            if entry.tenants.is_empty() {
                return Err(Error::Config(
                    "auth.bearer_tokens: a token must authorize at least one tenant".to_string(),
                ));
            }
            // `#34`: a token cannot hold a role in a tenant it isn't even a
            // member of — every `roles` key must also appear in `tenants`.
            for role_tenant_id in entry.roles.keys() {
                if !entry.tenants.iter().any(|t| t == role_tenant_id) {
                    return Err(Error::Config(format!(
                        "auth.bearer_tokens: roles declared for tenant '{role_tenant_id}', which is not in this token's own tenants list"
                    )));
                }
            }
        }

        // `#34`: shape-only checks — no network. `auth::OidcValidator`
        // deliberately never probes the issuer at config-load/reload time
        // (see its own doc for the reasoning); this is the full extent of
        // `oidc:` validation, so a reload with an unreachable (or even
        // nonexistent) issuer still passes here and swaps in cleanly, with
        // JWKS discovery deferred to the first bearer token that needs it.
        let mut seen_oidc_issuers = HashSet::new();
        for (path, oidc) in self.auth.oidc.iter().map(|oidc| ("auth.oidc", oidc)).chain(
            self.auth
                .trusted_issuers
                .iter()
                .map(|oidc| ("auth.trusted_issuers", oidc)),
        ) {
            if oidc.issuer.is_empty() {
                return Err(Error::Config(format!("{path}: issuer must not be empty")));
            }
            let issuer_url = url::Url::parse(&oidc.issuer).map_err(|source| {
                Error::Config(format!(
                    "{path}: issuer '{}' is not a valid URL: {source}",
                    oidc.issuer
                ))
            })?;
            if issuer_url.scheme() != "http" && issuer_url.scheme() != "https" {
                return Err(Error::Config(format!(
                    "{path}: issuer '{}' must be an http(s) URL",
                    oidc.issuer
                )));
            }
            if !oidc_endpoint_url_is_allowed(&issuer_url) {
                return Err(Error::Config(format!(
                    "{path}: issuer '{}' must use https unless it is a loopback development endpoint",
                    oidc.issuer
                )));
            }
            if !seen_oidc_issuers.insert(oidc.issuer.as_str()) {
                return Err(Error::Config(format!(
                    "{path}: duplicate trusted issuer '{}'",
                    oidc.issuer
                )));
            }
            if oidc.audience.is_empty() {
                return Err(Error::Config(format!("{path}: audience must not be empty")));
            }
            if oidc.claims.tenants.is_empty() {
                return Err(Error::Config(format!(
                    "{path}: claims.tenants must not be empty"
                )));
            }
            if oidc.jwks_ttl_s == 0 {
                return Err(Error::Config(format!(
                    "{path}: jwks_ttl_s must be greater than 0"
                )));
            }
        }

        if let Some(browser) = &self.auth.browser {
            let issuer_matches = self
                .auth
                .oidc
                .iter()
                .chain(self.auth.trusted_issuers.iter())
                .filter(|oidc| oidc.issuer == browser.issuer)
                .count();
            if issuer_matches != 1 {
                return Err(Error::Config(
                    "auth.browser.issuer must match exactly one configured OIDC issuer".to_string(),
                ));
            }
            if browser.client_id.trim().is_empty() {
                return Err(Error::Config(
                    "auth.browser.client_id must not be empty".to_string(),
                ));
            }
            if browser
                .client_secret_env
                .as_deref()
                .is_some_and(|name| name.trim().is_empty())
            {
                return Err(Error::Config(
                    "auth.browser.client_secret_env must not be empty".to_string(),
                ));
            }
            if browser.scopes.is_empty()
                || browser.scopes.iter().any(|scope| scope.trim().is_empty())
            {
                return Err(Error::Config(
                    "auth.browser.scopes must contain only non-empty values".to_string(),
                ));
            }
            if !(1..=MAX_CONTROL_BROWSER_SESSION_TTL_S).contains(&browser.session_ttl_s) {
                return Err(Error::Config(format!(
                    "auth.browser.session_ttl_s must be within [1, {MAX_CONTROL_BROWSER_SESSION_TTL_S}]"
                )));
            }
            if !(1..=MAX_CONTROL_BROWSER_LOGIN_TTL_S).contains(&browser.login_ttl_s) {
                return Err(Error::Config(format!(
                    "auth.browser.login_ttl_s must be within [1, {MAX_CONTROL_BROWSER_LOGIN_TTL_S}]"
                )));
            }
            if !(1..=MAX_CONTROL_BROWSER_SESSIONS).contains(&browser.max_sessions) {
                return Err(Error::Config(format!(
                    "auth.browser.max_sessions must be within [1, {MAX_CONTROL_BROWSER_SESSIONS}]"
                )));
            }

            let origin = url::Url::parse(&browser.public_origin).map_err(|_| {
                Error::Config("auth.browser.public_origin must be a valid URL origin".to_string())
            })?;
            if origin.host().is_none()
                || !oidc_endpoint_url_is_allowed(&origin)
                || !origin.username().is_empty()
                || origin.password().is_some()
                || origin.path() != "/"
                || origin.query().is_some()
                || origin.fragment().is_some()
            {
                return Err(Error::Config(
                    "auth.browser.public_origin must be an HTTPS origin (or loopback HTTP for development) without credentials, path, query, or fragment"
                        .to_string(),
                ));
            }
        }

        validate_settings_finality(
            &self.settings,
            &self.tenants,
            &self.catalogs,
            &self.collections,
            &profiles_by_id,
        )?;

        self.validate_policy()?;
        if self.registry.backend == RegistryBackend::File {
            self.validate_tenant_references(&tenant_ids)?;
        }
        self.validate_webhooks()?;

        Ok(())
    }

    /// Completes relational validation after the authoritative tenant and
    /// catalog/collection snapshots have been read. [`validate`](Self::validate)
    /// owns source-independent shape checks; this phase resolves tenant,
    /// catalog, and collection references against the rows that will
    /// actually be published in `ContextState`.
    pub fn validate_with_registry(
        &self,
        tenants: &[TenantDecl],
        snapshot: &RoutingSnapshot,
    ) -> Result<()> {
        self.validate()?;
        validate_registry_snapshot(self, tenants, snapshot)
    }

    /// `#115`: unique subscription ids, an absolute `http(s)` `url`, a
    /// non-empty `secret_env` name (the secret itself is never checked here
    /// — resolving/requiring the named environment variable is the delivery
    /// consumer's own boot-time concern, the identical split
    /// `StorageDecl.url_env`/`L2CacheConfig::Valkey.url_env` already draw),
    /// and — for the file-backed registry only, same caveat
    /// `validate_policy` documents for grant scopes — that every catalog/
    /// collection id named in `scope` resolves to a real one.
    fn validate_webhooks(&self) -> Result<()> {
        let catalog_by_id: HashMap<&str, &CatalogDecl> =
            self.catalogs.iter().map(|c| (c.id.as_str(), c)).collect();
        let collection_by_id: HashMap<&str, &CollectionDecl> = self
            .collections
            .iter()
            .map(|c| (c.id.as_str(), c))
            .collect();
        let check_refs = !self.catalogs.is_empty() || !self.collections.is_empty();

        let mut seen_ids = HashSet::new();
        for subscription in &self.webhooks {
            if subscription.id.is_empty() {
                return Err(Error::Config(
                    "webhooks: subscription id must not be empty".to_string(),
                ));
            }
            if !seen_ids.insert(subscription.id.as_str()) {
                return Err(Error::Config(format!(
                    "webhooks: duplicate subscription id '{}'",
                    subscription.id
                )));
            }
            let url = url::Url::parse(&subscription.url).map_err(|_| {
                Error::Config(format!(
                    "webhooks['{}']: url '{}' does not parse as an absolute URL",
                    subscription.id, subscription.url
                ))
            })?;
            if url.scheme() != "http" && url.scheme() != "https" {
                return Err(Error::Config(format!(
                    "webhooks['{}']: url must be an http(s) URL",
                    subscription.id
                )));
            }
            if subscription.secret_env.is_empty() {
                return Err(Error::Config(format!(
                    "webhooks['{}']: secret_env must not be empty",
                    subscription.id
                )));
            }
            if check_refs {
                for catalog_id in &subscription.scope.catalogs {
                    if !catalog_by_id.contains_key(catalog_id.as_str()) {
                        return Err(Error::Config(format!(
                            "webhooks['{}']: scope references unknown catalog '{catalog_id}'",
                            subscription.id
                        )));
                    }
                }
                for collection_id in &subscription.scope.collections {
                    if !collection_by_id.contains_key(collection_id.as_str()) {
                        return Err(Error::Config(format!(
                            "webhooks['{}']: scope references unknown collection '{collection_id}'",
                            subscription.id
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// `#34` policy-layer validation: role/grant shape, and — for the
    /// file-backed registry only (see below) — that every grant's scope
    /// references a real catalog/collection, and that a tenant-custom
    /// document's grants stay inside its own tenant (the config-time half of
    /// authorization directive 6's "tenant policies never widen across
    /// tenants" rule; the request-time half is structural — see
    /// `policy::authorize_resource`'s own doc). Filter templates are checked
    /// for CQL2-text syntax only, with every `{{claims.NAME}}` placeholder
    /// substituted by a dummy literal — the same "shape, not full semantics"
    /// scope `SchemaDecl::validate`'s own referential checks stay within,
    /// since a real check needs a derived `CollectionDescriptor` this
    /// function has no access to.
    ///
    /// Grant scope referential checks only run here when
    /// `catalogs`/`collections` are non-empty (the file registry backend).
    /// For `registry.backend: relational`, those lists are empty by the
    /// double-source rule; [`validate_registry_snapshot`] performs the same
    /// checks after the database snapshot is read. Tenant-policy ownership
    /// is likewise checked against the authoritative tenant snapshot by
    /// [`AppConfig::validate_with_registry`].
    fn validate_policy(&self) -> Result<()> {
        let catalog_by_id: HashMap<&str, &CatalogDecl> =
            self.catalogs.iter().map(|c| (c.id.as_str(), c)).collect();
        let collection_by_id: HashMap<&str, &CollectionDecl> = self
            .collections
            .iter()
            .map(|c| (c.id.as_str(), c))
            .collect();
        let check_refs = !self.catalogs.is_empty() || !self.collections.is_empty();

        let mut seen_role_names = HashSet::new();
        for role in &self.policy.roles {
            if role.name.is_empty() {
                return Err(Error::Config(
                    "policy.roles: role name must not be empty".to_string(),
                ));
            }
            if !seen_role_names.insert(role.name.as_str()) {
                return Err(Error::Config(format!(
                    "policy.roles: duplicate role name '{}'",
                    role.name
                )));
            }
            for grant in &role.grants {
                validate_grant(
                    &format!("policy.roles['{}']", role.name),
                    grant,
                    None,
                    &catalog_by_id,
                    &collection_by_id,
                    check_refs,
                )?;
            }
        }

        let mut seen_tenant_policy_tenants = HashSet::new();
        for tenant_policy in &self.policy.tenant_policies {
            if !seen_tenant_policy_tenants.insert(tenant_policy.tenant.as_str()) {
                return Err(Error::Config(format!(
                    "policy.tenant_policies: tenant '{}' has more than one tenant-custom policy document; declare its roles in a single document",
                    tenant_policy.tenant
                )));
            }
            let mut seen = HashSet::new();
            for role in &tenant_policy.roles {
                if role.name.is_empty() {
                    return Err(Error::Config(format!(
                        "policy.tenant_policies['{}']: role name must not be empty",
                        tenant_policy.tenant
                    )));
                }
                if !seen.insert(role.name.as_str()) {
                    return Err(Error::Config(format!(
                        "policy.tenant_policies['{}']: duplicate role name '{}'",
                        tenant_policy.tenant, role.name
                    )));
                }
                for grant in &role.grants {
                    validate_grant(
                        &format!(
                            "policy.tenant_policies['{}'].roles['{}']",
                            tenant_policy.tenant, role.name
                        ),
                        grant,
                        Some(tenant_policy.tenant.as_str()),
                        &catalog_by_id,
                        &collection_by_id,
                        check_refs,
                    )?;
                }
            }
        }

        Ok(())
    }

    /// Cross-checks operator-configured tenant references against the
    /// authoritative tenant snapshot. File-backed configs can do this in
    /// [`validate`](Self::validate); relational configs defer it until the
    /// tenant rows have been read.
    fn validate_tenant_references(&self, tenant_ids: &HashSet<&str>) -> Result<()> {
        for entry in &self.auth.bearer_tokens {
            for tenant_id in &entry.tenants {
                if !tenant_ids.contains(tenant_id.as_str()) {
                    return Err(Error::Config(format!(
                        "auth.bearer_tokens: references unknown tenant '{tenant_id}'"
                    )));
                }
            }
        }
        for tenant_policy in &self.policy.tenant_policies {
            if !tenant_ids.contains(tenant_policy.tenant.as_str()) {
                return Err(Error::Config(format!(
                    "policy.tenant_policies: references unknown tenant '{}'",
                    tenant_policy.tenant
                )));
            }
        }
        Ok(())
    }
}

/// Checks one [`GrantDecl`]: `lanes` non-empty, filter template a
/// syntactically valid CQL2-text expression once every `{{claims.NAME}}`
/// placeholder is substituted with a dummy literal, and — when `check_refs`
/// — every id in `grant.scope` resolves to a real catalog/collection, owned
/// by `owning_tenant` when this grant came from a tenant-custom document
/// (`owning_tenant: None` for a platform-shared role, which may reference
/// any tenant's catalogs/collections).
fn validate_grant(
    context: &str,
    grant: &GrantDecl,
    owning_tenant: Option<&str>,
    catalog_by_id: &HashMap<&str, &CatalogDecl>,
    collection_by_id: &HashMap<&str, &CollectionDecl>,
    check_refs: bool,
) -> Result<()> {
    if grant.lanes.is_empty() {
        return Err(Error::Config(format!(
            "{context}: a grant must name at least one lane"
        )));
    }
    if grant.filter.is_some() && grant.lanes.contains(&PolicyLane::Write) {
        return Err(Error::Config(format!(
            "{context}: a grant naming the 'write' lane must not declare a filter — row-level write conditions are not supported; split this into a filtered read grant and an unfiltered write grant"
        )));
    }
    if grant.filter.is_some() && grant.lanes.contains(&PolicyLane::Feed) {
        return Err(Error::Config(format!(
            "{context}: a grant naming the 'feed' lane must not declare a filter — the change feed serves compact envelopes only, never a payload a filter could narrow; split this into a filtered read grant and an unfiltered feed grant"
        )));
    }
    if let Some(template) = &grant.filter {
        validate_grant_filter_template(context, template)?;
    }
    if let Some(rate) = &grant.rate {
        // `#188`: the rate seam is wired into the features/STAC/write/feed
        // checkpoints only. A ceiling on a lane whose checkpoint never
        // charges it would look enforced in the document and be a decoration
        // in practice — refused by name here instead, the same "never
        // silently accept a value nothing reads" rule `AdmissionDecl`
        // already follows for a catalog-level admission override.
        for unwired in [PolicyLane::Tiles, PolicyLane::Places3d] {
            if grant.lanes.contains(&unwired) {
                return Err(Error::Config(format!(
                    "{context}: a grant naming the '{}' lane must not declare a rate condition — that lane's policy checkpoint does not charge rate ceilings in this build; split this into a separate grant for the lanes that do (features, stac, write, feed)",
                    match unwired {
                        PolicyLane::Tiles => "tiles",
                        _ => "places3d",
                    }
                )));
            }
        }
        rate.validate(context)?;
    }
    if !check_refs {
        return Ok(());
    }
    for catalog_id in &grant.scope.catalogs {
        let catalog = catalog_by_id.get(catalog_id.as_str()).ok_or_else(|| {
            Error::Config(format!(
                "{context}: grant references unknown catalog '{catalog_id}'"
            ))
        })?;
        if let Some(tenant) = owning_tenant {
            if catalog.tenant != tenant {
                return Err(Error::Config(format!(
                    "{context}: grant references catalog '{catalog_id}' owned by tenant '{}', outside this tenant-custom document's own tenant '{tenant}' — a tenant policy can never widen access to another tenant's resources",
                    catalog.tenant
                )));
            }
        }
    }
    for collection_id in &grant.scope.collections {
        let collection = collection_by_id
            .get(collection_id.as_str())
            .ok_or_else(|| {
                Error::Config(format!(
                    "{context}: grant references unknown collection '{collection_id}'"
                ))
            })?;
        if let Some(tenant) = owning_tenant {
            let owning_catalog_tenant = catalog_by_id
                .get(collection.catalog.as_str())
                .map(|c| c.tenant.as_str());
            if owning_catalog_tenant != Some(tenant) {
                return Err(Error::Config(format!(
                    "{context}: grant references collection '{collection_id}', outside this tenant-custom document's own tenant '{tenant}' — a tenant policy can never widen access to another tenant's resources"
                )));
            }
        }
    }
    Ok(())
}

/// Placeholder marker for ABAC claim substitution inside a grant's filter
/// template — `{{claims.NAME}}` — see `GrantDecl::filter`'s own doc. Scanned
/// by hand rather than with a `regex` dependency (the same small-grammar
/// philosophy `filter.rs`'s own module doc explains at length): the shape is
/// fixed and simple enough that a manual scan is both cheaper and, for a
/// codebase already avoiding `regex` elsewhere, more consistent.
const CLAIM_PLACEHOLDER_PREFIX: &str = "{{claims.";
const CLAIM_PLACEHOLDER_SUFFIX: &str = "}}";

/// Validates `template`'s CQL2-text syntax by substituting every
/// `{{claims.NAME}}` placeholder with a fixed dummy string literal and
/// running it through `filter::parse_text` — a shape-only check (a typo'd
/// operator or unbalanced parens fails boot immediately) that deliberately
/// does NOT check property names against a collection's descriptor (no
/// derived descriptor exists at config-load time; the real filter — with
/// real claim values substituted — is validated again at request time via
/// the same `filter::validate` a user-supplied `filter` query parameter
/// goes through, see `policy::authorize_resource`'s own doc).
fn validate_grant_filter_template(context: &str, template: &str) -> Result<()> {
    let mut substituted = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find(CLAIM_PLACEHOLDER_PREFIX) {
        let (before, after_prefix) = rest.split_at(start);
        substituted.push_str(before);
        let after_prefix = &after_prefix[CLAIM_PLACEHOLDER_PREFIX.len()..];
        let Some(end) = after_prefix.find(CLAIM_PLACEHOLDER_SUFFIX) else {
            return Err(Error::Config(format!(
                "{context}: filter template has an unterminated '{{{{claims....' placeholder"
            )));
        };
        let claim_name = &after_prefix[..end];
        if claim_name.is_empty() {
            return Err(Error::Config(format!(
                "{context}: filter template has an empty '{{{{claims.}}}}' placeholder"
            )));
        }
        substituted.push_str("'__policy_validation_placeholder__'");
        rest = &after_prefix[end + CLAIM_PLACEHOLDER_SUFFIX.len()..];
    }
    substituted.push_str(rest);

    crate::filter::parse_text(&substituted).map_err(|source| {
        Error::Config(format!(
            "{context}: filter template is not valid CQL2-text once claim placeholders are substituted: {source}"
        ))
    })?;
    Ok(())
}

/// Referential + shape check for one routing lane: non-empty, and every
/// named storage id resolves against `storage_ids`. Whether the named
/// storage's driver actually implements the lane's capability trait needs a
/// built `Router` and is checked in `Router::validate_catalog` instead —
/// this only catches a typo'd or unknown storage id before any driver
/// exists.
fn validate_lane_routing(
    collection_id: &str,
    lane: &str,
    routing: &LaneRouting,
    storage_ids: &HashSet<&str>,
) -> Result<()> {
    if routing.0.is_empty() {
        return Err(Error::Config(format!(
            "collection '{collection_id}': routing lane '{lane}' has no storage entries"
        )));
    }
    for storage_id in &routing.0 {
        if !storage_ids.contains(storage_id.as_str()) {
            return Err(Error::Config(format!(
                "collection '{collection_id}': routing lane '{lane}' references unknown storage '{storage_id}'"
            )));
        }
    }
    Ok(())
}

/// Cross-checks `catalogs`/`collections` against `tenant_ids`/`storage_ids`
/// and each other: unique internal/external ids at their own scope,
/// resolvable tenant/storage/catalog references, and each collection's own
/// routing/tiles/places3d/schema shape. Shared by [`AppConfig::validate`]
/// (the YAML path — `catalogs`/`collections` straight off the parsed
/// document) and [`validate_registry_snapshot`] (the relational registry
/// path, `#42` third slice — a [`RoutingSnapshot`] walked from a
/// `RegistryReader`) so both sources are held to exactly the same bar, with
/// exactly one implementation of that bar to keep in sync. `internal_ids`
/// must already hold every tenant's internal id when this is called — see
/// `AppConfig::validate`'s own call site for why tenants are seeded first
/// (a catalog or collection reusing a tenant's internal id is exactly what
/// this catches).
fn validate_catalogs_and_collections<'a>(
    tenant_ids: &HashSet<&'a str>,
    storage_ids: &HashSet<&'a str>,
    object_store_ids: &HashSet<&'a str>,
    catalogs: &'a [CatalogDecl],
    collections: &'a [CollectionDecl],
    internal_ids: &mut HashSet<&'a str>,
    known_profile_ids: &HashSet<&str>,
) -> Result<()> {
    let mut catalog_ids = HashSet::new();
    // Catalog external ids are unique per tenant, not globally — two
    // tenants may both declare a `default` catalog.
    let mut catalog_external_ids_by_tenant: HashMap<&str, HashSet<&str>> = HashMap::new();
    for catalog in catalogs {
        validate_settings(
            &format!("catalog '{}'.settings", catalog.id),
            &catalog.settings,
            known_profile_ids,
            false,
        )?;
        if !catalog_ids.insert(catalog.id.as_str()) {
            return Err(Error::Config(format!(
                "duplicate catalog id '{}'",
                catalog.id
            )));
        }
        if !internal_ids.insert(catalog.id.as_str()) {
            return Err(Error::Config(format!(
                "internal id '{}' is reused across declarations",
                catalog.id
            )));
        }
        if !tenant_ids.contains(catalog.tenant.as_str()) {
            return Err(Error::Config(format!(
                "catalog '{}' references unknown tenant '{}'",
                catalog.id, catalog.tenant
            )));
        }
        let external_id = catalog.external_id();
        if !catalog_external_ids_by_tenant
            .entry(catalog.tenant.as_str())
            .or_default()
            .insert(external_id)
        {
            return Err(Error::Config(format!(
                "catalog '{}': external_id '{external_id}' is already used by another catalog under tenant '{}'",
                catalog.id, catalog.tenant
            )));
        }
        // `#34`: a `shared_with` entry that names no real tenant would
        // otherwise just never match (`Subject::is_member_of` can't hold
        // membership in a tenant that doesn't exist) — silently inert
        // rather than a named boot error. Caught here, symmetrically with
        // every other referential check `validate_catalogs_and_collections`
        // already runs on `catalog`/`collection` declarations, for both the
        // YAML path (`AppConfig::validate`) and the relational registry
        // path (`validate_registry_snapshot`), since both call this
        // function.
        for tenant_id in &catalog.visibility.shared_with {
            if !tenant_ids.contains(tenant_id.as_str()) {
                return Err(Error::Config(format!(
                    "catalog '{}': visibility.shared_with references unknown tenant '{tenant_id}'",
                    catalog.id
                )));
            }
        }
    }

    let mut collection_ids = HashSet::new();
    // Collection external ids are unique per catalog, not globally.
    let mut collection_external_ids_by_catalog: HashMap<&str, HashSet<&str>> = HashMap::new();
    for collection in collections {
        validate_settings(
            &format!("collection '{}'.settings", collection.id),
            &collection.settings,
            known_profile_ids,
            false,
        )?;
        if !collection_ids.insert(collection.id.as_str()) {
            return Err(Error::Config(format!(
                "duplicate collection id '{}'",
                collection.id
            )));
        }
        if !internal_ids.insert(collection.id.as_str()) {
            return Err(Error::Config(format!(
                "internal id '{}' is reused across declarations",
                collection.id
            )));
        }
        if !storage_ids.contains(collection.storage.as_str()) {
            return Err(Error::Config(format!(
                "collection '{}' references unknown storage '{}'",
                collection.id, collection.storage
            )));
        }
        if let Some(object_store) = &collection.object_store {
            if !object_store_ids.contains(object_store.as_str()) {
                return Err(Error::Config(format!(
                    "collection '{}' references unknown object_store '{object_store}'",
                    collection.id
                )));
            }
        }
        if !catalog_ids.contains(collection.catalog.as_str()) {
            return Err(Error::Config(format!(
                "collection '{}' references unknown catalog '{}'",
                collection.id, collection.catalog
            )));
        }
        let external_id = collection.external_id();
        if !collection_external_ids_by_catalog
            .entry(collection.catalog.as_str())
            .or_default()
            .insert(external_id)
        {
            return Err(Error::Config(format!(
                "collection '{}': external_id '{external_id}' is already used by another collection under catalog '{}'",
                collection.id, collection.catalog
            )));
        }
        // `#34`: same referential check as `catalog.visibility.shared_with`
        // above, one level down.
        for tenant_id in &collection.visibility.shared_with {
            if !tenant_ids.contains(tenant_id.as_str()) {
                return Err(Error::Config(format!(
                    "collection '{}': visibility.shared_with references unknown tenant '{tenant_id}'",
                    collection.id
                )));
            }
        }
        if let Some(features) = &collection.routing.features {
            validate_lane_routing(&collection.id, "features", features, storage_ids)?;
        }
        if let Some(tiles) = &collection.routing.tiles {
            validate_lane_routing(&collection.id, "tiles", tiles, storage_ids)?;
        }
        if let Some(maps) = &collection.routing.maps {
            validate_lane_routing(&collection.id, "maps", maps, storage_ids)?;
        }
        if let Some(write) = &collection.routing.write {
            validate_lane_routing(&collection.id, "write", write, storage_ids)?;
            // `#25`: write is "exactly one driver, no fallback tail" — a
            // write has nowhere sensible to fall through to (see
            // `RoutingDecl`'s own doc).
            if write.0.len() != 1 {
                return Err(Error::Config(format!(
                    "collection '{}': routing lane 'write' must name exactly one storage, not {} (no fallback tail)",
                    collection.id,
                    write.0.len()
                )));
            }
        }
        if let Some(index) = &collection.routing.index {
            validate_lane_routing(&collection.id, "index", index, storage_ids)?;
            // `#67`: same "exactly one driver, no fallback tail" shape as
            // `write` — see `RoutingDecl`'s own doc.
            if index.0.len() != 1 {
                return Err(Error::Config(format!(
                    "collection '{}': routing lane 'index' must name exactly one storage, not {} (no fallback tail)",
                    collection.id,
                    index.0.len()
                )));
            }
        }
        if let Some(search) = &collection.routing.search {
            // `#67`: only the referential/non-empty shape check here — unlike
            // `write`/`index`, `search` may legitimately name more than one
            // storage (an ordered fallback tail, `RoutingDecl`'s own doc), so
            // there is no length restriction to add. Whether each named
            // storage's driver can actually serve this lane, and whether an
            // index-capable entry is one this collection's own `routing.index`
            // provisions, needs a built `Router` and is checked in
            // `Router::validate_catalog` instead (`validate_search_lane_*`).
            validate_lane_routing(&collection.id, "search", search, storage_ids)?;
        }
        collection.tiles.validate(&collection.id)?;
        validate_geometry_variants(
            &collection.id,
            &collection.tiles,
            &collection.geometry_variants,
        )?;
        if let Some(places3d) = &collection.places3d {
            places3d.validate(&collection.id, &collection.tiles)?;
        }
        if let Some(schema) = &collection.schema {
            schema.validate(&collection.id)?;
        }
    }

    Ok(())
}

/// Collects every declared profile id (`#111`), refusing a duplicate the
/// same way every other id kind in this module refuses one. Run before
/// `validate_profiles`/any `validate_settings` call so the returned set is
/// ready for the dangling-reference check every level's own call makes.
fn collect_profile_ids(profiles: &[ProfileDecl]) -> Result<HashSet<&str>> {
    let mut ids = HashSet::new();
    for profile in profiles {
        if !ids.insert(profile.id.as_str()) {
            return Err(Error::Config(format!(
                "duplicate profile id '{}'",
                profile.id
            )));
        }
    }
    Ok(ids)
}

/// Validates each profile's own settings fragment (`#111`): the same shape
/// checks `validate_settings` runs for every other level, plus one profile-
/// specific rule it doesn't need to enforce anywhere else — a profile's own
/// `settings.profile` must be unset. Composing profiles ("profile-of-
/// profiles") isn't a richer merge algebra this resolver refuses to grow
/// into; it's a sign the fragment was cut wrong, so it's refused outright
/// rather than accepted as a chain of its own.
///
/// `admission` (`#66`) is refused inside a profile fragment outright (the
/// trailing `false`) rather than merely inert at the wrong level the way a
/// literal catalog/collection declaration is: a profile can be referenced
/// from any level, and admission resolution never expands a profile
/// reference (see `admission::AdmissionDecl`'s own doc) — so there is no
/// level at which a profile-supplied admission override would ever take
/// effect, and accepting one here would be a silent no-op with no path to
/// ever become real.
fn validate_profiles(profiles: &[ProfileDecl], known_profile_ids: &HashSet<&str>) -> Result<()> {
    for profile in profiles {
        if profile.settings.profile.is_some() {
            return Err(Error::Config(format!(
                "profile '{}': a profile cannot itself reference another profile (no profile-of-profiles)",
                profile.id
            )));
        }
        // `#110`: a profile is a reusable fragment referenced from a real
        // level's own slot in the chain — "final" governs which LEVEL may
        // override a key, a question a profile fragment (which has no
        // level of its own) can't meaningfully answer either way.
        if !profile.settings.final_keys.is_empty() {
            return Err(Error::Config(format!(
                "profile '{}': a profile cannot declare settings keys final (finality is a property of a chain level, not a reusable fragment)",
                profile.id
            )));
        }
        validate_settings(
            &format!("profile '{}'.settings", profile.id),
            &profile.settings,
            known_profile_ids,
            false,
        )?;
    }
    Ok(())
}

fn validate_settings(
    context: &str,
    settings: &SettingsDecl,
    known_profile_ids: &HashSet<&str>,
    admission_allowed: bool,
) -> Result<()> {
    if let Some(profile_id) = &settings.profile {
        if !known_profile_ids.contains(profile_id.as_str()) {
            return Err(Error::Config(format!(
                "{context}.profile references unknown profile '{profile_id}'"
            )));
        }
    }
    let mut seen_final_keys = HashSet::new();
    for key in &settings.final_keys {
        if !SETTINGS_KEY_NAMES.contains(&key.as_str()) {
            return Err(Error::Config(format!(
                "{context}.final references unknown settings key '{key}'"
            )));
        }
        if !seen_final_keys.insert(key.as_str()) {
            return Err(Error::Config(format!(
                "{context}.final declares '{key}' more than once"
            )));
        }
    }
    if settings.slow_request_ms == Some(0) {
        return Err(Error::Config(format!(
            "{context}.slow_request_ms must be greater than 0"
        )));
    }
    if let Some(admission) = &settings.admission {
        if !admission_allowed {
            return Err(Error::Config(format!(
                "{context}.admission is only honored at the platform or tenant level \
                 (admission runs before routing resolves a catalog or collection); \
                 remove it from {context} or move it to the tenant's own settings"
            )));
        }
        if admission.weight == Some(0) {
            return Err(Error::Config(format!(
                "{context}.admission.weight must be at least 1"
            )));
        }
    }
    if settings.max_request_body_bytes == Some(0) {
        return Err(Error::Config(format!(
            "{context}.max_request_body_bytes must be greater than 0"
        )));
    }
    if settings.tile_vertex_budget == Some(0) {
        return Err(Error::Config(format!(
            "{context}.tile_vertex_budget must be greater than 0"
        )));
    }
    if settings.items_vertex_budget == Some(0) {
        return Err(Error::Config(format!(
            "{context}.items_vertex_budget must be greater than 0"
        )));
    }
    if settings.page_max_bytes == Some(0) {
        return Err(Error::Config(format!(
            "{context}.page_max_bytes must be greater than 0"
        )));
    }
    if settings.max_asset_bytes == Some(0) {
        return Err(Error::Config(format!(
            "{context}.max_asset_bytes must be greater than 0"
        )));
    }
    if let Some(batch) = &settings.batch {
        if batch.max_bytes == Some(0) {
            return Err(Error::Config(format!(
                "{context}.batch.max_bytes must be greater than 0"
            )));
        }
        if batch.max_items == Some(0) {
            return Err(Error::Config(format!(
                "{context}.batch.max_items must be greater than 0"
            )));
        }
        if batch.chunk_items == Some(0) {
            return Err(Error::Config(format!(
                "{context}.batch.chunk_items must be at least 1"
            )));
        }
    }
    if let Some(stac) = &settings.stac {
        stac.validate(context)?;
    }
    if let Some(tile_properties) = &settings.tile_properties {
        validate_tile_properties(context, tile_properties)?;
    }
    if let Some(colormap) = &settings.colormap {
        colormap.validate(context)?;
    }
    Ok(())
}

/// Validates the declaration-local tenant invariants shared by the file
/// configuration and a relational [`TenantReader`](crate::tenant::TenantReader)
/// snapshot. Cross-resource references (catalogs, policy grants, and
/// settings finality) remain in their existing whole-snapshot validators;
/// this helper owns the rules intrinsic to the tenant rows themselves so a
/// database row cannot bypass a check merely by avoiding YAML parsing.
pub(crate) fn validate_tenant_declarations(
    tenants: &[TenantDecl],
    known_profile_ids: &HashSet<&str>,
) -> Result<()> {
    let mut tenant_ids = HashSet::new();
    let mut tenant_external_ids = HashSet::new();
    for tenant in tenants {
        validate_settings(
            &format!("tenant '{}'.settings", tenant.id),
            &tenant.settings,
            known_profile_ids,
            true,
        )?;
        if !tenant_ids.insert(tenant.id.as_str()) {
            return Err(Error::Config(format!(
                "duplicate tenant id '{}'",
                tenant.id
            )));
        }
        let external_id = tenant.external_id();
        if !tenant_external_ids.insert(external_id) {
            return Err(Error::Config(format!(
                "duplicate tenant external_id '{external_id}'"
            )));
        }
        if RESERVED_TENANT_SEGMENTS.contains(&external_id) {
            return Err(Error::Config(format!(
                "tenant '{}': external_id '{external_id}' is a reserved top-level segment ({}); choose a different external_id",
                tenant.id,
                RESERVED_TENANT_SEGMENTS.join(", ")
            )));
        }
    }
    Ok(())
}

/// Eager, shape-only validation for `settings.tile_properties` (`#85`) — an
/// empty column name or a name repeated within the same list fails the whole
/// config at load time, the same "bad shape fails boot, not mid-request"
/// rule `StacConf::validate`'s own `stac.assets` check applies. Whether each
/// name actually names a real, projectable column is a separate, later
/// check (`descriptor::reconcile_tile_properties`) — that needs a derived
/// `CollectionDescriptor`, which doesn't exist yet at `AppConfig::validate`
/// time.
fn validate_tile_properties(context: &str, tile_properties: &[String]) -> Result<()> {
    let mut seen = HashSet::new();
    for property in tile_properties {
        if property.is_empty() {
            return Err(Error::Config(format!(
                "{context}.tile_properties declares an empty column name"
            )));
        }
        if !seen.insert(property.as_str()) {
            return Err(Error::Config(format!(
                "{context}.tile_properties declares '{property}' more than once"
            )));
        }
    }
    Ok(())
}

impl StacConf {
    /// Eager per-entry validation for `stac.assets` (`#36` slice 1),
    /// `stac.contacts` (`#187`) and `stac.lineage` (`#50`): an empty
    /// `href`, a `type` that doesn't look like a media type, a contact with
    /// an empty `name`, or a lineage block that asserts nothing (or asserts
    /// a blank) fails the whole config at load time — the same "bad shape
    /// fails boot, not mid-request" rule `TilesConf::validate`/
    /// `SchemaDecl::validate` already enforce for their own blocks. A
    /// duplicate asset id is impossible by construction (`assets` is a map
    /// keyed by id, not a list), so there is nothing to check for that
    /// here. Contacts are a list (order is operator-chosen and meaningful
    /// in the ISO projection), so duplicates are permitted — two people at
    /// the same organization are not a config error.
    fn validate(&self, context: &str) -> Result<()> {
        for (index, contact) in self.contacts.iter().enumerate() {
            if contact.name.trim().is_empty() {
                return Err(Error::Config(format!(
                    "{context}.stac.contacts[{index}].name must not be empty"
                )));
            }
        }
        // `#50` lineage: a declared block must assert something, and
        // nothing it asserts may be blank — refused by name here rather
        // than silently dropped (or worse, emitted as an empty
        // `gmd:LI_Lineage`) at projection time. See `LineageDecl`'s doc.
        if let Some(lineage) = &self.lineage {
            if lineage.is_empty() {
                return Err(Error::Config(format!(
                    "{context}.stac.lineage declares no statement, sources, or process_steps: \
                     declare at least one fact or remove the block entirely"
                )));
            }
            if lineage
                .statement
                .as_deref()
                .is_some_and(|statement| statement.trim().is_empty())
            {
                return Err(Error::Config(format!(
                    "{context}.stac.lineage.statement must not be blank"
                )));
            }
            for (index, source) in lineage.sources.iter().enumerate() {
                if source.description.trim().is_empty() {
                    return Err(Error::Config(format!(
                        "{context}.stac.lineage.sources[{index}].description must not be blank"
                    )));
                }
            }
            for (index, step) in lineage.process_steps.iter().enumerate() {
                if step.description.trim().is_empty() {
                    return Err(Error::Config(format!(
                        "{context}.stac.lineage.process_steps[{index}].description must not be blank"
                    )));
                }
            }
        }
        for (asset_id, asset) in &self.assets {
            if asset.href.is_empty() {
                return Err(Error::Config(format!(
                    "{context}.stac.assets['{asset_id}'].href must not be empty"
                )));
            }
            if let Some(media_type) = &asset.media_type {
                if !is_plausible_media_type(media_type) {
                    return Err(Error::Config(format!(
                        "{context}.stac.assets['{asset_id}'].type '{media_type}' does not look like a media type (expected 'type/subtype')"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// A minimal `type/subtype` shape check (RFC 6838's own top-level grammar) —
/// not a full media-type parser (this workspace vendors none), just enough
/// to catch a config typo (`"png"` instead of `"image/png"`, a stray empty
/// side) at boot instead of it silently reaching a client verbatim in a
/// STAC asset's `type` field.
fn is_plausible_media_type(value: &str) -> bool {
    match value.split_once('/') {
        Some((type_, subtype)) => {
            !type_.is_empty() && !subtype.is_empty() && !subtype.contains('/')
        }
        None => false,
    }
}

/// Normalized routing input `Router::build_from_snapshot` indexes — every
/// catalog and collection declaration, regardless of which source produced
/// them (`#42`, third slice). The default, file-backed path never actually
/// constructs one of these: `Router::build` reads `AppConfig.catalogs`/
/// `.collections` directly, borrowed rather than cloned. This type exists
/// for the one source with no `AppConfig` field to borrow from instead — a
/// relational registry walk (`registry::snapshot_from_registry`), which owns
/// what it collects because nothing else in the process does.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoutingSnapshot {
    pub catalogs: Vec<CatalogDecl>,
    pub collections: Vec<CollectionDecl>,
}

/// Cross-checks every policy grant's `scope.catalogs`/`scope.collections`
/// against a resolved `(catalog_by_id, collection_by_id)` pair (`#34`) —
/// factored out of [`AppConfig::validate_policy`] so
/// [`validate_registry_snapshot`] can run the identical check against the
/// relational backend's own resolved snapshot, symmetric with the YAML
/// path. Always runs with referential checks on (`validate_grant`'s
/// `check_refs: true`): by the time either caller has a `catalog_by_id`/
/// `collection_by_id` to pass in at all, every id those maps could resolve
/// is already known — there is nothing left to defer.
fn validate_policy_grant_refs(
    policy: &PolicyConfig,
    catalog_by_id: &HashMap<&str, &CatalogDecl>,
    collection_by_id: &HashMap<&str, &CollectionDecl>,
) -> Result<()> {
    for role in &policy.roles {
        for grant in &role.grants {
            validate_grant(
                &format!("policy.roles['{}']", role.name),
                grant,
                None,
                catalog_by_id,
                collection_by_id,
                true,
            )?;
        }
    }
    for tenant_policy in &policy.tenant_policies {
        for role in &tenant_policy.roles {
            for grant in &role.grants {
                validate_grant(
                    &format!(
                        "policy.tenant_policies['{}'].roles['{}']",
                        tenant_policy.tenant, role.name
                    ),
                    grant,
                    Some(tenant_policy.tenant.as_str()),
                    catalog_by_id,
                    collection_by_id,
                    true,
                )?;
            }
        }
    }
    Ok(())
}

/// Cross-checks a relational registry walk's result (`#42`, third slice —
/// `registry::snapshot_from_registry`) against `config`/`tenants` with
/// exactly the same referential-integrity rules [`AppConfig::validate`]
/// already applies to a YAML-declared `catalogs`/`collections` (both call
/// [`validate_catalogs_and_collections`]) — a catalog or collection the
/// database returned is held to the same bar a hand-written config document
/// is, before it ever reaches `Router::build_from_snapshot`. `config` must
/// already have passed its own `validate()` (`ConfigStore::load`'s
/// guarantee, and `context::build_router_and_resolver`'s own precondition) —
/// its `storages` are reused here as already known-unique, never re-checked.
/// `tenants` is the caller's own walked tenant snapshot (`#143`,
/// `tenant::snapshot_tenants`, already passed through
/// [`validate_tenant_snapshot`](crate::tenant::validate_tenant_snapshot) by
/// that caller) rather than `config.tenants` directly — for the file-backed
/// default that snapshot is exactly `config.tenants`, so this is a
/// normalization, not a behavior change.
///
/// `#34`: also runs [`validate_policy_grant_refs`] against this snapshot's
/// own `catalogs`/`collections` — the relational backend's eager
/// grant-scope referential check, deferred here (rather than at
/// `AppConfig::validate` time, alongside the YAML path's own check in
/// `AppConfig::validate_policy`) because a relational registry's
/// `catalogs`/`collections` are always empty by construction until a
/// snapshot is actually walked (see `AppConfig::validate`'s double-source
/// check) — there is nothing to check a grant's referenced id against any
/// earlier than this. A `policy.roles`/`policy.tenant_policies` grant
/// referencing an id this snapshot never published is a named boot error
/// here, not a silent "this grant never matches" left to discover at
/// request time.
pub fn validate_registry_snapshot(
    config: &AppConfig,
    tenants: &[TenantDecl],
    snapshot: &RoutingSnapshot,
) -> Result<()> {
    let storage_ids: HashSet<&str> = config.storages.iter().map(|s| s.id.as_str()).collect();
    let object_store_ids: HashSet<&str> =
        config.object_stores.iter().map(|s| s.id.as_str()).collect();
    let tenant_ids: HashSet<&str> = tenants.iter().map(|t| t.id.as_str()).collect();
    config.validate_tenant_references(&tenant_ids)?;
    let mut internal_ids: HashSet<&str> = tenant_ids.iter().copied().collect();
    // Profiles always come from the file document regardless of
    // `registry.backend` — so `config.profiles` is the right source even
    // when `tenants`/`catalogs`/`collections` are walked from a relational
    // snapshot.
    let known_profile_ids = collect_profile_ids(&config.profiles)?;
    validate_catalogs_and_collections(
        &tenant_ids,
        &storage_ids,
        &object_store_ids,
        &snapshot.catalogs,
        &snapshot.collections,
        &mut internal_ids,
        &known_profile_ids,
    )?;

    // `#110`: the relational backend's own catalogs/collections live in
    // `snapshot`, never in `config.catalogs`/`.collections` (always empty
    // there by the double-source rule) — so the finality walk must run
    // against the snapshot here, not rely on `AppConfig::validate`'s own
    // call, which only ever sees an empty list under this backend.
    let profiles_by_id: HashMap<&str, &SettingsDecl> = config
        .profiles
        .iter()
        .map(|profile| (profile.id.as_str(), &profile.settings))
        .collect();
    validate_settings_finality(
        &config.settings,
        tenants,
        &snapshot.catalogs,
        &snapshot.collections,
        &profiles_by_id,
    )?;

    let catalog_by_id: HashMap<&str, &CatalogDecl> = snapshot
        .catalogs
        .iter()
        .map(|c| (c.id.as_str(), c))
        .collect();
    let collection_by_id: HashMap<&str, &CollectionDecl> = snapshot
        .collections
        .iter()
        .map(|c| (c.id.as_str(), c))
        .collect();
    validate_policy_grant_refs(&config.policy, &catalog_by_id, &collection_by_id)
}

/// Walks the platform -> tenant -> catalog -> collection chain, accumulating
/// which settings keys are `final` (`#110`, `SettingsDecl::final_keys`) at
/// each level and refusing, by name, any level that declares (directly, or
/// through its own `profile:` reference) a key an ancestor already
/// finalized. Shared by [`AppConfig::validate`] (the YAML path) and
/// [`validate_registry_snapshot`] (the relational registry path) — exactly
/// the same "one implementation of the bar, two sources held to it"
/// treatment [`validate_catalogs_and_collections`] already gets, and run
/// only after that function has already confirmed every `catalog.tenant`/
/// `collection.catalog` reference resolves, so this walk never needs to
/// handle a dangling reference itself.
fn validate_settings_finality(
    platform: &SettingsDecl,
    tenants: &[TenantDecl],
    catalogs: &[CatalogDecl],
    collections: &[CollectionDecl],
    profiles_by_id: &HashMap<&str, &SettingsDecl>,
) -> Result<()> {
    let platform_final = finalized_here(&BTreeMap::new(), &platform.final_keys, "the platform");

    let mut tenant_final: HashMap<&str, FinalizedBy<'_>> = HashMap::new();
    for tenant in tenants {
        check_no_final_override(
            "tenant",
            &tenant.id,
            &tenant.settings,
            &platform_final,
            profiles_by_id,
        )?;
        // `#156`: a tenant may re-declare a key final (including
        // `admission`) — accepted, and inert for `admission` specifically,
        // since the only levels below a tenant are the catalog and
        // collection levels, where an `admission` declaration is already
        // refused outright by `validate_settings`. That is the same
        // accepted-but-inert treatment a collection-level `final:` (the
        // bottom of the four-level chain) already gets; refusing it instead
        // would make `admission` the one key whose `final:` shape rules
        // differ from every other key's, for no protective gain.
        tenant_final.insert(
            tenant.id.as_str(),
            finalized_here(
                &platform_final,
                &tenant.settings.final_keys,
                &format!("tenant '{}'", tenant.id),
            ),
        );
    }

    let mut catalog_final: HashMap<&str, FinalizedBy<'_>> = HashMap::new();
    for catalog in catalogs {
        let inherited = tenant_final
            .get(catalog.tenant.as_str())
            .cloned()
            .unwrap_or_else(|| platform_final.clone());
        check_no_final_override(
            "catalog",
            &catalog.id,
            &catalog.settings,
            &inherited,
            profiles_by_id,
        )?;
        catalog_final.insert(
            catalog.id.as_str(),
            finalized_here(
                &inherited,
                &catalog.settings.final_keys,
                &format!("catalog '{}'", catalog.id),
            ),
        );
    }

    for collection in collections {
        let inherited = catalog_final
            .get(collection.catalog.as_str())
            .cloned()
            .unwrap_or_else(|| platform_final.clone());
        check_no_final_override(
            "collection",
            &collection.id,
            &collection.settings,
            &inherited,
            profiles_by_id,
        )?;
        // A collection's own `final_keys` are validated for shape
        // (`validate_settings`) but never accumulated further — nothing
        // sits below a collection in the chain to enforce them against.
    }

    Ok(())
}

/// Which settings keys are final at some level, and — for the refusal
/// message — the name of the level that actually declared each one. A
/// `BTreeMap` rather than a `HashMap` so that a document violating more
/// than one final key always reports the same key first, instead of
/// whichever one hash iteration order happened to surface.
type FinalizedBy<'a> = BTreeMap<&'a str, String>;

/// Everything `inherited` already finalized, plus whatever `declared_here`
/// names, attributed to `level`. An ancestor's attribution wins on a key a
/// lower level re-declares final: the outermost level that pinned a key is
/// the one whose declaration a would-be overrider has to take up with, and
/// the nearer re-declaration changes nothing about what is forbidden.
fn finalized_here<'a>(
    inherited: &FinalizedBy<'a>,
    declared_here: &'a [String],
    level: &str,
) -> FinalizedBy<'a> {
    let mut finalized = inherited.clone();
    for key in declared_here {
        finalized
            .entry(key.as_str())
            .or_insert_with(|| level.to_string());
    }
    finalized
}

/// Whether `decl` itself provides a value for `key` — either declared
/// directly, or (when `decl` leaves it unset) through its own named
/// `profile:` reference. Mirrors the two candidates [`settings::
/// resolve_field`](crate::settings) checks at each level of its own walk,
/// so "does this level provide a value here" always agrees with what the
/// resolver would actually observe.
fn settings_declares_key(decl: &SettingsDecl, key: &str) -> bool {
    match key {
        "tile_caps" => decl.tile_caps.is_some(),
        "cache_ttl_s" => decl.cache_ttl_s.is_some(),
        "slow_request_ms" => decl.slow_request_ms.is_some(),
        "stac" => decl.stac.is_some(),
        "tile_properties" => decl.tile_properties.is_some(),
        "colormap" => decl.colormap.is_some(),
        "max_request_body_bytes" => decl.max_request_body_bytes.is_some(),
        "tile_vertex_budget" => decl.tile_vertex_budget.is_some(),
        "items_vertex_budget" => decl.items_vertex_budget.is_some(),
        "page_max_bytes" => decl.page_max_bytes.is_some(),
        "max_asset_bytes" => decl.max_asset_bytes.is_some(),
        "asset_media_types" => decl.asset_media_types.is_some(),
        // `#156`: without this arm, `admission` would be a name the
        // vocabulary accepts and the finality walk then silently never
        // enforces — the `_ => false` fallback below would swallow it. That
        // is precisely the silent-degradation failure the `final` mechanism
        // exists to avoid, so the arm and the `SETTINGS_KEY_NAMES` entry are
        // one change, never one without the other.
        "admission" => decl.admission.is_some(),
        "batch" => decl.batch.is_some(),
        "protocols" => decl.protocols.is_some(),
        // Unreachable once `validate_settings` has already refused an
        // unrecognized name in `final_keys` — `SETTINGS_KEY_NAMES` is the
        // only source `check_no_final_override` ever iterates over.
        _ => false,
    }
}

fn settings_provides_key(
    decl: &SettingsDecl,
    key: &str,
    profiles_by_id: &HashMap<&str, &SettingsDecl>,
) -> bool {
    if settings_declares_key(decl, key) {
        return true;
    }
    match &decl.profile {
        Some(profile_id) => profiles_by_id
            .get(profile_id.as_str())
            .is_some_and(|profile_decl| settings_declares_key(profile_decl, key)),
        None => false,
    }
}

/// Refuses, BY NAME, any key `decl` provides that an ancestor level already
/// declared final. The message names all three things an operator needs to
/// fix it: which key, which level declared it final, and which level tried
/// to override it — never a silent drop of the lower-level value.
fn check_no_final_override(
    level_label: &str,
    id: &str,
    decl: &SettingsDecl,
    finalized_by_ancestor: &FinalizedBy<'_>,
    profiles_by_id: &HashMap<&str, &SettingsDecl>,
) -> Result<()> {
    for (key, declared_by) in finalized_by_ancestor {
        if settings_provides_key(decl, key, profiles_by_id) {
            return Err(Error::Config(format!(
                "{level_label} '{id}': settings key '{key}' is declared final by {declared_by} and cannot be overridden here"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESIGN_DOC_YAML: &str = r##"
storages:
  - id: main
    driver: postgis
    url_env: DATABASE_URL
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    datetime: observed_at
    tiles: { minzoom: 0, maxzoom: 14, caps: { z0: 2000, z10: 20000 } }
    style: { fill: "#3388ff66", stroke: "#3366cc", stroke_width: 1.0 }
"##;

    #[test]
    fn parses_design_doc_example() {
        let config: AppConfig = serde_yaml::from_str(DESIGN_DOC_YAML).unwrap();
        config.validate().unwrap();

        assert_eq!(config.server, ServerConfig::default());
        assert_eq!(config.storages.len(), 1);
        assert_eq!(config.storages[0].driver, "postgis");
        assert_eq!(config.tenants[0].id, "public");

        let demo = &config.collections[0];
        assert_eq!(demo.datetime.as_deref(), Some("observed_at"));
        assert_eq!(demo.tiles.minzoom, 0);
        assert_eq!(demo.tiles.maxzoom, 14);
        assert_eq!(demo.tiles.caps.get(0), Some(2000));
        assert_eq!(demo.tiles.caps.get(10), Some(20000));
        assert_eq!(demo.tiles.caps.get(5), None);
        assert_eq!(demo.style.fill, "#3388ff66");
    }

    #[test]
    fn defaults_apply_when_sections_missing() {
        let config: AppConfig = serde_yaml::from_str("storages: []").unwrap();
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.server.request_timeout_s, 60);
        assert_eq!(config.server.drain_timeout_s, 10);
        assert_eq!(config.server.readiness_probe_interval_s, 5);
        assert_eq!(config.server.readiness_probe_timeout_s, 2);
        assert!(config.server.metrics_tenant_allowlist.is_empty());
        assert!(config.server.metrics_collection_allowlist.is_empty());
        assert_eq!(config.cache.memory_percent, 25.0);
        assert!(config.auth.browser.is_none());
    }

    fn browser_auth_yaml(public_origin: &str) -> String {
        format!(
            r#"
auth:
  trusted_issuers:
    - issuer: https://id.example.com
      audience: tellurion
      claims: {{ tenants: tenants }}
  browser:
    issuer: https://id.example.com
    client_id: control-ui
    public_origin: {public_origin}
"#
        )
    }

    #[test]
    fn browser_auth_defaults_and_https_callback_are_bounded() {
        let config: AppConfig =
            serde_yaml::from_str(&browser_auth_yaml("https://control.example.com")).unwrap();
        config.validate().unwrap();

        let browser = config.auth.browser.as_ref().unwrap();
        assert_eq!(browser.scopes, ["openid", "profile"]);
        assert_eq!(browser.session_ttl_s, 3_600);
        assert_eq!(browser.login_ttl_s, 300);
        assert_eq!(browser.max_sessions, 1_024);
        assert_eq!(
            browser.callback_url(),
            "https://control.example.com/_auth/control/callback"
        );
    }

    #[test]
    fn browser_auth_requires_a_configured_issuer_and_non_empty_client_fields() {
        for (label, yaml) in [
            (
                "unknown issuer",
                browser_auth_yaml("https://control.example.com").replace(
                    "issuer: https://id.example.com\n    client_id",
                    "issuer: https://other.example.com\n    client_id",
                ),
            ),
            (
                "empty client id",
                browser_auth_yaml("https://control.example.com")
                    .replace("client_id: control-ui", "client_id: ''"),
            ),
            (
                "empty scopes",
                format!(
                    "{}    scopes: []\n",
                    browser_auth_yaml("https://control.example.com")
                ),
            ),
        ] {
            let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
            assert!(config.validate().is_err(), "{label} must be rejected");
        }
    }

    #[test]
    fn browser_auth_rejects_unbounded_limits_and_non_origin_urls() {
        for (label, browser_line) in [
            ("zero session ttl", "    session_ttl_s: 0"),
            ("zero login ttl", "    login_ttl_s: 0"),
            ("zero capacity", "    max_sessions: 0"),
            ("path", "    public_origin: https://control.example.com/ui"),
            (
                "query",
                "    public_origin: https://control.example.com?x=1",
            ),
            (
                "fragment",
                "    public_origin: https://control.example.com#fragment",
            ),
            (
                "credentials",
                "    public_origin: https://user@control.example.com",
            ),
            (
                "remote http",
                "    public_origin: http://control.example.com",
            ),
        ] {
            let mut yaml = browser_auth_yaml("https://control.example.com");
            if browser_line.contains("public_origin") {
                yaml = yaml.replace(
                    "    public_origin: https://control.example.com",
                    browser_line,
                );
            } else {
                yaml.push_str(browser_line);
                yaml.push('\n');
            }
            let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
            assert!(config.validate().is_err(), "{label} must be rejected");
        }
    }

    #[test]
    fn browser_auth_enforces_upper_bounds_and_allows_loopback_http() {
        let local: AppConfig =
            serde_yaml::from_str(&browser_auth_yaml("http://localhost:8080")).unwrap();
        local.validate().unwrap();

        for (label, setting) in [
            (
                "session ttl",
                format!(
                    "    session_ttl_s: {}",
                    MAX_CONTROL_BROWSER_SESSION_TTL_S + 1
                ),
            ),
            (
                "login ttl",
                format!("    login_ttl_s: {}", MAX_CONTROL_BROWSER_LOGIN_TTL_S + 1),
            ),
            (
                "session capacity",
                format!("    max_sessions: {}", MAX_CONTROL_BROWSER_SESSIONS + 1),
            ),
        ] {
            let mut yaml = browser_auth_yaml("https://control.example.com");
            yaml.push_str(&setting);
            yaml.push('\n');
            let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
            assert!(config.validate().is_err(), "{label} must be bounded");
        }
    }

    #[test]
    fn browser_auth_origin_error_names_the_loopback_http_exception() {
        let config: AppConfig =
            serde_yaml::from_str(&browser_auth_yaml("http://control.example.com")).unwrap();
        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("loopback HTTP"), "{error}");
    }

    #[test]
    fn parses_observability_server_settings() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
server:
  drain_timeout_s: 20
  readiness_probe_interval_s: 7
  readiness_probe_timeout_s: 3
  metrics_tenant_allowlist: [public, partner]
  metrics_collection_allowlist:
    - tenant: public
      catalog: default
      collection: demo
"#,
        )
        .unwrap();

        assert_eq!(config.server.drain_timeout_s, 20);
        assert_eq!(config.server.readiness_probe_interval_s, 7);
        assert_eq!(config.server.readiness_probe_timeout_s, 3);
        assert_eq!(
            config.server.metrics_tenant_allowlist,
            vec!["public".to_string(), "partner".to_string()]
        );
        assert_eq!(
            config.server.metrics_collection_allowlist,
            vec![MetricCollectionRef {
                tenant: "public".to_string(),
                catalog: "default".to_string(),
                collection: "demo".to_string(),
            }]
        );
    }

    #[test]
    fn legacy_partial_server_config_uses_observability_defaults() {
        let config: AppConfig = serde_yaml::from_str("server: { port: 9000 }").unwrap();

        assert_eq!(config.server.port, 9000);
        assert_eq!(config.server.drain_timeout_s, 10);
        assert_eq!(config.server.readiness_probe_interval_s, 5);
        assert_eq!(config.server.readiness_probe_timeout_s, 2);
        assert!(config.server.metrics_tenant_allowlist.is_empty());
        assert!(config.server.metrics_collection_allowlist.is_empty());
    }

    #[test]
    fn rejects_zero_observability_durations() {
        for setting in [
            "drain_timeout_s",
            "readiness_probe_interval_s",
            "readiness_probe_timeout_s",
        ] {
            let config: AppConfig =
                serde_yaml::from_str(&format!("server: {{ {setting}: 0 }}")).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{setting} must be greater than zero"
            );
        }
    }

    #[test]
    fn rejects_zero_slow_request_ms_at_every_settings_level() {
        for settings in [
            "settings: { slow_request_ms: 0 }",
            "tenants: [ { id: public, settings: { slow_request_ms: 0 } } ]",
            "\
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public, settings: { slow_request_ms: 0 } } ]",
            "\
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections: [ { id: demo, catalog: default, storage: main, settings: { slow_request_ms: 0 } } ]",
        ] {
            let config: AppConfig = serde_yaml::from_str(settings).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{settings} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_zero_max_request_body_bytes_at_every_settings_level() {
        for settings in [
            "settings: { max_request_body_bytes: 0 }",
            "tenants: [ { id: public, settings: { max_request_body_bytes: 0 } } ]",
            "\
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public, settings: { max_request_body_bytes: 0 } } ]",
            "\
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections: [ { id: demo, catalog: default, storage: main, settings: { max_request_body_bytes: 0 } } ]",
        ] {
            let config: AppConfig = serde_yaml::from_str(settings).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{settings} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_a_zero_batch_field_at_every_settings_level() {
        for field in ["max_bytes", "max_items", "chunk_items"] {
            for settings in [
                format!("settings: {{ batch: {{ {field}: 0 }} }}"),
                format!("tenants: [ {{ id: public, settings: {{ batch: {{ {field}: 0 }} }} }} ]"),
                format!(
                    "\
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public, settings: {{ batch: {{ {field}: 0 }} }} }} ]"
                ),
                format!(
                    "\
storages: [ {{ id: main, driver: postgis, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections: [ {{ id: demo, catalog: default, storage: main, settings: {{ batch: {{ {field}: 0 }} }} }} ]"
                ),
            ] {
                let config: AppConfig = serde_yaml::from_str(&settings).unwrap();
                assert!(
                    matches!(config.validate(), Err(Error::Config(_))),
                    "{settings} must be rejected"
                );
            }
        }
    }

    #[test]
    fn rejects_zero_tile_vertex_budget_at_every_settings_level() {
        for settings in [
            "settings: { tile_vertex_budget: 0 }",
            "tenants: [ { id: public, settings: { tile_vertex_budget: 0 } } ]",
            "\
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public, settings: { tile_vertex_budget: 0 } } ]",
            "\
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections: [ { id: demo, catalog: default, storage: main, settings: { tile_vertex_budget: 0 } } ]",
        ] {
            let config: AppConfig = serde_yaml::from_str(settings).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{settings} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_zero_items_vertex_budget_at_every_settings_level() {
        for settings in [
            "settings: { items_vertex_budget: 0 }",
            "tenants: [ { id: public, settings: { items_vertex_budget: 0 } } ]",
            "\
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public, settings: { items_vertex_budget: 0 } } ]",
            "\
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections: [ { id: demo, catalog: default, storage: main, settings: { items_vertex_budget: 0 } } ]",
        ] {
            let config: AppConfig = serde_yaml::from_str(settings).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{settings} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_zero_page_max_bytes_at_every_settings_level() {
        for settings in [
            "settings: { page_max_bytes: 0 }",
            "tenants: [ { id: public, settings: { page_max_bytes: 0 } } ]",
            "\
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public, settings: { page_max_bytes: 0 } } ]",
            "\
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections: [ { id: demo, catalog: default, storage: main, settings: { page_max_bytes: 0 } } ]",
        ] {
            let config: AppConfig = serde_yaml::from_str(settings).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{settings} must be rejected"
            );
        }
    }

    // -- `colormap` (`#92`) ---------------------------------------------------

    #[test]
    fn rejects_a_ramp_colormap_whose_min_is_not_less_than_max() {
        let config: AppConfig = serde_yaml::from_str(
            "settings: { colormap: { kind: ramp, ramp: grayscale, min: 10.0, max: 10.0 } }",
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_a_stops_colormap_with_no_stops() {
        let config: AppConfig =
            serde_yaml::from_str("settings: { colormap: { kind: stops, stops: [] } }").unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_a_stops_colormap_with_out_of_order_values() {
        let config: AppConfig = serde_yaml::from_str(
            "settings: { colormap: { kind: stops, stops: [\
             { value: 10.0, rgba: [0, 0, 0, 255] }, \
             { value: 5.0, rgba: [255, 255, 255, 255] }\
             ] } }",
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_a_stops_colormap_with_a_duplicate_value() {
        let config: AppConfig = serde_yaml::from_str(
            "settings: { colormap: { kind: stops, stops: [\
             { value: 5.0, rgba: [0, 0, 0, 255] }, \
             { value: 5.0, rgba: [255, 255, 255, 255] }\
             ] } }",
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn accepts_a_well_formed_colormap_of_either_shape() {
        for settings in [
            "settings: { colormap: { kind: ramp, ramp: viridis, min: 0.0, max: 255.0 } }",
            "settings: { colormap: { kind: stops, stops: [\
             { value: 0.0, rgba: [0, 0, 0, 255] }, \
             { value: 255.0, rgba: [255, 255, 255, 255] }\
             ] } }",
        ] {
            let config: AppConfig = serde_yaml::from_str(settings).unwrap();
            assert!(config.validate().is_ok(), "{settings} should be accepted");
        }
    }

    // -- `stac.assets` (`#36` slice 1, "a real, driver-neutral assets
    // model") ---------------------------------------------------------------

    #[test]
    fn stac_assets_parse_with_the_full_asset_object_shape() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings:
  stac:
    assets:
      thumbnail:
        href: https://example.com/thumb.png
        type: image/png
        title: Thumbnail
        roles: [thumbnail]
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let asset = &config.settings.stac.as_ref().unwrap().assets["thumbnail"];
        assert_eq!(asset.href, "https://example.com/thumb.png");
        assert_eq!(asset.media_type.as_deref(), Some("image/png"));
        assert_eq!(asset.title.as_deref(), Some("Thumbnail"));
        assert_eq!(asset.roles, vec!["thumbnail".to_string()]);
    }

    /// `type`/`title`/`roles` are all genuinely optional per the STAC Asset
    /// Object spec — a declaration naming only `href` parses cleanly with
    /// the rest left `None`/empty, not defaulted to some fabricated value.
    #[test]
    fn stac_asset_optional_fields_default_to_absent() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings:
  stac:
    assets:
      doc:
        href: https://example.com/doc.pdf
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let asset = &config.settings.stac.as_ref().unwrap().assets["doc"];
        assert!(asset.media_type.is_none());
        assert!(asset.title.is_none());
        assert!(asset.roles.is_empty());
    }

    #[test]
    fn stac_assets_default_to_empty_when_the_stac_block_is_absent() {
        let config: AppConfig = serde_yaml::from_str("storages: []").unwrap();
        assert!(config.settings.stac.is_none());
    }

    #[test]
    fn rejects_a_stac_asset_with_an_empty_href() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings:
  stac:
    assets:
      thumbnail: { href: "" }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_a_stac_asset_with_a_malformed_media_type() {
        for media_type in ["png", "/png", "image/", "image//png"] {
            let config: AppConfig = serde_yaml::from_str(&format!(
                r#"
settings:
  stac:
    assets:
      thumbnail: {{ href: "https://example.com/thumb.png", type: "{media_type}" }}
"#
            ))
            .unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "type '{media_type}' must be rejected"
            );
        }
    }

    /// Same "runs at every level of the settings chain" guard
    /// `rejects_zero_slow_request_ms_at_every_settings_level` already
    /// proves for `slow_request_ms` — `stac.assets` validation is not
    /// wired up only for the platform-level `settings:` block.
    #[test]
    fn rejects_an_empty_stac_asset_href_at_every_settings_level() {
        const BAD_ASSET: &str = "stac: { assets: { thumbnail: { href: '' } } }";
        for settings in [
            format!("settings: {{ {BAD_ASSET} }}"),
            format!("tenants: [ {{ id: public, settings: {{ {BAD_ASSET} }} }} ]"),
            format!(
                "\
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public, settings: {{ {BAD_ASSET} }} }} ]"
            ),
            format!(
                "\
storages: [ {{ id: main, driver: postgis, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections: [ {{ id: demo, catalog: default, storage: main, settings: {{ {BAD_ASSET} }} }} ]"
            ),
        ] {
            let config: AppConfig = serde_yaml::from_str(&settings).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{settings} must be rejected"
            );
        }
    }

    // -- `stac.contacts` (`#187`, descriptor contact metadata) --------------

    #[test]
    fn stac_contacts_parse_with_the_full_contact_shape() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings:
  stac:
    contacts:
      - name: Ada Lovelace
        organization: Example Org
        email: ada@example.com
        role: pointOfContact
        url: https://example.com/ada
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let contacts = &config.settings.stac.as_ref().unwrap().contacts;
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].name, "Ada Lovelace");
        assert_eq!(contacts[0].organization.as_deref(), Some("Example Org"));
        assert_eq!(contacts[0].email.as_deref(), Some("ada@example.com"));
        assert_eq!(contacts[0].role.as_deref(), Some("pointOfContact"));
        assert_eq!(contacts[0].url.as_deref(), Some("https://example.com/ada"));
    }

    /// Only `name` is required — everything else parses as absent rather
    /// than being defaulted to a fabricated value, the same rule
    /// `stac_asset_optional_fields_default_to_absent` pins for assets.
    #[test]
    fn stac_contact_optional_fields_default_to_absent() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings:
  stac:
    contacts:
      - name: Grace Hopper
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let contact = &config.settings.stac.as_ref().unwrap().contacts[0];
        assert!(contact.organization.is_none());
        assert!(contact.email.is_none());
        assert!(contact.role.is_none());
        assert!(contact.url.is_none());
    }

    /// Adding `contacts` must not disturb any existing `stac:` block: one
    /// that declares only license/keywords still parses, with an empty
    /// contacts list rather than a parse error.
    #[test]
    fn stac_contacts_default_to_empty_for_a_block_that_never_mentions_them() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings:
  stac:
    license: CC-BY-4.0
    keywords: [imagery]
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert!(config.settings.stac.as_ref().unwrap().contacts.is_empty());
    }

    #[test]
    fn rejects_a_stac_contact_with_an_empty_name() {
        for name in ["\"\"", "\"   \""] {
            let config: AppConfig = serde_yaml::from_str(&format!(
                "settings: {{ stac: {{ contacts: [ {{ name: {name} }} ] }} }}"
            ))
            .unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "name {name} must be rejected"
            );
        }
    }

    /// Same "runs at every level of the settings chain" guard
    /// `rejects_an_empty_stac_asset_href_at_every_settings_level` already
    /// proves for assets.
    #[test]
    fn rejects_an_empty_stac_contact_name_at_every_settings_level() {
        const BAD_CONTACT: &str = "stac: { contacts: [ { name: '' } ] }";
        for settings in [
            format!("settings: {{ {BAD_CONTACT} }}"),
            format!("tenants: [ {{ id: public, settings: {{ {BAD_CONTACT} }} }} ]"),
            format!(
                "\
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public, settings: {{ {BAD_CONTACT} }} }} ]"
            ),
            format!(
                "\
storages: [ {{ id: main, driver: postgis, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections: [ {{ id: demo, catalog: default, storage: main, settings: {{ {BAD_CONTACT} }} }} ]"
            ),
        ] {
            let config: AppConfig = serde_yaml::from_str(&settings).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{settings} must be rejected"
            );
        }
    }

    // -- `stac.lineage` (`#50`, declared lineage/provenance) ----------------

    #[test]
    fn stac_lineage_parses_with_the_full_shape() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings:
  stac:
    lineage:
      statement: Digitised from the 1:25000 IGM series.
      sources:
        - description: IGM 1:25000 sheet 45
      process_steps:
        - description: Reprojected to EPSG:4326 with ogr2ogr
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let lineage = config
            .settings
            .stac
            .as_ref()
            .unwrap()
            .lineage
            .as_ref()
            .unwrap();
        assert_eq!(
            lineage.statement.as_deref(),
            Some("Digitised from the 1:25000 IGM series.")
        );
        assert_eq!(lineage.sources.len(), 1);
        assert_eq!(lineage.sources[0].description, "IGM 1:25000 sheet 45");
        assert_eq!(lineage.process_steps.len(), 1);
        assert_eq!(
            lineage.process_steps[0].description,
            "Reprojected to EPSG:4326 with ogr2ogr"
        );
    }

    /// A `stac:` block that never mentions `lineage` parses to `None` —
    /// absent, never an empty placeholder block — so every existing config
    /// keeps deserializing unchanged, the same compatibility bar
    /// `stac_contacts_default_to_empty_for_a_block_that_never_mentions_them`
    /// pins for contacts.
    #[test]
    fn stac_lineage_defaults_to_absent_for_a_block_that_never_mentions_it() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings:
  stac:
    license: CC-BY-4.0
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert!(config.settings.stac.as_ref().unwrap().lineage.is_none());
    }

    /// An absent `lineage` must not perturb the serialized `StacConf` at
    /// all (`skip_serializing_if`): a published registry `decl` written
    /// before this field existed keeps its exact bytes.
    #[test]
    fn an_absent_stac_lineage_is_skipped_on_serialize() {
        let conf = StacConf {
            license: Some("CC-BY-4.0".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&conf).unwrap();
        assert!(
            !json.contains("lineage"),
            "an undeclared lineage must not appear in serialized StacConf: {json}"
        );
    }

    /// `lineage: {}` asserts nothing; the projection could only fabricate
    /// an empty `gmd:LI_Lineage` or silently drop it, so the shape is
    /// refused by name at load.
    #[test]
    fn rejects_a_stac_lineage_that_declares_no_fact_at_all() {
        let config: AppConfig =
            serde_yaml::from_str("settings: { stac: { lineage: {} } }").unwrap();
        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.contains("stac.lineage declares no statement, sources, or process_steps"),
            "{error}"
        );
    }

    #[test]
    fn rejects_blank_stac_lineage_members() {
        for (lineage, expected) in [
            (
                "{ statement: '   ' }",
                "stac.lineage.statement must not be blank",
            ),
            (
                "{ sources: [ { description: '' } ] }",
                "stac.lineage.sources[0].description must not be blank",
            ),
            (
                "{ process_steps: [ { description: '  ' } ] }",
                "stac.lineage.process_steps[0].description must not be blank",
            ),
        ] {
            let config: AppConfig =
                serde_yaml::from_str(&format!("settings: {{ stac: {{ lineage: {lineage} }} }}"))
                    .unwrap();
            let error = config.validate().unwrap_err().to_string();
            assert!(error.contains(expected), "{lineage}: {error}");
        }
    }

    /// Same "runs at every level of the settings chain" guard the contacts
    /// and assets validations already prove for themselves.
    #[test]
    fn rejects_an_empty_stac_lineage_at_every_settings_level() {
        const BAD_LINEAGE: &str = "stac: { lineage: {} }";
        for settings in [
            format!("settings: {{ {BAD_LINEAGE} }}"),
            format!("tenants: [ {{ id: public, settings: {{ {BAD_LINEAGE} }} }} ]"),
            format!(
                "\
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public, settings: {{ {BAD_LINEAGE} }} }} ]"
            ),
            format!(
                "\
storages: [ {{ id: main, driver: postgis, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections: [ {{ id: demo, catalog: default, storage: main, settings: {{ {BAD_LINEAGE} }} }} ]"
            ),
        ] {
            let config: AppConfig = serde_yaml::from_str(&settings).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{settings} must be rejected"
            );
        }
    }

    // -- `settings.tile_properties` (`#85`, vector-tile property allowlist)
    // ------------------------------------------------------------------------

    #[test]
    fn tile_properties_parses_and_defaults_to_absent() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings:
  tile_properties: [name, pop, class]
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.settings.tile_properties,
            Some(vec![
                "name".to_string(),
                "pop".to_string(),
                "class".to_string()
            ])
        );

        let absent: AppConfig = serde_yaml::from_str("storages: []").unwrap();
        assert!(absent.settings.tile_properties.is_none());
    }

    #[test]
    fn rejects_an_empty_tile_properties_column_name() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings:
  tile_properties: [""]
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_a_repeated_tile_properties_column_name() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings:
  tile_properties: [name, name]
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    /// Same "runs at every level of the settings chain" guard
    /// `rejects_zero_slow_request_ms_at_every_settings_level` already proves
    /// for `slow_request_ms` — `tile_properties` validation is not wired up
    /// only for the platform-level `settings:` block.
    #[test]
    fn rejects_a_repeated_tile_properties_column_name_at_every_settings_level() {
        const BAD_LIST: &str = "tile_properties: [name, name]";
        for settings in [
            format!("settings: {{ {BAD_LIST} }}"),
            format!("tenants: [ {{ id: public, settings: {{ {BAD_LIST} }} }} ]"),
            format!(
                "\
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public, settings: {{ {BAD_LIST} }} }} ]"
            ),
            format!(
                "\
storages: [ {{ id: main, driver: postgis, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections: [ {{ id: demo, catalog: default, storage: main, settings: {{ {BAD_LIST} }} }} ]"
            ),
        ] {
            let config: AppConfig = serde_yaml::from_str(&settings).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{settings} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_empty_metric_collection_allowlist_segments() {
        for entry in [
            "{ tenant: '', catalog: default, collection: demo }",
            "{ tenant: public, catalog: '', collection: demo }",
            "{ tenant: public, catalog: default, collection: '' }",
        ] {
            let config: AppConfig = serde_yaml::from_str(&format!(
                "server: {{ metrics_collection_allowlist: [ {entry} ] }}"
            ))
            .unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{entry} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_empty_or_duplicate_metric_tenant_allowlist_entries() {
        for tenants in ["['']", "[public, public]"] {
            let config: AppConfig = serde_yaml::from_str(&format!(
                "server: {{ metrics_tenant_allowlist: {tenants} }}"
            ))
            .unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{tenants} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_duplicate_metric_collection_allowlist_triples() {
        let config: AppConfig = serde_yaml::from_str(
            "\
server:
  metrics_collection_allowlist:
    - { tenant: public, catalog: default, collection: demo }
    - { tenant: public, catalog: default, collection: demo }",
        )
        .unwrap();

        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_duplicate_storage_ids() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: main, driver: postgis, url_env: DATABASE_URL }
  - { id: main, driver: postgis, url_env: DATABASE_URL2 }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    /// `#39` acceptance test 6: a tenant `external_id` of `metrics` (or any
    /// other reserved top-level segment) fails boot with a named error —
    /// "named" in this codebase's established `Error::Config(String)`
    /// convention (see every other `validate` failure above): a message
    /// that names exactly what collided and why, not a generic refusal.
    #[test]
    fn rejects_a_tenant_external_id_that_collides_with_a_reserved_top_level_segment() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: internal-tenant-id, external_id: metrics } ]
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("metrics"), "message was: {message}");
                assert!(
                    message.contains("reserved"),
                    "message should name the reservation, was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn every_reserved_segment_is_refused_as_a_tenant_external_id() {
        for reserved in RESERVED_TENANT_SEGMENTS {
            let config: AppConfig = serde_yaml::from_str(&format!(
                "tenants: [ {{ id: t, external_id: {reserved} }} ]"
            ))
            .unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "'{reserved}' should be refused as a tenant external_id"
            );
        }
    }

    #[test]
    fn both_probe_routes_are_reserved_tenant_segments() {
        assert!(RESERVED_TENANT_SEGMENTS.contains(&"healthz"));
        assert!(RESERVED_TENANT_SEGMENTS.contains(&"readyz"));
    }

    /// A tenant whose external_id happens to equal its own internal id (the
    /// default, unconfigured case) is still checked against the reserved
    /// list — the rule applies regardless of whether `external_id` was
    /// explicit.
    #[test]
    fn a_tenant_internal_id_that_defaults_into_a_reserved_external_id_is_also_refused() {
        let config: AppConfig = serde_yaml::from_str("tenants: [ { id: ui } ]").unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn a_non_reserved_tenant_external_id_boots_cleanly() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: t, external_id: acme } ]
"#,
        )
        .unwrap();
        assert!(config.validate().is_ok());
    }

    /// Catalog external ids are unique per tenant, not globally — two
    /// tenants may both declare a `default` catalog without colliding.
    #[test]
    fn catalog_external_ids_may_repeat_across_different_tenants() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants:
  - { id: tenant-a, external_id: acme }
  - { id: tenant-b, external_id: globex }
catalogs:
  - { id: catalog-a, external_id: default, tenant: tenant-a }
  - { id: catalog-b, external_id: default, tenant: tenant-b }
"#,
        )
        .unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_duplicate_catalog_external_id_within_the_same_tenant() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: tenant-a, external_id: acme } ]
catalogs:
  - { id: catalog-a, external_id: default, tenant: tenant-a }
  - { id: catalog-b, external_id: default, tenant: tenant-a }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    /// Collection external ids are unique per catalog, not globally — two
    /// catalogs (even under the same tenant) may both declare a `demo`
    /// collection without colliding.
    #[test]
    fn collection_external_ids_may_repeat_across_different_catalogs() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: t, external_id: acme } ]
catalogs:
  - { id: catalog-a, external_id: a, tenant: t }
  - { id: catalog-b, external_id: b, tenant: t }
collections:
  - id: collection-a
    external_id: demo
    catalog: catalog-a
    storage: main
  - id: collection-b
    external_id: demo
    catalog: catalog-b
    storage: main
"#,
        )
        .unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_duplicate_collection_external_id_within_the_same_catalog() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: t, external_id: acme } ]
catalogs: [ { id: catalog-a, external_id: a, tenant: t } ]
collections:
  - id: collection-a
    external_id: demo
    catalog: catalog-a
    storage: main
  - id: collection-b
    external_id: demo
    catalog: catalog-a
    storage: main
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_unresolved_catalog_tenant_ref() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
catalogs: [ { id: catalog-a, tenant: missing } ]
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn internal_ids_must_be_unique_across_every_declaration_kind() {
        // A tenant and a catalog sharing the internal id "dup" — even though
        // each list is internally unique, the id is reused across kinds,
        // which cache-key/resolver code assumes never happens.
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: dup } ]
catalogs: [ { id: dup, tenant: dup } ]
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn external_id_defaults_to_the_internal_id_when_omitted() {
        let tenant = TenantDecl {
            id: "public".to_string(),
            external_id: None,
            settings: SettingsDecl::default(),
        };
        assert_eq!(tenant.external_id(), "public");

        let catalog = CatalogDecl {
            id: "default".to_string(),
            external_id: None,
            tenant: "public".to_string(),
            settings: SettingsDecl::default(),
            visibility: VisibilityDecl::default(),
        };
        assert_eq!(catalog.external_id(), "default");
    }

    #[test]
    fn rejects_unresolved_storage_ref() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: missing
    table: demo
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_unresolved_tenant_ref() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
collections:
  - id: demo
    catalog: missing
    storage: main
    table: demo
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_inverted_zoom_range() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 10, maxzoom: 5, caps: {} }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn effective_cap_is_exact_when_the_zoom_is_configured() {
        let caps = ZoomCaps(BTreeMap::from([(0, 2000), (10, 20000)]));
        assert_eq!(caps.effective(0), 2000);
        assert_eq!(caps.effective(10), 20000);
    }

    #[test]
    fn effective_cap_falls_back_to_the_nearest_lower_zoom() {
        let caps = ZoomCaps(BTreeMap::from([(0, 2000), (10, 20000)]));
        // Between the two configured zooms: inherits the lower one.
        assert_eq!(caps.effective(5), 2000);
        assert_eq!(caps.effective(9), 2000);
        // Above the highest configured zoom: inherits it too.
        assert_eq!(caps.effective(14), 20000);
    }

    #[test]
    fn effective_cap_uses_the_default_below_the_lowest_configured_zoom() {
        let caps = ZoomCaps(BTreeMap::from([(10, 20000)]));
        assert_eq!(caps.effective(0), DEFAULT_TILE_CAP);
        assert_eq!(caps.effective(9), DEFAULT_TILE_CAP);
        assert_eq!(caps.effective(10), 20000);
    }

    #[test]
    fn effective_cap_uses_the_default_for_an_empty_caps_table() {
        let caps = ZoomCaps::default();
        assert_eq!(caps.effective(0), DEFAULT_TILE_CAP);
        assert_eq!(caps.effective(24), DEFAULT_TILE_CAP);
    }

    #[test]
    fn rejects_cap_outside_zoom_range() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: { z10: 100 } }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_out_of_range_cache_memory_percent() {
        let config: AppConfig = serde_yaml::from_str("cache: { memory_percent: 400 }").unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));

        let config: AppConfig = serde_yaml::from_str("cache: { memory_percent: -1 }").unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn accepts_boundary_cache_memory_percent() {
        let config: AppConfig = serde_yaml::from_str("cache: { memory_percent: 0 }").unwrap();
        assert!(config.validate().is_ok());

        let config: AppConfig = serde_yaml::from_str("cache: { memory_percent: 100 }").unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn storage_pool_size_defaults_to_none_and_is_settable() {
        let config: AppConfig = serde_yaml::from_str(
            "storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]",
        )
        .unwrap();
        assert_eq!(config.storages[0].pool_size, None);

        let config: AppConfig = serde_yaml::from_str(
            "storages: [ { id: main, driver: postgis, url_env: DATABASE_URL, pool_size: 12 } ]",
        )
        .unwrap();
        assert_eq!(config.storages[0].pool_size, Some(12));
    }

    #[test]
    fn rejects_a_zero_storage_pool_size() {
        let config: AppConfig = serde_yaml::from_str(
            "storages: [ { id: main, driver: postgis, url_env: DATABASE_URL, pool_size: 0 } ]",
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn cache_l2_defaults_to_none() {
        let config: AppConfig = serde_yaml::from_str("cache: { memory_percent: 10 }").unwrap();
        assert_eq!(config.cache.l2, L2CacheConfig::None);

        // Same default when the whole `cache:` section is absent.
        let config: AppConfig = serde_yaml::from_str("storages: []").unwrap();
        assert_eq!(config.cache.l2, L2CacheConfig::None);
    }

    #[test]
    fn cache_l2_valkey_backend_parses_from_yaml() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
cache:
  memory_percent: 10
  l2: { backend: valkey, url_env: VALKEY_URL, ttl_s: 60 }
"#,
        )
        .unwrap();
        assert_eq!(
            config.cache.l2,
            L2CacheConfig::Valkey {
                url_env: "VALKEY_URL".to_string(),
                ttl_s: 60,
            }
        );
        assert!(config.validate().is_ok());
    }

    #[test]
    fn cache_l2_valkey_ttl_defaults_when_omitted() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
cache:
  l2: { backend: valkey, url_env: VALKEY_URL }
"#,
        )
        .unwrap();
        assert_eq!(
            config.cache.l2,
            L2CacheConfig::Valkey {
                url_env: "VALKEY_URL".to_string(),
                ttl_s: default_l2_ttl_s(),
            }
        );
    }

    #[test]
    fn rejects_zero_l2_ttl() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
cache:
  l2: { backend: valkey, url_env: VALKEY_URL, ttl_s: 0 }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn max_concurrency_defaults_to_none_and_is_settable() {
        let config: AppConfig = serde_yaml::from_str("").unwrap();
        assert_eq!(config.server.max_concurrency, None);

        let config: AppConfig = serde_yaml::from_str("server: { max_concurrency: 8 }").unwrap();
        assert_eq!(config.server.max_concurrency, Some(8));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn public_base_url_is_optional_and_must_be_a_safe_http_url() {
        let config: AppConfig = serde_yaml::from_str("").unwrap();
        assert_eq!(config.server.public_base_url, None);
        assert_eq!(config.server.public_href("/public"), "/public");

        let config: AppConfig = serde_yaml::from_str(
            "server: { public_base_url: 'https://maps.example.test/tellurion/' }",
        )
        .unwrap();
        assert_eq!(
            config.server.public_href("/public?limit=1"),
            "https://maps.example.test/tellurion/public?limit=1"
        );
        assert!(config.validate().is_ok());

        for invalid in [
            "/relative",
            "ftp://maps.example.test",
            "https://user:secret@maps.example.test",
            "https://maps.example.test?tenant=public",
            "https://maps.example.test#public",
        ] {
            let config: AppConfig =
                serde_yaml::from_str(&format!("server: {{ public_base_url: '{invalid}' }}"))
                    .unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "accepted invalid public base URL: {invalid}"
            );
        }
    }

    #[test]
    fn rejects_zero_max_concurrency() {
        let config: AppConfig = serde_yaml::from_str("server: { max_concurrency: 0 }").unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn descriptor_ttl_defaults_and_is_settable() {
        let config: AppConfig = serde_yaml::from_str("").unwrap();
        assert_eq!(config.server.descriptor_ttl_s, DEFAULT_DESCRIPTOR_TTL_S);

        let config: AppConfig = serde_yaml::from_str("server: { descriptor_ttl_s: 5 }").unwrap();
        assert_eq!(config.server.descriptor_ttl_s, 5);
    }

    // -- clustered applier lease (`#193`) ------------------------------------

    /// Absence is the default and the whole compatibility story: a config
    /// written before this field existed — and every config that simply
    /// does not want clustering — deserializes to no lease at all, and the
    /// applier contacts no coordinator.
    #[test]
    fn the_applier_lease_is_absent_unless_declared() {
        let config: AppConfig = serde_yaml::from_str("").unwrap();
        assert_eq!(config.server.index_applier.lease, None);

        let config: AppConfig =
            serde_yaml::from_str("server: { index_applier: { enabled: true } }").unwrap();
        assert!(config.server.index_applier.enabled);
        assert_eq!(
            config.server.index_applier.lease, None,
            "enabling the applier must not imply a lease"
        );
    }

    /// Declaring the key IS the opt-in — there is deliberately no
    /// `enabled` flag inside `LeaseDecl` to get out of sync with its
    /// presence. The namespace stays optional within it.
    #[test]
    fn declaring_the_lease_opts_in_and_the_namespace_stays_optional() {
        let config: AppConfig =
            serde_yaml::from_str("server: { index_applier: { lease: {} } }").unwrap();
        assert_eq!(
            config.server.index_applier.lease,
            Some(LeaseDecl { namespace: None })
        );

        let config: AppConfig = serde_yaml::from_str(
            "server:\n  index_applier:\n    lease:\n      namespace: staging\n",
        )
        .unwrap();
        assert_eq!(
            config.server.index_applier.lease,
            Some(LeaseDecl {
                namespace: Some("staging".to_string())
            })
        );
    }

    // -- registry scale-out (`#42`) ------------------------------------------

    #[test]
    fn descriptor_cache_capacity_defaults_and_is_settable() {
        let config: AppConfig = serde_yaml::from_str("").unwrap();
        assert_eq!(
            config.server.descriptor_cache_capacity,
            DEFAULT_DESCRIPTOR_CACHE_CAPACITY
        );

        let config: AppConfig =
            serde_yaml::from_str("server: { descriptor_cache_capacity: 5 }").unwrap();
        assert_eq!(config.server.descriptor_cache_capacity, 5);
    }

    #[test]
    fn registry_validation_defaults_to_eager() {
        let config: AppConfig = serde_yaml::from_str("").unwrap();
        assert_eq!(config.registry.validation, RegistryValidationMode::Eager);

        // Same default when the `registry:` section is present but empty.
        let config: AppConfig = serde_yaml::from_str("registry: {}").unwrap();
        assert_eq!(config.registry.validation, RegistryValidationMode::Eager);
    }

    #[test]
    fn registry_validation_lazy_parses_from_yaml() {
        let config: AppConfig = serde_yaml::from_str("registry: { validation: lazy }").unwrap();
        assert_eq!(config.registry.validation, RegistryValidationMode::Lazy);
    }

    #[test]
    fn registry_backend_defaults_to_file() {
        let config: AppConfig = serde_yaml::from_str("").unwrap();
        assert_eq!(config.registry.backend, RegistryBackend::File);

        let config: AppConfig = serde_yaml::from_str("registry: {}").unwrap();
        assert_eq!(config.registry.backend, RegistryBackend::File);

        let config: AppConfig = serde_yaml::from_str("registry: { validation: lazy }").unwrap();
        assert_eq!(config.registry.backend, RegistryBackend::File);
    }

    /// `#162`: the new selector is additive. Every config written before it
    /// existed — including one that names a `relational` backend — parses to
    /// `None`, which `registry::select_relational_implementation` reads as
    /// "the sole relational implementation this binary contains," exactly
    /// what `backend: relational` already meant.
    #[test]
    fn registry_implementation_defaults_to_none_on_every_pre_162_config() {
        for yaml in [
            "",
            "registry: {}",
            "registry: { validation: lazy }",
            "registry: { backend: file }",
        ] {
            let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
            assert_eq!(
                config.registry.implementation, None,
                "'{yaml}' must name no implementation"
            );
        }

        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.registry.implementation, None);
    }

    #[test]
    fn registry_implementation_parses_and_round_trips() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main, implementation: postgis }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.registry.implementation.as_deref(), Some("postgis"));

        // Serializing and re-reading preserves it — `/config/effective`
        // reports back what the operator actually selected.
        let round_tripped: AppConfig =
            serde_yaml::from_str(&serde_yaml::to_string(&config).unwrap()).unwrap();
        assert_eq!(
            round_tripped.registry.implementation.as_deref(),
            Some("postgis")
        );
    }

    /// Shape only, and only the case that could never resolve: an empty name
    /// names nothing. Whether a non-empty name is one this binary registered
    /// is deliberately boot's question, not the document's.
    #[test]
    fn an_empty_registry_implementation_is_refused_at_validate() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main, implementation: "" }
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => assert!(
                message.contains("registry.implementation"),
                "the refusal must name the key: {message}"
            ),
            other => panic!("expected a named Error::Config, got ok={}", other.is_ok()),
        }
    }

    /// A name this binary may or may not contain is NOT resolved here:
    /// `validate` runs with no registry in hand (config linting, tests,
    /// `tellurion-ingest`), so refusing an unknown name is boot's job.
    #[test]
    fn validate_does_not_resolve_an_implementation_name_against_any_registry() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main, implementation: not-compiled-in }
"#,
        )
        .unwrap();
        config
            .validate()
            .expect("shape is valid; resolving the name is boot's job");
    }

    #[test]
    fn registry_backend_relational_parses_from_yaml_alongside_validation() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
registry: { validation: lazy, backend: relational, storage: main }
"#,
        )
        .unwrap();
        assert_eq!(config.registry.validation, RegistryValidationMode::Lazy);
        assert_eq!(config.registry.backend, RegistryBackend::Relational);
        assert_eq!(config.registry.storage.as_deref(), Some("main"));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_a_relational_registry_backend_referencing_an_unknown_storage() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
registry: { backend: relational, storage: nonexistent }
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("nonexistent"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_relational_registry_backend_with_no_storage_set() {
        let config: AppConfig = serde_yaml::from_str("registry: { backend: relational }").unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("registry.storage"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn collection_physical_fields_default_to_none_when_omitted() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let demo = &config.collections[0];
        assert_eq!(demo.table, None);
        assert_eq!(demo.geometry, None);
        assert_eq!(demo.pk, None);
    }

    #[test]
    fn rejects_invalid_zoom_cap_key() {
        let result: std::result::Result<AppConfig, _> = serde_yaml::from_str(
            r#"
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: { bogus: 100 } }
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn places3d_defaults_apply_when_only_height_property_given() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    places3d: { height_property: height }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let places3d = config.collections[0].places3d.as_ref().unwrap();
        assert_eq!(places3d.height_property, "height");
        assert_eq!(places3d.min_height_property, None);
        assert_eq!(places3d.default_height, 0.0);
        assert_eq!(places3d.exaggeration, 1.0);
    }

    #[test]
    fn places3d_absent_is_none() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert!(config.collections[0].places3d.is_none());
    }

    #[test]
    fn places3d_honors_explicit_overrides() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    places3d:
      height_property: height
      min_height_property: min_height
      default_height: 3.5
      exaggeration: 2.0
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let places3d = config.collections[0].places3d.as_ref().unwrap();
        assert_eq!(places3d.min_height_property.as_deref(), Some("min_height"));
        assert_eq!(places3d.default_height, 3.5);
        assert_eq!(places3d.exaggeration, 2.0);
    }

    #[test]
    fn rejects_zero_places3d_exaggeration() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    places3d: { height_property: height, exaggeration: 0.0 }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_negative_places3d_exaggeration() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    places3d: { height_property: height, exaggeration: -1.0 }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    /// `#41`: `places3d.vertex_caps` is empty by default — a collection
    /// that never touches the `VolumeSource` lane declares nothing about
    /// it, exactly like `tiles.caps`'s own default before this field
    /// existed.
    #[test]
    fn places3d_vertex_caps_defaults_to_empty() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    places3d: { height_property: height }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let places3d = config.collections[0].places3d.as_ref().unwrap();
        assert_eq!(places3d.vertex_caps, ZoomCaps::default());
    }

    #[test]
    fn places3d_honors_an_explicit_vertex_cap() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 10 }
    places3d: { height_property: height, vertex_caps: { z5: 20000 } }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let places3d = config.collections[0].places3d.as_ref().unwrap();
        assert_eq!(places3d.vertex_caps.get(5), Some(20_000));
    }

    #[test]
    fn rejects_a_places3d_vertex_cap_zoom_outside_the_tiles_range() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5 }
    places3d: { height_property: height, vertex_caps: { z10: 20000 } }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_a_zero_places3d_vertex_cap() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    places3d: { height_property: height, vertex_caps: { z0: 0 } }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    // -- geometry_variants (`#104`) ------------------------------------------

    #[test]
    fn geometry_variants_defaults_to_empty() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert!(config.collections[0].geometry_variants.is_empty());
    }

    #[test]
    fn accepts_well_formed_non_overlapping_geometry_variants() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 14 }
    geometry_variants:
      - column: geom_z6
        minzoom: 0
        maxzoom: 6
      - column: geom_z11
        minzoom: 7
        maxzoom: 11
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let variants = &config.collections[0].geometry_variants;
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].column, "geom_z6");
        assert_eq!(variants[1].column, "geom_z11");
    }

    #[test]
    fn rejects_a_geometry_variant_with_an_empty_column_name() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    geometry_variants:
      - column: ""
        minzoom: 0
        maxzoom: 6
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("empty"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_geometry_variant_column_declared_more_than_once() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 14 }
    geometry_variants:
      - column: geom_z6
        minzoom: 0
        maxzoom: 3
      - column: geom_z6
        minzoom: 4
        maxzoom: 6
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("geom_z6"), "message was: {message}");
                assert!(message.contains("more than once"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_geometry_variant_with_minzoom_greater_than_maxzoom() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    geometry_variants:
      - column: geom_z6
        minzoom: 6
        maxzoom: 0
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("geom_z6"), "message was: {message}");
                assert!(message.contains("minzoom"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_geometry_variant_zoom_range_outside_the_tiles_range() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5 }
    geometry_variants:
      - column: geom_z10
        minzoom: 0
        maxzoom: 10
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("geom_z10"), "message was: {message}");
                assert!(message.contains("outside"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_overlapping_geometry_variant_zoom_ranges() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 14 }
    geometry_variants:
      - column: geom_z6
        minzoom: 0
        maxzoom: 6
      - column: geom_z11
        minzoom: 5
        maxzoom: 11
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("geom_z11"), "message was: {message}");
                assert!(message.contains("overlaps"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// Builds a bare `CollectionDecl` (no `AppConfig` wrapper) with the given
    /// `geometry_variants:` YAML block spliced in — enough to exercise
    /// `resolved_geometry_for_zoom` directly, since that method has no
    /// dependency on `AppConfig::validate` having run.
    fn geometry_variants_decl(geometry_variants_yaml: &str) -> CollectionDecl {
        let yaml = format!(
            "id: demo
catalog: default
storage: main
table: demo
geometry: geom
pk: id
{geometry_variants_yaml}"
        );
        serde_yaml::from_str(&yaml).expect("valid CollectionDecl yaml")
    }

    #[test]
    fn resolved_geometry_for_zoom_falls_back_to_the_base_column_when_no_variant_is_declared() {
        let decl = geometry_variants_decl("");
        assert_eq!(decl.resolved_geometry_for_zoom(5), "geom");
    }

    #[test]
    fn resolved_geometry_for_zoom_selects_the_variant_covering_the_zoom() {
        let decl = geometry_variants_decl(
            "geometry_variants:
  - column: geom_z6
    minzoom: 0
    maxzoom: 6
",
        );
        assert_eq!(decl.resolved_geometry_for_zoom(3), "geom_z6");
    }

    #[test]
    fn resolved_geometry_for_zoom_falls_back_to_the_base_column_outside_the_variants_range() {
        let decl = geometry_variants_decl(
            "geometry_variants:
  - column: geom_z6
    minzoom: 0
    maxzoom: 6
",
        );
        assert_eq!(decl.resolved_geometry_for_zoom(7), "geom");
    }

    /// Boundary zooms: the variant's own `minzoom`/`maxzoom` are inclusive
    /// endpoints, and the very next zoom past `maxzoom` already falls back.
    #[test]
    fn resolved_geometry_for_zoom_treats_the_variant_range_as_inclusive_at_both_ends() {
        let decl = geometry_variants_decl(
            "geometry_variants:
  - column: geom_z6
    minzoom: 2
    maxzoom: 6
",
        );
        assert_eq!(decl.resolved_geometry_for_zoom(1), "geom");
        assert_eq!(decl.resolved_geometry_for_zoom(2), "geom_z6");
        assert_eq!(decl.resolved_geometry_for_zoom(6), "geom_z6");
        assert_eq!(decl.resolved_geometry_for_zoom(7), "geom");
    }

    #[test]
    fn resolved_geometry_for_zoom_selects_among_multiple_non_overlapping_variants() {
        let decl = geometry_variants_decl(
            "geometry_variants:
  - column: geom_z6
    minzoom: 0
    maxzoom: 6
  - column: geom_z11
    minzoom: 7
    maxzoom: 11
",
        );
        assert_eq!(decl.resolved_geometry_for_zoom(3), "geom_z6");
        assert_eq!(decl.resolved_geometry_for_zoom(9), "geom_z11");
        assert_eq!(
            decl.resolved_geometry_for_zoom(14),
            "geom",
            "a zoom past every declared variant still falls back to the base column"
        );
    }

    // -- object_stores: s3 profile refusals (assets-and-object-storage
    // proposal, second slice) -----------------------------------------------

    const S3_OBJECT_STORE_YAML: &str = r#"
object_stores:
  - id: blobs
    profile: s3
    endpoint: https://minio.example.test:9000
    bucket: photos
    region: us-east-1
    access_key_env: TELLURION_TEST_ACCESS_KEY
    secret_key_env: TELLURION_TEST_SECRET_KEY
"#;

    #[test]
    fn accepts_a_well_formed_s3_object_store() {
        let config: AppConfig = serde_yaml::from_str(S3_OBJECT_STORE_YAML).unwrap();
        config.validate().unwrap();
        assert_eq!(config.object_stores.len(), 1);
        match &config.object_stores[0].profile {
            ObjectStoreProfile::S3 {
                presign_expiry_s, ..
            } => {
                assert_eq!(*presign_expiry_s, 900, "default presign expiry");
            }
            ObjectStoreProfile::Fs { .. } => panic!("expected an s3 profile"),
        }
    }

    #[test]
    fn rejects_duplicate_object_store_ids() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{S3_OBJECT_STORE_YAML}\
             \n  - id: blobs\n    profile: fs\n    root: /tmp\n"
        ))
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_an_fs_object_store_with_an_empty_root() {
        let config: AppConfig =
            serde_yaml::from_str("object_stores: [ { id: blobs, profile: fs, root: \"\" } ]")
                .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_an_s3_object_store_with_an_empty_credential_env_var_name() {
        for field in ["bucket", "region", "access_key_env", "secret_key_env"] {
            let yaml = S3_OBJECT_STORE_YAML.replacen(
                &format!("{field}: "),
                &format!("{field}: \"\" #"),
                1,
            );
            let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "empty '{field}' must be refused"
            );
        }
    }

    #[test]
    fn rejects_an_s3_object_store_with_an_unparseable_endpoint() {
        let yaml = S3_OBJECT_STORE_YAML.replace(
            "endpoint: https://minio.example.test:9000",
            "endpoint: \"not a url\"",
        );
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_an_s3_object_store_endpoint_that_is_not_http_or_https() {
        let yaml = S3_OBJECT_STORE_YAML.replace(
            "endpoint: https://minio.example.test:9000",
            "endpoint: \"ftp://minio.example.test\"",
        );
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_a_zero_presign_expiry() {
        let yaml = format!("{S3_OBJECT_STORE_YAML}    presign_expiry_s: 0\n");
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_a_presign_expiry_past_the_sigv4_maximum() {
        let yaml = format!(
            "{S3_OBJECT_STORE_YAML}    presign_expiry_s: {}\n",
            MAX_PRESIGN_EXPIRY_S + 1
        );
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn accepts_a_presign_expiry_at_the_sigv4_maximum() {
        let yaml = format!("{S3_OBJECT_STORE_YAML}    presign_expiry_s: {MAX_PRESIGN_EXPIRY_S}\n");
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn rejects_a_collection_referencing_an_unknown_object_store() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    object_store: nope
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    // -- declared schema (`#44`) ---------------------------------------------

    /// No-regression guard: a collection with no `schema:` key parses with
    /// `schema: None` — free-form stays the default, byte-for-byte the same
    /// as before this field existed.
    #[test]
    fn schema_absent_is_none() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert!(config.collections[0].schema.is_none());
    }

    #[test]
    fn schema_parses_declared_properties_with_defaults() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    schema:
      properties:
        - { name: name, type: string }
        - { name: population, type: integer, required: true }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let schema = config.collections[0].schema.as_ref().unwrap();
        assert!(schema.additional_properties, "defaults to true");
        assert_eq!(schema.properties.len(), 2);
        assert_eq!(schema.properties[0].name, "name");
        assert_eq!(schema.properties[0].type_, PropertyType::String);
        assert!(!schema.properties[0].required, "defaults to false");
        assert_eq!(schema.properties[1].name, "population");
        assert_eq!(schema.properties[1].type_, PropertyType::Integer);
        assert!(schema.properties[1].required);
    }

    #[test]
    fn schema_honors_an_explicit_additional_properties_false() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    schema:
      properties: [ { name: name, type: string } ]
      additional_properties: false
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert!(
            !config.collections[0]
                .schema
                .as_ref()
                .unwrap()
                .additional_properties
        );
    }

    #[test]
    fn schema_accepts_every_property_type() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    schema:
      properties:
        - { name: a, type: string }
        - { name: b, type: integer }
        - { name: c, type: number }
        - { name: d, type: boolean }
        - { name: e, type: date }
        - { name: f, type: datetime }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let types: Vec<PropertyType> = config.collections[0]
            .schema
            .as_ref()
            .unwrap()
            .properties
            .iter()
            .map(|p| p.type_)
            .collect();
        assert_eq!(
            types,
            vec![
                PropertyType::String,
                PropertyType::Integer,
                PropertyType::Number,
                PropertyType::Boolean,
                PropertyType::Date,
                PropertyType::DateTime,
            ]
        );
    }

    #[test]
    fn schema_rejects_an_unknown_property_type() {
        let result: std::result::Result<AppConfig, _> = serde_yaml::from_str(
            r#"
collections:
  - id: demo
    catalog: default
    storage: main
    schema:
      properties: [ { name: bogus, type: jsonb } ]
"#,
        );
        assert!(
            result.is_err(),
            "'jsonb' is not in PropertyType's closed set"
        );
    }

    #[test]
    fn schema_rejects_a_duplicate_property_name() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    schema:
      properties:
        - { name: name, type: string }
        - { name: name, type: integer }
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("name"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn property_type_from_sql_type_classifies_common_postgis_types() {
        assert_eq!(PropertyType::from_sql_type("text"), PropertyType::String);
        assert_eq!(
            PropertyType::from_sql_type("integer"),
            PropertyType::Integer
        );
        assert_eq!(
            PropertyType::from_sql_type("double precision"),
            PropertyType::Number
        );
        assert_eq!(
            PropertyType::from_sql_type("boolean"),
            PropertyType::Boolean
        );
        assert_eq!(PropertyType::from_sql_type("date"), PropertyType::Date);
        assert_eq!(
            PropertyType::from_sql_type("timestamp with time zone"),
            PropertyType::DateTime
        );
        assert_eq!(
            PropertyType::from_sql_type("jsonb"),
            PropertyType::String,
            "an unrecognized SQL type falls back to String"
        );
    }

    #[test]
    fn parses_styles_list() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
styles:
  - { id: basic, path: styles/basic.json }
  - { id: dark, path: styles/dark.json }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.styles.len(), 2);
        assert_eq!(config.styles[0].id, "basic");
        assert_eq!(config.styles[1].path, "styles/dark.json");
    }

    #[test]
    fn styles_default_to_empty() {
        let config: AppConfig = serde_yaml::from_str("storages: []").unwrap();
        assert!(config.styles.is_empty());
    }

    #[test]
    fn routing_omitted_defaults_both_lanes_to_none() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let routing = &config.collections[0].routing;
        assert!(routing.features.is_none());
        assert!(routing.tiles.is_none());
    }

    #[test]
    fn routing_lane_accepts_a_single_storage_id() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { tiles: main }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let routing = &config.collections[0].routing;
        assert_eq!(routing.tiles.as_ref().unwrap().0, vec!["main".to_string()]);
        assert!(routing.features.is_none());
    }

    /// `#86`: the maps lane parses and validates independently of `tiles`,
    /// and stays `None` (defaults to the single `storage`) when omitted.
    #[test]
    fn routing_maps_lane_accepts_a_single_storage_id() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { maps: main }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let routing = &config.collections[0].routing;
        assert_eq!(routing.maps.as_ref().unwrap().0, vec!["main".to_string()]);
        assert!(routing.tiles.is_none());
    }

    #[test]
    fn rejects_maps_routing_lane_referencing_unknown_storage() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { maps: missing }
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("maps"));
                assert!(message.contains("missing"));
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn routing_lane_accepts_an_ordered_list() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: main, driver: postgis, url_env: DATABASE_URL }
  - { id: mirror, driver: postgis, url_env: DATABASE_URL2 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing:
      features: [main, mirror]
      tiles: main
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let routing = &config.collections[0].routing;
        assert_eq!(
            routing.features.as_ref().unwrap().0,
            vec!["main".to_string(), "mirror".to_string()]
        );
        assert_eq!(routing.tiles.as_ref().unwrap().0, vec!["main".to_string()]);
    }

    #[test]
    fn rejects_routing_lane_referencing_unknown_storage() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { tiles: missing }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_empty_routing_lane_list() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { tiles: [] }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    /// `#25`: a single-storage `write` lane parses and validates cleanly —
    /// the one shape the write path actually resolves.
    #[test]
    fn routing_write_lane_accepts_a_single_storage_id() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { write: main }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let routing = &config.collections[0].routing;
        assert_eq!(routing.write.as_ref().unwrap().0, vec!["main".to_string()]);
    }

    /// `#25`: write has no fallback tail — naming more than one storage is a
    /// config error, not a silently-accepted preference order.
    #[test]
    fn routing_write_lane_rejects_more_than_one_storage() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: main, driver: postgis, url_env: DATABASE_URL }
  - { id: mirror, driver: postgis, url_env: DATABASE_URL2 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { write: [main, mirror] }
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("write"), "message was: {message}");
                assert!(message.contains("fallback"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn routing_write_lane_referencing_unknown_storage_is_rejected() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { write: missing }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    // -- `SchemaDecl::validate_feature_properties` (`#44`, write-side) -------

    fn feature_properties(
        pairs: &[(&str, serde_json::Value)],
    ) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn validate_feature_properties_accepts_a_well_typed_feature() {
        let schema = SchemaDecl {
            properties: vec![
                PropertyDecl {
                    name: "name".to_string(),
                    type_: PropertyType::String,
                    required: true,
                },
                PropertyDecl {
                    name: "population".to_string(),
                    type_: PropertyType::Integer,
                    required: false,
                },
            ],
            additional_properties: true,
        };
        let properties = feature_properties(&[
            ("name", serde_json::json!("acme")),
            ("population", serde_json::json!(42)),
        ]);
        assert!(schema.validate_feature_properties(&properties).is_ok());
    }

    #[test]
    fn validate_feature_properties_rejects_a_missing_required_property() {
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "name".to_string(),
                type_: PropertyType::String,
                required: true,
            }],
            additional_properties: true,
        };
        let properties = feature_properties(&[]);
        match schema.validate_feature_properties(&properties) {
            Err(Error::Invalid(message)) => {
                assert!(message.contains("name"), "message was: {message}");
                assert!(message.contains("required"), "message was: {message}");
            }
            other => panic!("expected Err(Invalid(_)), got {other:?}"),
        }
    }

    /// An explicit JSON `null` is treated the same as an absent key for the
    /// `required` check — `null` means "no value," not "here is a value."
    #[test]
    fn validate_feature_properties_treats_an_explicit_null_as_missing_for_a_required_property() {
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "name".to_string(),
                type_: PropertyType::String,
                required: true,
            }],
            additional_properties: true,
        };
        let properties = feature_properties(&[("name", serde_json::Value::Null)]);
        assert!(schema.validate_feature_properties(&properties).is_err());
    }

    /// `null` for an OPTIONAL property is fine — "no value" is a legitimate
    /// answer for something that was never required to have one.
    #[test]
    fn validate_feature_properties_accepts_null_for_an_optional_property() {
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "population".to_string(),
                type_: PropertyType::Integer,
                required: false,
            }],
            additional_properties: true,
        };
        let properties = feature_properties(&[("population", serde_json::Value::Null)]);
        assert!(schema.validate_feature_properties(&properties).is_ok());
    }

    #[test]
    fn validate_feature_properties_rejects_a_type_mismatch_naming_expected_and_actual() {
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "population".to_string(),
                type_: PropertyType::Integer,
                required: false,
            }],
            additional_properties: true,
        };
        let properties = feature_properties(&[("population", serde_json::json!("not-a-number"))]);
        match schema.validate_feature_properties(&properties) {
            Err(Error::Invalid(message)) => {
                assert!(message.contains("population"), "message was: {message}");
                assert!(message.contains("integer"), "message was: {message}");
                assert!(message.contains("string"), "message was: {message}");
            }
            other => panic!("expected Err(Invalid(_)), got {other:?}"),
        }
    }

    /// A JSON number with a fractional part is not a valid `Integer` —
    /// `Number` is the type for that.
    #[test]
    fn validate_feature_properties_rejects_a_fractional_number_for_an_integer_property() {
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "population".to_string(),
                type_: PropertyType::Integer,
                required: false,
            }],
            additional_properties: true,
        };
        let properties = feature_properties(&[("population", serde_json::json!(1.5))]);
        assert!(schema.validate_feature_properties(&properties).is_err());
    }

    #[test]
    fn validate_feature_properties_rejects_an_undeclared_property_when_closed() {
        let schema = SchemaDecl {
            properties: vec![PropertyDecl {
                name: "name".to_string(),
                type_: PropertyType::String,
                required: false,
            }],
            additional_properties: false,
        };
        let properties = feature_properties(&[
            ("name", serde_json::json!("acme")),
            ("extra", serde_json::json!("surprise")),
        ]);
        match schema.validate_feature_properties(&properties) {
            Err(Error::Invalid(message)) => {
                assert!(message.contains("extra"), "message was: {message}");
            }
            other => panic!("expected Err(Invalid(_)), got {other:?}"),
        }
    }

    /// The open-schema default (`additional_properties: true`) tolerates an
    /// undeclared property outright — matches this type's own read-side
    /// default.
    #[test]
    fn validate_feature_properties_accepts_an_undeclared_property_when_open() {
        let schema = SchemaDecl {
            properties: vec![],
            additional_properties: true,
        };
        let properties = feature_properties(&[("anything", serde_json::json!("goes"))]);
        assert!(schema.validate_feature_properties(&properties).is_ok());
    }

    /// A rejection collects every violation at once rather than stopping at
    /// the first — a caller gets the full picture in one round trip.
    #[test]
    fn validate_feature_properties_reports_every_violation_at_once() {
        let schema = SchemaDecl {
            properties: vec![
                PropertyDecl {
                    name: "name".to_string(),
                    type_: PropertyType::String,
                    required: true,
                },
                PropertyDecl {
                    name: "population".to_string(),
                    type_: PropertyType::Integer,
                    required: false,
                },
            ],
            additional_properties: false,
        };
        let properties = feature_properties(&[
            ("population", serde_json::json!("not-a-number")),
            ("extra", serde_json::json!(true)),
        ]);
        match schema.validate_feature_properties(&properties) {
            Err(Error::Invalid(message)) => {
                assert!(message.contains("name"), "message was: {message}");
                assert!(message.contains("population"), "message was: {message}");
                assert!(message.contains("extra"), "message was: {message}");
            }
            other => panic!("expected Err(Invalid(_)), got {other:?}"),
        }
    }

    #[test]
    fn property_type_matches_json_value_for_every_type() {
        assert!(PropertyType::String.matches_json_value(&serde_json::json!("a")));
        assert!(!PropertyType::String.matches_json_value(&serde_json::json!(1)));
        assert!(PropertyType::Integer.matches_json_value(&serde_json::json!(1)));
        assert!(!PropertyType::Integer.matches_json_value(&serde_json::json!(1.5)));
        assert!(PropertyType::Number.matches_json_value(&serde_json::json!(1.5)));
        assert!(PropertyType::Number.matches_json_value(&serde_json::json!(1)));
        assert!(PropertyType::Boolean.matches_json_value(&serde_json::json!(true)));
        assert!(!PropertyType::Boolean.matches_json_value(&serde_json::json!("true")));
        assert!(PropertyType::Date.matches_json_value(&serde_json::json!("2020-01-01")));
        assert!(
            PropertyType::DateTime.matches_json_value(&serde_json::json!("2020-01-01T00:00:00Z"))
        );
    }

    /// `#17`: no `auth:` section at all is the permissive default.
    #[test]
    fn auth_defaults_to_none_when_the_section_is_absent() {
        let config: AppConfig = serde_yaml::from_str("storages: []").unwrap();
        assert_eq!(config.auth, AuthConfig::default());
        assert!(!config.auth.is_configured());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn auth_static_bearer_tokens_parse_from_yaml() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: tenant-a } ]
auth:
  bearer_tokens:
    - { token: dev-token, tenants: [tenant-a] }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert!(config.auth.is_configured());
        assert_eq!(config.auth.bearer_tokens.len(), 1);
        assert_eq!(config.auth.bearer_tokens[0].token, "dev-token");
        assert_eq!(
            config.auth.bearer_tokens[0].tenants,
            vec!["tenant-a".to_string()]
        );
        assert!(config.auth.oidc.is_none());
    }

    #[test]
    fn rejects_a_bearer_token_referencing_an_unknown_tenant() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: tenant-a } ]
auth:
  bearer_tokens:
    - { token: dev-token, tenants: [nonexistent-tenant] }
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("nonexistent-tenant"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_an_empty_bearer_token_value() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: tenant-a } ]
auth:
  bearer_tokens:
    - { token: "", tenants: [tenant-a] }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    /// `#144`: the pre-`#144` document — an inline `token:` and no
    /// `token_env` anywhere — parses and validates byte-for-byte as it
    /// always did. This is the compatibility floor the whole slice stands
    /// on, pinned positively rather than argued for; `auth_static_bearer_
    /// tokens_parse_from_yaml` above is its sibling.
    #[test]
    fn a_config_written_before_the_credential_seam_still_validates() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: tenant-a } ]
auth:
  bearer_tokens:
    - { token: legacy-inline-token, tenants: [tenant-a], platform_admin: true }
"#,
        )
        .expect("a document with an inline token must still parse");
        config.validate().expect("and must still validate");
        assert_eq!(config.auth.bearer_tokens[0].token, "legacy-inline-token");
        assert!(config.auth.bearer_tokens[0].token_env.is_none());
    }

    #[test]
    fn a_bearer_principal_may_name_an_environment_variable_instead_of_a_token() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: tenant-a } ]
auth:
  bearer_tokens:
    - { token_env: TELLURION_SERVICE_TOKEN, tenants: [tenant-a] }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert!(config.auth.is_configured());
        assert_eq!(
            config.auth.bearer_tokens[0].token_env.as_deref(),
            Some("TELLURION_SERVICE_TOKEN")
        );
        assert!(config.auth.bearer_tokens[0].token.is_empty());
    }

    /// Exactly one credential location per principal. Both is ambiguous
    /// about which one is live; neither leaves the principal with no
    /// credential at all. Both are named refusals, never a precedence rule.
    #[test]
    fn a_bearer_principal_declaring_both_or_neither_credential_location_is_refused() {
        for (document, expected) in [
            (
                "- { token: inline, token_env: TELLURION_TOKEN, tenants: [tenant-a] }",
                "both",
            ),
            ("- { tenants: [tenant-a] }", "neither"),
            (
                "- { token_env: \"\", tenants: [tenant-a] }",
                "must not be empty",
            ),
        ] {
            let config: AppConfig = serde_yaml::from_str(&format!(
                "tenants: [ {{ id: tenant-a }} ]\nauth:\n  bearer_tokens:\n    {document}\n"
            ))
            .unwrap();
            match config.validate() {
                Err(Error::Config(message)) => assert!(
                    message.contains("auth.bearer_tokens") && message.contains(expected),
                    "message was: {message}"
                ),
                other => panic!("expected a named refusal for {document}, got {other:?}"),
            }
        }
    }

    #[test]
    fn two_bearer_principals_reading_one_environment_variable_are_refused() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: tenant-a }, { id: tenant-b } ]
auth:
  bearer_tokens:
    - { token_env: TELLURION_TOKEN, tenants: [tenant-a] }
    - { token_env: TELLURION_TOKEN, tenants: [tenant-b] }
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("TELLURION_TOKEN"), "{message}")
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// `#144`: every refusal on this path is rendered into a `PUT
    /// /config?dry_run=true` response body verbatim (`config_mutation.rs`
    /// puts `error.to_string()` in `detail`), so no message here may carry a
    /// token value. Asserted against the value actually present in the
    /// document being refused.
    #[test]
    fn no_bearer_validation_message_ever_carries_the_token_value() {
        const SECRET: &str = "s3cret-inline-token-value";
        for document in [
            format!("- {{ token: {SECRET}, tenants: [] }}"),
            format!("- {{ token: {SECRET}, token_env: TELLURION_TOKEN, tenants: [tenant-a] }}"),
            format!("- {{ token: {SECRET}, tenants: [no-such-tenant] }}"),
            format!("- {{ token: {SECRET}, tenants: [tenant-a], roles: {{ other: [reader] }} }}"),
            format!("- {{ token: {SECRET}, tenants: [tenant-a] }}\n    - {{ token: {SECRET}, tenants: [tenant-a] }}"),
        ] {
            let config: AppConfig = serde_yaml::from_str(&format!(
                "tenants: [ {{ id: tenant-a }} ]\nauth:\n  bearer_tokens:\n    {document}\n"
            ))
            .unwrap();
            let Err(error) = config.validate() else {
                panic!("expected a refusal for: {document}");
            };
            let message = error.to_string();
            assert!(
                !message.contains(SECRET),
                "a validation message leaked the token value: {message}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_bearer_token_entries() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: tenant-a }, { id: tenant-b } ]
auth:
  bearer_tokens:
    - { token: dup-token, tenants: [tenant-a] }
    - { token: dup-token, tenants: [tenant-b] }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_a_bearer_token_with_no_tenants() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: tenant-a } ]
auth:
  bearer_tokens:
    - { token: dev-token, tenants: [] }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    /// `#34`: the whole point of the struct (not tagged-enum) shape — a
    /// service account's static token and a human's OIDC issuer configured
    /// at once, both valid.
    #[test]
    fn static_bearer_tokens_and_oidc_coexist_in_one_auth_section() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: tenant-a } ]
auth:
  bearer_tokens:
    - { token: service-token, tenants: [tenant-a] }
  oidc:
    issuer: "https://idp.example.com"
    audience: "tellurion"
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert!(config.auth.is_configured());
        assert_eq!(config.auth.bearer_tokens.len(), 1);
        let oidc = config.auth.oidc.as_ref().expect("oidc block parsed");
        assert_eq!(oidc.issuer, "https://idp.example.com");
        assert_eq!(oidc.audience, "tellurion");
        // Defaults apply when `claims`/`clock_skew_s`/`jwks_ttl_s` are omitted.
        assert_eq!(oidc.claims.tenants, "tenants");
        assert_eq!(oidc.clock_skew_s, 60);
        assert_eq!(oidc.jwks_ttl_s, 300);
    }

    #[test]
    fn multiple_trusted_issuers_parse_and_validate() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
auth:
  trusted_issuers:
    - issuer: "https://idp-a.example.com"
      audience: "tellurion"
    - issuer: "https://idp-b.example.com"
      audience: "tellurion"
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.auth.trusted_issuers.len(), 2);
        assert!(config.auth.is_configured());
    }

    #[test]
    fn duplicate_trusted_issuer_is_rejected_across_legacy_and_list_forms() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
auth:
  oidc:
    issuer: "https://idp.example.com"
    audience: "tellurion"
  trusted_issuers:
    - issuer: "https://idp.example.com"
      audience: "tellurion"
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn oidc_claim_mapping_and_timing_knobs_override_from_yaml() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
auth:
  oidc:
    issuer: "https://idp.example.com"
    audience: "tellurion"
    claims:
      tenants: "groups"
    clock_skew_s: 30
    jwks_ttl_s: 120
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let oidc = config.auth.oidc.as_ref().unwrap();
        assert_eq!(oidc.claims.tenants, "groups");
        assert_eq!(oidc.clock_skew_s, 30);
        assert_eq!(oidc.jwks_ttl_s, 120);
    }

    #[test]
    fn rejects_an_empty_oidc_issuer() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
auth:
  oidc:
    issuer: ""
    audience: "tellurion"
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_an_oidc_issuer_that_is_not_a_url() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
auth:
  oidc:
    issuer: "not a url"
    audience: "tellurion"
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_an_oidc_issuer_with_a_non_http_scheme() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
auth:
  oidc:
    issuer: "ftp://idp.example.com"
    audience: "tellurion"
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_a_remote_plain_http_oidc_issuer() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
auth:
  trusted_issuers:
    - issuer: "http://idp.example.com"
      audience: "tellurion"
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn permits_plain_http_only_for_loopback_oidc_issuers() {
        for issuer in [
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
            "http://localhost:8080",
        ] {
            let yaml = format!(
                r#"
auth:
  trusted_issuers:
    - issuer: "{issuer}"
      audience: "tellurion"
"#
            );
            let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
            config.validate().unwrap();
        }
    }

    #[test]
    fn rejects_an_empty_oidc_audience() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
auth:
  oidc:
    issuer: "https://idp.example.com"
    audience: ""
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn rejects_a_zero_jwks_ttl() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
auth:
  oidc:
    issuer: "https://idp.example.com"
    audience: "tellurion"
    jwks_ttl_s: 0
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    /// `#34`: an unreachable/nonexistent issuer is a shape-only concern at
    /// validate time — no network call happens here, so this must pass.
    #[test]
    fn oidc_validate_never_probes_the_issuer_over_the_network() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
auth:
  oidc:
    issuer: "https://issuer.invalid.example"
    audience: "tellurion"
"#,
        )
        .unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn rejects_duplicate_style_ids() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
styles:
  - { id: basic, path: styles/basic.json }
  - { id: basic, path: styles/other.json }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    // -- named settings profiles (`#111`) ------------------------------------

    #[test]
    fn rejects_duplicate_profile_ids() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
profiles:
  - { id: heavy-raster, cache_ttl_s: 60 }
  - { id: heavy-raster, cache_ttl_s: 120 }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    /// A `profile:` reference to an unknown id is refused by name at load,
    /// the same treatment every other dangling reference in this module
    /// gets (storage, tenant, catalog, ...) — exercised at all four
    /// settings-chain levels, the same sweep
    /// `rejects_zero_slow_request_ms_at_every_settings_level` above runs
    /// for a different per-level rule.
    #[test]
    fn rejects_a_reference_to_an_unknown_profile_at_every_settings_level() {
        for settings in [
            "settings: { profile: does-not-exist }",
            "tenants: [ { id: public, settings: { profile: does-not-exist } } ]",
            "\
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public, settings: { profile: does-not-exist } } ]",
            "\
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections: [ { id: demo, catalog: default, storage: main, settings: { profile: does-not-exist } } ]",
        ] {
            let config: AppConfig = serde_yaml::from_str(settings).unwrap();
            assert!(
                matches!(config.validate(), Err(Error::Config(_))),
                "{settings} must be rejected"
            );
        }
    }

    /// The mirror of the rejection above: a reference naming a real profile
    /// id passes validation cleanly, at every level.
    #[test]
    fn accepts_a_reference_to_a_known_profile_at_every_settings_level() {
        for settings in [
            "\
profiles: [ { id: heavy-raster, cache_ttl_s: 60 } ]
settings: { profile: heavy-raster }",
            "\
profiles: [ { id: heavy-raster, cache_ttl_s: 60 } ]
tenants: [ { id: public, settings: { profile: heavy-raster } } ]",
            "\
profiles: [ { id: heavy-raster, cache_ttl_s: 60 } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public, settings: { profile: heavy-raster } } ]",
            "\
profiles: [ { id: heavy-raster, cache_ttl_s: 60 } ]
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections: [ { id: demo, catalog: default, storage: main, settings: { profile: heavy-raster } } ]",
        ] {
            let config: AppConfig = serde_yaml::from_str(settings).unwrap();
            config
                .validate()
                .unwrap_or_else(|error| panic!("{settings} must be accepted, got {error}"));
        }
    }

    /// No profile-of-profiles (`#111`): a profile's own `settings.profile`
    /// is refused outright, even when it names another real profile id —
    /// composing profiles is a sign the fragment was cut wrong, not a
    /// richer merge algebra this resolver grows into.
    #[test]
    fn rejects_a_profile_that_references_another_profile() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
profiles:
  - { id: base, cache_ttl_s: 60 }
  - { id: derived, profile: base, slow_request_ms: 500 }
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    /// A profile is a single id, never a list — a YAML sequence where a
    /// scalar `profile:` id is expected fails at parse time itself, before
    /// `validate` ever runs, the same "refused by shape" guarantee a plain
    /// `Option<String>` field gives every other single-value reference in
    /// this module.
    #[test]
    fn rejects_a_profile_list_at_parse_time() {
        let result: std::result::Result<AppConfig, _> = serde_yaml::from_str(
            r#"
settings: { profile: [a, b] }
"#,
        );
        assert!(result.is_err());
    }

    /// A profile's fragment is structurally limited to the settings surface
    /// `SettingsDecl` carries: routing/storage/identity keys simply have no
    /// field to land in, so declaring one alongside a real settings key is a
    /// no-op rather than smuggled-in behavior — `ProfileDecl` never gains a
    /// field for them.
    #[test]
    fn a_profile_cannot_express_routing_or_storage_keys() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
profiles:
  - { id: heavy-raster, cache_ttl_s: 60, storage: main, routing: { features: [main] } }
"#,
        )
        .unwrap();
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(config.profiles[0].settings.cache_ttl_s, Some(60));
    }

    // -- double-source rule (`#42`, third slice) -----------------------------

    /// A `relational` backend that ALSO declares `catalogs:` in the same
    /// document is refused outright — ambiguous double source is worse than
    /// either source alone (see `validate`'s own comment for why this isn't
    /// a precedence rule instead).
    #[test]
    fn rejects_a_relational_backend_that_also_declares_catalogs() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
registry: { backend: relational, storage: main }
catalogs: [ { id: default, tenant: public } ]
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("relational"), "message was: {message}");
                assert!(
                    message.contains("catalogs") || message.contains("collections"),
                    "message should name what collided, was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// Same rule, triggered by `collections:` alone (catalogs empty) —
    /// either source non-empty is enough to refuse the double source, not
    /// just the pair together.
    #[test]
    fn rejects_a_relational_backend_that_also_declares_collections() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
registry: { backend: relational, storage: main }
collections:
  - id: demo
    catalog: default
    storage: main
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    /// The `file` backend (the default) is entirely unaffected by the
    /// double-source rule — declaring `catalogs:`/`collections:` alongside
    /// it is the normal, expected shape, not a collision.
    #[test]
    fn the_file_backend_may_freely_declare_catalogs_and_collections() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
"#,
        )
        .unwrap();
        config.validate().unwrap();
    }

    // -- `validate_registry_snapshot` (`#42`, third slice) -------------------

    fn snapshot_operator_config() -> AppConfig {
        serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main }
"#,
        )
        .unwrap()
    }

    fn snapshot_tenants() -> Vec<TenantDecl> {
        vec![TenantDecl {
            id: "public".to_string(),
            external_id: None,
            settings: Default::default(),
        }]
    }

    fn snapshot_catalog(id: &str, tenant: &str) -> CatalogDecl {
        CatalogDecl {
            id: id.to_string(),
            external_id: None,
            tenant: tenant.to_string(),
            settings: SettingsDecl::default(),
            visibility: VisibilityDecl::default(),
        }
    }

    fn snapshot_collection(id: &str, catalog: &str, storage: &str) -> CollectionDecl {
        serde_yaml::from_str(&format!(
            "id: {id}\ncatalog: {catalog}\nstorage: {storage}\n"
        ))
        .unwrap()
    }

    /// A snapshot whose declarations are all internally consistent with the
    /// operator config (tenant/storage references resolve, ids unique)
    /// passes — the ordinary case every relational boot/reload hits.
    #[test]
    fn validate_registry_snapshot_accepts_a_consistent_snapshot() {
        let config = snapshot_operator_config();
        let snapshot = RoutingSnapshot {
            catalogs: vec![snapshot_catalog("default", "public")],
            collections: vec![snapshot_collection("demo", "default", "main")],
        };
        validate_registry_snapshot(&config, &snapshot_tenants(), &snapshot).unwrap();
    }

    #[test]
    fn relational_validation_accepts_database_tenant_references_without_a_yaml_duplicate() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main }
auth:
  bearer_tokens:
    - { token: db-token, tenants: [database-tenant] }
policy:
  tenant_policies:
    - { tenant: database-tenant, roles: [] }
"#,
        )
        .unwrap();
        let tenants = vec![TenantDecl {
            id: "database-tenant".to_string(),
            external_id: None,
            settings: Default::default(),
        }];

        config
            .validate()
            .expect("pre-read validation must not require a duplicate YAML tenant");
        config
            .validate_with_registry(&tenants, &RoutingSnapshot::default())
            .expect("post-read validation must resolve auth and policy against DB tenants");
    }

    #[test]
    fn relational_snapshot_rejects_a_bearer_token_referencing_a_stale_tenant() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main }
auth:
  bearer_tokens:
    - { token: stale-token, tenants: [stale-tenant] }
"#,
        )
        .unwrap();
        let tenants = vec![TenantDecl {
            id: "database-tenant".to_string(),
            external_id: None,
            settings: Default::default(),
        }];

        config
            .validate()
            .expect("tenant references are deferred until the DB snapshot exists");
        match config.validate_with_registry(&tenants, &RoutingSnapshot::default()) {
            Err(Error::Config(message)) => {
                assert!(message.contains("stale-tenant"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn relational_snapshot_rejects_a_tenant_policy_owned_by_a_stale_tenant() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main }
policy:
  tenant_policies:
    - { tenant: stale-tenant, roles: [] }
"#,
        )
        .unwrap();
        let tenants = vec![TenantDecl {
            id: "database-tenant".to_string(),
            external_id: None,
            settings: Default::default(),
        }];

        config
            .validate()
            .expect("tenant references are deferred until the DB snapshot exists");
        match config.validate_with_registry(&tenants, &RoutingSnapshot::default()) {
            Err(Error::Config(message)) => {
                assert!(message.contains("stale-tenant"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn relational_validation_rejects_a_stale_yaml_tenant_declaration() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: stale-tenant } ]
registry: { backend: relational, storage: main }
"#,
        )
        .unwrap();

        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("tenants"), "message was: {message}");
                assert!(message.contains("relational"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// The exact same referential-integrity bar `AppConfig::validate` holds
    /// a YAML `collections:` list to: a collection referencing a storage the
    /// operator config never declared fails here too.
    #[test]
    fn validate_registry_snapshot_rejects_an_unknown_storage_reference() {
        let config = snapshot_operator_config();
        let snapshot = RoutingSnapshot {
            catalogs: vec![snapshot_catalog("default", "public")],
            collections: vec![snapshot_collection(
                "demo",
                "default",
                "nonexistent-storage",
            )],
        };
        match validate_registry_snapshot(&config, &snapshot_tenants(), &snapshot) {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("nonexistent-storage"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// A catalog referencing a tenant this operator config doesn't declare
    /// fails validation the same way an unresolvable YAML `catalogs:` entry
    /// does (this is a distinct case from `snapshot_from_registry`'s own
    /// "never walked at all" behavior for a tenant absent from `config.
    /// tenants` — this test constructs the snapshot directly, bypassing the
    /// walk, to prove `validate_registry_snapshot` itself still catches an
    /// unresolvable tenant reference if one ever reaches it).
    #[test]
    fn validate_registry_snapshot_rejects_an_unknown_tenant_reference() {
        let config = snapshot_operator_config();
        let snapshot = RoutingSnapshot {
            catalogs: vec![snapshot_catalog("default", "nonexistent-tenant")],
            collections: vec![],
        };
        match validate_registry_snapshot(&config, &snapshot_tenants(), &snapshot) {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("nonexistent-tenant"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// Two collections sharing an internal id fails — the same "internal
    /// ids are globally unique" rule `AppConfig::validate` enforces across
    /// tenants/catalogs/collections applies to a relational snapshot's own
    /// declarations too.
    #[test]
    fn validate_registry_snapshot_rejects_a_duplicate_collection_internal_id() {
        let config = snapshot_operator_config();
        let snapshot = RoutingSnapshot {
            catalogs: vec![snapshot_catalog("default", "public")],
            collections: vec![
                snapshot_collection("demo", "default", "main"),
                snapshot_collection("demo", "default", "main"),
            ],
        };
        match validate_registry_snapshot(&config, &snapshot_tenants(), &snapshot) {
            Err(Error::Config(message)) => {
                assert!(message.contains("duplicate"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// A catalog whose internal id collides with a tenant's is exactly the
    /// cross-kind uniqueness `internal_ids`, seeded from `config.tenants`
    /// before this runs, exists to catch — proving that seeding actually
    /// happens for the snapshot path, not just the YAML one.
    #[test]
    fn validate_registry_snapshot_rejects_an_internal_id_reused_from_a_tenant() {
        let config = snapshot_operator_config();
        let snapshot = RoutingSnapshot {
            // "public" is this config's own tenant internal id.
            catalogs: vec![snapshot_catalog("public", "public")],
            collections: vec![],
        };
        match validate_registry_snapshot(&config, &snapshot_tenants(), &snapshot) {
            Err(Error::Config(message)) => {
                assert!(message.contains("reused"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// `#34`: a policy grant referencing a catalog/collection id this
    /// relational snapshot never published fails eagerly, named, at
    /// snapshot-load time — the symmetric counterpart to
    /// `rejects_a_grant_referencing_an_unknown_collection` below, for the
    /// relational backend.
    #[test]
    fn validate_registry_snapshot_rejects_a_grant_referencing_an_unknown_collection() {
        let mut config = snapshot_operator_config();
        config.policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope {
                        catalogs: vec![],
                        collections: vec!["does-not-exist".to_string()],
                    },
                    lanes: vec![PolicyLane::Features],
                    filter: None,
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let snapshot = RoutingSnapshot {
            catalogs: vec![snapshot_catalog("default", "public")],
            collections: vec![snapshot_collection("demo", "default", "main")],
        };
        match validate_registry_snapshot(&config, &snapshot_tenants(), &snapshot) {
            Err(Error::Config(message)) => {
                assert!(message.contains("does-not-exist"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// A grant referencing a collection the snapshot actually published
    /// passes — the ordinary case, proving the check above isn't simply
    /// always failing.
    #[test]
    fn validate_registry_snapshot_accepts_a_grant_referencing_a_published_collection() {
        let mut config = snapshot_operator_config();
        config.policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope {
                        catalogs: vec![],
                        collections: vec!["demo".to_string()],
                    },
                    lanes: vec![PolicyLane::Features],
                    filter: None,
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        let snapshot = RoutingSnapshot {
            catalogs: vec![snapshot_catalog("default", "public")],
            collections: vec![snapshot_collection("demo", "default", "main")],
        };
        validate_registry_snapshot(&config, &snapshot_tenants(), &snapshot).unwrap();
    }

    /// `#34`'s "lazy registry validation stays coherent" rule: at
    /// `AppConfig::validate()` time (before any relational snapshot is
    /// walked), a `registry.backend: relational` config's own
    /// `catalogs`/`collections` are always empty by construction — so a
    /// policy grant referencing an id that doesn't exist YET (it will only
    /// ever be known once a snapshot is walked) must NOT fail here. This is
    /// the config-time half `AppConfig::validate_policy`'s own doc
    /// describes; `validate_registry_snapshot_rejects_a_grant_referencing_
    /// an_unknown_collection` above is the eager check that actually catches
    /// it, once there is something to check against.
    #[test]
    fn validate_does_not_eagerly_check_grant_refs_for_the_relational_backend() {
        let mut config = snapshot_operator_config();
        config.policy = PolicyConfig {
            roles: vec![RoleDecl {
                name: "reader".to_string(),
                grants: vec![GrantDecl {
                    scope: GrantScope {
                        catalogs: vec![],
                        collections: vec!["not-yet-published".to_string()],
                    },
                    lanes: vec![PolicyLane::Features],
                    filter: None,
                    rate: None,
                }],
            }],
            tenant_policies: vec![],
        };
        config.validate().unwrap();
    }

    /// `#34`: a `shared_with` entry naming a tenant this config never
    /// declares would otherwise just silently never match
    /// (`Subject::is_member_of` can hold no membership in a tenant that
    /// doesn't exist) — caught here as a named boot error instead.
    #[test]
    fn rejects_a_catalog_visibility_shared_with_an_unknown_tenant() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs:
  - id: default
    tenant: public
    visibility: { shared_with: [nonexistent-tenant] }
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("nonexistent-tenant"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// Same check, one level down: a collection's own `visibility.
    /// shared_with` override.
    #[test]
    fn rejects_a_collection_visibility_shared_with_an_unknown_tenant() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    visibility: { shared_with: [nonexistent-tenant] }
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("nonexistent-tenant"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// A `shared_with` entry naming a tenant that IS declared passes — the
    /// ordinary case the two rejection tests above are the negative of.
    #[test]
    fn accepts_a_shared_with_entry_naming_a_real_tenant() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public }, { id: partner } ]
catalogs:
  - id: default
    tenant: public
    visibility: { shared_with: [partner] }
"#,
        )
        .unwrap();
        config.validate().unwrap();
    }

    // -- `#34` policy-layer validation ---------------------------------------

    const POLICY_BASE_CONFIG: &str = r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public }, { id: other } ]
catalogs:
  - { id: default, tenant: public }
  - { id: other-catalog, tenant: other }
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
  - id: other-demo
    catalog: other-catalog
    storage: main
    table: demo
    geometry: geom
    pk: id
"#;

    #[test]
    fn rejects_a_duplicate_platform_role_name() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - {{ name: reader, grants: [] }}\n    - {{ name: reader, grants: [] }}\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("duplicate"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_grant_with_no_lanes() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - {{ scope: {{}}, lanes: [] }}\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("lane"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// `#68`: a grant naming the `write` lane with a `filter` is a
    /// config-load error, named — row-level write conditions are out of
    /// scope until a real caller needs them (the enforced-or-refused
    /// principle: a filtered write grant must be refused at boot, never
    /// silently widened to unfiltered or silently ignored).
    #[test]
    fn rejects_a_write_grant_with_a_filter() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: writer\n      grants:\n        - scope: {{}}\n          lanes: [write]\n          filter: \"org = 'acme'\"\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("write"), "message was: {message}");
                assert!(message.contains("filter"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// A grant naming `write` alongside a read lane, with no filter, is
    /// unaffected by the check above — only the filter+write combination is
    /// rejected, never `write` itself.
    #[test]
    fn accepts_an_unfiltered_write_grant() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: writer\n      grants:\n        - scope: {{}}\n          lanes: [write]\n"
        ))
        .unwrap();
        config.validate().unwrap();
    }

    /// `#115`: the change-feed lane's own filter-carrying-grant refusal —
    /// same shape and same reason as `rejects_a_write_grant_with_a_filter`
    /// above (the feed serves compact envelopes, never a payload a filter
    /// could narrow).
    #[test]
    fn rejects_a_feed_grant_with_a_filter() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - scope: {{}}\n          lanes: [feed]\n          filter: \"org = 'acme'\"\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("feed"), "message was: {message}");
                assert!(message.contains("filter"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn accepts_an_unfiltered_feed_grant() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - scope: {{}}\n          lanes: [feed]\n"
        ))
        .unwrap();
        config.validate().unwrap();
    }

    // -- `#188`: rate conditions on a grant --------------------------------

    const RATE_YAML: &str =
        "          rate:\n            scope: principal\n            window_seconds: 60\n            ceiling: 100\n            on_counter_unavailable: strict\n";

    #[test]
    fn accepts_a_well_formed_rate_condition_on_a_wired_lane() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - scope: {{}}\n          lanes: [features, stac, write, feed]\n{RATE_YAML}"
        ))
        .unwrap();
        config.validate().unwrap();
        let declared = config.policy.roles[0].grants[0]
            .rate
            .as_ref()
            .expect("the rate block must round-trip through serde");
        assert_eq!(declared.scope, crate::rate_limit::RateScope::Principal);
        assert_eq!(declared.ceiling, 100);
        assert_eq!(
            declared.on_counter_unavailable,
            crate::rate_limit::CounterPosture::Strict
        );
    }

    /// The absent case stays exactly what it was before `#188`: no ceiling,
    /// no shape to validate, nothing to charge.
    #[test]
    fn a_grant_without_a_rate_block_declares_no_ceiling() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - scope: {{}}\n          lanes: [features]\n"
        ))
        .unwrap();
        config.validate().unwrap();
        assert!(config.policy.roles[0].grants[0].rate.is_none());
    }

    /// Every field of a rate block is required — a half-declared ceiling
    /// would have to invent the other half, which is a policy decision this
    /// crate has no standing to make (see `RateLimitDecl`'s own doc).
    #[test]
    fn a_rate_block_missing_its_failure_posture_does_not_even_parse() {
        let result: std::result::Result<AppConfig, _> = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - scope: {{}}\n          lanes: [features]\n          rate:\n            scope: principal\n            window_seconds: 60\n            ceiling: 100\n"
        ));
        let err = result.expect_err("a rate block with no declared posture must not parse");
        assert!(
            err.to_string().contains("on_counter_unavailable"),
            "message was: {err}"
        );
    }

    #[test]
    fn rejects_a_rate_condition_with_a_zero_ceiling() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - scope: {{}}\n          lanes: [features]\n          rate:\n            scope: principal\n            window_seconds: 60\n            ceiling: 0\n            on_counter_unavailable: strict\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("ceiling"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// `#188`: the tiles and 3D-places checkpoints do not charge ceilings in
    /// this build, so a ceiling declared for them is refused by name rather
    /// than accepted and quietly never enforced.
    #[test]
    fn rejects_a_rate_condition_on_a_lane_whose_checkpoint_does_not_charge_it() {
        for lane in ["tiles", "places3d"] {
            let config: AppConfig = serde_yaml::from_str(&format!(
                "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - scope: {{}}\n          lanes: [{lane}]\n{RATE_YAML}"
            ))
            .unwrap();
            match config.validate() {
                Err(Error::Config(message)) => {
                    assert!(message.contains(lane), "message was: {message}");
                    assert!(message.contains("rate"), "message was: {message}");
                }
                other => panic!("expected Err(Config(_)) for lane '{lane}', got {other:?}"),
            }
        }
    }

    /// The refusal is about the ceiling, not about the lane: those same
    /// lanes keep working exactly as before for a grant that declares none.
    #[test]
    fn accepts_a_tiles_grant_that_declares_no_rate_condition() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - scope: {{}}\n          lanes: [tiles, places3d]\n"
        ))
        .unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn rejects_a_rate_condition_on_a_tenant_custom_role_too() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  tenant_policies:\n    - tenant: public\n      roles:\n        - name: reader\n          grants:\n            - scope: {{}}\n              lanes: [features]\n              rate:\n                scope: principal\n                window_seconds: 0\n                ceiling: 100\n                on_counter_unavailable: graceful\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("window_seconds"), "message was: {message}");
                assert!(
                    message.contains("tenant_policies"),
                    "the message must point at the declaration: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_grant_referencing_an_unknown_collection() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - scope: {{ collections: [does-not-exist] }}\n          lanes: [features]\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("does-not-exist"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_grant_filter_template_that_is_not_valid_cql2_once_substituted() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - scope: {{}}\n          lanes: [features]\n          filter: \"org = \"\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("filter template"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn accepts_a_grant_filter_template_with_a_claim_placeholder() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  roles:\n    - name: reader\n      grants:\n        - scope: {{}}\n          lanes: [features]\n          filter: \"org = {{{{claims.org}}}}\"\n"
        ))
        .unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn a_tenant_custom_policy_referencing_a_different_tenants_catalog_is_rejected() {
        // `other-catalog` belongs to tenant `other`, not `public` — a
        // `public`-scoped tenant-custom document naming it would widen
        // `public`'s own policy into another tenant's resources.
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  tenant_policies:\n    - tenant: public\n      roles:\n        - name: reader\n          grants:\n            - scope: {{ catalogs: [other-catalog] }}\n              lanes: [features]\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("widen"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn a_tenant_custom_policy_referencing_its_own_tenants_catalog_is_accepted() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  tenant_policies:\n    - tenant: public\n      roles:\n        - name: reader\n          grants:\n            - scope: {{ catalogs: [default] }}\n              lanes: [features]\n"
        ))
        .unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn rejects_more_than_one_tenant_custom_document_for_the_same_tenant() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\npolicy:\n  tenant_policies:\n    - {{ tenant: public, roles: [] }}\n    - {{ tenant: public, roles: [] }}\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("more than one"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn a_bearer_token_role_for_a_tenant_it_is_not_a_member_of_is_rejected() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public }, { id: other } ]
auth:
  bearer_tokens:
    - token: t
      tenants: [public]
      roles:
        other: [reader]
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("other"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn a_bearer_token_role_for_a_tenant_it_is_a_member_of_is_accepted() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
auth:
  bearer_tokens:
    - token: t
      tenants: [public]
      roles:
        public: [reader]
      claims:
        org: acme
"#,
        )
        .unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn declaring_an_unknown_final_key_name_is_refused() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
settings:
  final: [not_a_real_settings_key]
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("not_a_real_settings_key"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn declaring_the_same_final_key_twice_at_one_level_is_refused() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
settings:
  final: [cache_ttl_s, cache_ttl_s]
"#,
        )
        .unwrap();
        assert!(matches!(config.validate(), Err(Error::Config(_))));
    }

    #[test]
    fn a_profile_declaring_final_keys_is_refused() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
profiles:
  - id: bad
    final: [cache_ttl_s]
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("bad"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// Declaring `final` at the collection level (the bottom of the chain)
    /// is accepted — it just has no effect, since nothing sits below a
    /// collection to enforce it against.
    #[test]
    fn a_collection_declaring_final_keys_of_its_own_is_accepted_as_a_no_op() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    settings:
      cache_ttl_s: 10
      final: [cache_ttl_s]
"#,
        )
        .unwrap();
        config.validate().unwrap();
    }

    /// `#110`, relational registry path: the same refusal, walked from a
    /// `RoutingSnapshot` rather than the YAML document's own
    /// `catalogs`/`collections` — proves `validate_registry_snapshot` holds
    /// the relational backend to the identical finality bar.
    #[test]
    fn validate_registry_snapshot_also_refuses_a_final_key_override() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main }
settings:
  tile_vertex_budget: 500000
  final: [tile_vertex_budget]
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let snapshot = RoutingSnapshot {
            catalogs: vec![CatalogDecl {
                id: "default".to_string(),
                external_id: None,
                tenant: "public".to_string(),
                settings: SettingsDecl::default(),
                visibility: VisibilityDecl::default(),
            }],
            collections: vec![serde_yaml::from_str(
                "id: demo
catalog: default
storage: main
table: demo
geometry: geom
pk: id
settings: { tile_vertex_budget: 1 }
",
            )
            .unwrap()],
        };

        match validate_registry_snapshot(&config, &snapshot_tenants(), &snapshot) {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("tile_vertex_budget"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    // ---- `admission` under `final:` (`#156`) -------------------------------
    //
    // `admission` is in the `SETTINGS_KEY_NAMES` vocabulary like every
    // other settings key, but the chain it governs is shorter: a catalog-
    // or collection-level `admission` is refused unconditionally by
    // `validate_settings` (admission runs before routing resolves either),
    // so the ONLY level a platform-level `final: [admission]` can actually
    // close is the tenant level. These tests are written against exactly
    // that level for that reason — a catalog-level version would pass
    // whether or not `admission` is in the vocabulary at all, and so would
    // prove nothing.

    /// The decisive case: the platform pins admission, a tenant tries to
    /// raise its own queue capacity and fair-share weight, and the load is
    /// refused BY NAME — naming the key, the level that declared it final,
    /// and the level that tried to override it.
    #[test]
    fn a_tenant_admission_override_under_a_platform_final_admission_is_refused_by_name() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
settings:
  admission: { queue_capacity: 32, weight: 1 }
  final: [admission]
tenants:
  - id: acme
    settings:
      admission: { queue_capacity: 100000, weight: 64 }
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("admission"), "message was: {message}");
                assert!(message.contains("the platform"), "message was: {message}");
                assert!(message.contains("tenant 'acme'"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// The same refusal over the relational registry path's own walk of the
    /// tenant snapshot — `validate_registry_snapshot` holds a
    /// database-supplied tenant to the identical bar.
    #[test]
    fn validate_registry_snapshot_also_refuses_a_tenant_admission_override() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main }
settings:
  admission: { queue_capacity: 32 }
  final: [admission]
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let tenants: Vec<TenantDecl> = vec![serde_yaml::from_str(
            "id: acme
settings: { admission: { queue_capacity: 100000 } }
",
        )
        .unwrap()];

        match validate_registry_snapshot(&config, &tenants, &RoutingSnapshot::default()) {
            Err(Error::Config(message)) => {
                assert!(message.contains("admission"), "message was: {message}");
                assert!(message.contains("tenant 'acme'"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// No invented governance: the identical tenant override loads fine
    /// when the platform never names `admission` in `final:`. Adding the
    /// name to the vocabulary changes nothing for a document that does not
    /// use it.
    #[test]
    fn a_tenant_admission_override_is_still_accepted_when_nothing_declares_it_final() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
settings:
  admission: { queue_capacity: 32, weight: 1 }
tenants:
  - id: acme
    settings:
      admission: { queue_capacity: 100000, weight: 64 }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.tenants[0]
                .settings
                .admission
                .as_ref()
                .and_then(|a| a.queue_capacity),
            Some(100_000)
        );
    }

    /// A `final: [admission]` closes `admission` and nothing else — a
    /// tenant may still override any key the platform did not pin.
    #[test]
    fn a_platform_final_admission_does_not_close_any_other_settings_key() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
settings:
  admission: { queue_capacity: 32 }
  cache_ttl_s: 60
  final: [admission]
tenants:
  - id: acme
    settings:
      cache_ttl_s: 5
"#,
        )
        .unwrap();
        config.validate().unwrap();
    }

    /// `admission` is a legal `final:` name at the tenant level too, and is
    /// accepted there — inert, exactly as a collection-level `final:` is
    /// (the tenant level is the bottom of `admission`'s own chain, since
    /// the catalog and collection levels cannot carry an `admission`
    /// override for it to bite on in the first place). Refusing it would
    /// make `admission` the one key with its own `final:` shape rules.
    #[test]
    fn declaring_admission_final_at_the_tenant_level_is_accepted_as_a_no_op() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants:
  - id: acme
    settings:
      admission: { queue_capacity: 100 }
      final: [admission]
catalogs: [ { id: default, tenant: acme } ]
"#,
        )
        .unwrap();
        config.validate().unwrap();
    }

    /// The standing refusal `final:` builds on: a catalog-level `admission`
    /// is refused whether or not any ancestor declared it final. Pinned
    /// here so the decisive test above is never quietly weakened into this
    /// one, which would pass with `admission` absent from the vocabulary.
    #[test]
    fn a_catalog_admission_declaration_is_refused_with_or_without_a_final_declaration() {
        for platform_final in ["  final: [admission]\n", ""] {
            let config: AppConfig = serde_yaml::from_str(&format!(
                "storages: [ {{ id: main, driver: postgis, url_env: DATABASE_URL }} ]\n\
                 settings:\n  admission: {{ queue_capacity: 32 }}\n{platform_final}\
                 tenants: [ {{ id: acme }} ]\n\
                 catalogs:\n  - id: default\n    tenant: acme\n    settings:\n      admission: {{ queue_capacity: 9 }}\n"
            ))
            .unwrap();
            match config.validate() {
                Err(Error::Config(message)) => {
                    assert!(
                        message.contains("only honored at the platform or tenant level"),
                        "message was: {message}"
                    );
                }
                other => panic!("expected Err(Config(_)), got {other:?}"),
            }
        }
    }

    /// Finality's walk consults a level's `profile:` reference, but no
    /// profile can ever supply `admission` — so for this key that branch is
    /// unreachable rather than a second way past a `final` declaration.
    /// Pinned here because adding `admission` to the vocabulary would be
    /// the change that opened such a path if the profile refusal ever
    /// lapsed.
    #[test]
    fn a_profile_cannot_supply_admission_so_finality_needs_no_profile_path_for_it() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
profiles:
  - id: greedy
    admission: { queue_capacity: 100000 }
settings:
  admission: { queue_capacity: 32 }
  final: [admission]
tenants:
  - id: acme
    settings:
      profile: greedy
"#,
        )
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("profile 'greedy'")
                        && message.contains("only honored at the platform or tenant level"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    // ---- webhooks (`#115`) -------------------------------------------------

    #[test]
    fn accepts_a_well_formed_webhook_subscription() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\nwebhooks:\n  - id: alerts\n    url: https://example.test/hook\n    scope: {{ collections: [demo] }}\n    secret_env: ALERTS_WEBHOOK_SECRET\n"
        ))
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.webhooks.len(), 1);
        assert!(config.webhooks[0].enabled, "enabled should default to true");
    }

    #[test]
    fn rejects_an_impossible_dead_letter_page_size_policy() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\nserver:\n  webhook_delivery:\n    dead_letter_default_page_size: 11\n    dead_letter_max_page_size: 10\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("dead_letter_default_page_size"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_an_empty_webhook_id() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\nwebhooks:\n  - id: \"\"\n    url: https://example.test/hook\n    secret_env: SECRET\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("empty"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_duplicate_webhook_id() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\nwebhooks:\n  - id: alerts\n    url: https://example.test/a\n    secret_env: A\n  - id: alerts\n    url: https://example.test/b\n    secret_env: B\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("duplicate"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_non_absolute_webhook_url() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\nwebhooks:\n  - id: alerts\n    url: not-a-url\n    secret_env: SECRET\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("alerts"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_non_http_webhook_url_scheme() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\nwebhooks:\n  - id: alerts\n    url: ftp://example.test/hook\n    secret_env: SECRET\n"
        ))
        .unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_an_empty_webhook_secret_env() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\nwebhooks:\n  - id: alerts\n    url: https://example.test/hook\n    secret_env: \"\"\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("secret_env"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_webhook_scope_referencing_an_unknown_collection() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\nwebhooks:\n  - id: alerts\n    url: https://example.test/hook\n    scope: {{ collections: [does-not-exist] }}\n    secret_env: SECRET\n"
        ))
        .unwrap();
        match config.validate() {
            Err(Error::Config(message)) => {
                assert!(message.contains("does-not-exist"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[test]
    fn a_disabled_webhook_subscription_still_parses_and_validates() {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "{POLICY_BASE_CONFIG}\nwebhooks:\n  - id: alerts\n    url: https://example.test/hook\n    secret_env: SECRET\n    enabled: false\n"
        ))
        .unwrap();
        config.validate().unwrap();
        assert!(!config.webhooks[0].enabled);
    }

    // -- collection kind + the records exposure key (`#192`) -----------------

    /// The backwards-compatibility guarantee, stated as a test: a collection
    /// written before `kind` existed still parses, and parses as `vector` —
    /// which is exactly how it already behaved.
    #[test]
    fn a_collection_written_before_kind_existed_parses_as_vector() {
        let decl: CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap();
        assert_eq!(decl.kind, CollectionKind::Vector);
    }

    #[test]
    fn every_kind_in_the_vocabulary_round_trips_through_yaml() {
        for (written, expected) in [
            ("vector", CollectionKind::Vector),
            ("raster", CollectionKind::Raster),
            ("record", CollectionKind::Record),
        ] {
            let decl: CollectionDecl = serde_yaml::from_str(&format!(
                "id: demo\ncatalog: default\nstorage: main\nkind: {written}\n"
            ))
            .unwrap();
            assert_eq!(decl.kind, expected, "kind: {written}");
        }
    }

    #[test]
    fn an_unknown_kind_is_refused_rather_than_defaulted() {
        assert!(serde_yaml::from_str::<CollectionDecl>(
            "id: demo\ncatalog: default\nstorage: main\nkind: thesaurus\n"
        )
        .is_err());
    }

    /// The exposure asymmetry, pinned. Five roots default to `enabled`
    /// because that is what they already were when `#185` gave operators a
    /// key to turn them off; `records` (`#192`) and `processes` (`#182`)
    /// default to `disabled` because what a deployment predating each already
    /// did is *not serve it at all*. Deriving `Default` on `ProtocolsConf`
    /// would silently break this.
    #[test]
    fn only_the_opt_in_roots_are_disabled_by_default() {
        let matrix = ProtocolsConf::default();
        assert!(matrix.features.is_enabled());
        assert!(matrix.features_write.is_enabled());
        assert!(matrix.tiles.is_enabled());
        assert!(matrix.styles.is_enabled());
        assert!(matrix.three_d_tiles.is_enabled());
        assert!(matrix.stac.is_enabled());
        assert!(
            !matrix.records.is_enabled(),
            "a deployment that never asked for the records lane must not be served it"
        );
        assert!(
            !matrix.processes.is_enabled(),
            "a deployment that never asked for the processes lane must not be served it"
        );
    }

    /// A `protocols:` block written before `records`/`processes` existed keeps
    /// every decision it stated and leaves both new keys at their own default
    /// — the whole-value-replacement rule this block already follows must not
    /// accidentally turn a new root on.
    #[test]
    fn a_protocols_block_written_before_the_opt_in_roots_existed_leaves_them_disabled() {
        let matrix: ProtocolsConf = serde_yaml::from_str("tiles: disabled\n").unwrap();
        assert!(!matrix.tiles.is_enabled());
        assert!(matrix.features.is_enabled());
        assert!(!matrix.records.is_enabled());
        assert!(!matrix.processes.is_enabled());
    }

    #[test]
    fn an_operator_can_ask_for_the_records_root_by_name() {
        let matrix: ProtocolsConf = serde_yaml::from_str("records: enabled\n").unwrap();
        assert!(matrix.records.is_enabled());
        // ... without that changing anything else.
        assert!(matrix.features.is_enabled());
        assert!(matrix.stac.is_enabled());
        assert!(!matrix.processes.is_enabled());
    }

    #[test]
    fn an_operator_can_ask_for_the_processes_root_by_name() {
        let matrix: ProtocolsConf = serde_yaml::from_str("processes: enabled\n").unwrap();
        assert!(matrix.processes.is_enabled());
        assert!(matrix.features.is_enabled());
        assert!(!matrix.records.is_enabled());
    }

    // -- the durable job ledger's config (`#182`) ----------------------------

    /// The backwards-compatibility guarantee for `server.processes`: a config
    /// written before it existed parses, and parses as "no ledger" — which is
    /// what makes the Processes root absent rather than broken.
    #[test]
    fn a_server_block_written_before_processes_existed_declares_no_ledger() {
        let server: ServerConfig = serde_yaml::from_str("port: 8080\n").unwrap();
        assert!(server.processes.is_none());
        assert!(ServerConfig::default().processes.is_none());
    }

    /// `storage` has no default, deliberately: guessing a location for a
    /// deployment's durable state is exactly the invented default this
    /// codebase refuses. A block that omits it does not parse at all.
    #[test]
    fn the_ledger_storage_has_no_default_to_guess() {
        assert!(serde_yaml::from_str::<ProcessesConfig>("poll_interval_ms: 500\n").is_err());
        let processes: ProcessesConfig = serde_yaml::from_str("storage: pg\n").unwrap();
        assert_eq!(processes.storage, "pg");
        // The two operational knobs DO have defaults — they are defaults for a
        // feature the operator has just explicitly asked for, not for one they
        // never mentioned.
        assert_eq!(processes.poll_interval_ms, 1_000);
        assert_eq!(processes.visibility_timeout_s, 300);
    }

    /// A ledger pointed at a storage that does not exist is refused at load,
    /// by name — the same referential-integrity treatment a `routing.<lane>`
    /// reference already gets, rather than a boot-time surprise.
    #[test]
    fn a_ledger_naming_an_undeclared_storage_is_refused_at_load() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
server: { processes: { storage: nope } }
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
"#,
        )
        .unwrap();
        let error = config
            .validate()
            .expect_err("an unknown storage is refused");
        assert!(
            error.to_string().contains("server.processes.storage")
                && error.to_string().contains("nope"),
            "the refusal must name the key and the value: {error}"
        );
    }

    #[test]
    fn a_ledger_naming_a_declared_storage_loads() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
server: { processes: { storage: main } }
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
"#,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(
            config.server.processes.as_ref().unwrap().storage.as_str(),
            "main"
        );
    }

    /// A zero poll interval would spin, and a zero visibility timeout would
    /// make every claim instantly re-claimable — both are refused rather than
    /// silently clamped to something the operator did not write.
    #[test]
    fn degenerate_ledger_intervals_are_refused_rather_than_clamped() {
        for block in [
            "{ storage: main, poll_interval_ms: 0 }",
            "{ storage: main, visibility_timeout_s: 0 }",
        ] {
            let config: AppConfig = serde_yaml::from_str(&format!(
                r#"
server: {{ processes: {block} }}
storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
"#
            ))
            .unwrap();
            assert!(config.validate().is_err(), "{block} must be refused");
        }
    }
}
