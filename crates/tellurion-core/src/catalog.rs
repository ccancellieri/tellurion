//! `CatalogSource` is the mandatory introspection entry point every driver
//! implements: given a storage, what collections can it serve, and what
//! physical shape do they have (table/layer name, geometry column, primary
//! key, srid, geometry type where knowable)? `Router::validate_catalog` uses
//! it to cross-check a configured collection's declared `table` against
//! physical reality once at boot, before the first request ever reaches it —
//! see the driver contract v2 design doc, section 1.

use std::time::SystemTime;

use async_trait::async_trait;

use crate::error::Result;

/// Physical metadata for one collection a storage can serve, as reported by
/// the backend itself — never operator-declared. Fields the backend cannot
/// determine are `None` rather than a guess; `#19` will lean on this same
/// shape to derive collection descriptors instead of requiring them in
/// config.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicalCollection {
    /// Table/layer name exactly as the backend reports it; compared against
    /// `CollectionDecl::table` at boot.
    pub name: String,
    pub geometry_column: Option<String>,
    pub primary_key: Option<String>,
    pub srid: Option<i32>,
    /// e.g. "POINT", "POLYGON", ...; `None` when the backend cannot answer.
    pub geometry_type: Option<String>,
}

/// A physical collection's spatial extent, transformed to CRS84 (lon/lat,
/// WGS84) — the CRS every OGC API Features `extent.spatial.bbox` entry is
/// expressed in regardless of the collection's native SRID. See `#27`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialExtent {
    /// `[minx, miny, maxx, maxy]` in CRS84 order.
    pub bbox: [f64; 4],
}

/// One non-geometry column of a collection's attribute schema, as reported
/// by the backend: its name and the backend's own broad type name (PostGIS:
/// `information_schema.columns.data_type`, e.g. `"text"`, `"integer"`,
/// `"timestamp with time zone"`) — never operator-declared, and never a full
/// SQL type (no length/precision/etc.), just enough to describe the shape.
/// Part of the richer descriptor (`#19`).
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeColumn {
    pub name: String,
    pub sql_type: String,
}

/// Per-feature vertex-count stats from a [`GeometryProfile`]'s sample
/// (`#101`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexStats {
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
    pub max: u64,
    /// Extrapolated total vertex count across the whole collection (the
    /// sample's mean vertex count times the collection's own row estimate) —
    /// `None` only when no row estimate was available to extrapolate
    /// against, the same opt-out [`CatalogSource::row_estimate`] already has
    /// elsewhere. An estimate, never an exact count — see
    /// [`GeometryProfile::sample_size`] for the confidence signal that goes
    /// with it.
    pub total_estimated: Option<u64>,
}

/// Geometry area/length percentiles from a [`GeometryProfile`]'s sample, in
/// the collection's native SRID units — area for a polygon-typed collection,
/// length for a line-typed one. Every field is `None` together for a
/// point-typed or heterogeneous (`GEOMETRY`) collection, where neither
/// concept applies uniformly across the sample.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FeatureSizeStats {
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub max: Option<f64>,
}

/// A per-collection geometry statistics profile (`#101`): a sampled summary
/// of how much geometry a collection actually contains and how it is
/// shaped — the density signal `tellurion-core::descriptor::heuristics`'
/// own doc comment used to say plainly did not exist. Every stat here comes
/// from `sample_size` sampled features, never a full-table scan (design
/// point 2: exact stats on a multi-million-row table at boot is
/// unacceptable) — `sample_size` travels alongside every other field
/// precisely so a consumer can judge how much confidence to place in the
/// rest of it. `computed_at` is the staleness signal design point 3 calls
/// for: this is derived data about a mutable table, so nothing that reads a
/// `GeometryProfile` should assume it reflects the table's current state
/// without checking this against its own tolerance — see
/// `Router::geometry_profile`/`Router::refresh_geometry_profile` for how a
/// caller obtains and explicitly refreshes one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometryProfile {
    /// How many rows the sampling query actually read — the driver's own
    /// count, never a requested target (block-level sampling means the real
    /// count can diverge from whatever percentage a driver aimed for).
    /// Always greater than zero: a sample that came back empty is reported
    /// as no profile at all (`Ok(None)`) rather than a profile of zeroes.
    pub sample_size: u64,
    pub computed_at: SystemTime,
    pub vertices: VertexStats,
    /// Vertices per unit area (native SRID units, e.g. degrees^2 for a
    /// collection stored in EPSG:4326) of the sampled features' own combined
    /// bounding box — a density observed within the region actually
    /// sampled, not extrapolated to the collection's full extent. `None`
    /// when that combined bbox has zero area (e.g. every sampled feature
    /// collapses to a single point).
    pub vertex_density_per_area: Option<f64>,
    /// Fraction of sampled features whose geometry is multi-part
    /// (more than one part, regardless of the geometry column's own
    /// declared type — a `GEOMETRY`-typed column can hold a mix).
    pub multi_part_fraction: f64,
    /// Mean ring count per sampled feature (exterior plus interior rings),
    /// summed across every part of a multi-part feature — full multi-polygon
    /// ring enumeration, not just the first part (see
    /// `tellurion-postgis::sql::build_geometry_profile_plan`'s `LATERAL`
    /// join over `generate_series(1, ST_NumGeometries(geom))` for how the
    /// enumeration stays bounded to the sampled row set). `None` for a
    /// collection whose geometry type has no ring concept (points, lines),
    /// or whose column is untyped/mixed `GEOMETRY` — the same "decline,
    /// don't guess" gating [`FeatureSizeStats`] already applies.
    pub mean_ring_count: Option<f64>,
    pub feature_size: FeatureSizeStats,
}

/// A collection's projection facts as the backend itself knows them
/// (`#36`, STAC `projection` extension): the georeferencing a driver can
/// read out of its own storage, never operator-declared and never guessed.
/// Every field is independently optional, and an absent field is a genuine
/// "this backend does not know" — a consumer must omit it, not default it
/// (an identity `transform` is the canonical example of a plausible-but-
/// invented value this struct's contract forbids).
///
/// - `epsg`: the EPSG code of the storage CRS. Overlaps with
///   [`PhysicalCollection::srid`] for SQL backends (where an SRID is read as
///   an EPSG code — see `crate::crs::epsg_uri`); a raster driver that knows
///   its CRS from file georeferencing (GeoTIFF GeoKeys) reports it here.
/// - `transform`: the row-major 2D affine pixel-to-CRS transform
///   `[a, b, c, d, e, f]` (x = a·col + b·row + c; y = d·col + e·row + f),
///   exactly the STAC `proj:transform` convention. Only a raster backend
///   has one; a vector table has no pixel grid, so the concept does not
///   apply and the field stays `None` — absence, not zero-knowledge shame.
/// - `shape`: `[height, width]` (Y first, X second — `proj:shape`'s own
///   order) of the full-resolution raster grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectionFacts {
    pub epsg: Option<i32>,
    pub transform: Option<[f64; 6]>,
    pub shape: Option<[u64; 2]>,
}

#[async_trait]
pub trait CatalogSource: Send + Sync {
    /// Enumerates every collection this storage can currently serve.
    async fn collections(&self) -> Result<Vec<PhysicalCollection>>;

    /// Spatial extent of one physical collection reported by [`collections`],
    /// transformed to CRS84. `Ok(None)` means no extent is available — an
    /// empty table, or a backend that cannot introspect one at all. The
    /// default declines so only drivers that can answer cheaply (PostGIS:
    /// `ST_EstimatedExtent`/`ST_Extent`) need to override it. See `#27`;
    /// `Router` caches the result with a TTL rather than calling this on
    /// every request.
    ///
    /// [`collections`]: CatalogSource::collections
    async fn extent(&self, _physical: &PhysicalCollection) -> Result<Option<SpatialExtent>> {
        Ok(None)
    }

    /// Cheap row-count estimate for one physical collection (PostGIS:
    /// `pg_class.reltuples`, no table scan). `Ok(None)` when the backend
    /// cannot answer — the default declines so only drivers that can
    /// estimate cheaply need to override it, mirroring [`extent`](Self::extent)'s
    /// opt-in shape. Feeds the heuristics module's per-zoom feature caps
    /// (`descriptor::heuristics::effective_feature_cap`); part of the richer
    /// descriptor (`#19`).
    async fn row_estimate(&self, _physical: &PhysicalCollection) -> Result<Option<u64>> {
        Ok(None)
    }

    /// `physical`'s non-geometry columns: name plus the backend's own broad
    /// type name. `Ok(None)` when the backend cannot introspect columns at
    /// all; `Ok(Some(vec![]))` is a legitimate answer for a collection with
    /// no non-geometry columns. Part of the richer descriptor (`#19`),
    /// exposed read-only for inspection — nothing computes from it yet.
    async fn attribute_schema(
        &self,
        _physical: &PhysicalCollection,
    ) -> Result<Option<Vec<AttributeColumn>>> {
        Ok(None)
    }

    /// The single timestamp/timestamptz/date column on `physical`, if there
    /// is exactly one — deliberately dumb: two or more candidate columns, or
    /// zero, both resolve to `Ok(None)` rather than guessing which one the
    /// operator meant. Feeds `CollectionDecl::datetime`'s override > derived
    /// precedence the same way `geometry`/`pk` derive (see
    /// `descriptor::merge_descriptor`) when the operator hasn't set it
    /// explicitly. Part of the richer descriptor (`#19`).
    async fn temporal_column(&self, _physical: &PhysicalCollection) -> Result<Option<String>> {
        Ok(None)
    }

    /// `physical`'s projection facts (`#36`, STAC `projection` extension) —
    /// the georeferencing this backend can read out of its own storage. The
    /// default declines (`Ok(None)`), the same "decline, don't guess" shape
    /// [`extent`](Self::extent)/[`row_estimate`](Self::row_estimate) use: a
    /// driver that never overrides this produces exactly today's behavior
    /// for every collection it serves, and only a driver that genuinely
    /// reads georeferencing from its storage (COG: GeoTIFF tags/GeoKeys;
    /// Zarr: the store's own declared georeferencing) ever answers. See
    /// [`ProjectionFacts`] for the per-field omission contract. SQL vector
    /// backends deliberately do NOT override this for their SRID — that
    /// already travels as [`PhysicalCollection::srid`] and reaches consumers
    /// through the derived descriptor's own `srid` carrier; reporting it
    /// twice would create two copies of one fact that could drift.
    async fn projection(&self, _physical: &PhysicalCollection) -> Result<Option<ProjectionFacts>> {
        Ok(None)
    }

    /// `#101`: a sampled per-collection geometry statistics profile —
    /// vertex counts, density, feature-size percentiles, and
    /// multi-part/ring shape — computed from a bounded sample rather than a
    /// full scan (see [`GeometryProfile`]'s own doc for the sampling
    /// contract). `Ok(None)` is the correct default for a driver with no
    /// cheap way to sample its own geometry, the same "decline, don't
    /// guess" shape [`extent`](Self::extent)/[`row_estimate`](Self::
    /// row_estimate) already have: a driver that never overrides this
    /// produces exactly today's behavior for every collection it serves —
    /// nothing anywhere in this workspace requires a profile to be present.
    /// PostGIS overrides this with a `TABLESAMPLE SYSTEM`-based sampled
    /// aggregate (`tellurion-postgis::sql::build_geometry_profile_plan`).
    async fn geometry_profile(
        &self,
        _physical: &PhysicalCollection,
    ) -> Result<Option<GeometryProfile>> {
        Ok(None)
    }
}
