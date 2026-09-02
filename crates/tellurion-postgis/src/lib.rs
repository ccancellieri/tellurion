//! PostGIS storage driver: implements the core storage traits (CatalogSource,
//! FeatureSource, TileSource, VolumeSource) via information_schema/
//! geometry_columns introspection and ST_AsGeoJSON / ST_AsMVT / ST_AsEWKB.
//! The only crate (besides `tellurion-ingest`) that may speak PostgreSQL.
//!
//! `VolumeSource` (`#41`) is the one capability with real internal
//! structure of its own: `ewkb.rs` hand-decodes the PolyhedralSurface Z/
//! TIN Z/MultiPolygon Z EWKB `driver.rs`'s volume query fetches, and
//! `volume.rs` holds the geometry-type contract, the world-to-tile-local
//! affine transform, and mesh assembly (triangulation via
//! `tellurion_render::triangulate_face` plus the per-zoom complexity cap).
//!
//! ## Known v0.1 limitations
//!
//! - **Single-column primary keys only, `bigint`, `uuid`, or `text`
//!   (`CollectionDecl::id_type`, `#87`/`#94`).** `Integer` (the default)
//!   parses keyset tokens and item ids as `i64` and casts the pk column
//!   `::bigint` in every comparison so `int4`/`int8` behave identically —
//!   byte-for-byte the original v0.1 behavior. `Uuid` parses/casts as
//!   `uuid::Uuid`/`::uuid` instead, and a server-assigned create mints it
//!   server-side (a `uuid` column's own `DEFAULT gen_random_uuid()`, read
//!   back via the same omit-from-`INSERT`+`RETURNING` shape a `bigserial`
//!   uses). `Text` parses/casts as `String`/`::text`; unlike the other two,
//!   create is CALLER-supplied (the feature body's own top-level `id`), a
//!   conflicting id is a named `409`, and keyset paging pins an explicit
//!   `COLLATE "C"` so ordering stays stable regardless of the database's own
//!   locale. A declared `id_type: uuid`/`text` collection whose physical pk
//!   doesn't match, or composite pks, are not supported — see `sql.rs`'s
//!   `PkValue`/`pk_sql_cast` and `driver.rs`'s `validate_id_type_for_create`
//!   for exactly what's checked and where.
//! - **MVT tiles carry only the pk as an attribute**, not the full feature
//!   properties, and never set `ST_AsMVT`'s native (unsigned-integer)
//!   feature id at all — the pk is always a plain `::text` tag, which is
//!   also what makes the tile lane already correct for a `Uuid`/`Text` pk
//!   with zero special-casing (`sql.rs`'s `build_mvt_plan` doc). `StyleConf`
//!   is a flat, collection-wide style (not data-driven), and `CollectionDecl`
//!   has no per-collection tile-attribute allowlist beyond `tile_properties`
//!   (`#85`), so there is nothing else to project into the tile today.
//! - **Pool sizing is cgroup-aware**: `StorageDecl.pool_size` overrides it
//!   outright; absent that, `clamp(effective_cpu_count * 2, 4, 32)` derives
//!   from the cgroup v2/v1 CPU quota (falling back to the host core count
//!   when neither is mounted) — see `pool::derive_pool_size` and
//!   `tellurion_core::resources::effective_cpu_count`.
//! - The `pg_class.reltuples` fast-path for `numberMatched` assumes the
//!   physical table name resolves under the default `search_path`; a
//!   mixed-case table name quoted differently at DDL time than the whitelist
//!   allows could make the estimate silently unavailable (`None`) without
//!   affecting correctness of the actual result rows. The estimate is
//!   clamped to 0 for a table that has never been `ANALYZE`d (Postgres
//!   reports `reltuples = -1` in that state).
//! - `collection.tiles.minzoom`/`maxzoom` are not enforced here — this
//!   driver serves whatever `TileCoord` it is asked for; enforcing the
//!   configured zoom range is a protocol-layer (`tellurion-tiles`) concern.

mod asset_sql;
mod cancel;
mod catalog;
mod driver;
mod error;
mod ewkb;
mod ident;
mod index_sql;
mod job_sql;
mod lease_sql;
mod pool;
mod registry;
mod sql;
mod stac_sql;
mod tenant;
/// Shared live-test harness (`#138`): advisory-locked fixture DDL, a
/// named refusal when the server is unreachable, and the one skip message
/// every live test prints. `test-support` feature only, so none of it is
/// ever linked into a production binary — the same shape `tellurion-core`'s
/// own `test-support` feature uses for its in-memory fakes.
#[cfg(feature = "test-support")]
pub mod test_harness;
mod volume;
mod write_sql;

pub use driver::PostgisDriverFactory;
pub use registry::{PostgisRegistryFactory, PostgisRegistryReader};
pub use tenant::{PostgisTenantFactory, PostgisTenantReader};

/// This crate's stable, config-facing name for the relational registry
/// backend it provides (`#162`) — what `PostgisRegistryFactory::name` and
/// `PostgisTenantFactory::name` both return, what `main` registers them
/// under, and what an operator writes as `registry.implementation`. Both
/// halves read it from here so they cannot drift apart, and so a test can
/// assert the wire name without duplicating the literal.
///
/// Deliberately the same string as this crate's storage driver name
/// (`storages[].driver: postgis`): one driver crate, one name, whichever
/// seam is naming it.
pub const RELATIONAL_IMPLEMENTATION_NAME: &str = "postgis";
