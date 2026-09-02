use std::io;

use tellurion_http_source::SourceError;

pub type Result<T> = std::result::Result<T, ArchiveError>;

/// A redacted archive-materialization refusal.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("archive exceeds a configured limit")]
    Limit,
    #[error("archive has an unsafe ZIP structure")]
    UnsafeZip,
    #[error("archive does not contain one complete Shapefile dataset")]
    InvalidDataset,
    #[error("archive decompression integrity check failed")]
    Integrity,
    #[error("archive spool deadline exceeded")]
    Deadline,
    #[error("archive spool capacity is unavailable")]
    Capacity,
    #[error("archive source failed")]
    Source(#[source] SourceError),
    #[error("archive storage failed")]
    Storage(#[source] io::Error),
    #[error("archive worker failed")]
    Worker,
}

impl From<io::Error> for ArchiveError {
    fn from(error: io::Error) -> Self {
        Self::Storage(error)
    }
}
