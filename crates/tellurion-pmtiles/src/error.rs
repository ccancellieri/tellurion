//! Internal error type for this crate — wraps the `pmtiles` crate's own
//! error plus this driver's own failure modes (a missing config env var, an
//! archive with no readable metadata). Converted to `tellurion_core::Error`
//! at the trait-impl boundary in `driver.rs`; nothing outside this crate
//! ever sees `PmtilesDriverError` directly.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PmtilesDriverError {
    #[error("environment variable '{0}' is not set")]
    MissingEnvVar(String),

    #[error(transparent)]
    Archive(#[from] pmtiles::PmtError),
}

pub type Result<T> = std::result::Result<T, PmtilesDriverError>;

impl From<PmtilesDriverError> for tellurion_core::Error {
    fn from(error: PmtilesDriverError) -> Self {
        match error {
            PmtilesDriverError::MissingEnvVar(_) => {
                tellurion_core::Error::Config(error.to_string())
            }
            PmtilesDriverError::Archive(_) => tellurion_core::Error::Storage(Box::new(error)),
        }
    }
}
