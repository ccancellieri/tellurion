//! Internal error type for this crate — wraps `flatgeobuf`/`geozero`/
//! `serde_json` failures plus this driver's own failure modes (a missing
//! config env var, a malformed keyset token, an unsupported datetime
//! filter). Converted to `tellurion_core::Error` at the trait-impl boundary
//! in `driver.rs`; nothing outside this crate ever sees
//! `FlatgeobufDriverError` directly.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FlatgeobufDriverError {
    #[error("environment variable '{0}' is not set")]
    MissingEnvVar(String),

    #[error(transparent)]
    Format(#[from] flatgeobuf::Error),

    #[error(transparent)]
    Geojson(#[from] geozero::error::GeozeroError),

    #[error("malformed feature JSON produced by the geojson writer: {0}")]
    Json(#[from] serde_json::Error),

    #[error(
        "keyset token '{0}' is not valid for this collection (expects a non-negative feature index)"
    )]
    InvalidToken(String),

    #[error("flatgeobuf driver does not support datetime filtering")]
    DatetimeUnsupported,

    #[error("flatgeobuf driver does not support the 'filter' parameter")]
    FilterUnsupported,

    #[error("background read task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

pub type Result<T> = std::result::Result<T, FlatgeobufDriverError>;

impl From<FlatgeobufDriverError> for tellurion_core::Error {
    fn from(error: FlatgeobufDriverError) -> Self {
        match error {
            FlatgeobufDriverError::MissingEnvVar(_) => {
                tellurion_core::Error::Config(error.to_string())
            }
            FlatgeobufDriverError::InvalidToken(_)
            | FlatgeobufDriverError::DatetimeUnsupported
            | FlatgeobufDriverError::FilterUnsupported => {
                tellurion_core::Error::Invalid(error.to_string())
            }
            FlatgeobufDriverError::Format(_)
            | FlatgeobufDriverError::Geojson(_)
            | FlatgeobufDriverError::Json(_)
            | FlatgeobufDriverError::Join(_) => tellurion_core::Error::Storage(Box::new(error)),
        }
    }
}
