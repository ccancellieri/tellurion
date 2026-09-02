//! Embedded, self-contained GeoPackage (SQLite) storage driver for
//! Tellurion (issue `#73`, first slice).
//!
//! ## Positioning
//!
//! Every other file-backed driver in this workspace (FlatGeobuf, GeoParquet,
//! Cloud-Optimized GeoTIFF, PMTiles) is read-only and off by default. This
//! one is different, on purpose: it is read/write/tiles-capable, and the
//! server's own `default` feature set turns it on alongside `postgis` (see
//! `tellurion-server/Cargo.toml`). The reason is the deployment shape it
//! makes possible — a single binary plus a single `.gpkg` file serves,
//! filters, tiles, *and accepts writes*, with no database service, no
//! container runtime, and no network connection string. A PostGIS-backed
//! deployment remains the right choice for scale or many concurrent
//! writers; this driver is the out-of-the-box story for everything smaller
//! than that.
//!
//! ## Storage config
//!
//! A `geopackage` storage reuses `StorageDecl.url_env` exactly as `postgis`/
//! `flatgeobuf`/`cog` do: the named environment variable holds the `.gpkg`
//! file's local filesystem path. The server never creates or provisions this
//! file — `DriverFactory::build` refuses cleanly, by name, when the path is
//! missing or is not a `.gpkg` file this driver's own catalog+schema
//! provisioning has touched (`error::GeopackageError::NotAGeoPackage`).
//! Provisioning (the feature table, the `gpkg_contents`/
//! `gpkg_geometry_columns`/`gpkg_spatial_ref_sys` rows, the R*Tree spatial
//! index, its maintenance triggers, and the outbox table) is
//! `tellurion-ingest geopackage create-tables`'s job, never this crate's or
//! the server's — the same "the server never runs DDL" rule every other
//! driver in this workspace follows.
//!
//! ## Concurrency: one writer, many readers
//!
//! The file is opened in SQLite's WAL journal mode, which allows any number
//! of concurrent readers alongside exactly one writer with no reader ever
//! blocking behind the writer or vice versa. This driver opens a small
//! round-robin pool of read-only connections plus a single dedicated writer
//! connection (`pool.rs`); every mutation this driver ever performs —
//! including the outbox insert, in the same transaction — serializes
//! through that one writer connection. This is an honest architectural
//! ceiling, not an oversight: a `.gpkg` file is a single local file, and
//! SQLite itself permits only one writer at a time regardless of how many
//! connections ask, so a workload with many concurrent writers should
//! reach for the PostGIS driver's connection-pooled, multi-writer backend
//! instead.
//!
//! ## Scope of this slice
//!
//! Deliberately narrow, and recorded here rather than silently assumed:
//!
//! - **bbox pushdown, plus exact `S_INTERSECTS` for 2D geometry.**
//!   `FeatureSource`/`TileSource` push a `bbox` query parameter through the
//!   GeoPackage spec's own R*Tree spatial index (Annex L). CQL2's
//!   `S_INTERSECTS` predicate is honored exactly — an R*Tree bbox prune
//!   narrows the candidate rows in SQL, then `intersects.rs` decodes each
//!   candidate's geometry and tests it for real — for every 2D geometry
//!   class this driver's `geo`-backed evaluator covers; a query or row
//!   geometry outside that (Z/M coordinates, or the predicate sitting
//!   beneath `OR`/`NOT`) is refused by name rather than silently answered by
//!   the coarse bbox test alone. The other six `S_*` binary spatial
//!   predicates stay refused by name entirely — see `sql.rs`'s own doc.
//! - **Narrow CRS support.** `FeatureSource::crs_capable` stays `false` (no
//!   OGC API Features Part 2 response reprojection) — items/item responses
//!   are always emitted in the collection's own storage CRS, unchanged — and
//!   since `#227` that is what the Features lane says on the wire: a
//!   projected collection on this driver advertises its storage CRS in
//!   `crs`/`storageCrs`, stamps it on `Content-Crs`, and refuses `crs=CRS84`
//!   by name rather than answering in metres under a header naming degrees.
//!   A 4326 collection is unaffected either way. The
//!   write lane accepts CRS84 into storage SRID 4326 (identity) or 3857 (the
//!   same exact spherical transform the tiles lane uses), and accepts a
//!   `Content-Crs` naming the collection's storage CRS for identity writes at
//!   any storage SRID. A CRS84 transformation to any other storage SRID is
//!   refused by name rather than relabelling coordinates. The
//!   tiles lane is similarly narrow but not CRS-rigid: a
//!   collection's stored SRID must be `3857` (served as-is) or `4326`
//!   (reprojected per-vertex to Web Mercator at tile-encode time, `#89` —
//!   the one closed-form spherical transform this embedded driver can do
//!   without a real geodesy dependency); any other SRID refuses by name
//!   rather than serving a geometrically distorted tile — see `driver.rs`'s
//!   `TileSource` doc.
//! - **No SQLite registry backend.** Catalog/collection declarations still
//!   come from YAML config or the relational (PostGIS) registry; this slice
//!   does not add a third registry backend.
//! - **No derived-index/search parity.** This driver advertises
//!   `WriteSink`/`OutboxSource` only — no `IndexSink`/`SearchSource`. A
//!   collection whose `routing.search`/`routing.index` names a `geopackage`
//!   storage fails the router's own capability validation, exactly like any
//!   other driver that doesn't claim those lanes.
//! - **No styles/PNG rendering beyond what the existing render pipeline
//!   already gives every MVT source.** This driver only ever produces MVT
//!   bytes; PNG rendering of those bytes is `tellurion-render`'s existing,
//!   driver-agnostic job.
//! - **`CollectionDecl::id_type` must stay `Integer` (`#87`).** The
//!   GeoPackage format itself mandates an `INTEGER PRIMARY KEY` feature id
//!   column — this is not a gap this driver could close by adding code, the
//!   way PostGIS's `Uuid` support closed one there. A collection declaring
//!   `id_type: uuid` (or anything else non-default) against a
//!   `geopackage`-backed storage refuses by name, unconditionally, the
//!   first time an id reaches this driver (`error::GeopackageError::
//!   IdTypeUnsupported`, `driver.rs`'s `item_inner`/`write_apply_inner`)
//!   rather than silently misreading the id string as a failed integer
//!   parse.

mod catalog;
mod crs;
mod driver;
mod error;
mod functions;
mod gpb;
mod ident;
mod intersects;
mod pool;
mod sql;
mod write_sql;

pub use driver::GeopackageDriverFactory;
pub use error::{GeopackageError, Result};
