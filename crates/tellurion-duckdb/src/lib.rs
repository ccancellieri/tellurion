//! DuckDB storage driver: read-only `CatalogSource` + `FeatureSource` over a
//! local `.duckdb` file (bundled engine — no system `libduckdb`, no external
//! service). Implements the driver contract's mandatory `CatalogSource` plus
//! the optional `FeatureSource` capability; never `TileSource` — a
//! collection routed to a `duckdb` storage on the `tiles` lane fails at boot
//! with the router's ordinary missing-capability error, the same shape
//! `tellurion-flatgeobuf`/`tellurion-geoparquet` take. See `driver.rs` for
//! the full contract: the pk/cursor mapping, the multi-collection model and
//! geometry-column auto-detection, the CQL2 filter scope, and — most
//! load-bearing — the "EXTENSION note" explaining why this driver never
//! loads DuckDB's `spatial` extension and instead reads a plain WKB `BLOB`
//! geometry column.
//!
//! This is the embedded *analytical* counterpart to
//! `tellurion-geopackage`'s embedded *transactional* driver: same "single
//! binary plus a single local file, no service" positioning, columnar
//! DuckDB engine instead of row-oriented SQLite, read-only instead of
//! read/write/tiles — see the server crate's `duckdb` cargo feature and its
//! proof test.

mod catalog;
mod driver;
mod error;
mod ident;
mod pool;
mod sql;

pub use driver::DuckdbDriverFactory;
