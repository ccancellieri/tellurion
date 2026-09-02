//! FlatGeobuf storage driver: read-only `CatalogSource` + `FeatureSource`
//! over a local `.fgb` file (via the `flatgeobuf` crate, BSD-2-Clause), with
//! bbox queries accelerated by the file's own packed Hilbert R-tree index.
//! Implements the driver contract's mandatory `CatalogSource` plus the
//! optional `FeatureSource` capability; never `TileSource` — a collection
//! routed to a `flatgeobuf` storage on the `tiles` lane fails at boot with
//! the router's ordinary missing-capability error. See `driver.rs` for the
//! full contract, the pk/cursor mapping, and the config shape.
//!
//! This is the features-lane counterpart of the PMTiles tiles proof (`#18`):
//! a second database-free driver serving traffic through the same contract
//! as the bundled PostGIS driver, this time on the `features` lane — see
//! the server crate's `flatgeobuf` cargo feature and its proof test.

mod driver;
mod error;

pub use driver::FlatgeobufDriverFactory;
