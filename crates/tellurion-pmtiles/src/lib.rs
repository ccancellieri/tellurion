//! PMTiles storage driver: read-only `CatalogSource` + `TileSource` over a
//! local PMTiles v3 archive (via the `pmtiles` crate, MIT OR Apache-2.0).
//! Implements the driver contract's mandatory `CatalogSource` plus the
//! optional `TileSource` capability; never `FeatureSource` — a collection
//! routed to a `pmtiles` storage on the `features` lane fails at boot with
//! the router's ordinary missing-capability error. See `driver.rs` for the
//! full contract and config shape.
//!
//! This is the acceptance proof for `#18`: a second real driver serving
//! traffic through the same contract as the bundled PostGIS driver, with
//! the server crate's `postgis` feature (and every database crate it
//! pulls in) compiled out — see the server crate's `pmtiles` cargo
//! feature and its proof test.

mod driver;
mod error;

pub use driver::PmtilesDriverFactory;
