//! Internal error type for this crate — wraps `duckdb`/`geozero`/`serde_json`
//! failures plus this driver's own failure modes (a missing config env var,
//! an unprovisioned or malformed catalog, a malformed keyset token, an
//! unsupported filter/datetime query). Converted to `tellurion_core::Error`
//! at the trait-impl boundary in `driver.rs`; nothing outside this crate ever
//! sees `DuckdbDriverError` directly.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DuckdbDriverError {
    #[error("environment variable '{0}' is not set")]
    MissingEnvVar(String),

    #[error("'{0}' does not exist or is not a readable .duckdb file")]
    MissingFile(String),

    #[error(transparent)]
    Duckdb(#[from] duckdb::Error),

    #[error(transparent)]
    Geojson(#[from] geozero::error::GeozeroError),

    #[error("malformed feature JSON produced by the geojson writer: {0}")]
    Json(#[from] serde_json::Error),

    #[error("identifier '{0}' is not a valid table/column name")]
    InvalidIdentifier(String),

    #[error("collection '{collection}': declared table '{table}' does not exist in this database")]
    MissingTable { collection: String, table: String },

    #[error(
        "collection '{collection}': declared geometry column '{column}' does not exist on table '{table}'"
    )]
    MissingGeometryColumn {
        collection: String,
        table: String,
        column: String,
    },
    #[error(
        "collection '{collection}': table '{table}' has zero or more than one BLOB column and no `geometry` override was configured to pin one"
    )]
    AmbiguousGeometryColumn { collection: String, table: String },
    #[error(
        "collection '{collection}': geometry column '{column}' on table '{table}' has DuckDB type '{sql_type}', expected BLOB (WKB) — see this crate's own docs for why the spatial extension's native GEOMETRY type is never used"
    )]
    GeometryColumnNotBlob {
        collection: String,
        table: String,
        column: String,
        sql_type: String,
    },
    #[error(
        "collection '{collection}': declared primary key column '{pk}' does not exist on table '{table}'"
    )]
    MissingPrimaryKeyColumn {
        collection: String,
        table: String,
        pk: String,
    },
    #[error(
        "collection '{collection}': table '{table}' declares no single-column PRIMARY KEY and no `pk` override was configured"
    )]
    NoPrimaryKey { collection: String, table: String },
    #[error(
        "collection '{collection}': primary key column '{pk}' on table '{table}' has DuckDB type '{sql_type}', expected an integer type"
    )]
    PrimaryKeyNotInteger {
        collection: String,
        table: String,
        pk: String,
        sql_type: String,
    },

    #[error(
        "keyset token '{0}' is not valid for this collection (expects an integer primary key value)"
    )]
    InvalidToken(String),

    #[error("duckdb driver does not support datetime filtering")]
    DatetimeUnsupported,

    /// A `Filter` variant outside this driver's declared basic-comparison
    /// subset (`sql::compile_filter`'s own doc names exactly what compiles).
    #[error("duckdb driver does not support this filter construct: {0}")]
    FilterUnsupported(&'static str),

    /// Defensive: a value decoded from a row that this driver's own schema
    /// validation should already have ruled out (e.g. an attribute column of
    /// a type `duckdb_value_to_json` doesn't recognize). Expected to never
    /// fire in practice — see that function's own doc.
    #[error("failed to decode duckdb row: {0}")]
    Decode(String),

    #[error("background read task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

pub type Result<T> = std::result::Result<T, DuckdbDriverError>;

impl From<DuckdbDriverError> for tellurion_core::Error {
    fn from(error: DuckdbDriverError) -> Self {
        match error {
            DuckdbDriverError::MissingEnvVar(_)
            | DuckdbDriverError::MissingFile(_)
            | DuckdbDriverError::MissingTable { .. }
            | DuckdbDriverError::MissingGeometryColumn { .. }
            | DuckdbDriverError::AmbiguousGeometryColumn { .. }
            | DuckdbDriverError::GeometryColumnNotBlob { .. }
            | DuckdbDriverError::MissingPrimaryKeyColumn { .. }
            | DuckdbDriverError::NoPrimaryKey { .. }
            | DuckdbDriverError::PrimaryKeyNotInteger { .. } => {
                tellurion_core::Error::Config(error.to_string())
            }
            DuckdbDriverError::InvalidToken(_)
            | DuckdbDriverError::DatetimeUnsupported
            | DuckdbDriverError::FilterUnsupported(_)
            | DuckdbDriverError::InvalidIdentifier(_) => {
                tellurion_core::Error::Invalid(error.to_string())
            }
            DuckdbDriverError::Duckdb(_)
            | DuckdbDriverError::Geojson(_)
            | DuckdbDriverError::Json(_)
            | DuckdbDriverError::Decode(_)
            | DuckdbDriverError::Join(_) => tellurion_core::Error::Storage(Box::new(error)),
        }
    }
}
