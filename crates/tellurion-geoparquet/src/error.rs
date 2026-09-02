//! Internal error type for this crate — wraps `parquet`/`arrow`/`geozero`/
//! `serde_json` failures plus this driver's own failure modes (a missing
//! config env var, a missing or malformed "geo" metadata document, a
//! malformed keyset token, an unsupported datetime filter). Converted to
//! `tellurion_core::Error` at the trait-impl boundary in `driver.rs`;
//! nothing outside this crate ever sees `GeoparquetDriverError` directly.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GeoparquetDriverError {
    #[error("environment variable '{0}' is not set")]
    MissingEnvVar(String),

    #[error(transparent)]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error(transparent)]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error(transparent)]
    Geojson(#[from] geozero::error::GeozeroError),

    #[error("malformed JSON (feature output or 'geo' metadata): {0}")]
    Json(#[from] serde_json::Error),

    #[error(
        "file has no '{}' key-value metadata entry — not a valid GeoParquet file",
        crate::geo_metadata::GEO_METADATA_KEY
    )]
    MissingGeoMetadata,

    #[error("malformed 'geo' metadata: {0}")]
    InvalidGeoMetadata(String),

    /// Anything wrong decoding one already-opened batch against the schema
    /// this driver already validated at header time (a column downcast that
    /// should always succeed given the matched `DataType`, a covering/
    /// geometry column absent from a batch that the schema promised it):
    /// defensive, expected to never fire in practice, but honest failure
    /// beats a panic or silently wrong output — see `driver.rs`'s `downcast`
    /// helper.
    #[error("failed to decode geoparquet row: {0}")]
    Decode(String),

    #[error(
        "keyset token '{0}' is not valid for this collection (expects a non-negative row position)"
    )]
    InvalidToken(String),

    #[error("geoparquet driver does not support datetime filtering")]
    DatetimeUnsupported,
}

pub type Result<T> = std::result::Result<T, GeoparquetDriverError>;

impl From<GeoparquetDriverError> for tellurion_core::Error {
    fn from(error: GeoparquetDriverError) -> Self {
        match error {
            GeoparquetDriverError::MissingEnvVar(_) => {
                tellurion_core::Error::Config(error.to_string())
            }
            GeoparquetDriverError::InvalidToken(_) | GeoparquetDriverError::DatetimeUnsupported => {
                tellurion_core::Error::Invalid(error.to_string())
            }
            GeoparquetDriverError::Parquet(_)
            | GeoparquetDriverError::Arrow(_)
            | GeoparquetDriverError::Geojson(_)
            | GeoparquetDriverError::Json(_)
            | GeoparquetDriverError::MissingGeoMetadata
            | GeoparquetDriverError::InvalidGeoMetadata(_)
            | GeoparquetDriverError::Decode(_) => tellurion_core::Error::Storage(Box::new(error)),
        }
    }
}
