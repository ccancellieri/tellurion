//! Cloud-Optimized GeoTIFF storage driver: read-only `CatalogSource` +
//! `RasterSource` over a tiled GeoTIFF with overviews (via the pure-Rust
//! `tiff` crate, MIT OR Apache-2.0 — no GDAL, no C dependency), served
//! either from a local file or, entirely through bounded ranged HTTP GET
//! requests, from an `http(s)://` URL (`#37` slice 2 — see `reader.rs`'s
//! `CogSource` and `remote.rs`). Implements the driver contract's mandatory
//! `CatalogSource` plus the optional `RasterSource` capability; never
//! `FeatureSource`/`TileSource` — a collection routed to a `cog` storage on
//! the `features` lane, or asking its `tiles` lane for MVT, fails with the
//! router's ordinary missing-capability refusal. See `driver.rs` for the
//! full contract and config shape, `reader.rs` for the GeoTIFF decode
//! itself, `remote.rs` for the ranged-HTTP source, and `tiling.rs` for the
//! Web Mercator tile <-> pixel-window math.
//!
//! First-slice scope (`#37`): tiled layout only, 8-bit grayscale/RGB/RGBA/
//! paletted (categorical, `reader.rs`'s own doc), uncompressed/LZW/Deflate
//! compression, EPSG:4326 (WGS84 geographic) CRS only. `#92` (still first-slice) adds a config-driven single-band
//! colormap (`colormap.rs`) and warps the EPSG:4326 windowed read into
//! WebMercatorQuad tiles (`tiling.rs`'s own doc) — reprojection FROM any
//! CRS other than EPSG:4326 stays entirely out of scope, as does anything
//! beyond a single band (multi-band composites/band math) and the OGC API
//! Maps/Coverages endpoints.
//!
//! `#254` adds this crate's SECOND driver beside the single-COG one: the
//! `cog-mosaic` driver (`mosaic.rs`), which serves one raster TileSet
//! composed from a bounded manifest of COG sources (`manifest.rs`). It is
//! not a second decode path — it plans and composes results from the very
//! same `driver::TileRead` (`tiling::plan_window` -> `reader::read_window`
//! -> `tiling::resample_to_tile`), under the same per-request pixel budget,
//! feeding the same byte-budgeted tile cache at the response boundary. Its
//! bounds — 1..=32 unique sources, at most four concurrent constituent
//! reads, ascending-source-id composition order, and "fail the whole tile if
//! any selected source read fails" — are documented in full on `mosaic.rs`'s
//! own module doc, and every one of them is a refusal by name.
//!
//! [`author_cog`] is this crate's other half: the authoring lane that
//! *produces* a COG this driver can serve, from a plain single-resolution
//! GeoTIFF — see `author.rs`'s own doc. The `tellurion-ingest` CLI's `cog
//! author` subcommand is its only caller today. [`author_mosaic_manifest`]
//! is its `cog-mosaic` counterpart: the only place a mosaic manifest comes
//! from, called by `tellurion-ingest cog mosaic`. The server never authors
//! one.

mod author;
mod colormap;
mod driver;
mod error;
mod geokeys;
mod manifest;
mod mosaic;
mod reader;
mod remote;
#[cfg(test)]
mod test_support;
mod tiff_write;
mod tiling;

pub use author::{author_cog, AuthorOptions, AuthorReport, ResampleMode};
pub use driver::CogDriverFactory;
pub use error::CogError;
pub use manifest::{
    author_mosaic_manifest, ManifestSource, MosaicAuthorReport, MosaicManifest, MANIFEST_VERSION,
    MAX_SOURCES,
};
pub use mosaic::{MosaicDriverFactory, DRIVER_NAME as MOSAIC_DRIVER_NAME, MAX_CONCURRENT_READS};
