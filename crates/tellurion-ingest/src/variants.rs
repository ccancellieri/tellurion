//! Materializes a declared `geometry_variants` column (`#104`, `#201`) — the
//! producer half of the "sidecar generalization" line of work whose reader
//! half `#200` finished.
//!
//! A `GeometryVariantDecl` is entirely declarative: the operator produces a
//! pre-generalized geometry column and tellurion only ever *reads* whichever
//! one `CollectionDecl::resolved_geometry_for_zoom` selects. That contract
//! is unchanged here — this module is the operator's tooling, not a server
//! code path. Nothing in `tellurion-server` learns a new behavior because
//! this command exists; it is exactly the "ingest owns all DDL" posture
//! `outbox.rs`/`index.rs`/`geopackage.rs` already hold, extended to the one
//! piece of physical schema a declared variant needs.
//!
//! ## What "materialize" does, per backend
//!
//! Both backends do the same four things, idempotently, so a rerun
//! repopulates rather than duplicating:
//!
//! 1. add the variant column if it is absent, typed exactly like the base
//!    geometry column (same geometry type, same SRID) — that equality is not
//!    cosmetic, it is what `Router::refuse_invalid_geometry_variants` checks
//!    at boot;
//! 2. register it wherever the backend's own catalog surface reads columns
//!    from, so `CatalogSource::collections` reports it at all (PostGIS's
//!    `geometry_columns` view derives that from the column's typmod and
//!    needs no separate registration; GeoPackage needs an explicit
//!    `gpkg_geometry_columns` row — see [`GEOPACKAGE_SECOND_COLUMN_NOTE`]);
//! 3. populate every row by simplifying the base geometry with the tolerance
//!    [`derive_tolerance_in_storage_units`] derives from the variant's own
//!    zoom range;
//! 4. give it the index the tiles lane actually prunes with — see "Indexing"
//!    below, where the two backends genuinely differ.
//!
//! ## Indexing: the two backends differ, on purpose
//!
//! **PostGIS gets a GiST index on the variant column.** The PostGIS MVT
//! candidate fragment resolves *one* geometry column per tile and uses it for
//! both the projection and the bbox predicate
//! (`tellurion-postgis::sql::build_mvt_candidate_fragment`: `t.{geom} &&
//! <the tile envelope, transformed into the collection's own storage CRS when
//! that differs from the tile grid's — `#262`>). At a zoom the variant covers,
//! `{geom}` *is* the variant, so an unindexed variant column turns every tile
//! in that zoom range into a sequential scan. Same access method and the same
//! `"<table>_<column>_gix"` name `seed.rs` gives the base column.
//!
//! A variant needs no CRS question of its own, and `#262` did not give it
//! one: step 1 below types the variant column exactly like the base column,
//! *same SRID included*, and `Router::refuse_invalid_geometry_variants`
//! refuses a config whose variant SRID differs from its base at boot. So the
//! collection's single storage SRID describes whichever column a tile
//! resolves to, and the transform the fragment derives from it is the same
//! one either way.
//!
//! **GeoPackage deliberately gets no R*Tree for the variant.** A GeoPackage
//! R*Tree is an optional per-column extension (spec Annex L), and `#200`
//! decided — explicitly, in `tellurion-geopackage::sql::build_tile_plan`'s
//! own doc — that the GeoPackage tiles lane keeps pruning on the *base*
//! column's `rtree_<table>_<geometry>` even while reading a variant, because
//! a simplification never adds vertices and so never grows the base
//! envelope: the base index remains a sound prune for the variant's rows.
//! Provisioning `rtree_<table>_<variant>` here would therefore create an
//! index no query in this workspace ever names, plus the six Annex L
//! maintenance triggers needed to keep it from silently rotting — pure write
//! amplification and file growth for zero reads. This is a decision, not an
//! omission: if a later slice teaches the GeoPackage tile plan to prune on
//! the variant's own index, that slice provisions it here and the two land
//! together.
//!
//! ## Not a maintained secondary
//!
//! This is a batch backfill. Nothing keeps the variant column in step with
//! later writes to the base column — `#201`'s own non-goal, and a later
//! slice's job (a derivation consumer on the existing outbox lane). Rerun
//! the command after a bulk load.

use std::path::PathBuf;

use anyhow::Context;
use geo::SimplifyVwPreserve;
use geozero::{wkb::GpkgWkb, CoordDimensions, ToGeo, ToWkb};
use rusqlite::{Connection, OptionalExtension};
use tellurion_core::config::GeometryVariantDecl;
use tellurion_core::descriptor::heuristics::simplify_tolerance_meters;
use tellurion_core::{AppConfig, CollectionDecl, StorageDecl};

/// Equatorial meters per CRS84 degree: `2 * pi * 6378137 / 360`. Kept as a
/// local copy of `tellurion-postgis::sql::WORLD_CRS84_METERS_PER_DEGREE`
/// (a `pub(crate)` constant in a driver crate whose SQL this crate
/// deliberately never depends on) for the same reason every DDL module here
/// keeps its own `quote_ident`: see this crate's own top-level doc.
const WORLD_CRS84_METERS_PER_DEGREE: f64 = 111_319.490_793_273_57;

/// The two storage SRIDs a tolerance can be derived for without linking a
/// projection engine — the same two the tiles lanes themselves serve
/// (`3857` natively, `4326` reprojected at encode time, `#89`).
const DERIVABLE_SRIDS: [i32; 2] = [3857, 4326];

/// Why a `.gpkg` file provisioned to spec cannot hold a second registered
/// geometry column without an explicit operator decision — quoted into the
/// refusal an operator sees, so the message carries its own reasoning.
pub(crate) const GEOPACKAGE_SECOND_COLUMN_NOTE: &str =
    "the GeoPackage spec's own gpkg_geometry_columns definition carries \
     'CONSTRAINT uk_gc_table_name UNIQUE (table_name)', i.e. a feature table may register at \
     most one geometry column; a geometry variant is by construction a second one. \
     tellurion-geopackage's CatalogSource reads that table (catalog.rs's list_feature_tables \
     joins gpkg_contents against gpkg_geometry_columns), so an unregistered variant column \
     stays invisible to the boot-time check and the config keeps being refused. Rerun with \
     --allow-second-geometry-column to rebuild gpkg_geometry_columns without that one \
     constraint; every other constraint, and every existing row, is preserved. The file stays \
     readable by any GeoPackage reader, but it is no longer strictly conformant on this point";

pub struct MaterializeArgs {
    /// Tellurion config YAML — read only, never written.
    pub config: PathBuf,
    /// Internal id of the collection whose `geometry_variants` to
    /// materialize.
    pub collection: String,
    /// Materialize only this declared variant column. Omitted materializes
    /// every variant the collection declares.
    pub variant: Option<String>,
    /// Explicit simplification tolerance, in the storage CRS's own units.
    /// Overrides the derivation entirely; required for a storage SRID the
    /// derivation refuses to guess at (see
    /// [`derive_tolerance_in_storage_units`]).
    pub tolerance: Option<f64>,
    /// GeoPackage only: consent to rebuilding `gpkg_geometry_columns`
    /// without its `uk_gc_table_name` unique constraint. See
    /// [`GEOPACKAGE_SECOND_COLUMN_NOTE`].
    pub allow_second_geometry_column: bool,
    /// Print the plan (and, for PostGIS, the exact DDL/DML) without touching
    /// the backend at all — the same escape hatch every `create-tables`
    /// command in this crate offers.
    pub dry_run: bool,
}

/// Everything one variant's materialization needs, resolved from config plus
/// one backend introspection round-trip. Kept as a plain value so the
/// tolerance derivation and the SQL text can be asserted without a backend.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VariantPlan {
    pub(crate) table: String,
    pub(crate) base_geometry: String,
    pub(crate) variant_column: String,
    /// The base column's geometry type, exactly as the backend spells it
    /// (`GEOMETRY`/`POINT`/... for both PostGIS's `geometry_columns.type`
    /// and GeoPackage's `gpkg_geometry_columns.geometry_type_name`).
    pub(crate) geometry_type: String,
    pub(crate) srid: i32,
    /// Simplification tolerance in the storage CRS's own units — a distance
    /// on both backends; see [`derive_tolerance_in_storage_units`].
    pub(crate) tolerance: f64,
}

/// The simplification tolerance for a variant covering `[minzoom, maxzoom]`
/// on a collection stored in `srid`, expressed in that CRS's own units.
///
/// ## The derivation
///
/// The yardstick is one tile pixel, and it is not invented here: it is
/// [`simplify_tolerance_meters`] (`tellurion-core`'s
/// `descriptor::heuristics`, `#19`), the Web Mercator ground distance one
/// 256px tile pixel covers at a zoom — the very number the PostGIS tiles
/// lane already hands `ST_SimplifyPreserveTopology` when it renders that
/// zoom live. Detail finer than one pixel cannot be seen at that zoom, so
/// dropping it costs nothing visible.
///
/// **Which zoom.** `maxzoom`, the *finest* zoom the variant serves — not
/// `minzoom`, and not a midpoint. The tiles lane reads this one column for
/// every zoom in `[minzoom, maxzoom]` (`resolved_geometry_for_zoom`), so the
/// column has to still look right at the most detailed of them. One zoom
/// level coarser doubles the pixel, so the same tolerance is strictly
/// conservative — under-simplified, never over-simplified — everywhere else
/// in the range. Picking `minzoom` would do the opposite: it would erase
/// detail that is plainly visible at `maxzoom`.
///
/// **Which units.** `ST_SimplifyPreserveTopology` (and this module's
/// GeoPackage arm) works in the geometry's own CRS units, so the meters
/// above have to land in those:
///
/// - SRID `3857` — already meters. Used unchanged.
/// - SRID `4326` — divided by [`WORLD_CRS84_METERS_PER_DEGREE`], the same
///   equatorial meters-per-degree constant (`2 * pi * 6378137 / 360`, OGC
///   17-083r4 SS5.2.1) `tellurion-postgis` already uses to convert this exact
///   number for its own CRS84 tile arm. Equatorial is the conservative end
///   of that conversion: away from the equator a degree of longitude spans
///   `cos(lat)` times *fewer* meters, so the degree tolerance derived here is
///   smaller than one pixel there, and the column keeps more detail than it
///   strictly needs rather than less.
/// - anything else — refused by name. An honest conversion needs a
///   projection engine this CLI does not link, and guessing a number would
///   silently deform a collection's geometry. `--tolerance` is the explicit
///   way through, and it is the caller's units, not this function's problem.
///
/// `None` means "no derivation for this SRID"; the caller turns that into
/// the refusal that names `--tolerance`.
pub(crate) fn derive_tolerance_in_storage_units(
    variant: &GeometryVariantDecl,
    srid: i32,
) -> Option<f64> {
    let meters = simplify_tolerance_meters(variant.maxzoom);
    match srid {
        3857 => Some(meters),
        4326 => Some(meters / WORLD_CRS84_METERS_PER_DEGREE),
        _ => None,
    }
}

/// Whitelist-validates and double-quotes `name` for use as a SQL
/// identifier — this module's own local copy of the rule every other DDL
/// module in this crate hand-keeps (`index.rs`, `outbox.rs`, `seed.rs`,
/// `geopackage.rs`); see any of those for why it stays a local copy rather
/// than a shared helper. The same character set is valid in both Postgres
/// and SQLite, so one copy serves both arms here.
fn quote_ident(name: &str) -> anyhow::Result<String> {
    let mut chars = name.chars();
    let first = chars
        .next()
        .filter(|c| c.is_ascii_alphabetic() || *c == '_');
    if first.is_none() || name.len() > 63 || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        anyhow::bail!(
            "'{name}' is not a valid SQL identifier: only ASCII letters, digits, and '_' are allowed, it may not start with a digit, and it may not exceed 63 bytes"
        );
    }
    Ok(format!("\"{name}\""))
}

/// Single-quotes and escapes `value` for a SQL string literal — free-form
/// text, never an identifier; same distinction `geopackage.rs` documents.
fn quote_sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// A PostGIS geometry type name, restricted to the spellings the
/// `geometry_columns` view itself produces, so it can be interpolated into
/// a typmod. Never operator input — always read back from the base column —
/// but validated anyway, since it reaches SQL text as an identifier-shaped
/// token rather than a bind parameter.
fn quote_geometry_type(name: &str) -> anyhow::Result<String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        anyhow::bail!(
            "'{name}' is not a geometry type name this command will interpolate into a column type"
        );
    }
    Ok(name.to_string())
}

/// The full PostGIS materialization, as one idempotent statement batch.
///
/// * `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` types the variant
///   `geometry(<type>,<srid>)` — a typmod, not a bare `geometry`: the
///   `geometry_columns` view derives the SRID and type it reports from
///   exactly that, and a bare column would report srid `0`/type `GEOMETRY`
///   and fail `Router::refuse_invalid_geometry_variants`'s equality check
///   against the base column.
/// * The `UPDATE` is unconditional and rewrites every row, which is what
///   makes a rerun a *repopulation* rather than a no-op: the point of
///   rerunning is to pick up base-column writes that landed since (this
///   command is a batch backfill, see the module doc).
/// * `CREATE INDEX IF NOT EXISTS` gives the variant the same GiST index,
///   under the same `"<table>_<column>_gix"` name, `seed.rs` gives the base
///   column — see the module doc's "Indexing" section for why PostGIS gets
///   one and GeoPackage does not.
///
/// `ST_SimplifyPreserveTopology` (not `ST_Simplify`) for the same reason the
/// live tiles lane uses it: it never collapses a ring or drops a component,
/// so the materialized column keeps the base column's geometry type — which
/// the typmod above requires and the boot check compares.
pub(crate) fn postgis_materialize_sql(plan: &VariantPlan) -> anyhow::Result<String> {
    let table = quote_ident(&plan.table)?;
    let base = quote_ident(&plan.base_geometry)?;
    let variant = quote_ident(&plan.variant_column)?;
    let index = quote_ident(&format!("{}_{}_gix", plan.table, plan.variant_column))?;
    let geometry_type = quote_geometry_type(&plan.geometry_type)?;
    let srid = plan.srid;
    let tolerance = plan.tolerance;
    Ok(format!(
        "ALTER TABLE {table} ADD COLUMN IF NOT EXISTS {variant} geometry({geometry_type},{srid});
UPDATE {table} SET {variant} = ST_SimplifyPreserveTopology({base}, {tolerance:e});
CREATE INDEX IF NOT EXISTS {index} ON {table} USING GIST ({variant});"
    ))
}

/// The one `gpkg_geometry_columns` row that makes a GeoPackage variant
/// column visible to `tellurion-geopackage`'s `CatalogSource` — and
/// therefore to the boot-time check. `INSERT OR IGNORE` so a rerun is a
/// no-op rather than a primary-key violation, the same idempotence shape
/// `geopackage.rs`'s own provisioning DDL uses.
fn geopackage_register_variant_sql(plan: &VariantPlan) -> String {
    format!(
        "INSERT OR IGNORE INTO gpkg_geometry_columns (table_name, column_name, geometry_type_name, srs_id, z, m) VALUES ({table}, {column}, {geometry_type}, {srid}, 0, 0);",
        table = quote_sql_string(&plan.table),
        column = quote_sql_string(&plan.variant_column),
        geometry_type = quote_sql_string(&plan.geometry_type),
        srid = plan.srid,
    )
}

/// Does this file's `gpkg_geometry_columns` still carry the spec's
/// `uk_gc_table_name` unique constraint? Read off `sqlite_master.sql` (the
/// verbatim `CREATE TABLE` text) rather than `PRAGMA index_list`, because
/// SQLite materializes a table-level `UNIQUE` as an auto-index whose name
/// (`sqlite_autoindex_...`) says nothing about which constraint produced it.
///
/// A file provisioned by some other GeoPackage writer that omitted the
/// constraint needs no rebuild at all — hence a check, not an
/// unconditional migration.
pub(crate) fn declares_table_name_unique(create_sql: &str) -> bool {
    let normalized = create_sql.to_ascii_lowercase();
    normalized.contains("uk_gc_table_name")
        || normalized.contains("unique (table_name)")
        || normalized.contains("unique(table_name)")
}

/// Rebuilds `gpkg_geometry_columns` without `uk_gc_table_name`, preserving
/// every other constraint the spec defines and every existing row. See
/// [`GEOPACKAGE_SECOND_COLUMN_NOTE`] for what this costs and why nothing
/// less will do; only ever reached behind `--allow-second-geometry-column`.
///
/// SQLite cannot drop a table constraint in place (the `UNIQUE` is backed by
/// an implicit auto-index that `DROP INDEX` refuses), so the rebuild is the
/// documented `CREATE new / INSERT SELECT / DROP old / RENAME` dance, run
/// inside the caller's transaction so a failure leaves the original table
/// untouched.
fn geopackage_relax_geometry_columns_sql() -> &'static str {
    "CREATE TABLE gpkg_geometry_columns_tellurion_new (
    table_name TEXT NOT NULL,
    column_name TEXT NOT NULL,
    geometry_type_name TEXT NOT NULL,
    srs_id INTEGER NOT NULL,
    z TINYINT NOT NULL,
    m TINYINT NOT NULL,
    CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name),
    CONSTRAINT fk_gc_tn FOREIGN KEY (table_name) REFERENCES gpkg_contents(table_name),
    CONSTRAINT fk_gc_srs FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys(srs_id)
);
INSERT INTO gpkg_geometry_columns_tellurion_new (table_name, column_name, geometry_type_name, srs_id, z, m)
    SELECT table_name, column_name, geometry_type_name, srs_id, z, m FROM gpkg_geometry_columns;
DROP TABLE gpkg_geometry_columns;
ALTER TABLE gpkg_geometry_columns_tellurion_new RENAME TO gpkg_geometry_columns;"
}

/// Simplifies one decoded geometry with `tolerance`, expressed as a distance
/// in the storage CRS's own units — the same quantity PostGIS's
/// `ST_SimplifyPreserveTopology` takes, so one `--tolerance` and one
/// derivation serve both backends.
///
/// The algorithm is `geo`'s topology-preserving Visvalingam-Whyatt
/// (`SimplifyVwPreserve`), the only simplification in this workspace's
/// dependency set that guarantees the result stays a valid, non-
/// self-intersecting geometry — matching the guarantee the *name*
/// `ST_SimplifyPreserveTopology` makes on the PostGIS side. Keeping that
/// guarantee matters more here than matching PostGIS's Douglas-Peucker
/// point-selection exactly: this column is persisted, and a persisted
/// invalid ring would outlive the run that wrote it.
///
/// VW's own threshold is an **area** (the triangle a candidate vertex forms
/// with its neighbours), not a distance, so the one-pixel yardstick converts
/// the only way a length converts to an area: squared. `tolerance` is one
/// tile pixel's ground size, `tolerance * tolerance` is that pixel's ground
/// area, and a vertex contributing less than one pixel of area is exactly
/// the sub-pixel detail the derivation set out to drop.
///
/// Point/MultiPoint geometries pass through untouched — there is no vertex
/// to drop without dropping the feature itself — as do the collection types
/// `geo`'s simplification does not implement, which reach here only from a
/// backend whose base column declared them.
fn simplify_in_storage_units(
    geometry: geo_types::Geometry<f64>,
    tolerance: f64,
) -> geo_types::Geometry<f64> {
    use geo_types::Geometry;
    let area_epsilon = tolerance * tolerance;
    match geometry {
        Geometry::LineString(g) => Geometry::LineString(g.simplify_vw_preserve(&area_epsilon)),
        Geometry::MultiLineString(g) => {
            Geometry::MultiLineString(g.simplify_vw_preserve(&area_epsilon))
        }
        Geometry::Polygon(g) => Geometry::Polygon(g.simplify_vw_preserve(&area_epsilon)),
        Geometry::MultiPolygon(g) => Geometry::MultiPolygon(g.simplify_vw_preserve(&area_epsilon)),
        // `geo` implements `SimplifyVwPreserve` for the four line/area
        // types above and nothing else, so a collection recurses into its
        // members one at a time rather than reaching for a second
        // algorithm — same tolerance, same guarantee, member by member.
        Geometry::GeometryCollection(g) => {
            Geometry::GeometryCollection(geo_types::GeometryCollection(
                g.0.into_iter()
                    .map(|member| simplify_in_storage_units(member, tolerance))
                    .collect(),
            ))
        }
        // `Point`/`MultiPoint` (no removable vertex without dropping the
        // feature) and `Line`/`Rect`/`Triangle` (fixed vertex counts a WKB
        // body never decodes to anyway) pass through untouched.
        other => other,
    }
}

/// Registers the five scalar SQL functions the GeoPackage spec's own Annex
/// L R*Tree maintenance triggers call — `ST_MinX`/`ST_MaxX`/`ST_MinY`/
/// `ST_MaxY`/`ST_IsEmpty` — on a connection this command is about to write
/// through.
///
/// Not optional, and not about the variant's own indexing: SQLite resolves
/// the function names in a trigger body when the *triggering statement is
/// prepared*, not lazily when a `WHEN` clause turns out true. The
/// `rtree_<table>_<geometry>_update4` trigger `geopackage create-tables`
/// installs is an unqualified `AFTER UPDATE ON <table>`, so it is attached
/// to this module's `UPDATE ... SET <variant> = ?` even though its `WHEN
/// OLD.id != NEW.id` guard can never fire here — and without these five
/// registered, that `UPDATE` fails to compile at all with `no such
/// function: ST_IsEmpty`. `tellurion-geopackage::functions` registers the
/// same five on every connection *it* opens; this DDL-only module opens its
/// own (see `geopackage.rs`'s doc for why), so it carries its own copy.
///
/// The implementations read the GPB header's own envelope when the blob
/// carries one (which every geometry this workspace writes does), and fall
/// back to decoding the WKB body when it does not — never a guess.
fn register_gpkg_envelope_functions(conn: &Connection) -> anyhow::Result<()> {
    use rusqlite::functions::FunctionFlags;
    use rusqlite::types::ValueRef;

    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    for (name, index) in [
        ("ST_MinX", 0usize),
        ("ST_MaxX", 1),
        ("ST_MinY", 2),
        ("ST_MaxY", 3),
    ] {
        conn.create_scalar_function(name, 1, flags, move |ctx| {
            Ok(match ctx.get_raw(0) {
                ValueRef::Null => None,
                other => gpkg_xy_envelope(other.as_blob()?).map(|envelope| envelope[index]),
            })
        })
        .with_context(|| format!("registering the '{name}' R*Tree trigger function"))?;
    }
    conn.create_scalar_function("ST_IsEmpty", 1, flags, move |ctx| {
        Ok(match ctx.get_raw(0) {
            ValueRef::Null => None,
            // GPB header flags, bit 4: the spec's own empty-geometry flag.
            other => other
                .as_blob()?
                .get(3)
                .map(|flags| i64::from((flags >> 4) & 1)),
        })
    })
    .context("registering the 'ST_IsEmpty' R*Tree trigger function")?;
    Ok(())
}

/// `[minx, maxx, miny, maxy]` for a stored GPB blob — the header's own
/// envelope when its indicator code says one is present, else the bounding
/// rectangle of the decoded WKB body. `None` for a blob with neither (an
/// empty geometry), which is exactly when the R*Tree triggers' `WHEN`
/// guards keep the row out of the index anyway.
fn gpkg_xy_envelope(blob: &[u8]) -> Option<[f64; 4]> {
    const HEADER_FIXED_LEN: usize = 8;
    let flags = *blob.get(3)?;
    let little_endian = flags & 0b0000_0001 == 1;
    let envelope_code = (flags >> 1) & 0b0000_0111;
    if envelope_code >= 1 && blob.len() >= HEADER_FIXED_LEN + 32 {
        let mut values = [0f64; 4];
        for (index, value) in values.iter_mut().enumerate() {
            let start = HEADER_FIXED_LEN + index * 8;
            let bytes: [u8; 8] = blob[start..start + 8].try_into().ok()?;
            *value = if little_endian {
                f64::from_le_bytes(bytes)
            } else {
                f64::from_be_bytes(bytes)
            };
        }
        return Some(values);
    }
    use geo::BoundingRect;
    let rect = GpkgWkb(blob).to_geo().ok()?.bounding_rect()?;
    Some([rect.min().x, rect.max().x, rect.min().y, rect.max().y])
}

/// Resolves the collection and its storage out of a validated config.
fn resolve_collection<'a>(
    config: &'a AppConfig,
    collection_id: &str,
) -> anyhow::Result<(&'a CollectionDecl, &'a StorageDecl)> {
    let collection = config
        .collections
        .iter()
        .find(|c| c.id == collection_id)
        .ok_or_else(|| {
            anyhow::anyhow!("config declares no collection with id '{collection_id}'")
        })?;
    let storage = config
        .storages
        .iter()
        .find(|s| s.id == collection.storage)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "collection '{collection_id}' names storage '{}', which the config does not declare",
                collection.storage
            )
        })?;
    Ok((collection, storage))
}

/// The declared variants this run will materialize: all of them, or the one
/// `--variant` names. A `--variant` the collection does not declare is
/// refused rather than materialized as an undeclared column — the config is
/// the source of truth here, exactly as it is for the tiles lane.
fn selected_variants<'a>(
    collection: &'a CollectionDecl,
    wanted: Option<&str>,
) -> anyhow::Result<Vec<&'a GeometryVariantDecl>> {
    if collection.geometry_variants.is_empty() {
        anyhow::bail!(
            "collection '{}' declares no geometry_variants; nothing to materialize",
            collection.id
        );
    }
    let Some(wanted) = wanted else {
        return Ok(collection.geometry_variants.iter().collect());
    };
    let found = collection
        .geometry_variants
        .iter()
        .find(|variant| variant.column == wanted)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "collection '{}' declares no geometry_variants entry for column '{wanted}' (declared: {})",
                collection.id,
                collection
                    .geometry_variants
                    .iter()
                    .map(|v| v.column.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    Ok(vec![found])
}

/// The tolerance for one variant: `--tolerance` if given, else the
/// derivation, else a refusal that names both the SRID and the way out.
fn tolerance_for(
    collection_id: &str,
    variant: &GeometryVariantDecl,
    srid: i32,
    override_tolerance: Option<f64>,
) -> anyhow::Result<f64> {
    if let Some(tolerance) = override_tolerance {
        if !(tolerance.is_finite() && tolerance > 0.0) {
            anyhow::bail!("--tolerance must be a finite, positive number, got {tolerance}");
        }
        return Ok(tolerance);
    }
    derive_tolerance_in_storage_units(variant, srid).ok_or_else(|| {
        anyhow::anyhow!(
            "collection '{collection_id}': geometry_variants entry '{}' is stored in srid {srid}, \
             for which this command derives no tolerance (it derives one only for {:?}, whose \
             units it can reach from a tile pixel without a projection engine). Pass an explicit \
             --tolerance in that CRS's own units instead.",
            variant.column,
            DERIVABLE_SRIDS
        )
    })
}

pub async fn materialize(args: MaterializeArgs) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(&args.config)
        .with_context(|| format!("reading config '{}'", args.config.display()))?;
    let config: AppConfig = serde_yaml::from_str(&text)
        .with_context(|| format!("parsing config '{}'", args.config.display()))?;
    // The same shape validation the server runs at boot — it is what
    // guarantees the zoom ranges this command's tolerance derivation reads
    // are well-formed and non-overlapping (`validate_geometry_variants`).
    config
        .validate()
        .with_context(|| format!("validating config '{}'", args.config.display()))?;

    let (collection, storage) = resolve_collection(&config, &args.collection)?;
    let variants = selected_variants(collection, args.variant.as_deref())?;
    let table = tellurion_core::descriptor::target_table(collection).to_string();

    match storage.driver.as_str() {
        "postgis" => materialize_postgis(&args, collection, storage, &table, &variants).await,
        "geopackage" => materialize_geopackage(&args, collection, storage, &table, &variants),
        other => anyhow::bail!(
            "collection '{}' is served by storage '{}' (driver '{other}'); geometry variants can \
             only be materialized for the 'postgis' and 'geopackage' drivers, the two that can \
             serve one",
            collection.id,
            storage.id
        ),
    }
}

/// The base geometry column plus its type and SRID, read back from the
/// backend rather than assumed: the variant column has to match it exactly
/// for `Router::refuse_invalid_geometry_variants` to accept the config, and
/// only the backend knows what the base column really is.
#[derive(Debug)]
struct BaseColumn {
    column: String,
    geometry_type: String,
    srid: i32,
}

/// Picks the base geometry column out of everything the backend reports for
/// one table, applying the same rule `Router` does: a `geometry:` pin names
/// it; otherwise the sole reported column is it, and more than one candidate
/// is refused rather than arbitrarily picked (`refuse_ambiguous_geometry_
/// column`'s own rule, restated here because this CLI reads the backend
/// directly rather than through a `CatalogSource`).
fn pick_base_column(
    collection: &CollectionDecl,
    table: &str,
    mut candidates: Vec<BaseColumn>,
) -> anyhow::Result<BaseColumn> {
    if candidates.is_empty() {
        anyhow::bail!("table '{table}' reports no geometry column at all");
    }
    if let Some(pinned) = collection.geometry.as_deref() {
        return candidates
            .into_iter()
            .find(|candidate| candidate.column == pinned)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "collection '{}' pins geometry column '{pinned}', which table '{table}' does not report",
                    collection.id
                )
            });
    }
    // A variant column materialized by an earlier run is itself reported
    // here; it is never the base, so it must not turn a single-column table
    // into an "ambiguous" one.
    let declared_variants: Vec<&str> = collection
        .geometry_variants
        .iter()
        .map(|v| v.column.as_str())
        .collect();
    candidates.retain(|candidate| !declared_variants.contains(&candidate.column.as_str()));
    match candidates.len() {
        0 => anyhow::bail!(
            "table '{table}' reports only columns this collection declares as geometry_variants, \
             leaving no base geometry column; pin one with the collection's 'geometry' config key"
        ),
        1 => Ok(candidates.remove(0)),
        _ => anyhow::bail!(
            "table '{table}' reports {} geometry columns ({}) and collection '{}' pins none — set \
             its 'geometry' config key to the base column",
            candidates.len(),
            candidates
                .iter()
                .map(|c| c.column.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            collection.id
        ),
    }
}

async fn materialize_postgis(
    args: &MaterializeArgs,
    collection: &CollectionDecl,
    storage: &StorageDecl,
    table: &str,
    variants: &[&GeometryVariantDecl],
) -> anyhow::Result<()> {
    let client = crate::db::connect(&storage.url_env).await?;
    let rows = client
        .query(
            "SELECT f_geometry_column, type, srid FROM geometry_columns \
             WHERE f_table_schema = 'public' AND f_table_name = $1",
            &[&table],
        )
        .await
        .with_context(|| format!("reading geometry_columns for table '{table}'"))?;
    let candidates = rows
        .iter()
        .map(|row| BaseColumn {
            column: row.get(0),
            geometry_type: row.get(1),
            srid: row.get(2),
        })
        .collect();
    let base = pick_base_column(collection, table, candidates)?;

    for variant in variants {
        let plan = VariantPlan {
            table: table.to_string(),
            base_geometry: base.column.clone(),
            variant_column: variant.column.clone(),
            geometry_type: base.geometry_type.clone(),
            srid: base.srid,
            tolerance: tolerance_for(&collection.id, variant, base.srid, args.tolerance)?,
        };
        let sql = postgis_materialize_sql(&plan)?;
        // Always printed, dry run or not — same requirement every
        // `create-tables` command in this crate follows.
        println!("{sql}");
        if args.dry_run {
            continue;
        }
        // `#272`: the `BEGIN`/`COMMIT` this already had becomes an
        // advisory-locked one, on the subject table's own name. `ALTER TABLE
        // ... ADD COLUMN IF NOT EXISTS` is safe on its own (it holds an
        // `AccessExclusiveLock` and re-reads the catalog under it), but the
        // `CREATE INDEX IF NOT EXISTS` beside it is not: `CREATE INDEX`
        // holds only a `ShareLock` on the table, which is compatible with
        // itself, so two concurrent materializations both pass the
        // existence check and the loser fails on
        // `pg_class_relname_nsp_index`. Two operators materializing
        // different variants of the same table is exactly the case, and
        // this command is the one that can hold the lock for a while — it
        // backfills the column — so a second one gets the named
        // `PROVISIONING LOCK BUSY` refusal rather than a silent wait.
        crate::provision::apply_ddl(&client, table, &sql)
            .await
            .with_context(|| {
                format!(
                    "materializing geometry variant '{}' on table '{table}'",
                    variant.column
                )
            })?;
        tracing::info!(
            table = %table,
            column = %variant.column,
            tolerance = plan.tolerance,
            "materialized the geometry variant column and its GiST index"
        );
    }
    Ok(())
}

fn materialize_geopackage(
    args: &MaterializeArgs,
    collection: &CollectionDecl,
    storage: &StorageDecl,
    table: &str,
    variants: &[&GeometryVariantDecl],
) -> anyhow::Result<()> {
    let path = std::env::var(&storage.url_env).map_err(|_| {
        anyhow::anyhow!(
            "environment variable '{}' (storage '{}') is not set",
            storage.url_env,
            storage.id
        )
    })?;
    // `#272`: no advisory lock on the GeoPackage arm — SQLite's
    // single-writer transaction already gives the atomicity PostgreSQL's
    // `IF NOT EXISTS` lacks — but it does get the busy timeout, so a
    // concurrent writer is waited for and then named rather than failing
    // instantly with SQLite's own "database is locked". See
    // `provision::open_geopackage`.
    let mut conn = crate::provision::open_geopackage(std::path::Path::new(&path))?;
    register_gpkg_envelope_functions(&conn)?;

    let candidates = {
        let mut stmt = conn.prepare(
            "SELECT column_name, geometry_type_name, srs_id FROM gpkg_geometry_columns \
             WHERE table_name = ?1",
        )?;
        let rows = stmt.query_map([table], |row| {
            Ok(BaseColumn {
                column: row.get(0)?,
                geometry_type: row.get(1)?,
                srid: row.get(2)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let base = pick_base_column(collection, table, candidates)?;

    for variant in variants {
        let plan = VariantPlan {
            table: table.to_string(),
            base_geometry: base.column.clone(),
            variant_column: variant.column.clone(),
            geometry_type: base.geometry_type.clone(),
            srid: base.srid,
            tolerance: tolerance_for(&collection.id, variant, base.srid, args.tolerance)?,
        };
        println!(
            "-- geopackage: table {}, base {}, variant {}, srid {}, tolerance {:e} (storage units)",
            plan.table, plan.base_geometry, plan.variant_column, plan.srid, plan.tolerance
        );
        println!("{}", geopackage_add_column_sql(&plan)?);
        println!("{}", geopackage_register_variant_sql(&plan));
        if args.dry_run {
            continue;
        }
        let updated = geopackage_apply(&mut conn, &plan, args.allow_second_geometry_column)?;
        tracing::info!(
            path = %path,
            table = %plan.table,
            column = %plan.variant_column,
            tolerance = plan.tolerance,
            rows = updated,
            "materialized the geometry variant column (no R*Tree: see this module's own doc)"
        );
    }
    Ok(())
}

/// `ALTER TABLE ... ADD COLUMN`, with the base column's own declared type
/// text. SQLite has no `IF NOT EXISTS` for `ADD COLUMN`, so the caller
/// checks `PRAGMA table_info` first — the idempotence this statement itself
/// cannot express.
fn geopackage_add_column_sql(plan: &VariantPlan) -> anyhow::Result<String> {
    let table = quote_ident(&plan.table)?;
    let column = quote_ident(&plan.variant_column)?;
    let geometry_type = quote_geometry_type(&plan.geometry_type)?;
    Ok(format!(
        "ALTER TABLE {table} ADD COLUMN {column} {geometry_type};"
    ))
}

fn geopackage_column_exists(conn: &Connection, table: &str, column: &str) -> anyhow::Result<bool> {
    let ident = quote_ident(table)?;
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({ident})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        if row.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The whole GeoPackage side effect, in one transaction: relax the metadata
/// constraint if consent was given and it is in the way, add the column if
/// absent, register it, then rewrite every row's variant geometry. Returns
/// the number of rows written.
fn geopackage_apply(
    conn: &mut Connection,
    plan: &VariantPlan,
    allow_second_geometry_column: bool,
) -> anyhow::Result<usize> {
    let transaction = conn.transaction()?;

    let already_registered: i64 = transaction.query_row(
        "SELECT count(*) FROM gpkg_geometry_columns WHERE table_name = ?1 AND column_name = ?2",
        rusqlite::params![&plan.table, &plan.variant_column],
        |row| row.get(0),
    )?;
    if already_registered == 0 {
        let create_sql: Option<String> = transaction
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'gpkg_geometry_columns'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let create_sql = create_sql.ok_or_else(|| {
            anyhow::anyhow!(
                "'{}' has no gpkg_geometry_columns table; provision it with 'tellurion-ingest geopackage create-tables' first",
                plan.table
            )
        })?;
        if declares_table_name_unique(&create_sql) {
            if !allow_second_geometry_column {
                anyhow::bail!(
                    "cannot register geometry variant column '{}' on table '{}': {}",
                    plan.variant_column,
                    plan.table,
                    GEOPACKAGE_SECOND_COLUMN_NOTE
                );
            }
            transaction
                .execute_batch(geopackage_relax_geometry_columns_sql())
                .context("rebuilding gpkg_geometry_columns without uk_gc_table_name")?;
            tracing::warn!(
                table = %plan.table,
                "rebuilt gpkg_geometry_columns without its uk_gc_table_name unique constraint so a second geometry column can be registered"
            );
        }
    }

    if !geopackage_column_exists(&transaction, &plan.table, &plan.variant_column)? {
        transaction
            .execute_batch(&geopackage_add_column_sql(plan)?)
            .with_context(|| format!("adding column '{}'", plan.variant_column))?;
    }
    transaction
        .execute_batch(&geopackage_register_variant_sql(plan))
        .with_context(|| format!("registering column '{}'", plan.variant_column))?;

    let written = geopackage_populate(&transaction, plan)?;
    transaction.commit()?;
    Ok(written)
}

/// Reads every row's base geometry, simplifies it, and writes the result
/// into the variant column. A `NULL` or undecodable-as-empty base geometry
/// writes `NULL` — the same "no geometry" state the base column itself is
/// allowed to hold, and what the tiles lane already skips.
///
/// The whole pass runs in the caller's transaction: either the column is
/// fully populated or the file is untouched. Rows are read into memory one
/// batch of `(rowid, blob)` at a time rather than streamed, because the
/// `UPDATE` below writes to the same table the `SELECT` reads — SQLite
/// permits that on separate statements but the read cursor's view of a
/// rewritten page is not something this command should depend on.
fn geopackage_populate(
    conn: &rusqlite::Transaction<'_>,
    plan: &VariantPlan,
) -> anyhow::Result<usize> {
    let table = quote_ident(&plan.table)?;
    let base = quote_ident(&plan.base_geometry)?;
    let variant = quote_ident(&plan.variant_column)?;

    let rows: Vec<(i64, Option<Vec<u8>>)> = {
        let mut stmt = conn.prepare(&format!("SELECT rowid, {base} FROM {table}"))?;
        let mapped = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut update = conn.prepare(&format!(
        "UPDATE {table} SET {variant} = ?1 WHERE rowid = ?2"
    ))?;
    let mut written = 0usize;
    for (rowid, blob) in rows {
        let simplified =
            match blob {
                None => None,
                Some(bytes) => Some(simplify_gpkg_blob(&bytes, plan).with_context(|| {
                    format!("simplifying row {rowid} of table '{}'", plan.table)
                })?),
            };
        update.execute(rusqlite::params![simplified, rowid])?;
        written += 1;
    }
    Ok(written)
}

/// GPB blob in, GPB blob out: decode the GeoPackage geometry BLOB to
/// `geo_types`, simplify, re-encode.
///
/// The re-encode goes through `geozero`'s own GeoPackage WKB dialect, whose
/// header this crate deliberately does not hand-roll: it writes the same
/// little-endian, envelope-indicator-1 (`[minx, maxx, miny, maxy]`) header
/// `tellurion-geopackage::gpb` writes and reads, which is what keeps the
/// driver able to read what this command wrote. The envelope is recomputed
/// from the *simplified* geometry, never copied from the input — a
/// simplification can only shrink a bounding box, and a stale (larger)
/// envelope in the header would misreport the row to any reader that trusts
/// it.
fn simplify_gpkg_blob(bytes: &[u8], plan: &VariantPlan) -> anyhow::Result<Vec<u8>> {
    let geometry = GpkgWkb(bytes)
        .to_geo()
        .map_err(|err| anyhow::anyhow!("decoding the stored GeoPackage geometry: {err}"))?;
    let simplified = simplify_in_storage_units(geometry, plan.tolerance);
    let envelope = xy_envelope(&simplified);
    simplified
        .to_gpkg_wkb(CoordDimensions::xy(), Some(plan.srid), envelope)
        .map_err(|err| anyhow::anyhow!("encoding the simplified GeoPackage geometry: {err}"))
}

/// `[minx, maxx, miny, maxy]` — the GPB envelope-indicator-1 ordering (which
/// is *not* the more common `[minx, miny, maxx, maxy]`), matching both
/// `geozero`'s writer and `tellurion-geopackage::gpb`'s reader. An empty
/// geometry yields an empty vector, which makes `geozero` write the
/// no-envelope header instead of four `NaN`s.
fn xy_envelope(geometry: &geo_types::Geometry<f64>) -> Vec<f64> {
    use geo::BoundingRect;
    match geometry.bounding_rect() {
        Some(rect) => vec![rect.min().x, rect.max().x, rect.min().y, rect.max().y],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variant(column: &str, minzoom: u8, maxzoom: u8) -> GeometryVariantDecl {
        GeometryVariantDecl {
            column: column.to_string(),
            minzoom,
            maxzoom,
        }
    }

    fn plan(tolerance: f64) -> VariantPlan {
        VariantPlan {
            table: "demo".to_string(),
            base_geometry: "geom".to_string(),
            variant_column: "geom_z6".to_string(),
            geometry_type: "GEOMETRY".to_string(),
            srid: 4326,
            tolerance,
        }
    }

    /// The derivation is exactly "one 256px tile pixel at the variant's
    /// finest zoom", in the storage CRS's units — not a constant this module
    /// invented. Pinned against `tellurion-core`'s own heuristic so a change
    /// there cannot silently drift the materialized column away from what
    /// the live tiles lane simplifies to at the same zoom.
    #[test]
    fn tolerance_for_3857_is_the_tile_pixel_in_meters_at_the_variants_maxzoom() {
        let derived = derive_tolerance_in_storage_units(&variant("geom_z6", 0, 6), 3857).unwrap();
        assert_eq!(derived, simplify_tolerance_meters(6));
    }

    #[test]
    fn tolerance_for_4326_is_that_same_pixel_converted_to_equatorial_degrees() {
        let derived = derive_tolerance_in_storage_units(&variant("geom_z6", 0, 6), 4326).unwrap();
        assert_eq!(
            derived,
            simplify_tolerance_meters(6) / WORLD_CRS84_METERS_PER_DEGREE
        );
        // Sanity on the constant itself: one whole-world zoom-0 pixel is
        // 360/256 degrees of longitude by construction of the Web Mercator
        // grid, which is what the meters-per-degree conversion has to
        // reproduce at the equator.
        let z0 = derive_tolerance_in_storage_units(&variant("geom_z0", 0, 0), 4326).unwrap();
        assert!(
            (z0 - 360.0 / 256.0).abs() < 1e-6,
            "zoom-0 degree tolerance was {z0}"
        );
    }

    /// `maxzoom`, never `minzoom`: the variant is read at every zoom in its
    /// range, so the finest one sets the budget. Halving the pixel each
    /// level up means a variant reaching one zoom deeper gets exactly half
    /// the tolerance.
    #[test]
    fn tolerance_follows_maxzoom_and_halves_per_extra_zoom_level() {
        let shallow = derive_tolerance_in_storage_units(&variant("v", 0, 6), 3857).unwrap();
        let deep = derive_tolerance_in_storage_units(&variant("v", 0, 7), 3857).unwrap();
        assert!((deep * 2.0 - shallow).abs() < 1e-9);
        // The range's low end does not enter the derivation at all.
        let same_max = derive_tolerance_in_storage_units(&variant("v", 4, 6), 3857).unwrap();
        assert_eq!(shallow, same_max);
    }

    /// No magic constant for a CRS whose units this command cannot reach
    /// from a tile pixel — refused by name, with `--tolerance` as the
    /// documented way through.
    #[test]
    fn no_tolerance_is_derived_for_a_srid_the_command_cannot_convert_into() {
        assert!(derive_tolerance_in_storage_units(&variant("v", 0, 6), 25832).is_none());
        let err = tolerance_for("demo", &variant("v", 0, 6), 25832, None).unwrap_err();
        assert!(format!("{err}").contains("--tolerance"), "error was: {err}");
    }

    #[test]
    fn an_explicit_tolerance_wins_over_the_derivation_and_must_be_positive() {
        assert_eq!(
            tolerance_for("demo", &variant("v", 0, 6), 3857, Some(12.5)).unwrap(),
            12.5
        );
        assert!(tolerance_for("demo", &variant("v", 0, 6), 3857, Some(0.0)).is_err());
        assert!(tolerance_for("demo", &variant("v", 0, 6), 3857, Some(-1.0)).is_err());
    }

    /// Every statement is idempotent, the column carries the base column's
    /// full typmod (which is what makes `geometry_columns` report a matching
    /// srid/type at boot), and the variant gets the same GiST index under
    /// the same naming convention `seed.rs` gives the base column.
    #[test]
    fn postgis_sql_is_idempotent_and_typmods_and_indexes_the_variant() {
        let sql = postgis_materialize_sql(&plan(1.5)).unwrap();
        assert!(
            sql.contains(
                "ALTER TABLE \"demo\" ADD COLUMN IF NOT EXISTS \"geom_z6\" geometry(GEOMETRY,4326)"
            ),
            "sql was: {sql}"
        );
        assert!(
            sql.contains("UPDATE \"demo\" SET \"geom_z6\" = ST_SimplifyPreserveTopology(\"geom\","),
            "sql was: {sql}"
        );
        assert!(
            sql.contains(
                "CREATE INDEX IF NOT EXISTS \"demo_geom_z6_gix\" ON \"demo\" USING GIST (\"geom_z6\")"
            ),
            "sql was: {sql}"
        );
    }

    #[test]
    fn postgis_sql_refuses_an_identifier_that_fails_whitelisting() {
        let mut bad = plan(1.5);
        bad.variant_column = "geom-z6".to_string();
        assert!(postgis_materialize_sql(&bad).is_err());
        let mut bad = plan(1.5);
        bad.geometry_type = "GEOMETRY); DROP TABLE demo; --".to_string();
        assert!(postgis_materialize_sql(&bad).is_err());
    }

    /// The spec's own `gpkg_geometry_columns` definition is recognized as
    /// carrying the constraint (however it is spelled), and a writer that
    /// left it out is recognized as not needing the rebuild.
    #[test]
    fn the_table_name_unique_constraint_is_detected_in_the_shapes_that_carry_it() {
        assert!(declares_table_name_unique(
            "CREATE TABLE gpkg_geometry_columns (table_name TEXT NOT NULL, \
             CONSTRAINT uk_gc_table_name UNIQUE (table_name))"
        ));
        assert!(declares_table_name_unique(
            "CREATE TABLE gpkg_geometry_columns (table_name TEXT NOT NULL UNIQUE(table_name))"
        ));
        assert!(!declares_table_name_unique(
            "CREATE TABLE gpkg_geometry_columns (table_name TEXT NOT NULL, column_name TEXT \
             NOT NULL, CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name))"
        ));
    }

    /// The rebuild keeps every other constraint the spec defines and every
    /// existing row — only `uk_gc_table_name` goes.
    #[test]
    fn the_metadata_rebuild_drops_only_the_one_constraint() {
        let sql = geopackage_relax_geometry_columns_sql();
        assert!(!sql.contains("uk_gc_table_name"));
        assert!(sql.contains("CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name)"));
        assert!(sql.contains("CONSTRAINT fk_gc_tn FOREIGN KEY"));
        assert!(sql.contains("CONSTRAINT fk_gc_srs FOREIGN KEY"));
        assert!(sql.contains("SELECT table_name, column_name, geometry_type_name, srs_id, z, m FROM gpkg_geometry_columns"));
    }

    /// Simplification actually drops sub-tolerance detail and keeps the
    /// shape's defining vertices — and a point, having no removable vertex,
    /// comes through untouched.
    #[test]
    fn simplification_drops_sub_tolerance_vertices_and_leaves_points_alone() {
        let line: geo_types::Geometry<f64> = geo_types::LineString::from(vec![
            (0.0, 0.0),
            (5.0, 0.000_1),
            (10.0, 0.0),
            (15.0, 10.0),
        ])
        .into();
        let simplified = simplify_in_storage_units(line, 1.0);
        let geo_types::Geometry::LineString(simplified) = simplified else {
            panic!("a LineString stays a LineString");
        };
        assert_eq!(simplified.0.len(), 3, "the collinear-ish wiggle is dropped");

        let point: geo_types::Geometry<f64> = geo_types::Point::new(1.0, 2.0).into();
        assert_eq!(
            simplify_in_storage_units(point.clone(), 1.0),
            point,
            "a point has no vertex to drop"
        );
    }

    /// A GPB blob round-trips through the codec this command writes with,
    /// and the header it produces is the one `tellurion-geopackage::gpb`
    /// reads: little-endian, envelope indicator 1, the collection's srid.
    #[test]
    fn a_simplified_blob_is_a_well_formed_gpb_with_a_recomputed_envelope() {
        let line: geo_types::Geometry<f64> =
            geo_types::LineString::from(vec![(0.0, 0.0), (1.0, 0.000_01), (2.0, 0.0)]).into();
        let blob = line
            .to_gpkg_wkb(
                CoordDimensions::xy(),
                Some(4326),
                vec![0.0, 2.0, 0.0, 0.000_01],
            )
            .unwrap();

        let mut plan = plan(0.5);
        plan.srid = 4326;
        let out = simplify_gpkg_blob(&blob, &plan).unwrap();

        assert_eq!(&out[0..2], b"GP");
        assert_eq!(out[2], 0, "GPB version 0");
        assert_eq!(out[3] & 0b0000_0001, 1, "little-endian header");
        assert_eq!((out[3] >> 1) & 0b0000_0111, 1, "2D envelope, indicator 1");
        assert_eq!(i32::from_le_bytes(out[4..8].try_into().unwrap()), 4326);
        // Envelope is recomputed from the simplified geometry, in the
        // spec's [minx, maxx, miny, maxy] order.
        let envelope: Vec<f64> = (0..4)
            .map(|i| f64::from_le_bytes(out[8 + i * 8..16 + i * 8].try_into().unwrap()))
            .collect();
        assert_eq!(envelope[0], 0.0);
        assert_eq!(envelope[1], 2.0);
        assert_eq!(
            envelope[2], 0.0,
            "the dropped mid vertex no longer stretches maxy"
        );
        assert_eq!(envelope[3], 0.0);

        // And the driver's own reader accepts what we wrote.
        let decoded = GpkgWkb(&out[..]).to_geo().unwrap();
        let geo_types::Geometry::LineString(decoded) = decoded else {
            panic!("a LineString stays a LineString");
        };
        assert_eq!(decoded.0.len(), 2);
    }

    fn collection_yaml(variants: &str) -> CollectionDecl {
        serde_yaml::from_str(&format!(
            "id: demo\ncatalog: default\nstorage: main\ntiles:\n  minzoom: 0\n  maxzoom: 14\ngeometry_variants:\n{variants}"
        ))
        .unwrap()
    }

    #[test]
    fn selected_variants_defaults_to_every_declared_entry() {
        let decl = collection_yaml("  - column: geom_z6\n    minzoom: 0\n    maxzoom: 6\n  - column: geom_z11\n    minzoom: 7\n    maxzoom: 11\n");
        let all = selected_variants(&decl, None).unwrap();
        assert_eq!(all.len(), 2);
        let one = selected_variants(&decl, Some("geom_z11")).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].column, "geom_z11");
    }

    #[test]
    fn selected_variants_refuses_a_column_the_config_never_declared() {
        let decl = collection_yaml("  - column: geom_z6\n    minzoom: 0\n    maxzoom: 6\n");
        assert!(selected_variants(&decl, Some("geom_z11")).is_err());
        let bare = collection_yaml("  []\n");
        assert!(selected_variants(&bare, None).is_err());
    }

    /// A variant column an earlier run already materialized is reported by
    /// the backend alongside the base column; it must not make the base
    /// column look ambiguous, or the command would stop being idempotent
    /// after its own first success.
    #[test]
    fn a_previously_materialized_variant_does_not_make_the_base_column_ambiguous() {
        let decl = collection_yaml("  - column: geom_z6\n    minzoom: 0\n    maxzoom: 6\n");
        let base = pick_base_column(
            &decl,
            "demo",
            vec![
                BaseColumn {
                    column: "geom".to_string(),
                    geometry_type: "GEOMETRY".to_string(),
                    srid: 4326,
                },
                BaseColumn {
                    column: "geom_z6".to_string(),
                    geometry_type: "GEOMETRY".to_string(),
                    srid: 4326,
                },
            ],
        )
        .unwrap();
        assert_eq!(base.column, "geom");
    }

    #[test]
    fn two_undeclared_geometry_columns_with_no_pin_are_refused_rather_than_picked() {
        let decl = collection_yaml("  []\n");
        let err = pick_base_column(
            &decl,
            "demo",
            vec![
                BaseColumn {
                    column: "geom".to_string(),
                    geometry_type: "GEOMETRY".to_string(),
                    srid: 4326,
                },
                BaseColumn {
                    column: "other".to_string(),
                    geometry_type: "GEOMETRY".to_string(),
                    srid: 4326,
                },
            ],
        )
        .unwrap_err();
        assert!(format!("{err}").contains("'geometry' config key"), "{err}");
    }

    /// End-to-end against a real `.gpkg` file provisioned by this crate's
    /// own `geopackage create-tables`: the variant column is added,
    /// registered so `tellurion-geopackage`'s catalog join reports it,
    /// populated with a simplified geometry, and — crucially — rerunning
    /// repopulates rather than duplicating anything.
    #[tokio::test]
    async fn materializing_against_a_real_file_is_idempotent_and_registers_the_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("variants.gpkg");
        crate::geopackage::create_tables(crate::geopackage::CreateTablesArgs {
            path: path.clone(),
            table: "demo".to_string(),
            geometry: "geom".to_string(),
            srid: 3857,
            geometry_type: "GEOMETRY".to_string(),
            columns: vec![],
            dry_run: false,
        })
        .await
        .expect("provisions the fixture file");

        // One detailed line whose middle vertex sits far below the
        // zoom-6 pixel, written straight in (no driver needed: the base
        // column's own R*Tree triggers only fire for `geom`, and this test
        // is about the variant column).
        let detail = 0.000_1;
        let line: geo_types::Geometry<f64> =
            geo_types::LineString::from(vec![(0.0, 0.0), (100.0, detail), (200.0, 0.0)]).into();
        let blob = line
            .to_gpkg_wkb(
                CoordDimensions::xy(),
                Some(3857),
                vec![0.0, 200.0, 0.0, detail],
            )
            .unwrap();
        {
            // Deliberately NOT dropping the R*Tree triggers `create-tables`
            // installed: registering the five functions they call is what
            // the command itself has to do (see
            // `register_gpkg_envelope_functions`), so the fixture writes the
            // same way rather than around it.
            let conn = Connection::open(&path).unwrap();
            register_gpkg_envelope_functions(&conn).unwrap();
            conn.execute(
                "INSERT INTO demo (id, geom) VALUES (1, ?1)",
                rusqlite::params![blob],
            )
            .unwrap();
            // The insert trigger really did index the base column — proof
            // the registered functions are the working ones, not stubs.
            let indexed: i64 = conn
                .query_row("SELECT count(*) FROM rtree_demo_geom", [], |row| row.get(0))
                .unwrap();
            assert_eq!(indexed, 1);
        }

        let plan = VariantPlan {
            table: "demo".to_string(),
            base_geometry: "geom".to_string(),
            variant_column: "geom_z6".to_string(),
            geometry_type: "GEOMETRY".to_string(),
            srid: 3857,
            tolerance: derive_tolerance_in_storage_units(&variant("geom_z6", 0, 6), 3857).unwrap(),
        };

        let mut conn = Connection::open(&path).unwrap();
        register_gpkg_envelope_functions(&conn).unwrap();
        // Without consent, the spec's own unique constraint refuses by name
        // rather than being quietly worked around.
        let refused = geopackage_apply(&mut conn, &plan, false).unwrap_err();
        assert!(
            format!("{refused}").contains("uk_gc_table_name"),
            "error was: {refused}"
        );

        assert_eq!(geopackage_apply(&mut conn, &plan, true).unwrap(), 1);
        // Rerunning repopulates; it does not add a second column, a second
        // registration row, or a second feature row.
        assert_eq!(geopackage_apply(&mut conn, &plan, true).unwrap(), 1);

        let registrations: i64 = conn
            .query_row(
                "SELECT count(*) FROM gpkg_geometry_columns WHERE table_name = 'demo'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(registrations, 2, "base column plus exactly one variant");
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM demo", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);

        let (base_blob, variant_blob): (Vec<u8>, Vec<u8>) = conn
            .query_row("SELECT geom, geom_z6 FROM demo WHERE id = 1", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        let base_vertices = match GpkgWkb(&base_blob[..]).to_geo().unwrap() {
            geo_types::Geometry::LineString(g) => g.0.len(),
            other => panic!("unexpected base geometry {other:?}"),
        };
        let variant_vertices = match GpkgWkb(&variant_blob[..]).to_geo().unwrap() {
            geo_types::Geometry::LineString(g) => g.0.len(),
            other => panic!("unexpected variant geometry {other:?}"),
        };
        assert_eq!(base_vertices, 3, "the base column is never rewritten");
        assert_eq!(
            variant_vertices, 2,
            "the variant column dropped the sub-pixel vertex"
        );

        // No R*Tree was provisioned for the variant — a deliberate decision,
        // see this module's own doc.
        let variant_rtree: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'rtree_demo_geom_z6'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(variant_rtree, 0);
    }

    /// Live-database test: the PostGIS arm against a real instance, twice,
    /// proving the column is added once, repopulated on rerun, and reported
    /// by `geometry_columns` with the base column's own srid and type — the
    /// exact three facts `Router::refuse_invalid_geometry_variants` checks
    /// at boot. Skips gracefully unless `TELLURION_TEST_DATABASE_URL` is
    /// set, matching `outbox.rs`/`index.rs`'s own live tests.
    #[tokio::test]
    async fn postgis_materialization_is_idempotent_and_satisfies_the_boot_time_check() {
        let Ok(url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
            eprintln!(
                "skipping postgis_materialization_is_idempotent_and_satisfies_the_boot_time_check: TELLURION_TEST_DATABASE_URL not set"
            );
            return;
        };
        let table = "tellurion_ingest_variants_test_table";
        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to the test database");
        client
            .batch_execute(&format!(
                "DROP TABLE IF EXISTS {table} CASCADE;
                 CREATE TABLE {table} (id bigserial PRIMARY KEY, geom geometry(Geometry,4326));
                 INSERT INTO {table} (geom) VALUES (ST_GeomFromText('LINESTRING(0 0, 5 0.000001, 10 0)', 4326));"
            ))
            .await
            .expect("provisions the test table");

        let plan = VariantPlan {
            table: table.to_string(),
            base_geometry: "geom".to_string(),
            variant_column: "geom_z6".to_string(),
            geometry_type: "GEOMETRY".to_string(),
            srid: 4326,
            tolerance: derive_tolerance_in_storage_units(&variant("geom_z6", 0, 6), 4326).unwrap(),
        };
        let sql = postgis_materialize_sql(&plan).unwrap();
        client
            .batch_execute(&sql)
            .await
            .expect("first run succeeds");
        client
            .batch_execute(&sql)
            .await
            .expect("rerunning is idempotent");

        // One variant row in `geometry_columns`, sharing the base column's
        // srid and type — what the boot-time check compares.
        let row = client
            .query_one(
                "SELECT srid, type FROM geometry_columns \
                 WHERE f_table_schema = 'public' AND f_table_name = $1 AND f_geometry_column = $2",
                &[&table, &"geom_z6"],
            )
            .await
            .expect("the variant column is reported by geometry_columns");
        assert_eq!(row.get::<_, i32>(0), 4326);
        assert_eq!(row.get::<_, String>(1), "GEOMETRY");

        let (base_points, variant_points): (i32, i32) = {
            let row = client
                .query_one(
                    &format!("SELECT ST_NPoints(geom), ST_NPoints(geom_z6) FROM {table}"),
                    &[],
                )
                .await
                .expect("both columns are populated");
            (row.get(0), row.get(1))
        };
        assert_eq!(base_points, 3, "the base column is never rewritten");
        assert_eq!(
            variant_points, 2,
            "the variant dropped the sub-pixel vertex"
        );

        let indexes: i64 = client
            .query_one(
                "SELECT count(*) FROM pg_indexes WHERE tablename = $1 AND indexname = $2",
                &[&table, &format!("{table}_geom_z6_gix")],
            )
            .await
            .expect("index lookup")
            .get(0);
        assert_eq!(indexes, 1, "the variant carries its own GiST index, once");

        client
            .batch_execute(&format!("DROP TABLE {table} CASCADE"))
            .await
            .expect("clean up the test table");
    }
}
