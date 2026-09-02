//! Tellurion core: configuration model, `ConfigStore`, `StyleStore`, storage
//! capability traits (`CatalogSource`, `FeatureSource`, `TileSource`,
//! `VolumeSource`), derived collection descriptors, the `Router`, the
//! `Resolver` (external id -> internal id, `#39`), settings inheritance, the
//! tile cache, shared error types, and `AppContext`. This crate defines
//! traits only — no concrete database dependency, ever.

pub mod admission;
pub mod applier;
pub mod asset;
pub mod audit;
pub mod auth;
pub mod batch;
pub mod bootstrap;
pub mod cache;
#[cfg(feature = "valkey")]
pub mod cache_l2_valkey;
pub mod catalog;
mod cgroup_v1;
mod cgroup_v2;
pub mod config;
pub mod config_store;
pub mod context;
pub mod control_admin_path;
pub mod control_admin_policy;
pub mod control_model;
pub mod control_path;
pub mod control_policy;
pub mod control_runtime;
pub mod control_store;
pub mod crs;
pub mod descriptor;
pub mod error;
pub mod extension;
pub mod feed;
pub mod filter;
pub mod hint;
pub mod identity;
pub mod invalidation;
pub mod items_budget;
pub mod job;
pub mod lease;
pub mod links;
pub mod locking;
pub mod objectstore;
pub mod observability;
pub mod outbox;
pub mod page_bytes;
pub mod policy;
pub mod problem;
pub mod process;
pub mod query_params;
pub mod rate_limit;
pub mod reconcile;
pub mod registry;
pub mod resolver;
pub mod resources;
pub mod retention;
pub mod router;
pub mod settings;
mod sigv4;
pub mod stac_sidecar;
pub mod storage;
pub mod style_store;
pub mod tenant;
pub mod tile_budget;
pub mod timefmt;
pub mod tms;
pub mod webhooks;

pub use admission::{
    AdmissionConfig, AdmissionDecl, AdmissionOutcome, AdmissionPermit, AdmissionRegistry,
    AdmissionRejection, DEFAULT_ADMISSION_QUEUE_CAPACITY, DEFAULT_ADMISSION_QUEUE_DEADLINE_MS,
    DEFAULT_ADMISSION_WEIGHT,
};
pub use applier::{drain_once, run_applier};
/// Test-only fakes (`asset.rs`/`objectstore.rs`'s own `test-support`
/// feature): never linked into a production build, but re-exported at the
/// crate root so another crate's own test suite can drive the real
/// `Router`/handlers against them without a live database or filesystem —
/// see `tellurion-stac::tests::asset_handlers`.
#[cfg(feature = "test-support")]
pub use asset::InMemoryAssetRecordStore;
pub use asset::{
    abandon_resumable_upload, append_resumable_upload, complete_resumable_upload, complete_upload,
    compute_sha256, create_resumable_upload, decode_base64, delete_asset, encode_base64,
    finalize_presigned_upload, parse_repr_digest, presign_upload, register_managed,
    register_remote, resumable_upload_offset, verify_digest, AssetKind, AssetPolicy, AssetRecord,
    AssetRecordEntry, AssetRecordStore, AssetState, Digest, FinalizeOutcome, NewAssetKind,
    NewAssetRecord, RegisterManagedRequest, RegisterRemoteRequest,
};
pub use audit::{AuditRecord, ConfigAuditLog};
pub use auth::{
    build_authorizer, build_authorizer_with_bindings, resolve_bearer_credentials,
    resolve_bearer_credentials_from, AuthDecision, Credential, DenyReason, PlatformAdminDecision,
    ResolvedBearerCredentials, StaticBearerAuthorizer, Subject, TenantAuthorizer,
};
pub use batch::{
    stage_batch_feature, validate_geojson_bbox, BatchConfig, BatchDecl, BatchOutcomeLine,
    BatchSummary, BatchTerminalCondition, GeoJsonSequenceDecoder, GeoJsonSequenceItem,
    DEFAULT_BATCH_CHUNK_ITEMS, DEFAULT_BATCH_MAX_BYTES, DEFAULT_BATCH_MAX_ITEMS,
    GEO_JSON_RECORD_SEPARATOR,
};
#[cfg(any(test, feature = "test-support"))]
pub use bootstrap::assert_control_bootstrap_contract;
pub use bootstrap::{
    diff_control_snapshots, export_control_snapshot, initialize_control_store,
    initialize_control_store_with_mode, migrate_control_store, migrate_control_store_with_mode,
    BootEnvelope, ControlMigrationPlan, ControlStartup, ControlStoreLocator, SeedStatus,
};
pub use cache::{
    Encoding, L2Cache, L2CacheAdapter, L2Tier, L2TierState, LayeredCache, MapCrs, MapLane,
    MetricsTileCache, MokaTileCache, PopulateFuture, TileCache, TileKey,
};
#[cfg(feature = "valkey")]
pub use cache_l2_valkey::ValkeyL2Cache;
pub use catalog::{
    AttributeColumn, CatalogSource, FeatureSizeStats, GeometryProfile, PhysicalCollection,
    ProjectionFacts, SpatialExtent, VertexStats,
};
pub use config::{
    validate_registry_snapshot, AppConfig, AssetDecl, AuthConfig, BearerTokenDecl, CacheConfig,
    CatalogDecl, ChangeFeedConfig, CollectionDecl, CollectionKind, ColorRamp, ColormapConf,
    ColormapStop, ContactDecl, ControlBrowserAuthConfig, GeometryVariantDecl, GrantDecl,
    GrantScope, IdType, IndexApplierConfig, L2CacheConfig, LaneRouting, LeaseDecl, LineageDecl,
    LineageProcessStepDecl, LineageSourceDecl, ObjectStoreDecl, ObjectStoreProfile,
    OutboxRetentionConfig, Places3dConf, PolicyConfig, PolicyLane, ProcessesConfig, ProfileDecl,
    PropertyDecl, PropertyType, ProtocolExposure, ProtocolsConf, RegistryBackend, RegistryConfig,
    RegistryValidationMode, RoleDecl, RoutingDecl, RoutingSnapshot, SchemaDecl, SearchConf,
    ServerConfig, ServiceAssetsMode, SettingsDecl, StacConf, StacProvider, StorageDecl, StyleConf,
    StyleRef, TenantDecl, TenantPolicyDecl, TileInvalidationConfig, TilesConf, VisibilityDecl,
    WebhookDeliveryConfig, WebhookSubscriptionDecl, ZoomCaps, DEFAULT_DESCRIPTOR_CACHE_CAPACITY,
    DEFAULT_DESCRIPTOR_TTL_S, DEFAULT_TILE_CAP, RESERVED_TENANT_SEGMENTS, SETTINGS_KEY_NAMES,
};
pub use config_store::{ConfigStore, ConfigVersion, FileConfigStore, VersionedConfig};
pub use context::{build_router_and_resolver, mvt_key, AppContext, ContextState, MvtFetch};
pub use control_admin_path::{
    canonicalize_control_path, CanonicalControlPath, CompiledPathPattern, ControlPathError,
};
pub use control_admin_policy::{
    authorize_control, authorize_control_canonical, authorize_control_mutation, explain_control,
    explain_control_canonical, role_binding_target_id, validate_delegated_policy,
    validate_delegated_role_binding, AuthorizedControlMutation,
    ControlDecision as MutationControlDecision, ControlEvaluation, ControlMiddlewareError,
    ControlRequestContext as MutationControlRequestContext, ControlRouteDescriptor,
    ControlRouteRegistry, DelegationError, ValidatedControlSnapshot,
};
pub use control_model::{
    apply_control_changes, preview_control_changes, validate_control_event_page,
    AppliedControlChangeSet, AuditRequestContext, BootstrapOutcome, ControlChangeSet,
    ControlCommit, ControlEvent, ControlEventCursor, ControlOperation, ControlPreview,
    ControlRevision, ControlScope, ControlSnapshot, PathPolicy, PolicyCondition, PolicyEffect,
    PrincipalIdentity, RoleBinding, VersionedControlOperation, VersionedControlSnapshot,
};
pub use control_path::{decoded_segments, PathPattern};
pub use control_policy::{
    AdminResource, ControlDecision, ControlDecisionContext, ControlPolicySet,
    ControlRequestContext, DecisionBasis,
};
pub use control_runtime::{ControlRuntimeSnapshot, ControlRuntimeStatus};
#[cfg(any(test, feature = "test-support"))]
pub use control_store::{assert_control_store_contract, InMemoryControlStore};
pub use control_store::{
    validate_control_bootstrap_seed, ControlAuditRecord, ControlBootstrapMode, ControlStore,
};
pub use crs::{
    advertised_crs, content_crs_uri, epsg_uri, is_lat_lon_order, parse_content_crs_header,
    resolve as resolve_crs, supported_crs, swap_bbox_axes, RequestedCrs, CRS84_URI,
};
pub use descriptor::canonical::{
    CanonicalCapabilities, CanonicalDescriptor, CanonicalField, CanonicalProperty, CanonicalSchema,
    CanonicalStac, Provenance,
};
pub use descriptor::heuristics;
pub use descriptor::CollectionDescriptor;
pub use error::{Error, Result, StorageError};
pub use extension::NamedRegistry;
pub use feed::{
    build_page, decode_cursor, encode_cursor, FeedEntry, FeedOperation, FeedPage,
    FEED_ENVELOPE_SCHEMA_VERSION,
};
pub use filter::{
    CaseInsensitiveCompareOp, CompareOp, Filter, GeometryLiteral, Literal, SpatialOp, TemporalOp,
    TemporalValue, WktGeometry, CQL2_CONFORMANCE_CLASSES,
};
pub use hint::{Hint, Hints, READ_SOURCE_HEADER};
pub use identity::{AuthenticatedSubject, IdentityError, TrustedIssuerSet};
pub use invalidation::{drain_once_for_generations, run_generation_consumer, GenerationStore};
/// Test-only fake (`job.rs`'s `test-support` feature): never linked into a
/// production build, but re-exported at the crate root so `tellurion-processes`'
/// own test suite can drive the real handlers against a ledger without a live
/// database — see that type's own doc for the invariants it reproduces from
/// the real table, and the ones it deliberately cannot.
#[cfg(feature = "test-support")]
pub use job::InMemoryJobStore;
pub use job::{JobLedger, JobOutcome, JobRecord, JobScope, JobStatus, JobStore, JobSubmission};
pub use lease::{Lease, LeaseBinding, LeaseGuard, LeaseHold, LeaseKey, INDEX_APPLIER_CONSUMER};
pub use links::{ContributedLink, LinkAnchor, LinkContributor, LinkContributors, ResourceRef};
pub use locking::{
    compute_feature_etag, format_http_date, if_match_satisfied, is_unmodified_since,
    parse_http_date, parse_stored_timestamp, RowVersion, LOCKING_CONFORMANCE_CLASSES,
    OPTIMISTIC_LOCKING_ETAGS_CLASS, OPTIMISTIC_LOCKING_TIMESTAMPS_CLASS,
};
#[cfg(feature = "test-support")]
pub use objectstore::InMemoryObjectStore;
pub use objectstore::{
    FsObjectStore, ListableObjectStore, ListedObject, ObjectKey, ObjectMetadata, ObjectStore,
    ObjectStoreError, PathAddressedObjectStore, PresignedObjectStore, ResumableUploadStore,
    S3ObjectStore,
};
pub use outbox::{
    BatchItemOutcome, BatchItemResult, IndexSink, Mutation, MutationKind, Obligation,
    ObligationExtent, OutboxSource, SearchPage, SearchQuery, SearchSource, Sequence, WriteSink,
    FEATURES_PART4_FEATURES_CLASS,
};
pub use policy::{authorize_resource, enforce_rate_limits, PolicyDecision, ResourceContext};
pub use problem::{Problem, PROBLEM_JSON};
pub use process::{
    JobControlOption, ProcessDescription, ProcessLane, ProcessRegistry, ProcessRunner,
    ProcessTarget,
};
pub use rate_limit::{
    CounterKey, CounterPosture, CounterUnavailable, InProcessRateCounter, RateCharge, RateCounter,
    RateLimitDecl, RateObservation, RateRefusal, RateRefusalCause, RateScope, RateVerdict,
    DEFAULT_RATE_COUNTER_KEY_CAPACITY, MAX_RATE_WINDOW_SECONDS,
};
pub use reconcile::{reconcile, BrokenAsset, OrphanedObject, ReconcileReport};
pub use registry::{
    build_registry_reader, snapshot_from_registry, snapshot_from_registry_with_page_size,
    FileRegistryReader, Page, PageRequest, RegistryReader, RelationalRegistryFactories,
    RelationalRegistryFactory,
};
pub use resolver::{Resolver, StaticResolver};
pub use resources::effective_cpu_count;
pub use retention::{compute_floor, ConsumerLag, RetentionFloor};
pub use router::{DriverFactory, Registry, Router, SearchResolution, ServedSource, StorageDriver};
pub use settings::{
    resolve_effective_settings, resolve_effective_settings_with_provenance, EffectiveSettings,
    EffectiveSettingsProvenance, SettingsLevel, SettingsProvenance, DEFAULT_ITEMS_VERTEX_BUDGET,
    DEFAULT_MAX_ASSET_BYTES, DEFAULT_MAX_REQUEST_BODY_BYTES, DEFAULT_SETTINGS_CACHE_TTL_S,
    DEFAULT_TILE_VERTEX_BUDGET,
};
pub use stac_sidecar::StacMetadataSource;
pub use storage::{
    advertised_vector_layers, is_volume_capable_geometry_type, DatetimeRange, FeaturePage,
    FeatureSource, ItemsQuery, RasterSource, RasterWindow, TileCoord, TileSource, VolumeMesh,
    VolumeSource,
};
pub use style_store::{FileStyleStore, StyleStore};
pub use tenant::{
    build_tenant_reader, snapshot_tenants, validate_tenant_snapshot, FileTenantReader,
    RelationalTenantFactories, RelationalTenantFactory, TenantReader,
};
pub use tile_budget::{
    decide_tile_path, TileSimplificationPath, VertexBudget, VERTEX_BUDGET_RETRY_TOLERANCE_FACTOR,
};
pub use timefmt::{format_rfc3339_millis, parse_utc_datetime_text};
pub use tms::{world_crs84_tile_bounds_deg, TileMatrixSet};
pub use webhooks::{
    backoff_delay, hmac_sha256_hex, run_webhook_consumer, DeadLetterEntry, ReqwestDeliverer,
    WebhookConsumerSettings, WebhookDeliverer, WebhookRetryPolicy, WebhookSubscriptionRuntime,
    SIGNATURE_HEADER,
};
