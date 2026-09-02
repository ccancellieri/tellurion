//! GeoParquet storage driver: read-only `CatalogSource` + `FeatureSource`
//! over a local `.parquet` file written per the GeoParquet spec (WKB
//! geometry column plus file-level `"geo"` key-value metadata), with bbox
//! queries pruned by GeoParquet 1.1's row-group `covering` bbox statistics
//! when the file carries them. Implements the driver contract's mandatory
//! `CatalogSource` plus the optional `FeatureSource` capability; never
//! `TileSource` — a collection routed to a `geoparquet` storage on the
//! `tiles` lane fails at boot with the router's ordinary missing-capability
//! error. See `driver.rs` for the full contract, the pk/cursor mapping, the
//! row-group pruning strategy, and the dependency-choice rationale.
//!
//! Mirrors `tellurion-flatgeobuf`'s own shape: a second database-free,
//! file-backed driver serving traffic through the same contract as the
//! bundled PostGIS driver, on the `features` lane — see the server crate's
//! `geoparquet` cargo feature and its proof test.

mod driver;
mod error;
mod geo_metadata;
mod input;

pub use driver::{GeoparquetBackend, GeoparquetDriverFactory};
pub use input::GeoparquetInput;
#[cfg(feature = "remote")]
pub use input::RemoteParquetReader;
