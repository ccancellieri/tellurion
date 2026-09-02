//! Secure, Unix-only materialization of one ZIP-wrapped Shapefile dataset.
//!
//! The spool relies on POSIX owner-only permissions for every working and
//! extracted file. Windows remains unsupported until equivalent restrictive
//! ACL handling is implemented.

#[cfg(not(unix))]
compile_error!("tellurion-shapefile requires Unix owner-only filesystem permissions");

mod archive;
mod crs;
mod driver;
mod error;

pub use archive::{ArchiveLimits, ArchiveSpool, ValidatedShapefile};
pub use driver::{ScanLimits, ShapefileBackend, ShapefileDriverFactory};
pub use error::{ArchiveError, Result};
