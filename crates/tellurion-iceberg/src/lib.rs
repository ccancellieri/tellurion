//! Apache Iceberg storage driver: read-only feature serving over an
//! Iceberg table resolved through a REST catalog, pinned to whatever
//! snapshot is current the first time this driver is touched. Implements
//! the driver contract's mandatory `CatalogSource` plus the optional
//! `FeatureSource` capability — see `driver.rs`'s crate docs for the full
//! design (table access, snapshot pinning, paging, the planned-file
//! cache) and for exactly what this slice does not do.
//!
//! Storage: every read goes through this crate's own `FileIO`
//! (`fileio.rs`) — the local filesystem, plus anything speaking the S3
//! protocol, over `tellurion-core`'s existing `ObjectStore` port rather
//! than a second S3 client. GCS and ADLS are refused by name at table load.
//!
//! Iceberg has no native geometry type: the geometry column (WKB bytes) and
//! its four covering bbox columns are pure operator declarations carried
//! inside `StorageDecl.url_env`'s value (see `location.rs` for the exact
//! shape and the reasoning for reusing that one field rather than adding a
//! new one).

mod driver;
mod error;
mod fileio;
mod location;
#[cfg(test)]
mod test_support;

pub use driver::IcebergDriverFactory;
