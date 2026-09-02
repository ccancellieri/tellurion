//! Workspace-wide error type. Every crate that returns a fallible result across
//! a crate boundary uses this enum so protocol handlers can map errors to HTTP
//! status without knowing which driver produced them.

/// Boxed so `tellurion-core` never names a concrete driver error type
/// (drivers live in crates core does not depend on).
pub type StorageError = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found")]
    NotFound,

    #[error("collection '{collection}' does not support capability '{capability}'")]
    CapabilityUnsupported {
        collection: String,
        capability: String,
    },

    #[error("configuration error: {0}")]
    Config(String),

    #[error("storage error: {0}")]
    Storage(#[source] StorageError),

    #[error("operation timed out")]
    Timeout,

    #[error("invalid request: {0}")]
    Invalid(String),

    /// `#91`: a write-lane request body exceeded the configured
    /// `settings.max_request_body_bytes` cap, caught from the streamed
    /// length before the body was ever fully buffered. Distinct from
    /// `Invalid` so `Problem::from_core_error` can map it to `413` instead
    /// of `400` — a size refusal, not a shape one. Also the direct-upload
    /// asset lane's own cap (`asset.rs`, `RegisterManagedRequest.
    /// declared_size` against `AssetPolicy.max_asset_bytes`, and the
    /// declared-size-capped body read at `.../assets/{key}/data`).
    #[error("request body exceeds the {limit}-byte limit")]
    PayloadTooLarge { limit: u64 },

    /// The assets-and-object-storage proposal's named `409`: a `PUT
    /// .../assets/{key}` (or `.../data`) refused because the target is
    /// already claimed by a different declaration, or is not in the state
    /// the operation requires — see `asset.rs`'s own "idempotent PUT vs. a
    /// genuine conflict" doc.
    #[error("conflict: {0}")]
    Conflict(String),

    /// A declared media type outside a collection's configured asset
    /// allow-list (`asset.rs::AssetPolicy`) — `415`, named, checked at
    /// registration before any storage I/O.
    #[error("unsupported media type: {0}")]
    UnsupportedMediaType(String),

    /// A request was well-formed but semantically invalid in a way that
    /// isn't a plain `400` — currently only a declared-vs-computed RFC 9530
    /// digest mismatch on a direct asset upload (`asset.rs::
    /// complete_upload`): the asset still transitions to `failed` (a
    /// successful state change), but the response itself names why the
    /// bytes were refused. Maps to `422`.
    #[error("unprocessable: {0}")]
    UnprocessableEntity(String),

    /// An exact Features or STAC item response crossed the collection's
    /// cumulative source-geometry vertex budget. Distinct from the generic
    /// `UnprocessableEntity` variant so callers can preserve the structured
    /// diagnostic and stable problem code without parsing message text.
    #[error(
        "collection '{collection}' exact item response crosses its {limit}-vertex budget at feature '{feature_id}' ({cumulative_vertices} cumulative vertices)"
    )]
    ItemsVertexBudgetExceeded {
        collection: String,
        feature_id: String,
        cumulative_vertices: u64,
        limit: u64,
    },

    /// `#110`: a compare-and-swap `ConfigStore::write` whose caller-supplied
    /// expected version doesn't match the store's current one — a
    /// concurrent writer already applied a different change since the
    /// caller last read the document. Named separately from the
    /// pre-existing [`Error::Conflict`] (an unrelated asset-upload race) so
    /// `Problem::from_core_error` can report a distinct, config-specific
    /// problem `code` without parsing `Conflict`'s free-text message to
    /// tell the two apart. Both fields are already-rendered version
    /// strings (`ConfigVersion::to_string`), never the token type itself —
    /// this module has no dependency on `config_store`, the same "no
    /// upward dependency" shape every other error variant here keeps.
    #[error("config version conflict: expected '{expected}', current version is '{current}'")]
    VersionConflict { expected: String, current: String },

    #[error("control-plane validation error: {0}")]
    ControlValidation(String),

    #[error(
        "control event order violation: ({revision}, {ordinal}) does not follow ({previous_revision}, {previous_ordinal})"
    )]
    ControlEventOrder {
        previous_revision: u64,
        previous_ordinal: u32,
        revision: u64,
        ordinal: u32,
    },

    #[error("control store is not initialized")]
    ControlUninitialized,

    #[error("control revision conflict: expected {expected}, current revision is {current}")]
    ControlRevisionConflict { expected: u64, current: u64 },

    #[error("idempotency key '{key}' was already used for a different control changeset")]
    ControlIdempotencyConflict { key: String },

    #[error("idempotency key '{key}' was replayed by a different authenticated request")]
    ControlIdempotencyAuthorizationConflict { key: String },

    #[error(
        "entity version conflict for '{resource}': expected '{expected}', current version is '{current}'"
    )]
    ControlEntityVersionConflict {
        resource: String,
        expected: String,
        current: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
