#[derive(Debug, thiserror::Error)]
pub enum MemoryDriverError {
    #[error("configuration error: {0}")]
    Configuration(String),

    #[error("invalid query: {0}")]
    InvalidQuery(String),
}

impl From<MemoryDriverError> for tellurion_core::Error {
    fn from(value: MemoryDriverError) -> Self {
        match value {
            MemoryDriverError::Configuration(message) => Self::Config(message),
            MemoryDriverError::InvalidQuery(message) => Self::Invalid(message),
        }
    }
}
