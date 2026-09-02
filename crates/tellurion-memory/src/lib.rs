//! A small, immutable reference implementation of Tellurion's storage-driver
//! contract, intended for examples and contract tests.
//!
//! [`MemoryDataset`] validates one GeoJSON FeatureCollection and derives the
//! physical facts a [`tellurion_core::CatalogSource`] must report.
//! [`MemoryDriver`] exposes those datasets through the mandatory catalog and
//! optional feature capabilities. [`MemoryDriverFactory`] demonstrates normal
//! registry construction while accepting preloaded drivers by storage id, so
//! this fixture adds no runtime configuration or server wiring.

mod dataset;
mod driver;
mod error;

pub use dataset::MemoryDataset;
pub use driver::{MemoryDriver, MemoryDriverFactory};
pub use error::MemoryDriverError;
