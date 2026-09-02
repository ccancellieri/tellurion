//! Internal error type for this crate — every variant maps to a specific
//! `tellurion_core::Error` category at the trait-impl boundary in
//! `driver.rs`, never a driver-specific HTTP error. Nothing outside this
//! crate ever sees `IcebergDriverError` directly.
//!
//! Every variant here is a boot-time configuration or request-shape problem
//! (`Error::Config`/`Error::Invalid`) except `Iceberg`, which wraps a
//! genuine backend read failure — REST catalog request, manifest read, or
//! data file read (`Error::Storage`) — matching the driver contract's "map
//! errors at the driver boundary" rule.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IcebergDriverError {
    #[error("storage '{storage}': environment variable '{var}' is not set")]
    MissingEnvVar { storage: String, var: String },

    #[error(
        "iceberg storage location is missing the required '{field}' declaration \
         (expected '<rest-catalog-uri>?namespace=...&table=...&geometry=...&bbox=xmin,ymin,xmax,ymax')"
    )]
    MissingDeclaration { field: &'static str },

    #[error(
        "iceberg storage location has a malformed query parameter '{0}' (expected 'key=value')"
    )]
    MalformedQuery(String),

    #[error(
        "iceberg storage location's 'bbox' declaration '{0}' must name exactly 4 comma-separated \
         columns in xmin,ymin,xmax,ymax order"
    )]
    InvalidBboxDeclaration(String),

    #[error("iceberg storage location's 'plan_cache_ttl_s' declaration '{0}' is not a valid number of seconds")]
    InvalidPlanCacheTtl(String),

    /// The table's own metadata puts its files on a storage scheme this
    /// driver does not implement — GCS or ADLS (recognized and refused by
    /// name), or a scheme it has never heard of. Raised at table load,
    /// before a single byte is served, and never downgraded to a fallback:
    /// see `fileio.rs`'s crate docs for why the S3 protocol and the local
    /// filesystem are the whole of what this driver covers.
    #[error("table '{table}': storage location '{location}': {detail}")]
    UnsupportedStorageScheme {
        table: String,
        location: String,
        detail: String,
    },

    /// The table IS on S3, and the storage locator does not carry the four
    /// `s3_*` declarations reading it requires (`location.rs`). Names the
    /// missing key rather than reporting "S3 is not configured", and says
    /// where it belongs — which is the locator in this storage's `url_env`,
    /// never `config.yaml`.
    #[error(
        "table '{table}': storage location '{location}' is on S3, but this storage's locator \
         declares no '{field}'. Add '{field}=...' to the locator held in this storage's \
         'url_env' environment variable (S3 settings are never read from config.yaml)"
    )]
    MissingS3Declaration {
        table: String,
        location: String,
        field: &'static str,
    },

    /// The locator NAMES an environment variable for an S3 credential and
    /// that variable is not set. The message names the variable, never its
    /// expected value, and this error never carries a credential.
    #[error(
        "table '{table}': the iceberg storage locator names '{var}' as the environment variable \
         holding an S3 credential, but it is not set"
    )]
    MissingS3Credential { table: String, var: String },

    #[error("table '{table}': has no current snapshot")]
    NoCurrentSnapshot { table: String },

    #[error(
        "table '{table}': declared geometry column '{column}' is not present in the table schema"
    )]
    GeometryColumnNotFound { table: String, column: String },

    #[error(
        "table '{table}': declared geometry column '{column}' has schema type '{actual}', \
         expected 'binary' (WKB bytes)"
    )]
    GeometryColumnWrongType {
        table: String,
        column: String,
        actual: String,
    },

    #[error("table '{table}': declared bbox column '{column}' is not present in the table schema")]
    BboxColumnNotFound { table: String, column: String },

    #[error(
        "table '{table}': declared bbox column '{column}' has schema type '{actual}', expected \
         'double' or 'float'"
    )]
    BboxColumnWrongType {
        table: String,
        column: String,
        actual: String,
    },

    /// A CQL2 construct `compile_predicate` in `driver.rs` cannot faithfully
    /// turn into an Iceberg predicate: a spatial operator
    /// against WKB geometry, a function Iceberg has no equivalent for
    /// (`LIKE`/`BETWEEN`/`CASEI`), or a property/literal type combination
    /// with no compilable mapping (comparing a text literal to a numeric
    /// column, a temporal function's operand, the declared geometry column
    /// itself, ...). Always a clean, named refusal — this driver never
    /// silently drops part of a filter or serves unfiltered rows in its
    /// place.
    #[error("cannot compile CQL2 filter on property '{property}': {reason}")]
    FilterPropertyUnsupported { property: String, reason: String },

    #[error(
        "collection '{0}' has no datetime column configured but a datetime filter was supplied"
    )]
    NoDatetimeColumn(String),

    #[error(
        "table '{table}': declared datetime column '{column}' is not present in the table schema"
    )]
    DatetimeColumnNotFound { table: String, column: String },

    #[error(
        "table '{table}': declared datetime column '{column}' has schema type '{actual}', \
         expected 'timestamptz'"
    )]
    DatetimeColumnWrongType {
        table: String,
        column: String,
        actual: String,
    },

    #[error(
        "datetime value '{value}' for column '{column}' is not a valid RFC 3339 timestamp: {cause}"
    )]
    InvalidDatetimeLiteral {
        column: String,
        value: String,
        cause: String,
    },

    #[error("malformed paging token '{0}'")]
    InvalidToken(String),

    /// A paging token names a snapshot other than the one this backend
    /// pinned at load time (see `driver.rs`'s "Snapshot pinning" and
    /// "Snapshot-pinned paging" docs) — a token minted against this
    /// collection before a process restart observed a newer commit, most
    /// likely. Refused rather than silently reinterpreted against the
    /// wrong snapshot, which would silently mix two inconsistent views of
    /// the table across one page boundary.
    #[error("paging token was issued for snapshot {found}, but this collection is pinned to snapshot {expected}")]
    TokenSnapshotMismatch { expected: i64, found: i64 },

    /// A paging token names a filter/datetime fingerprint other than the one
    /// this request compiled (`driver.rs`'s `query_predicate_fingerprint`) —
    /// a token minted under one filter, replayed under a different (or
    /// absent) one. Refused for exactly the same reason a snapshot mismatch
    /// is: resuming it would silently splice two different result sets
    /// across one page boundary.
    #[error(
        "paging token was issued for filter fingerprint {found:016x}, but this request's filter \
         fingerprints as {expected:016x}"
    )]
    TokenFilterMismatch { expected: u64, found: u64 },

    #[error("{0}")]
    Decode(String),

    #[error(transparent)]
    Iceberg(#[from] iceberg::Error),
}

pub type Result<T> = std::result::Result<T, IcebergDriverError>;

impl From<IcebergDriverError> for tellurion_core::Error {
    fn from(error: IcebergDriverError) -> Self {
        match &error {
            IcebergDriverError::Iceberg(_) => tellurion_core::Error::Storage(Box::new(error)),
            IcebergDriverError::FilterPropertyUnsupported { .. }
            | IcebergDriverError::NoDatetimeColumn(_)
            | IcebergDriverError::InvalidDatetimeLiteral { .. }
            | IcebergDriverError::InvalidToken(_)
            | IcebergDriverError::TokenSnapshotMismatch { .. }
            | IcebergDriverError::TokenFilterMismatch { .. } => {
                tellurion_core::Error::Invalid(error.to_string())
            }
            _ => tellurion_core::Error::Config(error.to_string()),
        }
    }
}
