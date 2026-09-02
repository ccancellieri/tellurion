//! Zarr v2 storage driver: read-only `CatalogSource` + `RasterSource` over a
//! local Zarr v2 array store (hand-rolled reader — no third-party Zarr
//! crate; see this doc's "crate vs. hand-rolled" section below), served as
//! PNG raster tiles through the same capability-trait routing and render
//! pipeline `tellurion-cog` uses for Cloud-Optimized GeoTIFF. Implements the
//! driver contract's mandatory `CatalogSource` plus the optional
//! `RasterSource` capability; never `FeatureSource`/`TileSource` — a
//! collection routed to a `zarr` storage on the `features` lane, or asking
//! its `tiles` lane for MVT, fails with the router's ordinary
//! missing-capability refusal. See `driver.rs` for the full contract and
//! config shape, `reader.rs`/`metadata.rs` for the store decode itself, and
//! `tiling.rs` for the Web Mercator tile <-> pixel-window math.
//!
//! ## First-slice scope (`#37`)
//!
//! - Zarr v2 only: `zarr_format: 2`, `order: "C"`, no filter pipeline,
//!   `dimension_separator` `.` or `/`, compressor raw/gzip/zlib.
//! - Dtype: `u8`/`i8`/`u16`/`i16`/`i32`/`f32`/`f64`, always little-endian.
//! - One `zarr` storage backs exactly one collection, the same "one storage,
//!   one physical source" shape `tellurion-cog` uses for a single GeoTIFF
//!   file — either a single array directory (`.zarray`/`.zattrs` directly at
//!   its root), or a `multiscales` resolution pyramid (a `.zgroup` whose
//!   `.zattrs` declares one; see `reader`'s own "Multiscale pyramids" doc).
//!   Any other hierarchical group (a `.zgroup` declaring no pyramid) is still
//!   refused.
//! - Rank >= 2: the array's trailing two dimensions are always read as
//!   `(y, x)`; any leading dimensions (time, level, ...) are pinned to a
//!   single, store-declared index per request — never varied on the wire.
//! - Georeferencing is an explicit, unambiguous declaration in the store's
//!   own `.zattrs` (`tellurion:extent_crs84`, `tellurion:fixed_index`),
//!   never guessed from an existing CF/xarray/rioxarray convention and never
//!   read from `config.yaml` — see `metadata`'s own doc for why.
//! - A colormap (`CollectionDecl.settings.colormap`, the same config surface
//!   `tellurion-cog` uses) is mandatory to serve PNG tiles at all — a raw
//!   Zarr sample has no inherent visual meaning of its own.
//! - Bounded memory throughout: a per-chunk element cap (checked once at
//!   open time), a per-request native-pixel-window cap, and a per-request
//!   aggregate decompressed-element cap (guards against a pathologically
//!   small chunk shape under a large window) — see `metadata::
//!   MAX_CHUNK_ELEMENTS`, `driver::MAX_WINDOW_ELEMENTS`, and
//!   `reader::MAX_REQUEST_DECODE_ELEMENTS`.
//! - Overview/pyramid serving (`#37` follow-up): a store whose root `.zattrs`
//!   declares an OME-NGFF-shaped `multiscales` pyramid (`metadata::
//!   parse_multiscales`'s own doc explains why this driver consumes that
//!   convention rather than inventing one) is read at whichever level best
//!   matches the requested tile's own resolution (`tiling::select_overview`,
//!   the same policy `tellurion-cog` already established for a COG's own
//!   overview pyramid), never at native resolution downsampled per request.
//!   A plain, non-pyramid store keeps its original (`#37` first slice)
//!   behavior exactly: native resolution, world-bounds-clamped. AUTHORING a
//!   pyramid (writing new resolution levels) is still out of scope — this
//!   driver only ever reads one a store already provides.
//!
//! Deliberately out of scope, called out in the issue as later slices or not
//! named at all: Zarr v3; a filter pipeline; any compressor beyond
//! raw/gzip/zlib (blosc, zstd, lz4, bz2); on-the-wire dimension selection
//! (EDR); reprojection from any CRS other than the store's own already-CRS84
//! declaration; and the authoring/ingest lane that would produce a
//! serving-optimized Zarr layout (single-resolution OR pyramid) in the first
//! place.
//!
//! ## Crate vs. hand-rolled
//!
//! The `zarrs` crate (MIT OR Apache-2.0) was evaluated against hand-rolling
//! this reader. Even with every optional feature disabled beyond the
//! minimum needed to read local gzip-compressed chunks
//! (`default-features = false, features = ["filesystem", "gzip"]`), its own
//! dependency tree still pulls in a **build-dependency on `libz-sys`** — a C
//! library requiring a C compiler toolchain (`cc`, `pkg-config`, `vcpkg`) —
//! plus roughly 125 additional crates (`rayon`, a dozen `zarrs_*` subcrates,
//! `itertools`, `inventory`, `derive_more`, `positioned-io`, `walkdir`, ...),
//! none of which this driver's fixed, narrow format subset needs. That fails
//! this workspace's own bar for a dependency in an AGPL codebase: pure-Rust,
//! no C toolchain requirement, and no more of a crate pulled in than is
//! actually used. The Zarr v2 subset this slice actually serves — two small
//! JSON metadata documents, C-order chunk files, and two already-pure-Rust
//! DEFLATE variants (`flate2`, already resolved via `tellurion-cog`'s own
//! `tiff` dependency) — is small enough that this workspace's existing
//! precedent for a fixed format subset (hand-rolled SigV4, hand-rolled ISO
//! 19139 XML, the pure-Rust TIFF driver itself) clearly applies: hand-roll
//! it. This crate adds exactly one new direct dependency, `flate2` itself,
//! with the same `rust_backend` (miniz_oxide) feature selection
//! `tellurion-cog` already made.

mod colormap;
mod driver;
mod error;
mod metadata;
mod reader;
mod store;
#[cfg(test)]
mod test_support;
mod tiling;

pub use driver::ZarrDriverFactory;
