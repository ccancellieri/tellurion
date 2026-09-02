//! Pure SQL builders: `CollectionDecl` + query in, SQL text + typed params
//! out. No I/O, no PostgreSQL connection — golden-tested against exact
//! strings. All identifiers are whitelist-quoted (`ident.rs`); every value
//! is bound as a parameter, never interpolated.
//!
//! The primary key's value-space is per-collection (`CollectionDecl::
//! id_type`, `#87`): `Integer` (the default, and every collection that
//! predates `#87`) parses keyset tokens and item ids as `i64`. The pk
//! column is still cast to `::bigint` in the `pk_value` the `items` SELECT
//! list projects (so the Rust reader always decodes a consistent width
//! regardless of whether the physical column is `int4` or `int8`), but
//! deliberately *not* on the comparison/`ORDER BY` side of a keyset
//! predicate or the single-item equality lookup: PostgreSQL's built-in
//! cross-type integer operators already compare an `int4` column against
//! an `i64`-bound parameter correctly and use the column's own btree index
//! directly, with no cast needed in the SQL text at all. Casting the
//! column there instead (the v0.1 behavior) turns it into an opaque
//! function application the planner can no longer match to that index —
//! confirmed against a live multi-million-row table, where the cast alone
//! turned an `Index Scan` into a full (parallel) sequential scan plus an
//! external disk sort, on both keyset paging and the single-item lookup.
//! `Uuid` parses them as [`uuid::Uuid`] instead and casts the pk column to
//! `::uuid` on every comparison, so ordering (keyset paging's `ORDER BY`/
//! `WHERE >`) is always over the pk's own real type, never a string
//! standing in for one — unaffected by the `Integer` fix above (a
//! same-type cast is a no-op the planner sees straight through) and out of
//! scope for it regardless (no equivalent evidence gathered for a `Uuid`
//! pk stored as `text`, which the cast may still matter for). `Text`
//! (`#94`) parses them as a plain `String` and casts the pk column to
//! `::text`; unlike `Integer`/`Uuid`, a `text` column's comparison order
//! depends on the database's own collation, so keyset paging additionally
//! pins an explicit `COLLATE "C"` (byte order) on both the `WHERE`/`ORDER BY`
//! comparisons in `build_items_plan` — stable and complete regardless of
//! which locale a given deployment's database happens to use, never trusting
//! whatever `text`'s default collation resolves to there. [`PkValue`] is the
//! small enum-dispatched component that carries this choice from
//! `CollectionDecl::id_type` down to a bound [`SqlParam`] and a cast string,
//! resolved once at the boundary (`PkValue::parse`) rather than re-decided at
//! each call site.

use tokio_postgres::types::ToSql;

use tellurion_core::{
    world_crs84_tile_bounds_deg, CaseInsensitiveCompareOp, CollectionDecl, CompareOp, Filter,
    GeometryLiteral, IdType, ItemsQuery, Literal, RequestedCrs, SpatialOp, TemporalOp,
    TemporalValue, TileCoord, TileMatrixSet,
};

use crate::error::{PostgisError, Result};
use crate::ident::{quote_ident, quote_literal, quote_sql_string};

/// A parsed primary-key value, dispatched on `CollectionDecl::id_type`
/// (`#87`) — the "small enum-dispatched component" every id-bearing
/// boundary (path params, keyset tokens, outbox-derived reads) resolves
/// through exactly once, rather than each call site guessing or falling back
/// integer-then-uuid-then-text. `PartialOrd`/`Ord` are never derived:
/// comparing across variants (a caller mixing id types) has no sensible
/// meaning, and every real comparison this module needs happens in SQL, over
/// the pk's own column type, not in Rust. `Copy` is deliberately not derived
/// either — `Text`'s `String` payload can't be — so every caller that needs
/// the value more than once clones or borrows explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PkValue {
    Integer(i64),
    Uuid(uuid::Uuid),
    /// A caller-supplied `text` pk value (`#94`) — unlike `Integer`/`Uuid`,
    /// nothing here mints one server-side; `driver.rs`'s `create_inner` reads
    /// it straight out of the create request's own feature body.
    Text(String),
}

impl PkValue {
    /// Parses `raw` per `id_type`, by name — never integer-parse-then-
    /// uuid-fallback (or vice versa). `None` for a `raw` that doesn't fit
    /// the declared type, letting every caller keep its own existing
    /// not-found/invalid distinction (a `GET` treats this as "no such item";
    /// a `PUT`/`DELETE`/keyset token treats it as a named, rejected request —
    /// see `driver.rs`/this module's own callers). `Text` has no syntactic
    /// constraint of its own — any string is a legal caller-supplied id — so
    /// this never returns `None` for `IdType::Text`.
    pub(crate) fn parse(id_type: IdType, raw: &str) -> Option<Self> {
        match id_type {
            IdType::Integer => raw.parse::<i64>().ok().map(PkValue::Integer),
            IdType::Uuid => uuid::Uuid::parse_str(raw).ok().map(PkValue::Uuid),
            IdType::Text => Some(PkValue::Text(raw.to_string())),
        }
    }

    /// The bound parameter this value sends over the wire — a native typed
    /// bind (`Bigint`/`Uuid`/`Text`), never a guessed cast, matching every
    /// other natively-typed `SqlParam` variant in this module.
    pub(crate) fn as_sql_param(&self) -> SqlParam {
        match self {
            PkValue::Integer(v) => SqlParam::Bigint(*v),
            PkValue::Uuid(v) => SqlParam::Uuid(*v),
            PkValue::Text(v) => SqlParam::Text(v.clone()),
        }
    }
}

impl std::fmt::Display for PkValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PkValue::Integer(v) => write!(f, "{v}"),
            PkValue::Uuid(v) => write!(f, "{v}"),
            PkValue::Text(v) => write!(f, "{v}"),
        }
    }
}

/// The SQL type name `id_type`'s pk column casts to on every WHERE/ORDER BY
/// comparison — `Integer` -> `"bigint"` (works for both `int4` and `int8`
/// physical columns, this module's own top-level doc), `Uuid` -> `"uuid"`,
/// `Text` -> `"text"`.
pub(crate) fn pk_sql_cast(id_type: IdType) -> &'static str {
    match id_type {
        IdType::Integer => "bigint",
        IdType::Uuid => "uuid",
        IdType::Text => "text",
    }
}

/// MVT encoding grid resolution passed to both `ST_AsMVT` and
/// `ST_AsMVTGeom` — the coordinate space geometry gets quantized to inside a
/// tile, independent of the 256px tile a client actually renders it at.
/// `descriptor::heuristics::tile_buffer_px` derives the tile buffer from
/// this same constant, so the two always agree.
pub(crate) const MVT_EXTENT: u32 = 4096;

/// Equatorial meters per CRS84 degree (`#190`): `2 * pi * 6378137 / 360`,
/// the exact conversion OGC 17-083r4 SS5.2.1 defines the WorldCRS84Quad
/// scale ladder with. `driver.rs` divides its meters-calibrated
/// simplification tolerance by this before handing it to the CRS84 arm of
/// [`build_mvt_candidate_fragment`], whose `ST_SimplifyPreserveTopology`
/// runs in degrees there.
pub(crate) const WORLD_CRS84_METERS_PER_DEGREE: f64 = 111_319.490_793_273_57;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SqlParam {
    Int4(i32),
    Bigint(i64),
    Float8(f64),
    Text(String),
    Bool(bool),
    /// A native `uuid`-typed bind (`#87`, `tokio-postgres`'s `with-uuid-1`
    /// feature) — the `Uuid` id-type counterpart of `Bigint` above: bound
    /// with its own OID, never coerced through `Text` + a SQL-text cast the
    /// way an unknown-typed property value is (`write_sql.rs`'s own doc).
    Uuid(uuid::Uuid),
    /// A `text[]` bind (`#202`) — the one param shape a batched `= ANY($N)`
    /// lookup needs, so a page of N feature ids costs one placeholder and
    /// one round trip instead of N placeholders in an `IN (...)` list (which
    /// would also give the planner a differently-shaped statement for every
    /// distinct page size). Never used to smuggle a list of identifiers:
    /// every identifier in this crate still goes through
    /// `ident::quote_ident`, values alone are bound.
    TextArray(Vec<String>),
}

impl SqlParam {
    pub(crate) fn boxed(&self) -> Box<dyn ToSql + Sync + Send> {
        match self {
            SqlParam::Int4(v) => Box::new(*v),
            SqlParam::Bigint(v) => Box::new(*v),
            SqlParam::Float8(v) => Box::new(*v),
            SqlParam::Text(v) => Box::new(v.clone()),
            SqlParam::Bool(v) => Box::new(*v),
            SqlParam::Uuid(v) => Box::new(*v),
            SqlParam::TextArray(v) => Box::new(v.clone()),
        }
    }
}

pub(crate) struct ItemsPlan {
    pub(crate) sql: String,
    pub(crate) params: Vec<SqlParam>,
    /// How `items_inner` should populate `numberMatched` — see
    /// [`CountPlan`]'s own doc. Present in some shape only when the query
    /// carries no bbox/datetime/CQL2 filter (an estimate of a filtered
    /// result set would be misleading, and a full `COUNT` on a filtered
    /// query is exactly the unbounded scan the design forbids).
    pub(crate) count: CountPlan,
}

/// How an unfiltered [`ItemsPlan`] wants its caller to populate
/// `numberMatched`. A filtered query is always [`CountPlan::None`]; an
/// unfiltered one prefers the already-resolved [`CountPlan::Cached`]
/// estimate — `CollectionDecl::row_estimate`, itself the very same
/// `pg_class.reltuples` estimate the [`CountPlan::Query`] fallback below
/// queries live, just resolved once per the router's `descriptor_ttl`
/// cadence (`Router::resolved_descriptor`) rather than on every single
/// request — over issuing a second sequential round trip through the
/// connection pool for information the collection already carries for
/// free. `CountPlan::Query` is the pre-existing live-query behavior,
/// kept as a fallback for a collection whose `table`/`geometry`/`pk` are
/// all pinned (`Router::effective_decl`'s documented "an operator who has
/// fully pinned a collection's physical shape has asked this server to
/// trust that declaration outright" contract, which deliberately never
/// derives `row_estimate` either) — the one shape that never gets a cached
/// estimate to reuse.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CountPlan {
    /// No count semantics apply — a filtered query, byte-for-byte the
    /// pre-existing behavior.
    None,
    /// Use this already-resolved estimate directly; no additional query.
    Cached(u64),
    /// No cached estimate is available for this collection; run this live
    /// `pg_class.reltuples` query instead — the pre-`#1` behavior,
    /// unchanged.
    Query(String, Vec<SqlParam>),
}

#[cfg(test)]
impl CountPlan {
    fn is_none(&self) -> bool {
        matches!(self, CountPlan::None)
    }

    fn is_some(&self) -> bool {
        !self.is_none()
    }
}

/// Which row a GeoJSON `properties` projection reads its columns from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PropertyRow {
    /// The base table itself, aliased `t` — every unbudgeted plan, and
    /// `build_item_plan`'s budgeted arm (which still reads straight off the
    /// table).
    TableAlias,
    /// `build_items_plan`'s vertex-budgeted arm, whose `candidates` CTE
    /// carries the bare row composite as `t AS source_row` (`#1`) so that no
    /// per-column conversion happens for a row the budget then refuses.
    BudgetedComposite,
}

impl PropertyRow {
    /// The composite the `to_jsonb` fallback renders whole.
    fn row(self) -> &'static str {
        match self {
            PropertyRow::TableAlias => "t",
            PropertyRow::BudgetedComposite => "source_row",
        }
    }

    /// What each named column is prefixed with in the `jsonb_build_object`
    /// form. The table alias keeps the plain `t."col"` column reference the
    /// rest of this module emits; the CTE's carried composite genuinely
    /// needs the parenthesized `(source_row)."col"` field-selection form,
    /// which is also why the table alias does *not* use it — `(t)."col"`
    /// is legal but builds a whole-row datum the planner then picks a field
    /// out of, which is exactly the per-column-conversion cost `#278` is
    /// removing.
    fn column_prefix(self) -> &'static str {
        match self {
            PropertyRow::TableAlias => "t.",
            PropertyRow::BudgetedComposite => "(source_row).",
        }
    }
}

/// `jsonb_build_object` is variadic over `"any"`, so PostgreSQL's
/// `FUNC_MAX_ARGS` (100 on a stock build) caps one call at 50 key/value
/// pairs — a 51st pair fails with `cannot pass more than 100 arguments to a
/// function`, at plan time, for every request. A collection with more
/// property columns than this chunks into several calls concatenated with
/// `||`, which merges the objects key-wise; since every key here is a
/// distinct column name, the merge can never drop or overwrite one, and
/// `jsonb`'s own key ordering makes the result byte-identical to the
/// single-call form.
const MAX_JSONB_BUILD_OBJECT_PAIRS: usize = 50;

/// The GeoJSON `properties` expression: every column of `row` except the
/// geometry and the pk, including an optional `datetime` column.
///
/// Two shapes, chosen by whether this collection carries a backend-derived
/// column list (`CollectionDecl::attribute_columns` — `CollectionDescriptor
/// ::attributes`, the same derivation `CanonicalSchema` is built from,
/// carried down by `Router::effective_decl`):
///
/// - **With one (`#278`): `jsonb_build_object('col', t."col", ...)`.** Only
///   the columns that are actually kept are rendered at all.
/// - **Without one: `to_jsonb(row) - <geometry> - <pk>`,** byte for byte the
///   pre-`#278` expression. Reached by a collection whose `table`/`geometry`/
///   `pk` are all pinned (`Router::effective_decl`'s fully-pinned fast path
///   derives no descriptor at all, deliberately — see its own doc) and by a
///   backend that cannot introspect columns. Nothing here invents a column
///   list for those; they keep exactly the SQL, and exactly the output, they
///   had before.
///
/// The two produce identical bytes. `to_jsonb` renders each column through
/// `datum_to_jsonb`, and `jsonb_build_object` renders each argument through
/// the very same function, so every type — including `json`/`jsonb` (embedded,
/// not re-escaped), `numeric` (trailing zeros preserved), `timestamptz`
/// (rendered in UTC), arrays, and SQL `NULL` (a JSON `null`, not an absent
/// key) — comes out the same; and both sides are `jsonb`, whose key ordering
/// is normalized identically regardless of the order the keys were built in.
/// What changes is that the geometry column is never rendered: `to_jsonb`
/// serializes it to hex WKB through `geometry`'s own output function and the
/// adjacent `- <geometry>` then throws that away, which on a page of large
/// geometries costs more than everything the response keeps (`#278`: 245 ms
/// of the 393 ms measured on the `#1` fixture).
///
/// ## Why not `CanonicalSchema` itself
///
/// `CanonicalSchema` is the *merged* view — backend attributes refined by a
/// declared `SchemaDecl` — and is therefore not a column list: it drops
/// backend columns a declared schema didn't mention when that schema sets
/// `additional_properties: false`, and it adds declared properties the
/// backend never reported. Either one would change what `properties` a
/// response carries. This reads the backend half `CanonicalSchema` is itself
/// built from, so the projection follows the physical table exactly.
///
/// ## Refusals and the un-quotable column
///
/// Every column name is whitelist-quoted (`ident.rs`), on both the key and
/// the reference side. A name that whitelist rejects — legal in PostgreSQL
/// (`"my-col"`), and served correctly today by `to_jsonb`, which never
/// spells a column name in SQL text at all — falls the whole projection back
/// to the `to_jsonb` form rather than either refusing the request or
/// dropping that column from `properties`. Refusing would break a collection
/// that works today for a pure-performance change, and dropping it silently
/// is exactly what this workspace does not do; the fallback keeps the bytes
/// and only loses the speedup, for that one collection.
fn properties_expr(
    collection: &CollectionDecl,
    row: PropertyRow,
    pk_key: &str,
    geom_key: &str,
) -> String {
    let fallback = || format!("to_jsonb({}) - {geom_key} - {pk_key}", row.row());
    let Some(attributes) = collection.attribute_columns.as_deref() else {
        return fallback();
    };
    let pk = collection.resolved_pk();
    let geometry = collection.resolved_geometry();
    let mut pairs: Vec<String> = Vec::with_capacity(attributes.len());
    for column in attributes {
        // The derived attribute list already excludes the geometry column
        // (`catalog::ATTRIBUTE_SCHEMA_SQL`) but not the pk; both are
        // excluded by name here regardless, so this projection matches the
        // `- {geom_key} - {pk_key}` it replaces even if a collection's
        // resolved geometry ever diverges from the one the descriptor was
        // derived against.
        if column.name == pk || column.name == geometry {
            continue;
        }
        let (Ok(key), Ok(reference)) = (quote_literal(&column.name), quote_ident(&column.name))
        else {
            return fallback();
        };
        pairs.push(format!("{key},{}{reference}", row.column_prefix()));
    }
    if pairs.is_empty() {
        // `chunks` yields nothing at all for an empty list, and a table
        // whose only columns are the geometry and the pk has no properties
        // to build — `jsonb_build_object()` is `{}`, exactly what
        // `to_jsonb(t) - <geometry> - <pk>` answers for that same table.
        return "jsonb_build_object()".to_string();
    }
    pairs
        .chunks(MAX_JSONB_BUILD_OBJECT_PAIRS)
        .map(|chunk| format!("jsonb_build_object({})", chunk.join(",")))
        .collect::<Vec<_>>()
        .join(" || ")
}

/// `geom_expr` is whatever `ST_AsGeoJSON` should read — the bare quoted
/// column (`crs` handling below leaves it untouched) or a `reprojected_geom_
/// expr` wrapper; `geom_key` (used only to exclude the physical column from
/// the `to_jsonb` fallback in [`properties_expr`]) is always the plain
/// quoted-literal column name regardless of `crs`, since reprojection never
/// changes which column `t` actually carries. `properties` is built off the
/// same unreprojected row for the same reason: `crs` moves coordinates, not
/// columns, so both projections here honor it through `geom_expr` alone and
/// neither shape of `properties` can diverge on it.
fn feature_expr(
    collection: &CollectionDecl,
    pk: &str,
    geom_expr: &str,
    pk_key: &str,
    geom_key: &str,
) -> String {
    format!(
        "json_build_object('type','Feature','id',{pk}::text,'geometry',ST_AsGeoJSON({geom_expr})::json,'properties',{properties})",
        properties = properties_expr(collection, PropertyRow::TableAlias, pk_key, geom_key)
    )
}

/// The geometry expression `feature_expr` should read to honor `requested`
/// (`crs` query parameter, OGC API Features Part 2 CRS by Reference, `#33`
/// follow-up) against `storage_srid` (`CollectionDecl::srid`, itself derived
/// from `PhysicalCollection::srid` — see `tellurion_core::descriptor`):
///
/// - `RequestedCrs::Omitted` (no `crs` parameter at all): `geom` untouched,
///   byte-for-byte this crate's pre-Part-2-CRS SQL, regardless of
///   `storage_srid` — the "no behavior change" rule every CRS-aware builder
///   in this module follows.
/// - `RequestedCrs::Crs84`: `ST_Transform(geom, 4326)` only when
///   `storage_srid` is known and isn't already 4326 — genuine reprojection,
///   skipped when it would be a no-op.
/// - `RequestedCrs::Storage`: `ST_FlipCoordinates(geom)` only when
///   `storage_srid` is exactly `4326` — the classic Part 2 axis-order trap
///   (`tellurion_core::crs`'s own module doc): CRS84 and EPSG:4326-by-
///   authority share a datum but disagree on coordinate order, and
///   PostGIS/GeoJSON always emit the raw X,Y pair, so honoring "give me the
///   storage CRS" when that CRS is EPSG:4326 means swapping the two
///   coordinates, not reprojecting anything. No SRID this crate's
///   `crs::supported_crs` can ever advertise besides CRS84 and this
///   collection's own storage SRID is reachable here, so a `Storage` request
///   against any other native SRID (e.g. a projected CRS, x/y-ordered by
///   convention) needs neither transform nor flip.
fn reprojected_geom_expr(geom: &str, requested: RequestedCrs, storage_srid: Option<i32>) -> String {
    match requested {
        RequestedCrs::Omitted => geom.to_string(),
        RequestedCrs::Crs84 => match storage_srid {
            Some(srid) if srid != 4326 => format!("ST_Transform({geom}, 4326)"),
            _ => geom.to_string(),
        },
        RequestedCrs::Storage => match storage_srid {
            Some(4326) => format!("ST_FlipCoordinates({geom})"),
            _ => geom.to_string(),
        },
    }
}

/// The `ST_MakeEnvelope(...)` (optionally `ST_Transform`-wrapped) SQL a
/// `bbox` items-query parameter compiles to, honoring `bbox_crs` (`bbox-crs`
/// query parameter, Part 2) against `storage_srid` — the bbox-input
/// counterpart of [`reprojected_geom_expr`]. `values` are already axis-
/// normalized to longitude-first order by the caller (`tellurion-features`'
/// handler, via `crs::swap_bbox_axes`) before reaching here, exactly the way
/// every other bound literal in this module arrives pre-validated.
///
/// - [`RequestedCrs::Omitted`] — Part 1 Requirement 23
///   (`/req/core/fc-bbox-definition`) clause C: "If the bounding box consists
///   of four numbers, the coordinate reference system of the values SHALL be
///   interpreted as WGS 84 longitude/latitude
///   (`http://www.opengis.net/def/crs/OGC/1.3/CRS84`) unless a different
///   coordinate reference system is specified in a parameter `bbox-crs`",
///   restated by Part 2 Requirement 8
///   (`/req/crs/fc-bbox-crs-valid-default-value`): "If the `bbox-crs`
///   parameter is not specified then the coordinate values of the `bbox`
///   parameter SHALL be assumed to be in the default CRS specified in OGC
///   API - Features - Part 1: Core".
/// - [`RequestedCrs::Crs84`] — Part 2 Requirement 9
///   (`/req/crs/fc-bbox-crs-action`) with `bbox-crs` naming CRS84 explicitly:
///   "the server SHALL perform the necessary internal transformations to
///   properly fetch data from within the specified bounding box".
///
///   The two arms say the same thing about the same four numbers — *these
///   coordinates are CRS84* — so they compile identically, exactly as
///   [`geometry_literal_expr`]'s do since `#247`: the envelope is built at
///   [`CRS84_SRID`] and then genuinely reprojected into the storage CRS when
///   that is a different one, skipped when it would be a no-op. Part 2's own
///   Abstract Test 10 (`/conf/crs/bbox-crs-parameter-default`) is that
///   equivalence spelled as a test — send a `bbox` with `bbox-crs` naming the
///   default CRS, "send the same request, but with no `bbox-crs` parameter",
///   and "verify that the responses include the same features".
///
///   `#217` compiled the `Omitted` arm without the transform, under the rule
///   that an unconfigured deployment's bytes do not change. `#255` is the case
///   where that rule was protecting nothing: on a projected storage the
///   untransformed envelope reaches PostGIS tagged 4326 beside a 3857 column
///   and, unlike `ST_Intersects` in the filter lane, the `&&` operator does
///   **not** raise on mixed SRIDs —
///   `SELECT ST_SetSRID(ST_MakePoint(1,1),3857) && ST_MakeEnvelope(0,0,2,2,4326)`
///   answers `t` against a live PostGIS 3.4 — so degrees were compared against
///   metres and the request answered `200` with rows that are simply the wrong
///   ones. That is a direct violation of Part 1 Requirement 24
///   (`/req/core/fc-bbox-response`) clause A, "Only features that have a
///   spatial geometry that intersects the bounding box SHALL be part of the
///   result set", and no client can detect it. On a storage that *is* CRS84 —
///   every 4326 collection, which is every live demo — the match below still
///   produces the identical envelope it always did, which is why the transform
///   is conditional on the SRID rather than unconditional.
/// - [`RequestedCrs::Storage`]: the envelope is built directly at
///   `storage_srid` (falling back to [`CRS84_SRID`] if somehow unknown — never
///   produced by `crs::resolve` itself, which can't resolve to `Storage`
///   without a known storage SRID in the first place) — no transform, since
///   the values are already in that SRID's own coordinate system. No
///   `ST_FlipCoordinates` counterpart to [`geometry_literal_expr`]'s is needed
///   here: an authority-axis-order `bbox-crs` is normalized to
///   longitude-first in Rust by the caller, above.
///
/// A driver that cannot perform the transform at all cannot reach the
/// projected `Omitted`/`Crs84` case: the protocol handlers refuse that request
/// by name first (`tellurion-features`' items handler, and `tellurion-stac`'s
/// `unservable_bbox_reason` on both its `/items` and `/search` lanes), rather
/// than let it compare a CRS84 envelope against projected coordinates under a
/// `200`.
fn bbox_envelope_sql(
    params: &mut Vec<SqlParam>,
    bbox: [f64; 4],
    bbox_crs: RequestedCrs,
    storage_srid: Option<i32>,
) -> String {
    for value in bbox {
        params.push(SqlParam::Float8(value));
    }
    let n = params.len();
    let (p1, p2, p3, p4) = (n - 3, n - 2, n - 1, n);
    let envelope = |srid: i32| format!("ST_MakeEnvelope(${p1}, ${p2}, ${p3}, ${p4}, {srid})");
    match bbox_crs {
        RequestedCrs::Omitted | RequestedCrs::Crs84 => {
            let literal = envelope(CRS84_SRID);
            match storage_srid {
                Some(srid) if srid != CRS84_SRID => format!("ST_Transform({literal}, {srid})"),
                _ => literal,
            }
        }
        RequestedCrs::Storage => envelope(storage_srid.unwrap_or(CRS84_SRID)),
    }
}

pub(crate) fn build_items_plan(
    collection: &CollectionDecl,
    query: &ItemsQuery,
) -> Result<ItemsPlan> {
    let table = quote_ident(collection.resolved_table())?;
    let pk = quote_ident(collection.resolved_pk())?;
    let geom = quote_ident(collection.resolved_geometry())?;
    let pk_key = quote_literal(collection.resolved_pk())?;
    let geom_key = quote_literal(collection.resolved_geometry())?;

    let cast = pk_sql_cast(collection.id_type);
    // `#94`: a `text` pk's own comparison order is collation-dependent — the
    // same table could page differently depending on the database's locale.
    // `COLLATE "C"` pins byte order explicitly, so keyset paging is stable
    // and complete regardless of deployment. `Uuid` keeps the plain cast
    // (its own comparison is already over a real binary type, not text).
    // `Integer` is deliberately the bare column with no cast at all (see
    // this module's own top-level doc): PostgreSQL's cross-type integer
    // operators already compare it correctly against the bound `i64`
    // parameter and use its own btree index directly, whereas casting it
    // to `::bigint` here would hide it behind a function application the
    // planner can no longer match to that index.
    let pk_order_expr = match collection.id_type {
        IdType::Text => format!("{pk}::{cast} COLLATE \"C\""),
        IdType::Uuid => format!("{pk}::{cast}"),
        IdType::Integer => pk.clone(),
    };
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<SqlParam> = Vec::new();

    if let Some(token) = &query.token {
        let parsed = PkValue::parse(collection.id_type, token)
            .ok_or_else(|| PostgisError::InvalidToken(token.clone()))?;
        params.push(parsed.as_sql_param());
        // `Integer`'s comparison column is deliberately bare now (see this
        // module's own top-level doc), so nothing left in the SQL text
        // anchors the placeholder's type to `bigint` for Postgres's
        // parameter-type inference — without this suffix it infers the
        // physical column's own type (`int4`) instead and then refuses to
        // serialize the `i64` this crate always binds for an `Integer` id.
        // `Uuid`/`Text` already anchor it via their own (unchanged) LHS
        // cast, so they need no suffix here.
        let param_suffix = match collection.id_type {
            IdType::Integer => "::bigint",
            IdType::Uuid | IdType::Text => "",
        };
        clauses.push(format!("{pk_order_expr} > ${}{param_suffix}", params.len()));
    }

    if let Some(bbox) = query.bbox {
        let envelope = bbox_envelope_sql(&mut params, bbox, query.bbox_crs, collection.srid);
        clauses.push(format!("{geom} && {envelope}"));
    }

    if let Some(range) = &query.datetime {
        let dt_col = collection
            .datetime
            .as_deref()
            .ok_or_else(|| PostgisError::NoDatetimeColumn(collection.id.clone()))?;
        let dt_col = quote_ident(dt_col)?;
        if let Some(start) = &range.start {
            params.push(SqlParam::Text(start.clone()));
            clauses.push(format!("{dt_col} >= ${}::text::timestamptz", params.len()));
        }
        if let Some(end) = &range.end {
            params.push(SqlParam::Text(end.clone()));
            clauses.push(format!("{dt_col} <= ${}::text::timestamptz", params.len()));
        }
    }

    if let Some(filter) = &query.filter {
        clauses.push(compile_filter(
            filter,
            &mut params,
            FilterCrs::requested(query.filter_crs, collection.srid),
        )?);
    }

    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };

    // Fetch one extra row so the caller can detect a next page without a
    // second round trip; never OFFSET.
    params.push(SqlParam::Bigint(i64::from(query.limit).saturating_add(1)));
    let limit_idx = params.len();

    let geom_expr = reprojected_geom_expr(&geom, query.crs, collection.srid);
    let feature = feature_expr(collection, &pk, &geom_expr, &pk_key, &geom_key);
    let sql = match collection.settings.items_vertex_budget {
        Some(vertex_budget) => {
            params.push(SqlParam::Bigint(
                i64::try_from(vertex_budget).unwrap_or(i64::MAX),
            ));
            let budget_idx = params.len();
            let candidate_order_expr = match collection.id_type {
                IdType::Text => "pk_value COLLATE \"C\"",
                IdType::Integer | IdType::Uuid => "pk_value",
            };
            let output_geom =
                reprojected_geom_expr("source_geom", query.crs, collection.srid);
            // `#1`: `properties` is built from `source_row` HERE, inside the
            // budget's own `CASE`, and not in the `candidates` scan below.
            // `to_jsonb` on the whole row renders EVERY column through its
            // type's output function — including the geometry, whose text
            // form is hex WKB that the very next `- {geom_key}` throws away.
            // On a page carrying a few high-vertex features that discarded
            // hex dwarfs the properties actually kept (measured on a
            // 3M-row OSM-shaped fixture whose first ascending-pk page holds
            // 292k vertices: 11MB of geometry hex built and dropped against
            // 65kB of properties retained), and it cost MORE than the
            // `ST_AsGeoJSON` encoding this `CASE` already guards — so
            // computing it in `candidates` left `items_vertex_budget`
            // bounding only the smaller half of the response-side cost it
            // exists to bound (`#148`). Carrying the bare row composite
            // through the CTEs instead is free (it is the tuple the scan
            // already materialized) and defers every per-column conversion
            // to the rows that are actually served. `build_item_plan`'s own
            // budgeted arm already had this shape; this is the page plan
            // catching up.
            let output_properties = properties_expr(
                collection,
                PropertyRow::BudgetedComposite,
                &pk_key,
                &geom_key,
            );
            let output_feature = format!(
                "json_build_object('type','Feature','id',feature_id,'geometry',ST_AsGeoJSON({output_geom})::json,'properties',{output_properties})"
            );
            format!(
                "WITH candidates AS (SELECT {pk}::{cast} AS pk_value, {pk}::text AS feature_id, {geom} AS source_geom, t AS source_row FROM {table} AS t{where_sql} ORDER BY {pk_order_expr} ASC LIMIT ${limit_idx}), counted AS (SELECT *, row_number() OVER (ORDER BY {candidate_order_expr} ASC) AS page_position, sum(COALESCE(ST_NPoints(source_geom), 0)) OVER (ORDER BY {candidate_order_expr} ASC ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS cumulative_vertices FROM candidates) SELECT pk_value, CASE WHEN page_position < ${limit_idx} AND cumulative_vertices <= ${budget_idx} THEN {output_feature} ELSE NULL END AS feature, cumulative_vertices, page_position FROM counted ORDER BY {candidate_order_expr} ASC"
            )
        }
        None => format!(
            "SELECT {pk}::{cast} AS pk_value, {feature} AS feature FROM {table} AS t{where_sql} ORDER BY {pk_order_expr} ASC LIMIT ${limit_idx}"
        ),
    };

    let count = if query.bbox.is_none() && query.datetime.is_none() && query.filter.is_none() {
        match collection.row_estimate {
            Some(estimate) => CountPlan::Cached(estimate),
            None => CountPlan::Query(
                // `reltuples` is -1 for a table that has never been
                // ANALYZEd (the state right after a fresh ingest, before
                // autovacuum's first pass); GREATEST clamps that sentinel
                // to 0 so a brand-new collection reports an honest "no
                // rows yet" estimate instead of silently losing
                // `numberMatched` to a negative value that doesn't fit
                // `u64`.
                "SELECT GREATEST(reltuples, 0)::bigint AS estimate FROM pg_class WHERE oid = to_regclass($1)"
                    .to_string(),
                vec![SqlParam::Text(collection.resolved_table().to_string())],
            ),
        }
    } else {
        CountPlan::None
    };

    Ok(ItemsPlan { sql, params, count })
}

/// SQL operator text for `op`.
fn compare_op_sql(op: CompareOp) -> &'static str {
    match op {
        CompareOp::Eq => "=",
        CompareOp::Ne => "<>",
        CompareOp::Lt => "<",
        CompareOp::Gt => ">",
        CompareOp::Le => "<=",
        CompareOp::Ge => ">=",
    }
}

/// Compiles a `tellurion_core::Filter` (`#33`) to a parenthesized boolean SQL
/// expression, pushing every literal onto `params` and referencing it only
/// by `$N` placeholder — never string-interpolated, matching every other
/// builder in this module. Every property name reaching here has already
/// passed `tellurion_core::filter::validate` against the collection's
/// descriptor (checked by `tellurion-features`' handler before `items` is
/// ever called), but `quote_ident` still whitelist-validates it again here
/// as defense in depth, exactly as `ident.rs`'s own doc comment describes
/// for every identifier that ends up in SQL text.
///
/// Comparison predicates cast the *column* to the literal's own type
/// (`::text`/`::double precision`/`::boolean`) rather than casting the bound
/// parameter to a guessed column type: `sql.rs` compiles from a
/// `CollectionDecl` alone, which carries no attribute-type information (that
/// lives on `CollectionDescriptor`, computed by `Router` and not threaded
/// this deep) — so the literal's own CQL2 type, which is always known here,
/// is the one reliable source of truth for what comparison to run. This
/// mirrors the existing datetime-filter clause's own pattern of binding the
/// literal as text and doing the type-specific cast in SQL text.
/// A scalar `Literal`'s SQL cast text plus its bound `SqlParam` — shared by
/// `Compare`, `Between`, and `In` below, all three of which cast the
/// *column* to the literal's own type rather than the parameter to a guessed
/// column type; see `compile_filter`'s own doc for why.
fn literal_cast_and_param(value: &Literal) -> (&'static str, SqlParam) {
    match value {
        Literal::Text(s) => ("::text", SqlParam::Text(s.clone())),
        Literal::Number(n) => ("::double precision", SqlParam::Float8(*n)),
        Literal::Bool(b) => ("::boolean", SqlParam::Bool(*b)),
    }
}

/// The SRID of CRS84 — the CRS OGC API — Features Part 3 Requirement 7
/// (`/req/filter/filter-crs-wgs84`) says a filter's spatial literals are
/// expressed in when no `filter-crs` parameter was supplied, and the SRID
/// every literal in this module is *built* at before any transform. Named
/// rather than spelled `4326` inline in [`geometry_literal_expr`] only
/// because that function also builds literals at a *different*,
/// per-collection SRID, so the two readings need to be told apart at a
/// glance.
const CRS84_SRID: i32 = 4326;

/// Which CRS a `Filter`'s own spatial literals are expressed in, as the
/// filter compiler needs to see it (`#217`): the resolved `filter-crs` query
/// parameter, plus the storage SRID it has to be reconciled against. Bundled
/// into one `Copy` value rather than threaded as two more parameters through
/// [`compile_filter`]'s recursion, and constructed at each entry point so a
/// reader can see at the call site which lane honours a client-declared CRS
/// and which does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FilterCrs {
    requested: RequestedCrs,
    storage_srid: Option<i32>,
}

impl FilterCrs {
    /// The `filter-crs` a client actually named, resolved against this
    /// collection's own supported CRS set by `tellurion-features`' handler
    /// before it ever reached a driver ([`ItemsQuery::filter_crs`]).
    fn requested(requested: RequestedCrs, storage_srid: Option<i32>) -> Self {
        Self {
            requested,
            storage_srid,
        }
    }

    /// The reading every lane whose only filter is a server-side `#34` ABAC
    /// grant filter keeps: a grant is authored by the deployment, not by a
    /// client, so there is no `filter-crs` to honour and never was one —
    /// [`RequestedCrs::Omitted`], the same reading the items lane gives an
    /// unparameterized request.
    ///
    /// `storage_srid` **is** carried (`#247`). Before that issue the `Omitted`
    /// arm ignored it, so passing one would have suggested a CRS choice this
    /// lane does not have; now the arm transforms a CRS84 literal into a
    /// projected storage CRS, and withholding the SRID here would leave a
    /// grant filter comparing a 4326 literal against a 3857 column — the
    /// mixed-SRID `500` `#247` exists to remove, reappearing on the
    /// single-item and MVT lanes for a deployment whose *own* policy grant is
    /// the only filter in the query. Every grant this workspace can express
    /// authors its spatial literals in CRS84 (the one CRS every filter
    /// compiler here reads a literal in — `tellurion_core::crs::
    /// crs84_literals_need_transform`'s own doc), so the SRID is all this lane
    /// needs to compile the same predicate `build_items_plan` already compiles
    /// for the identical grant.
    fn grant_only(storage_srid: Option<i32>) -> Self {
        Self {
            requested: RequestedCrs::Omitted,
            storage_srid,
        }
    }
}

/// A `GeometryLiteral`'s SQL expression, bound as a `$N`-parameterized
/// `ST_MakeEnvelope`/`ST_GeomFromGeoJSON`/`ST_GeomFromText` call and brought
/// into the storage CRS the geometry column it will be compared against
/// actually lives in — shared by `Intersects` and `Spatial` below, the two
/// binary spatial predicates this module compiles.
///
/// `crs` is OGC API — Features Part 3: Filtering (19-079r2)'s `filter-crs`
/// parameter (`#217`), and this function is where Requirements 7 and 8 are
/// actually honoured. The three arms mirror [`bbox_envelope_sql`]'s own,
/// which does the identical job for the `bbox`/`bbox-crs` pair under Part 2:
///
/// - [`RequestedCrs::Omitted`] — Requirement 7 (`/req/filter/
///   filter-crs-wgs84`): "the server SHALL process all geometries in the
///   filter expression using CRS84 ... as the coordinate reference system".
/// - [`RequestedCrs::Crs84`] — Requirement 8 (`/req/filter/
///   filter-crs-param`) with `filter-crs` naming CRS84 explicitly.
///
///   These two say the same thing about the same numbers — *these
///   coordinates are CRS84* — so they compile identically: the literal is
///   built at [`CRS84_SRID`] and then genuinely reprojected into the storage
///   CRS when that is a different one, skipped when it would be a no-op.
///
///   `#217` compiled the `Omitted` arm without the transform, deliberately:
///   changing it changes the SQL of a request an existing deployment already
///   serves, which is the campaign's first rule. `#247` is the case where
///   that rule was protecting nothing. On a *projected* storage the untransformed
///   literal reaches PostGIS tagged 4326 beside a 3857 column, and
///   `ST_Intersects` refuses the comparison outright — "Operation on mixed
///   SRID geometries" — so a plain conformant `filter=S_INTERSECTS(geom,
///   BBOX(...))` carrying no `filter-crs` at all answered `500`. There is no
///   working behaviour on that side of the branch to keep byte-for-byte, and
///   erroring is not "processing the geometries in CRS84". On a storage that
///   *is* CRS84 — every 4326 collection, which is every live demo — the match
///   below still produces the identical literal it always did, which is why
///   the transform is conditional on the SRID rather than unconditional.
///
///   This also puts the read-filter lane back in step with the write lane,
///   which has treated the two readings identically since Part 4 landed:
///   `write_sql::input_geom_expr`'s own `Omitted`/`Crs84` arm already
///   `ST_Transform`s a CRS84 request body into a projected storage SRID,
///   for the same reason — the coordinates genuinely are CRS84, so they must
///   be converted rather than relabelled.
/// - [`RequestedCrs::Storage`] — Requirement 8 with `filter-crs` naming this
///   collection's own storage CRS: the coordinates are already in that CRS,
///   so there is nothing to reproject — except the axis order when the CRS
///   is EPSG:4326 referenced by authority, which is latitude-before-
///   longitude (`tellurion_core::crs`'s own "Axis order" module doc). That
///   is the one case where honouring `filter-crs` changes which features
///   match without changing a single coordinate value, and it is exactly
///   what silently ignoring the parameter used to get wrong.
///
/// A driver that cannot perform the transform at all cannot reach this
/// function with a projected `storage_srid` and a spatial literal: the
/// protocol handlers refuse that request by name first
/// (`tellurion-features`' items handler and `tellurion-stac`'s
/// `unservable_filter_reason`, both keyed on
/// `tellurion_core::crs::crs84_literals_need_transform`), rather than let it
/// evaluate a CRS84 literal against projected coordinates under a `200`.
fn geometry_literal_expr(
    geometry: &GeometryLiteral,
    params: &mut Vec<SqlParam>,
    crs: FilterCrs,
) -> String {
    match crs.requested {
        RequestedCrs::Omitted | RequestedCrs::Crs84 => {
            let literal = geometry_literal_at(geometry, params, CRS84_SRID);
            match crs.storage_srid {
                Some(srid) if srid != CRS84_SRID => format!("ST_Transform({literal}, {srid})"),
                _ => literal,
            }
        }
        RequestedCrs::Storage => {
            // `crs::resolve` cannot produce `Storage` without a known
            // storage SRID in the first place, so the fallback is
            // unreachable — spelled the same defensive way
            // `bbox_envelope_sql`'s own `Storage` arm spells it.
            let srid = crs.storage_srid.unwrap_or(CRS84_SRID);
            let literal = geometry_literal_at(geometry, params, srid);
            if tellurion_core::crs::is_lat_lon_order(srid) {
                format!("ST_FlipCoordinates({literal})")
            } else {
                literal
            }
        }
    }
}

/// One `GeometryLiteral`, bound and tagged with `srid` — the SRID-agnostic
/// half of [`geometry_literal_expr`], which decides *which* SRID that is and
/// whether the result needs an `ST_Transform` wrapper on top.
///
/// With `srid` at [`CRS84_SRID`] this emits exactly the text this module
/// emitted for every spatial literal before `#217`. That is a statement about
/// *this* function only: since `#247` its CRS84 output is additionally
/// `ST_Transform`-wrapped by the caller whenever the collection's storage SRID
/// is a projected one, so the module's emitted SQL is byte-for-byte the
/// pre-`#217` text for a CRS84 storage and deliberately not for any other.
fn geometry_literal_at(
    geometry: &GeometryLiteral,
    params: &mut Vec<SqlParam>,
    srid: i32,
) -> String {
    match geometry {
        GeometryLiteral::Bbox([minx, miny, maxx, maxy]) => {
            params.push(SqlParam::Float8(*minx));
            params.push(SqlParam::Float8(*miny));
            params.push(SqlParam::Float8(*maxx));
            params.push(SqlParam::Float8(*maxy));
            let n = params.len();
            format!(
                "ST_MakeEnvelope(${}, ${}, ${}, ${}, {srid})",
                n - 3,
                n - 2,
                n - 1,
                n
            )
        }
        GeometryLiteral::GeoJson(value) => {
            params.push(SqlParam::Text(value.to_string()));
            format!("ST_SetSRID(ST_GeomFromGeoJSON(${}), {srid})", params.len())
        }
        GeometryLiteral::Wkt(geometry) => {
            params.push(SqlParam::Text(geometry.to_wkt_text()));
            format!("ST_SetSRID(ST_GeomFromText(${}), {srid})", params.len())
        }
    }
}

/// SQL function name for a `SpatialOp` — the binary spatial predicates CQL2
/// adds beyond `S_INTERSECTS`. Every one of the seven maps to the
/// identically-named PostGIS function with the same argument order (see
/// `SpatialOp`'s own doc).
fn spatial_op_sql(op: SpatialOp) -> &'static str {
    match op {
        SpatialOp::Within => "ST_Within",
        SpatialOp::Contains => "ST_Contains",
        SpatialOp::Disjoint => "ST_Disjoint",
        SpatialOp::Touches => "ST_Touches",
        SpatialOp::Overlaps => "ST_Overlaps",
        SpatialOp::Crosses => "ST_Crosses",
        SpatialOp::Equals => "ST_Equals",
    }
}

fn compile_filter(filter: &Filter, params: &mut Vec<SqlParam>, crs: FilterCrs) -> Result<String> {
    match filter {
        Filter::Compare {
            property,
            op,
            value,
        } => {
            let column = quote_ident(property)?;
            let op_sql = compare_op_sql(*op);
            let (cast, param) = literal_cast_and_param(value);
            params.push(param);
            Ok(format!("({column}{cast} {op_sql} ${})", params.len()))
        }
        Filter::IsNull { property, negated } => {
            let column = quote_ident(property)?;
            let not = if *negated { " NOT" } else { "" };
            Ok(format!("({column} IS{not} NULL)"))
        }
        // Postgres's `LIKE` already uses `%`/`_` wildcards and a backslash
        // escape by default — the same convention CQL2's own `LIKE` grammar
        // uses — so `pattern` needs no translation, only casting the column
        // to `::text` the same way every other string comparison here does.
        Filter::Like {
            property,
            pattern,
            negated,
        } => {
            let column = quote_ident(property)?;
            let not = if *negated { " NOT" } else { "" };
            params.push(SqlParam::Text(pattern.clone()));
            Ok(format!("({column}::text{not} LIKE ${})", params.len()))
        }
        Filter::Between {
            property,
            low,
            high,
            negated,
        } => {
            let column = quote_ident(property)?;
            let not = if *negated { "NOT " } else { "" };
            let (cast, low_param) = literal_cast_and_param(low);
            params.push(low_param);
            let low_idx = params.len();
            let (_, high_param) = literal_cast_and_param(high);
            params.push(high_param);
            let high_idx = params.len();
            Ok(format!(
                "({column}{cast} {not}BETWEEN ${low_idx} AND ${high_idx})"
            ))
        }
        Filter::In {
            property,
            values,
            negated,
        } => {
            let column = quote_ident(property)?;
            let not = if *negated { "NOT " } else { "" };
            // `IN ()` is a syntax error in both CQL2 encodings (unreachable
            // through either parser), but a hand-built `Filter` could still
            // carry an empty list — resolved the same harmless-identity way
            // `compile_bool_chain` resolves an empty `AND`/`OR`: `IN`
            // matches nothing, `NOT IN` matches everything.
            if values.is_empty() {
                return Ok(if *negated {
                    "TRUE".to_string()
                } else {
                    "FALSE".to_string()
                });
            }
            let (cast, _) = literal_cast_and_param(&values[0]);
            let mut placeholders = Vec::with_capacity(values.len());
            for value in values {
                let (_, param) = literal_cast_and_param(value);
                params.push(param);
                placeholders.push(format!("${}", params.len()));
            }
            Ok(format!(
                "({column}{cast} {not}IN ({}))",
                placeholders.join(", ")
            ))
        }
        // `CASEI(property) = CASEI('literal')`/`<>`: `lower()` is Postgres's
        // own case-folding builtin, not a stand-in for CQL2's full Unicode
        // case folding — under the common `C`/`POSIX` collation it folds
        // ASCII bytes only, and even under a Unicode-friendly locale it's
        // simple per-character mapping, not full folding (`ß`/`ss` never
        // matches). That gap is exactly why `case-insensitive-comparison`
        // stays out of `tellurion_core::filter::CQL2_CONFORMANCE_CLASSES`
        // despite this arm compiling the shape without error.
        Filter::CaseInsensitiveCompare {
            property,
            op,
            value,
        } => {
            let column = quote_ident(property)?;
            let op_sql = match op {
                CaseInsensitiveCompareOp::Eq => "=",
                CaseInsensitiveCompareOp::Ne => "<>",
            };
            params.push(SqlParam::Text(value.clone()));
            Ok(format!(
                "(lower({column}::text) {op_sql} lower(${}))",
                params.len()
            ))
        }
        Filter::And(items) => compile_bool_chain(items, "AND", params, crs),
        Filter::Or(items) => compile_bool_chain(items, "OR", params, crs),
        Filter::Not(inner) => Ok(format!("(NOT {})", compile_filter(inner, params, crs)?)),
        Filter::Intersects { property, geometry } => {
            let column = quote_ident(property)?;
            let geom_expr = geometry_literal_expr(geometry, params, crs);
            Ok(format!("ST_Intersects({column}, {geom_expr})"))
        }
        Filter::Spatial {
            property,
            op,
            geometry,
        } => {
            let column = quote_ident(property)?;
            let geom_expr = geometry_literal_expr(geometry, params, crs);
            let func = spatial_op_sql(*op);
            Ok(format!("{func}({column}, {geom_expr})"))
        }
        Filter::After { property, instant } => {
            let column = quote_ident(property)?;
            params.push(SqlParam::Text(instant.clone()));
            Ok(format!("({column} > ${}::text::timestamptz)", params.len()))
        }
        Filter::Before { property, instant } => {
            let column = quote_ident(property)?;
            params.push(SqlParam::Text(instant.clone()));
            Ok(format!("({column} < ${}::text::timestamptz)", params.len()))
        }
        Filter::During {
            property,
            start,
            end,
        } => {
            let column = quote_ident(property)?;
            params.push(SqlParam::Text(start.clone()));
            let start_idx = params.len();
            params.push(SqlParam::Text(end.clone()));
            let end_idx = params.len();
            Ok(format!(
                "({column} >= ${start_idx}::text::timestamptz AND {column} <= ${end_idx}::text::timestamptz)"
            ))
        }
        Filter::Temporal {
            property,
            op,
            value,
        } => {
            let column = quote_ident(property)?;
            Ok(temporal_op_sql(*op, &column, value, params))
        }
    }
}

/// Compiles one of the twelve `TemporalOp` variants ([`Filter::Temporal`])
/// to a bound-parameter SQL boolean expression, per this module's own
/// top-level doc: `column` is the property's own instant value, treated as
/// the degenerate Allen interval `[p, p]`; `value` is the literal's interval
/// `[start, end]` (`start == end` for a [`TemporalValue::Instant`] — the
/// degenerate case again). Every formula below is Allen's own interval
/// relation between `[p, p]` (`a1 = a2 = p`) and `[start, end]`
/// (`b1 = start`, `b2 = end`) — see each arm's comment for the
/// un-substituted relation. `bind` pushes one `::text` parameter per
/// *occurrence* in a formula (never reusing a placeholder index across
/// occurrences, even same-bound ones like `Overlaps`'s two references to
/// `start`), so the number of bound parameters always matches the number of
/// `$N` placeholders the generated text actually contains — the same
/// parameter-per-occurrence discipline `compile_filter`'s `Between`/`In`
/// arms already follow. `Overlaps`/`OverlappedBy`/`StartedBy`/
/// `FinishedBy`/`Contains` all require the *first* interval to have
/// positive duration, which `[p, p]` structurally never does, so those five
/// compile to a condition that can never be satisfied by any row — the
/// mathematically correct answer for "does an instant overlap/contain/get
/// started-or-finished-by a proper interval", not a stub.
fn temporal_op_sql(
    op: TemporalOp,
    column: &str,
    value: &TemporalValue,
    params: &mut Vec<SqlParam>,
) -> String {
    let (start, end): (&str, &str) = match value {
        TemporalValue::Instant(instant) => (instant.as_str(), instant.as_str()),
        TemporalValue::Interval(start, end) => (start.as_str(), end.as_str()),
    };
    let mut bind = |text: &str| {
        params.push(SqlParam::Text(text.to_string()));
        format!("${}::text::timestamptz", params.len())
    };
    match op {
        // equals: a1=b1 AND a2=b2
        TemporalOp::Equals => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} = {b1} AND {column} = {b2})")
        }
        // disjoint: a2<b1 OR a1>b2 (before or after)
        TemporalOp::Disjoint => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} < {b1} OR {column} > {b2})")
        }
        // intersects: NOT disjoint
        TemporalOp::Intersects => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} >= {b1} AND {column} <= {b2})")
        }
        // meets: a2=b1
        TemporalOp::Meets => {
            let b1 = bind(start);
            format!("({column} = {b1})")
        }
        // met-by: b2=a1
        TemporalOp::MetBy => {
            let b2 = bind(end);
            format!("({column} = {b2})")
        }
        // starts: a1=b1 AND a2<b2
        TemporalOp::Starts => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} = {b1} AND {column} < {b2})")
        }
        // started-by: a1=b1 AND a2>b2
        TemporalOp::StartedBy => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} = {b1} AND {column} > {b2})")
        }
        // finishes: a2=b2 AND a1>b1
        TemporalOp::Finishes => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} = {b2} AND {column} > {b1})")
        }
        // finished-by: a2=b2 AND a1<b1
        TemporalOp::FinishedBy => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} = {b2} AND {column} < {b1})")
        }
        // overlaps: a1<b1 AND b1<a2 AND a2<b2
        TemporalOp::Overlaps => {
            let (b1_lo, b1_hi, b2) = (bind(start), bind(start), bind(end));
            format!("({column} < {b1_lo} AND {b1_hi} < {column} AND {column} < {b2})")
        }
        // overlapped-by: b1<a1 AND a1<b2 AND b2<a2
        TemporalOp::OverlappedBy => {
            let (b1, b2_lo, b2_hi) = (bind(start), bind(end), bind(end));
            format!("({b1} < {column} AND {column} < {b2_lo} AND {b2_hi} < {column})")
        }
        // contains: a1<b1 AND a2>b2
        TemporalOp::Contains => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} < {b1} AND {column} > {b2})")
        }
    }
}

/// Shared `AND`/`OR` compilation: every item must compile, and an empty
/// `items` (CQL2 never produces one from either parser, but a hand-built
/// `Filter` could) resolves to a harmless `TRUE`/`FALSE` identity rather than
/// a malformed empty-parens SQL fragment.
fn compile_bool_chain(
    items: &[Filter],
    joiner: &str,
    params: &mut Vec<SqlParam>,
    crs: FilterCrs,
) -> Result<String> {
    if items.is_empty() {
        return Ok(if joiner == "AND" {
            "TRUE".to_string()
        } else {
            "FALSE".to_string()
        });
    }
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        parts.push(compile_filter(item, params, crs)?);
    }
    Ok(format!("({})", parts.join(&format!(" {joiner} "))))
}

/// `filter` is a `#34` ABAC grant filter, AND-merged into the single-row
/// `WHERE` clause exactly the way `build_items_plan` merges one into the
/// items-list query — an item that exists but that the filter excludes
/// simply matches zero rows, giving the caller the same `Ok(None)` a
/// genuinely absent id produces (no distinct "found but filtered" signal).
/// `None` compiles to plain `{pk} = $1` for an `Integer` collection (no
/// cast — see this module's own top-level doc for why); a `Uuid`/`Text`
/// collection casts `::uuid`/`::text` instead (`#87`) — `pk_value`'s own
/// variant, resolved by the caller via `PkValue::parse(collection.id_type,
/// ...)`, always matches the shape this function emits.
pub(crate) fn build_item_plan(
    collection: &CollectionDecl,
    pk_value: PkValue,
    filter: Option<&Filter>,
    requested_crs: RequestedCrs,
) -> Result<(String, Vec<SqlParam>)> {
    let table = quote_ident(collection.resolved_table())?;
    let pk = quote_ident(collection.resolved_pk())?;
    let geom = quote_ident(collection.resolved_geometry())?;
    let pk_key = quote_literal(collection.resolved_pk())?;
    let geom_key = quote_literal(collection.resolved_geometry())?;
    // Same index-preserving shape as `build_items_plan`'s `pk_order_expr`
    // (see this module's own top-level doc): `Integer` compares the bare
    // column directly rather than casting it to `::bigint`, so a plain
    // btree index on the pk still serves this equality lookup instead of
    // forcing a full scan. `Uuid`/`Text` are unchanged.
    let pk_predicate = match collection.id_type {
        IdType::Integer => pk.clone(),
        IdType::Uuid | IdType::Text => format!("{pk}::{}", pk_sql_cast(collection.id_type)),
    };
    // Same parameter-type-inference reasoning as `build_items_plan`'s token
    // clause: `Integer`'s bare LHS leaves the placeholder's type otherwise
    // inferred as the physical column's own (`int4`), which then rejects
    // the `i64` this crate always binds for an `Integer` id.
    let param_suffix = match collection.id_type {
        IdType::Integer => "::bigint",
        IdType::Uuid | IdType::Text => "",
    };

    let geom_expr = reprojected_geom_expr(&geom, requested_crs, collection.srid);
    let feature = feature_expr(collection, &pk, &geom_expr, &pk_key, &geom_key);
    let mut params = vec![pk_value.as_sql_param()];
    let filter_clause = match filter {
        Some(filter) => format!(
            " AND {}",
            compile_filter(filter, &mut params, FilterCrs::grant_only(collection.srid))?
        ),
        None => String::new(),
    };
    let sql = match collection.settings.items_vertex_budget {
        Some(vertex_budget) => {
            params.push(SqlParam::Bigint(
                i64::try_from(vertex_budget).unwrap_or(i64::MAX),
            ));
            let budget_idx = params.len();
            let vertex_count = format!("COALESCE(ST_NPoints({geom}), 0)::bigint");
            format!(
                "SELECT CASE WHEN {vertex_count} <= ${budget_idx} THEN {feature} ELSE NULL END AS feature, {vertex_count} AS cumulative_vertices, {pk}::text AS feature_id FROM {table} AS t WHERE {pk_predicate} = $1{param_suffix}{filter_clause}"
            )
        }
        None => format!(
            "SELECT {feature} AS feature FROM {table} AS t WHERE {pk_predicate} = $1{param_suffix}{filter_clause}"
        ),
    };

    Ok((sql, params))
}

/// `collection.tile_properties`, each whitelist-quoted and selected straight
/// off `t` (`#85`) — the columns a client can style/filter on beyond the pk
/// every tile already carries under the reserved `id` property. Every entry
/// has already been reconciled against this collection's derived attribute
/// schema at boot-or-first-touch (`descriptor::reconcile_tile_properties`),
/// so this never has to re-check existence or type — it only has to quote
/// and select. Empty (the default) contributes nothing, keeping the MVT
/// subquery's own SELECT list byte-for-byte the pre-`#85` `id`+`geom` shape.
fn tile_property_columns(tile_properties: &[String]) -> Result<String> {
    tile_property_column_list(tile_properties, "t.")
}

/// Shared by [`tile_property_columns`] (selecting straight off the base
/// table, aliased `t`) and `build_mvt_budgeted_plan` (`#90`, re-projecting
/// the same column names off its own `budgeted` CTE, which carries no table
/// alias) — same whitelist-quoting, different prefix.
fn tile_property_column_list(tile_properties: &[String], prefix: &str) -> Result<String> {
    let mut columns = String::new();
    for property in tile_properties {
        let quoted = quote_ident(property)?;
        columns.push_str(", ");
        columns.push_str(prefix);
        columns.push_str(&quoted);
    }
    Ok(columns)
}

/// The `tile_env` CTE prelude every MVT-family plan below shares, binding
/// `ST_TileEnvelope`'s three positional args to `$1`/`$2`/`$3`.
const TILE_ENV_CTE: &str = "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom)";

/// The `#190` WorldCRS84Quad counterpart of [`TILE_ENV_CTE`]: the tile's
/// CRS84-degrees envelope, bound as four `float8` corners computed in Rust
/// by `tellurion_core::world_crs84_tile_bounds_deg` — Postgres has no
/// built-in for the 2x1-rooted CRS84 grid (`ST_TileEnvelope`'s custom
/// `bounds` argument still assumes a SQUARE `2^z` matrix), and computing
/// the box where the grid registry lives means the SQL lane can never
/// drift from the bounds the handlers validated `x`/`y` against.
const WORLD_CRS84_TILE_ENV_CTE: &str =
    "WITH tile_env AS (SELECT ST_MakeEnvelope($1, $2, $3, $4, 4326) AS geom)";

/// A collection's storage SRID as the tile lane reads it (`#262`): the
/// declared one, or [`CRS84_SRID`] when the backend reported none.
///
/// The fallback is not a guess about the data, it is the rule the rest of
/// this module already applies to the same unknown — `bbox_envelope_sql`'s
/// `RequestedCrs::Storage` arm resolves an unknown SRID the same way, and
/// `tellurion_core::crs::crs84_literals_need_transform` answers `false` for
/// `None` for the same reason: nothing is known that could make a transform
/// necessary, and inventing one would move a deployment whose bytes are
/// correct today. It is also what keeps a no-SRID collection's tile SQL
/// byte-for-byte what it has always been.
fn tile_storage_srid(collection: &CollectionDecl) -> i32 {
    collection.srid.unwrap_or(CRS84_SRID)
}

/// The tile envelope, expressed in the CRS of the geometry column the `&&`
/// candidate predicate compares it against (`#262`).
///
/// `tile_env.geom` is always in the *grid's* own CRS
/// ([`TileMatrixSet::crs_srid`]) — `ST_TileEnvelope` returns EPSG:3857, and
/// [`WORLD_CRS84_TILE_ENV_CTE`] builds its box at 4326. The stored geometry
/// is in the collection's storage CRS. Before `#262` the predicate spelled
/// one fixed answer for each grid (`ST_Transform(tile_env.geom, 4326)` for
/// mercator, a bare `tile_env.geom` for CRS84), which is correct if and only
/// if the storage is 4326 — and `&&`, unlike `ST_Intersects`, does not
/// object to mixed SRIDs, so a projected collection compared metres against
/// degrees and matched nothing rather than raising:
///
/// ```sql
/// SELECT ST_SetSRID(ST_MakePoint(1113195, 1118890), 3857)
///        && ST_Transform(ST_TileEnvelope(8, 135, 120), 4326);
/// -- f
/// ```
///
/// A tile carries no CRS declaration of its own, so there is no third option
/// where the wrong content is served and annotated: OGC API — Tiles Part 1
/// Requirement 5 (`/req/core/tc-success`) clause B requires the response to
/// "represent elements inside or intersecting with the spatial extent of the
/// geographical area of the tile identified by the tile matrix, tile row,
/// and tile column of the tileset's tile matrix set", and Requirement 6
/// clause B allows an empty response only when "the tile has no content due
/// to lack of data in the area". PostGIS can transform, so it transforms.
///
/// Rule 1: a CRS84-equivalent storage (4326, or unknown — see
/// [`tile_storage_srid`]) produces exactly the text each grid produced
/// before `#262`, character for character, which is every live Render demo.
///
/// The transform is applied to the envelope polygon's vertices, so for a
/// storage CRS whose graticule curves relative to the grid's, the prune is
/// the box of the four transformed corners rather than the exact
/// reprojected footprint — the same approximation `bbox_envelope_sql`
/// already makes for a request `bbox` (`#255`), and a prune only: the tile's
/// real clip happens in the grid's own CRS inside `ST_AsMVTGeom`, and the
/// MVT buffer already widens the kept area. It is exact for the case that
/// motivated `#262`, a 3857 storage on the mercator grid, where no transform
/// is emitted at all.
fn tile_envelope_in_storage_crs(tms: TileMatrixSet, storage_srid: i32) -> String {
    if storage_srid == tms.crs_srid() {
        "tile_env.geom".to_string()
    } else {
        format!("ST_Transform(tile_env.geom, {storage_srid})")
    }
}

/// The stored geometry, expressed in the tile grid's own CRS (`#262`) — the
/// mirror of [`tile_envelope_in_storage_crs`], for the other side of the
/// tile pipeline.
///
/// Everything downstream of this expression is in grid units: the
/// simplification tolerance `driver.rs` derives per grid
/// (`build_mvt_candidate_fragment`'s own doc: mercator metres, or CRS84
/// degrees), and `ST_AsMVTGeom`'s clip against `tile_env.geom`. So the
/// geometry has to arrive in the grid's CRS whatever it is stored in.
///
/// Rule 1 again, and note this is not symmetrical-looking by accident: the
/// mercator arm has always emitted `ST_Transform(t.<geom>, 3857)`, which is
/// already right for any storage SRID PostGIS can transform *from*, so a
/// 4326 collection is untouched; the WorldCRS84Quad arm emitted a bare
/// `t.<geom>`, right only for 4326 storage, and a 4326 collection is
/// untouched there too because 4326 *is* that grid's CRS. What changes is
/// only the projected case, where one arm was silently wrong and the other
/// silently right.
fn storage_geom_in_grid_crs(geom_expr: &str, tms: TileMatrixSet, storage_srid: i32) -> String {
    if storage_srid == tms.crs_srid() {
        geom_expr.to_string()
    } else {
        format!("ST_Transform({geom_expr}, {})", tms.crs_srid())
    }
}

/// The candidate-row subquery `build_mvt_plan`, `build_mvt_vertex_total_plan`,
/// and `build_mvt_budgeted_plan` (`#90`) all select from: the same
/// envelope/simplify/clip/bbox/filter/`LIMIT $6` shape, factored out once so
/// the three can never drift out of agreement on which rows a tile is built
/// from. Returns the bare `SELECT ... LIMIT $6` text (no enclosing
/// parentheses or alias — each caller wraps it its own way, after
/// `TILE_ENV_CTE`) plus the six fixed params (`$1..$6`) any filter params
/// are numbered after.
///
/// ## The MVT wire-format "id" is never set — `#87`
///
/// `ST_AsMVT`'s protobuf feature id (an *unsigned integer* by the MVT spec)
/// is a distinct, optional 5th argument this fragment's callers never pass —
/// the pk is exposed only as an ordinary `{pk}::text AS id` attribute in the
/// row `ST_AsMVT` encodes, a plain UTF-8 tag like any `tile_property_columns`
/// entry. That choice predates `#87` (a collection-wide "no per-feature
/// native id" decision, made once here) and is what makes this fragment
/// already correct for a non-integer pk with zero special-casing: `::text`
/// never fails to cast any SQL type, so an `Integer` and a `Uuid` collection
/// produce byte-for-byte identical SQL shape here (`mvt_plan_shape`'s own
/// SQL-golden test and `mvt_plan_is_identical_regardless_of_id_type` below
/// both prove this), and this is a documented, honest choice — never a
/// lossy silent cast of a value that can't fit the wire format's real
/// integer id — not an oversight `#87` is expected to "fix".
/// `tolerance` is expressed in the tile grid's own CRS units (`#190`):
/// mercator METERS for `WebMercatorQuad` (exactly the pre-`#190` contract),
/// CRS84 DEGREES for `WorldCRS84Quad` — `driver.rs` converts before calling
/// in, since `ST_SimplifyPreserveTopology` runs over the geometry in the
/// grid's own CRS either way.
fn build_mvt_candidate_fragment(
    collection: &CollectionDecl,
    tms: TileMatrixSet,
    coord: TileCoord,
    tolerance: f64,
    buffer: u32,
    cap: u64,
    filter: Option<&Filter>,
) -> Result<(String, Vec<SqlParam>)> {
    let table = quote_ident(collection.resolved_table())?;
    let pk = quote_ident(collection.resolved_pk())?;
    // `#104`: reads whichever declared `geometry_variants` column covers
    // `coord.z`, falling back to the base `geometry` column when none does
    // — see `CollectionDecl::resolved_geometry_for_zoom`. Every other MVT
    // caller (`build_mvt_plan`/`build_mvt_vertex_total_plan`/
    // `build_mvt_budgeted_plan`) shares this fragment, so all three honor
    // the same per-zoom selection automatically.
    let geom = quote_ident(collection.resolved_geometry_for_zoom(coord.z))?;

    // `#262`: the two sides of this fragment's CRS boundary, decided once
    // from the grid's own CRS and the collection's storage SRID rather than
    // spelled per-arm. A declared `geometry_variants` column is required to
    // carry the base column's SRID (`Router::refuse_invalid_geometry_
    // variants` refuses the config at boot otherwise), so `collection.srid`
    // describes whichever column `geom` above resolved to and a variant
    // needs no separate question asked of it.
    let storage_srid = tile_storage_srid(collection);
    let tile_env_geom = tile_envelope_in_storage_crs(tms, storage_srid);
    let source_geom = storage_geom_in_grid_crs(&format!("t.{geom}"), tms, storage_srid);

    // `#190`: the two grids differ in exactly three places — how the tile
    // envelope is bound (`TILE_ENV_CTE` vs. `WORLD_CRS84_TILE_ENV_CTE`,
    // chosen by the callers via `tile_env_cte`), which CRS the geometry is
    // clipped/simplified in (mercator meters vs. CRS84 degrees — `#262`
    // derives both directions of that from `tms.crs_srid()` above, where
    // `#190` had assumed 4326 storage on both arms exactly as the
    // pre-`#190` `&& ST_Transform(tile_env.geom, 4326)` predicate did), and
    // where the fixed params sit (the CRS84 envelope needs four corner
    // params before them). On a CRS84-equivalent storage the mercator arm
    // is still byte-for-byte the pre-`#190` SQL.
    let (mut params, tile_geom_expr, envelope_pred, limit_ref) = match tms {
        TileMatrixSet::WebMercatorQuad => {
            // `ST_TileEnvelope(zoom, x, y, ...)` takes `integer` (int4) for
            // all three positional arguments; binding x/y as int8 fails to
            // type-check against Postgres's inferred parameter types
            // ("WrongType") even though `TileCoord` stores them as `u32`.
            // Safe because the config-enforced zoom ceiling (24) keeps the
            // matrix side within i32 range.
            let x =
                i32::try_from(coord.x).map_err(|_| PostgisError::TileCoordOutOfRange(coord.x))?;
            let y =
                i32::try_from(coord.y).map_err(|_| PostgisError::TileCoordOutOfRange(coord.y))?;
            let params = vec![
                SqlParam::Int4(i32::from(coord.z)),
                SqlParam::Int4(x),
                SqlParam::Int4(y),
                SqlParam::Float8(tolerance),
                // `tile_buffer_px` derives `buffer` from `MVT_EXTENT`
                // (currently 256), well within i32 range regardless of
                // extent.
                SqlParam::Int4(i32::try_from(buffer).unwrap_or(i32::MAX)),
                SqlParam::Bigint(i64::try_from(cap).unwrap_or(i64::MAX)),
            ];
            (
                params,
                format!("ST_SimplifyPreserveTopology({source_geom}, $4), tile_env.geom, {MVT_EXTENT}, $5"),
                format!("t.{geom} && {tile_env_geom}"),
                "$6",
            )
        }
        TileMatrixSet::WorldCrs84Quad => {
            let [minlon, minlat, maxlon, maxlat] = world_crs84_tile_bounds_deg(coord);
            let params = vec![
                SqlParam::Float8(minlon),
                SqlParam::Float8(minlat),
                SqlParam::Float8(maxlon),
                SqlParam::Float8(maxlat),
                SqlParam::Float8(tolerance),
                SqlParam::Int4(i32::try_from(buffer).unwrap_or(i32::MAX)),
                SqlParam::Bigint(i64::try_from(cap).unwrap_or(i64::MAX)),
            ];
            (
                params,
                format!(
                    "ST_SimplifyPreserveTopology({source_geom}, $5), tile_env.geom, {MVT_EXTENT}, $6"
                ),
                // A CRS84 storage shares this grid's own SRID, so neither
                // side of the predicate carries a transform — which is what
                // `#190` hardcoded, and what `#262` now derives.
                format!("t.{geom} && {tile_env_geom}"),
                "$7",
            )
        }
    };

    // Pushed after the fixed params above, so a filter's own placeholders
    // start right past them (`$7` for the mercator grid, `$8` for CRS84)
    // regardless of whether one is present. Postgres binds a parameter by
    // number, not by where `$N` appears in the SQL text, so filter params
    // referenced inside the `WHERE` clause below (which appears before the
    // fixed `LIMIT` textually) still resolve correctly.
    let filter_clause = match filter {
        Some(filter) => format!(
            " AND {}",
            compile_filter(filter, &mut params, FilterCrs::grant_only(collection.srid))?
        ),
        None => String::new(),
    };

    let property_columns = tile_property_columns(&collection.tile_properties)?;

    let sql = format!(
        "SELECT {pk}::text AS id, ST_AsMVTGeom({tile_geom_expr}, true) AS geom{property_columns} FROM {table} AS t, tile_env WHERE {envelope_pred}{filter_clause} LIMIT {limit_ref}"
    );

    Ok((sql, params))
}

/// The `tile_env` CTE matching [`build_mvt_candidate_fragment`]'s parameter
/// layout for `tms` — the two must always be chosen together, which is why
/// every plan builder below goes through this one function.
fn tile_env_cte(tms: TileMatrixSet) -> &'static str {
    match tms {
        TileMatrixSet::WebMercatorQuad => TILE_ENV_CTE,
        TileMatrixSet::WorldCrs84Quad => WORLD_CRS84_TILE_ENV_CTE,
    }
}

/// `filter` is a `#34` ABAC grant filter, AND-merged into this subquery's own
/// `WHERE` clause the same way `build_items_plan` merges one into the
/// items-list query. `None` compiles to the pre-`#34` SQL, byte-for-byte.
/// `tms` (`#190`) picks the tile-envelope CTE + candidate fragment pair —
/// see [`build_mvt_candidate_fragment`]; `WebMercatorQuad` compiles to the
/// pre-`#190` SQL, byte-for-byte.
pub(crate) fn build_mvt_plan(
    collection: &CollectionDecl,
    tms: TileMatrixSet,
    coord: TileCoord,
    tolerance: f64,
    buffer: u32,
    cap: u64,
    filter: Option<&Filter>,
) -> Result<(String, Vec<SqlParam>)> {
    // The MVT layer name a client actually sees and must reference in a
    // style's `source-layer` (`#49`) — always the collection's PUBLIC id,
    // never its internal one. A collection whose `external_id` differs from
    // `id` (a config-level alias/rename) would otherwise embed a name no
    // client can derive from anything the API exposes. `external_id` is
    // free-form config text (no identifier charset restriction), so this
    // uses `quote_sql_string` (escapes, never rejects) rather than
    // `quote_literal` (identifier-charset whitelist, meant for column names).
    let layer = quote_sql_string(collection.external_id());

    let (fragment, params) =
        build_mvt_candidate_fragment(collection, tms, coord, tolerance, buffer, cap, filter)?;

    let cte = tile_env_cte(tms);
    let sql = format!(
        "{cte} SELECT ST_AsMVT(mvt, {layer}, {MVT_EXTENT}, 'geom') FROM ({fragment}) AS mvt"
    );

    Ok((sql, params))
}

/// A cheap pre-flight probe for `#90`'s per-tile vertex budget: the same
/// candidate rows `build_mvt_plan` would encode, but aggregated to a single
/// `SUM(ST_NPoints(geom))` instead of built into MVT bytes — no geometry
/// ever crosses back into Rust, just one integer. `driver.rs` runs this
/// first and only reaches for the truncating [`build_mvt_budgeted_plan`]
/// when the total comes back over budget; an under-budget tile always takes
/// the untouched `build_mvt_plan` path above, so its wire bytes stay
/// byte-for-byte what they were before `#90`.
pub(crate) fn build_mvt_vertex_total_plan(
    collection: &CollectionDecl,
    tms: TileMatrixSet,
    coord: TileCoord,
    tolerance: f64,
    buffer: u32,
    cap: u64,
    filter: Option<&Filter>,
) -> Result<(String, Vec<SqlParam>)> {
    let (fragment, params) =
        build_mvt_candidate_fragment(collection, tms, coord, tolerance, buffer, cap, filter)?;

    let cte = tile_env_cte(tms);
    let sql = format!(
        "{cte} SELECT COALESCE(SUM(ST_NPoints(geom)), 0)::bigint FROM ({fragment}) AS budget_probe"
    );

    Ok((sql, params))
}

/// The truncating counterpart of `build_mvt_plan` (`#90`): only reached once
/// [`build_mvt_vertex_total_plan`] has already reported the candidate set
/// over `vertex_budget`. Runs a cumulative `SUM(ST_NPoints(geom)) OVER
/// (ORDER BY id)` across the same candidate rows and keeps only the prefix
/// whose running total still fits — the row that would tip the budget over,
/// and everything after it (the running sum only grows), is dropped. This
/// is "dropping the marginal geometry" from the `#90` proposal, not
/// generalization: no geometry is altered, some are simply never encoded.
/// The `ORDER BY id` this needs (for the running sum to mean anything) only
/// ever runs on this rarer, already-over-budget path — the common,
/// under-budget path never adds one, so it never risks reordering an
/// otherwise-unaffected tile's features.
///
/// `#190` pushed this over clippy's argument ceiling by one (`tms`); every
/// argument is a distinct, non-optional plan input shared with the other
/// two MVT builders, so a params struct here would only rename the problem.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_mvt_budgeted_plan(
    collection: &CollectionDecl,
    tms: TileMatrixSet,
    coord: TileCoord,
    tolerance: f64,
    buffer: u32,
    cap: u64,
    vertex_budget: u64,
    filter: Option<&Filter>,
) -> Result<(String, Vec<SqlParam>)> {
    let layer = quote_sql_string(collection.external_id());

    let (fragment, mut params) =
        build_mvt_candidate_fragment(collection, tms, coord, tolerance, buffer, cap, filter)?;
    params.push(SqlParam::Bigint(
        i64::try_from(vertex_budget).unwrap_or(i64::MAX),
    ));
    let budget_param = params.len();

    // Re-selects `id`/`geom`/properties by name rather than `SELECT *` —
    // `budgeted` also carries `running_vertices`, an internal accounting
    // column that must never reach `ST_AsMVT` (it would otherwise leak into
    // every feature's attribute table as a bogus property).
    let property_columns = tile_property_column_list(&collection.tile_properties, "")?;
    let cte = tile_env_cte(tms);
    let sql = format!(
        "{cte}, candidate AS ({fragment}), budgeted AS (SELECT *, SUM(ST_NPoints(geom)) OVER (ORDER BY id) AS running_vertices FROM candidate) SELECT ST_AsMVT(mvt, {layer}, {MVT_EXTENT}, 'geom') FROM (SELECT id, geom{property_columns} FROM budgeted WHERE running_vertices <= ${budget_param}) AS mvt"
    );

    Ok((sql, params))
}

/// Builds the volume-tile fetch (`#41` part 4): reprojects to EPSG:3857 in
/// SQL (`ST_Transform`) so `driver.rs` only has to apply the affine
/// world-to-tile-local step in Rust (`volume::TileTransform`), then hands
/// back raw `ST_AsEWKB` bytes for `ewkb::decode_solid`. No simplification
/// and no MVT buffer — unlike [`build_mvt_plan`], a solid's real geometry is
/// wanted exactly as stored, and `cap` is the same per-zoom row limit
/// `mvt_tile` uses (`descriptor::heuristics::effective_feature_cap`), so a
/// table with an enormous number of solids in one tile still returns
/// bounded, not the whole thing.
///
/// `filter` is a `#34` ABAC grant filter (`#70`), AND-merged into this
/// query's own `WHERE` clause the same way `build_mvt_plan` merges one into
/// the MVT subquery's — its own placeholders numbered after the four fixed
/// params, even though they appear in the SQL text before the `$4` `LIMIT`
/// (Postgres binds by number, not by where `$N` appears in the text, same
/// as `build_mvt_plan`'s own doc explains). `None` compiles to the pre-`#70`
/// SQL, byte-for-byte.
///
/// `#262`: this lane's tile envelope meets a stored geometry exactly the way
/// [`build_mvt_candidate_fragment`]'s does, and had the identical
/// 4326-storage assumption baked into its own `&& ST_Transform(tile_env.geom,
/// 4326)`. It shares the same two helpers rather than a second copy of the
/// rule — a `#41` volume collection stored in a projected CRS returned an
/// empty 3D tile for the same reason, and would have kept doing so if only
/// the MVT fragment had been fixed. The grid is always
/// [`TileMatrixSet::WebMercatorQuad`] here (`ST_TileEnvelope`, and
/// `volume::TileTransform`'s affine step is defined in mercator metres);
/// this lane serves no second grid.
pub(crate) fn build_volume_plan(
    collection: &CollectionDecl,
    coord: TileCoord,
    cap: u64,
    filter: Option<&Filter>,
) -> Result<(String, Vec<SqlParam>)> {
    let table = quote_ident(collection.resolved_table())?;
    let geom = quote_ident(collection.resolved_geometry())?;

    let storage_srid = tile_storage_srid(collection);
    let tms = TileMatrixSet::WebMercatorQuad;
    let tile_env_geom = tile_envelope_in_storage_crs(tms, storage_srid);
    let source_geom = storage_geom_in_grid_crs(&format!("t.{geom}"), tms, storage_srid);

    // Same i32 range guard `build_mvt_plan` applies — see its own comment.
    let x = i32::try_from(coord.x).map_err(|_| PostgisError::TileCoordOutOfRange(coord.x))?;
    let y = i32::try_from(coord.y).map_err(|_| PostgisError::TileCoordOutOfRange(coord.y))?;

    let mut params = vec![
        SqlParam::Int4(i32::from(coord.z)),
        SqlParam::Int4(x),
        SqlParam::Int4(y),
        SqlParam::Bigint(i64::try_from(cap).unwrap_or(i64::MAX)),
    ];

    let filter_clause = match filter {
        Some(filter) => format!(
            " AND {}",
            compile_filter(filter, &mut params, FilterCrs::grant_only(collection.srid))?
        ),
        None => String::new(),
    };

    let sql = format!(
        "{TILE_ENV_CTE} SELECT ST_AsEWKB({source_geom}) FROM {table} AS t, tile_env WHERE t.{geom} && {tile_env_geom}{filter_clause} LIMIT $4"
    );

    Ok((sql, params))
}

/// A real (never-estimated) extent: `ST_Extent` scans every row, so this is
/// only the fallback for when `catalog::ESTIMATED_EXTENT_SQL` comes back
/// empty (a table with no `pg_statistic` row yet, e.g. never `ANALYZE`d).
/// The box is transformed to CRS84 (`ST_Transform(..., 4326)`) before its
/// bounds are extracted, so a non-4326 native SRID reprojects the actual
/// rectangle rather than relabeling its corner coordinates.
pub(crate) fn build_real_extent_plan(
    table: &str,
    geometry_column: &str,
    srid: i32,
) -> Result<(String, Vec<SqlParam>)> {
    let table_ident = quote_ident(table)?;
    let geom_ident = quote_ident(geometry_column)?;
    let sql = format!(
        "SELECT ST_XMin(t) AS minx, ST_YMin(t) AS miny, ST_XMax(t) AS maxx, ST_YMax(t) AS maxy \
         FROM (SELECT ST_Transform(ST_SetSRID(ST_Extent({geom_ident})::geometry, $1), 4326) AS t \
         FROM {table_ident}) sub"
    );
    Ok((sql, vec![SqlParam::Int4(srid)]))
}

/// `#101`: target sample size a [`sample_percentage`] aims for — large
/// enough for stable percentiles, small enough that even the cheapest
/// `TABLESAMPLE SYSTEM` pass stays a bounded, sub-second read regardless of
/// table size.
const GEOMETRY_PROFILE_TARGET_SAMPLE_ROWS: f64 = 2_000.0;

/// Floor on the sampling percentage once a row estimate is known: even a
/// huge table gets sampled at least this much, so `TABLESAMPLE`'s
/// block-level granularity still has a reasonable chance of returning rows
/// rather than nothing.
const GEOMETRY_PROFILE_MIN_SAMPLE_PCT: f64 = 1.0;

/// Sampling percentage used when the row estimate is unknown or zero (a
/// table that was never `ANALYZE`d) — deliberately small and fixed rather
/// than defaulting to 100%: design point 2 (`#101`) is explicit that exact
/// stats on a multi-million-row table at boot is unacceptable, and a
/// never-analyzed table is exactly the case where the size is least known.
/// A small table landing on an empty sample this way is an accepted,
/// self-describing outcome (`geometry_profile` reports `Ok(None)`, never a
/// profile of zeroes) rather than something this function tries to avoid by
/// guessing the table is small.
const GEOMETRY_PROFILE_FALLBACK_SAMPLE_PCT: f64 = 5.0;

/// The `TABLESAMPLE SYSTEM` percentage to request for a `row_estimate` rows
/// table, aiming for [`GEOMETRY_PROFILE_TARGET_SAMPLE_ROWS`] sampled rows —
/// see [`build_geometry_profile_plan`]'s own doc for how this is used.
/// Clamped so a small table (below the target row count) still samples
/// (close to) everything, and a huge one never drops below
/// [`GEOMETRY_PROFILE_MIN_SAMPLE_PCT`].
pub(crate) fn sample_percentage(row_estimate: Option<u64>) -> f64 {
    match row_estimate {
        Some(rows) if rows > 0 => ((GEOMETRY_PROFILE_TARGET_SAMPLE_ROWS / rows as f64) * 100.0)
            .clamp(GEOMETRY_PROFILE_MIN_SAMPLE_PCT, 100.0),
        _ => GEOMETRY_PROFILE_FALLBACK_SAMPLE_PCT,
    }
}

/// `#101`: the sampled geometry-profile aggregate query — `TABLESAMPLE
/// SYSTEM` performs cheap block-level sampling (never a full scan) at
/// `sample_pct` (see [`sample_percentage`]), then every aggregate below
/// reads only the sampled rows, so cost scales with sample size, not table
/// size. `geometry_type` picks the feature-size metric: `ST_Area` for a
/// polygon-typed collection, `ST_Length` for a line-typed one, `NULL` for a
/// point-typed or heterogeneous (`GEOMETRY`) one where neither concept
/// applies uniformly — decided once here in Rust rather than per row in SQL,
/// since the physical column's own reported type already answers it.
/// `mean_ring_count` enumerates every part of a multi-part feature — a
/// `LATERAL` join over `generate_series(1, ST_NumGeometries(geom))` summing
/// each part's own exterior-plus-interior ring count — rather than the
/// original `ST_GeometryN(geom, 1)` proxy, which only ever looked at the
/// first part and silently missed rings anywhere else in the feature. The
/// `LATERAL` join stays correlated to each already-sampled row, so the
/// enumeration cost still scales with sample size, not table size — the
/// same TABLESAMPLE'd row set every other aggregate here reads. Gated the
/// same way `size_metric` above is: `NULL` for a non-polygon or untyped/
/// mixed geometry column, where "ring count" has no uniform meaning either.
pub(crate) fn build_geometry_profile_plan(
    table: &str,
    geometry_column: &str,
    geometry_type: Option<&str>,
    sample_pct: f64,
) -> Result<(String, Vec<SqlParam>)> {
    let table_ident = quote_ident(table)?;
    let geom_ident = quote_ident(geometry_column)?;

    let size_metric = match geometry_type {
        Some(t) if t.contains("POLYGON") => "ST_Area(geom)".to_string(),
        Some(t) if t.contains("LINE") || t.contains("CURVE") => "ST_Length(geom)".to_string(),
        _ => "NULL::double precision".to_string(),
    };

    let (ring_join, ring_metric) = match geometry_type {
        Some(t) if t.contains("POLYGON") => (
            " CROSS JOIN LATERAL ( \
                 SELECT sum(COALESCE(ST_NumInteriorRings(ST_GeometryN(geom, part.n)), 0) + 1) AS total \
                 FROM generate_series(1, ST_NumGeometries(geom)) AS part(n) \
             ) rings"
                .to_string(),
            "rings.total::double precision".to_string(),
        ),
        _ => (String::new(), "NULL::double precision".to_string()),
    };

    let sql = format!(
        // `TABLESAMPLE SYSTEM` takes a `real` (`float4`) argument. A bare
        // `$1::real` would type the placeholder itself as `real`, which
        // this driver's one floating-point bind variant (`SqlParam::Float8`,
        // `double precision`) can't satisfy — so `$1` is cast to `double
        // precision` first (typing the placeholder the way every other
        // `Float8` bind in this driver already is), then down to `real` to
        // satisfy `TABLESAMPLE`'s own grammar.
        "WITH sample AS ( \
             SELECT {geom_ident} AS geom FROM {table_ident} \
             TABLESAMPLE SYSTEM ($1::double precision::real) \
             WHERE {geom_ident} IS NOT NULL \
         ), metrics AS ( \
             SELECT \
                 geom, \
                 ST_NPoints(geom) AS vertices, \
                 ST_NumGeometries(geom) > 1 AS is_multi, \
                 {ring_metric} AS ring_count, \
                 {size_metric} AS feature_size \
             FROM sample{ring_join} \
         ) \
         SELECT \
             count(*)::bigint AS sample_size, \
             avg(vertices)::double precision AS vertex_mean, \
             percentile_cont(0.5) WITHIN GROUP (ORDER BY vertices) AS vertex_median, \
             percentile_cont(0.95) WITHIN GROUP (ORDER BY vertices) AS vertex_p95, \
             max(vertices)::bigint AS vertex_max, \
             sum(vertices)::bigint AS vertex_sum, \
             avg(CASE WHEN is_multi THEN 1.0 ELSE 0.0 END)::double precision AS multi_part_fraction, \
             avg(ring_count)::double precision AS mean_ring_count, \
             percentile_cont(0.5) WITHIN GROUP (ORDER BY feature_size) AS size_p50, \
             percentile_cont(0.95) WITHIN GROUP (ORDER BY feature_size) AS size_p95, \
             max(feature_size) AS size_max, \
             ST_Area(ST_Extent(geom)) AS sample_bbox_area \
         FROM metrics"
    );
    Ok((sql, vec![SqlParam::Float8(sample_pct)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tellurion_core::{AttributeColumn, DatetimeRange, WktGeometry};

    fn collection() -> CollectionDecl {
        serde_yaml::from_str(
            r#"
id: demo
catalog: default
storage: main
table: demo
geometry: geom
pk: id
datetime: observed_at
"#,
        )
        .unwrap()
    }

    #[test]
    fn items_plan_with_no_filters() {
        let plan = build_items_plan(&collection(), &ItemsQuery::default()).unwrap();
        assert_eq!(
            plan.sql,
            "SELECT \"id\"::bigint AS pk_value, json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"demo\" AS t ORDER BY \"id\" ASC LIMIT $1"
        );
        assert!(matches!(plan.params.as_slice(), [SqlParam::Bigint(11)]));
        // `collection()` carries no cached `row_estimate`, so an unfiltered
        // query falls back to the live query — `CountPlan::Cached` is
        // covered separately below.
        let CountPlan::Query(count_sql, count_params) = plan.count else {
            panic!("unfiltered query with no cached estimate must fall back to CountPlan::Query");
        };
        // `GREATEST(reltuples, 0)` clamps the "never analyzed" -1 sentinel
        // so a freshly ingested collection reports 0 rather than losing the
        // estimate entirely to a negative-to-u64 conversion failure.
        assert!(count_sql.contains("GREATEST(reltuples, 0)"));
        assert!(matches!(count_params.as_slice(), [SqlParam::Text(t)] if t == "demo"));
    }

    #[test]
    fn budgeted_items_plan_counts_bounded_candidates_before_geojson_encoding() {
        let mut collection = collection();
        collection.settings.items_vertex_budget = Some(50_000);
        let plan = build_items_plan(&collection, &ItemsQuery::default()).unwrap();

        assert!(plan.sql.starts_with("WITH candidates AS ("));
        assert!(plan.sql.contains("LIMIT $1), counted AS ("));
        assert!(plan.sql.contains("ST_NPoints(source_geom)"));
        assert!(plan
            .sql
            .contains("sum(COALESCE(ST_NPoints(source_geom), 0)) OVER"));
        assert!(plan.sql.contains("page_position < $1"));
        assert!(plan.sql.contains("cumulative_vertices <= $2"));
        assert!(plan.sql.contains("THEN json_build_object("));
        // The candidate scan carries the bare row composite; every
        // per-column conversion happens behind the budget's `CASE`.
        assert!(plan.sql.contains("t AS source_row"));
        assert!(plan.sql.contains("'properties',to_jsonb(source_row)"));
        let candidate_scan = plan.sql.split("LIMIT $1").next().unwrap();
        assert!(
            !candidate_scan.contains("ST_AsGeoJSON"),
            "the bounded candidate scan must not encode geometry"
        );
        // `#1`: `to_jsonb` on the whole row renders the geometry column
        // through its own output function (hex WKB) purely so the adjacent
        // `- 'geom'` can discard it — measurably MORE expensive than the
        // `ST_AsGeoJSON` above on a page of high-vertex features. Hoisting
        // it back into the candidate scan would leave `items_vertex_budget`
        // bounding only part of the cost it exists to bound (`#148`).
        assert!(
            !candidate_scan.contains("to_jsonb"),
            "the bounded candidate scan must not render properties either: \
             to_jsonb serializes the geometry column only to drop it, so \
             running it before the budget is applied defeats the budget"
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Bigint(11), SqlParam::Bigint(50_000)]
        ));
    }

    /// `#1`: when the router has already resolved a `row_estimate` for this
    /// collection (the common case — see `CountPlan`'s own doc), an
    /// unfiltered items query must skip the live `pg_class` round trip
    /// entirely rather than issuing it every request.
    #[test]
    fn items_plan_with_no_filters_and_a_cached_row_estimate_skips_the_live_count_query() {
        let mut with_estimate = collection();
        with_estimate.row_estimate = Some(2_163_542);
        let plan = build_items_plan(&with_estimate, &ItemsQuery::default()).unwrap();
        assert_eq!(plan.count, CountPlan::Cached(2_163_542));
    }

    #[test]
    fn items_plan_with_token() {
        let query = ItemsQuery {
            token: Some("42".to_string()),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert_eq!(
            plan.sql,
            "SELECT \"id\"::bigint AS pk_value, json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"demo\" AS t WHERE \"id\" > $1::bigint ORDER BY \"id\" ASC LIMIT $2"
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Bigint(42), SqlParam::Bigint(11)]
        ));
        assert!(
            plan.count.is_some(),
            "token alone does not disable the estimate"
        );
    }

    #[test]
    fn items_plan_rejects_unparsable_token() {
        let query = ItemsQuery {
            token: Some("not-a-number".to_string()),
            ..ItemsQuery::default()
        };
        assert!(matches!(
            build_items_plan(&collection(), &query),
            Err(PostgisError::InvalidToken(_))
        ));
    }

    #[test]
    fn items_plan_with_bbox_disables_the_estimate() {
        let query = ItemsQuery {
            bbox: Some([1.0, 2.0, 3.0, 4.0]),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert_eq!(
            plan.sql,
            "SELECT \"id\"::bigint AS pk_value, json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"demo\" AS t WHERE \"geom\" && ST_MakeEnvelope($1, $2, $3, $4, 4326) ORDER BY \"id\" ASC LIMIT $5"
        );
        assert!(plan.count.is_none());
    }

    #[test]
    fn items_plan_with_open_datetime_start_only() {
        let query = ItemsQuery {
            datetime: Some(DatetimeRange {
                start: Some("2020-01-01T00:00:00Z".to_string()),
                end: None,
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert_eq!(
            plan.sql,
            "SELECT \"id\"::bigint AS pk_value, json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"demo\" AS t WHERE \"observed_at\" >= $1::text::timestamptz ORDER BY \"id\" ASC LIMIT $2"
        );
        assert!(plan.count.is_none());
    }

    #[test]
    fn items_plan_with_all_filters_orders_params_token_bbox_datetime_limit() {
        let query = ItemsQuery {
            limit: 5,
            token: Some("7".to_string()),
            bbox: Some([1.0, 2.0, 3.0, 4.0]),
            datetime: Some(DatetimeRange {
                start: Some("2020-01-01T00:00:00Z".to_string()),
                end: Some("2020-12-31T00:00:00Z".to_string()),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert_eq!(
            plan.sql,
            "SELECT \"id\"::bigint AS pk_value, json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"demo\" AS t WHERE \"id\" > $1::bigint AND \"geom\" && ST_MakeEnvelope($2, $3, $4, $5, 4326) AND \"observed_at\" >= $6::text::timestamptz AND \"observed_at\" <= $7::text::timestamptz ORDER BY \"id\" ASC LIMIT $8"
        );
        assert_eq!(plan.params.len(), 8);
    }

    #[test]
    fn items_plan_rejects_datetime_filter_without_a_configured_column() {
        let mut collection = collection();
        collection.datetime = None;
        let query = ItemsQuery {
            datetime: Some(DatetimeRange {
                start: Some("2020-01-01T00:00:00Z".to_string()),
                end: None,
            }),
            ..ItemsQuery::default()
        };
        assert!(matches!(
            build_items_plan(&collection, &query),
            Err(PostgisError::NoDatetimeColumn(_))
        ));
    }

    #[test]
    fn item_plan_by_pk() {
        let (sql, params) = build_item_plan(
            &collection(),
            PkValue::Integer(42),
            None,
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"demo\" AS t WHERE \"id\" = $1::bigint"
        );
        assert!(matches!(params.as_slice(), [SqlParam::Bigint(42)]));
    }

    #[test]
    fn budgeted_item_plan_counts_raw_geometry_before_conditional_encoding() {
        let mut collection = collection();
        collection.settings.items_vertex_budget = Some(50_000);
        let (sql, params) =
            build_item_plan(&collection, PkValue::Integer(42), None, RequestedCrs::Crs84).unwrap();

        assert!(sql.contains("COALESCE(ST_NPoints(\"geom\"), 0)::bigint"));
        assert!(sql.contains("CASE WHEN COALESCE(ST_NPoints(\"geom\"), 0)::bigint <= $2"));
        assert!(sql.contains("THEN json_build_object("));
        assert!(sql.contains("ST_AsGeoJSON(\"geom\")"));
        assert!(matches!(
            params.as_slice(),
            [SqlParam::Bigint(42), SqlParam::Bigint(50_000)]
        ));
    }

    /// `#34`: a grant filter AND-merges into the single-row `WHERE` clause,
    /// with its own placeholder numbered after the pk's `$1`.
    #[test]
    fn item_plan_by_pk_with_filter_ands_the_filter_clause() {
        let filter = tellurion_core::filter::parse_text("org = 'acme'").unwrap();
        let (sql, params) = build_item_plan(
            &collection(),
            PkValue::Integer(42),
            Some(&filter),
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"demo\" AS t WHERE \"id\" = $1::bigint AND (\"org\"::text = $2)"
        );
        assert!(matches!(
            params.as_slice(),
            [SqlParam::Bigint(42), SqlParam::Text(org)] if org == "acme"
        ));
    }

    /// `#87`: a `Uuid` id-type collection casts the pk column (and binds the
    /// parameter) as `uuid`, not `bigint` — ordering/equality over the pk's
    /// own real type, never its string form pretending to be a number.
    #[test]
    fn item_plan_by_pk_casts_uuid_for_a_uuid_id_type_collection() {
        let mut uuid_collection = collection();
        uuid_collection.id_type = IdType::Uuid;
        let id = uuid::Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let (sql, params) = build_item_plan(
            &uuid_collection,
            PkValue::Uuid(id),
            None,
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"demo\" AS t WHERE \"id\"::uuid = $1"
        );
        assert!(matches!(params.as_slice(), [SqlParam::Uuid(v)] if *v == id));
    }

    /// `#94`: a `Text` id-type collection casts the pk column (and binds the
    /// parameter) as `text`, not `bigint`. No `COLLATE "C"` here — equality
    /// doesn't need a collation pin the way keyset paging's ordering
    /// comparisons do (this module's own doc, `build_items_plan`).
    #[test]
    fn item_plan_by_pk_casts_text_for_a_text_id_type_collection() {
        let mut text_collection = collection();
        text_collection.id_type = IdType::Text;
        let (sql, params) = build_item_plan(
            &text_collection,
            PkValue::Text("acme-1".to_string()),
            None,
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"demo\" AS t WHERE \"id\"::text = $1"
        );
        assert!(matches!(params.as_slice(), [SqlParam::Text(v)] if v == "acme-1"));
    }

    /// `#87`: a keyset token on a `Uuid` id-type collection parses/casts/
    /// binds as `uuid`, mirroring `items_plan_with_token`'s `Integer` case.
    #[test]
    fn items_plan_with_a_uuid_token_casts_and_binds_uuid() {
        let mut uuid_collection = collection();
        uuid_collection.id_type = IdType::Uuid;
        let id = uuid::Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let query = ItemsQuery {
            token: Some(id.to_string()),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&uuid_collection, &query).unwrap();
        assert!(
            plan.sql.contains("\"id\"::uuid > $1")
                && plan.sql.contains("ORDER BY \"id\"::uuid ASC"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Uuid(v), SqlParam::Bigint(11)] if *v == id
        ));
    }

    #[test]
    fn items_plan_rejects_a_token_that_is_not_a_valid_uuid_for_a_uuid_id_type_collection() {
        let mut uuid_collection = collection();
        uuid_collection.id_type = IdType::Uuid;
        let query = ItemsQuery {
            token: Some("not-a-uuid".to_string()),
            ..ItemsQuery::default()
        };
        assert!(matches!(
            build_items_plan(&uuid_collection, &query),
            Err(PostgisError::InvalidToken(_))
        ));
    }

    /// `#94`: keyset paging over a `Text` pk pins an explicit `COLLATE "C"`
    /// on both the `WHERE`/`ORDER BY` comparisons — this module's own doc
    /// explains why (a plain `text` comparison's default collation is
    /// deployment-dependent, `COLLATE "C"` is not). `Integer`/`Uuid` never
    /// get this suffix; `items_plan_with_token`/`items_plan_with_a_uuid_
    /// token_casts_and_binds_uuid` above prove their SQL is unchanged.
    #[test]
    fn items_plan_with_a_text_token_pins_collate_c_on_where_and_order_by() {
        let mut text_collection = collection();
        text_collection.id_type = IdType::Text;
        let query = ItemsQuery {
            token: Some("Alpha".to_string()),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&text_collection, &query).unwrap();
        assert!(
            plan.sql.contains("\"id\"::text COLLATE \"C\" > $1")
                && plan.sql.contains("ORDER BY \"id\"::text COLLATE \"C\" ASC"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Text(v), SqlParam::Bigint(11)] if v == "Alpha"
        ));
    }

    /// `#94`: a keyset token on a `Text` id-type collection never fails to
    /// parse — any string is a legal caller-supplied text id — unlike
    /// `Uuid`'s `items_plan_rejects_a_token_that_is_not_a_valid_uuid_for_a_
    /// uuid_id_type_collection` above.
    #[test]
    fn items_plan_accepts_any_string_as_a_text_token() {
        let mut text_collection = collection();
        text_collection.id_type = IdType::Text;
        let query = ItemsQuery {
            token: Some("not-a-number-or-a-uuid".to_string()),
            ..ItemsQuery::default()
        };
        assert!(build_items_plan(&text_collection, &query).is_ok());
    }

    #[test]
    fn mvt_plan_shape() {
        let coord = TileCoord { z: 10, x: 3, y: 5 };
        let (sql, params) = build_mvt_plan(
            &collection(),
            TileMatrixSet::WebMercatorQuad,
            coord,
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom) SELECT ST_AsMVT(mvt, 'demo', 4096, 'geom') FROM (SELECT \"id\"::text AS id, ST_AsMVTGeom(ST_SimplifyPreserveTopology(ST_Transform(t.\"geom\", 3857), $4), tile_env.geom, 4096, $5, true) AS geom FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && ST_Transform(tile_env.geom, 4326) LIMIT $6) AS mvt"
        );
        assert!(matches!(
            params.as_slice(),
            [
                SqlParam::Int4(10),
                SqlParam::Int4(3),
                SqlParam::Int4(5),
                SqlParam::Float8(v),
                SqlParam::Int4(256),
                SqlParam::Bigint(2000),
            ] if (*v - 9.5).abs() < f64::EPSILON
        ));
    }

    /// `#190`: the WorldCRS84Quad plan binds the CRS84-degrees envelope
    /// computed by `tellurion_core::world_crs84_tile_bounds_deg` (here z2
    /// tile (3, 1): `[-45, 0, 0, 45]`), simplifies/clips in the storage's
    /// own 4326 degrees with NO mercator transform anywhere, and shifts the
    /// tolerance/buffer/cap placeholders past the four envelope corners —
    /// while the mercator golden strings above stay byte-for-byte
    /// unchanged.
    #[test]
    fn mvt_plan_shape_for_world_crs84_quad() {
        let coord = TileCoord { z: 2, x: 3, y: 1 };
        let (sql, params) = build_mvt_plan(
            &collection(),
            TileMatrixSet::WorldCrs84Quad,
            coord,
            0.001,
            256,
            2000,
            None,
        )
        .unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_MakeEnvelope($1, $2, $3, $4, 4326) AS geom) SELECT ST_AsMVT(mvt, 'demo', 4096, 'geom') FROM (SELECT \"id\"::text AS id, ST_AsMVTGeom(ST_SimplifyPreserveTopology(t.\"geom\", $5), tile_env.geom, 4096, $6, true) AS geom FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && tile_env.geom LIMIT $7) AS mvt"
        );
        assert!(matches!(
            params.as_slice(),
            [
                SqlParam::Float8(minlon),
                SqlParam::Float8(minlat),
                SqlParam::Float8(maxlon),
                SqlParam::Float8(maxlat),
                SqlParam::Float8(tol),
                SqlParam::Int4(256),
                SqlParam::Bigint(2000),
            ] if *minlon == -45.0 && *minlat == 0.0 && *maxlon == 0.0 && *maxlat == 45.0
                && (*tol - 0.001).abs() < f64::EPSILON
        ));
    }

    /// `#190`: a `#34` grant filter on the CRS84 grid numbers its
    /// placeholders after the SEVEN fixed params (the mercator counterpart
    /// above proves `$7`) — the fragment's params vector, not a hardcoded
    /// index, is what feeds `compile_filter`.
    #[test]
    fn world_crs84_mvt_plan_numbers_filter_params_after_the_seven_fixed_ones() {
        let coord = TileCoord { z: 2, x: 3, y: 1 };
        let filter = tellurion_core::filter::parse_text("org = 'acme'").unwrap();
        let (sql, params) = build_mvt_plan(
            &collection(),
            TileMatrixSet::WorldCrs84Quad,
            coord,
            0.001,
            256,
            2000,
            Some(&filter),
        )
        .unwrap();
        assert!(
            sql.contains("AND (\"org\"::text = $8) LIMIT $7"),
            "sql was: {sql}"
        );
        assert_eq!(params.len(), 8);
        assert!(matches!(&params[7], SqlParam::Text(org) if org == "acme"));
    }

    /// **Rule 1 for `#262`, checked character for character.** A
    /// CRS84-equivalent storage — SRID 4326, or none reported — compiles to
    /// exactly the two golden strings above, on both grids, with not one
    /// extra `ST_Transform` anywhere. That is every collection every live
    /// Render demo serves, and `scripts/italy-contract-smoke.sh` cannot
    /// catch a regression in it from the outside: its one collection is
    /// 4326, so a projected-storage bug and a 4326 regression would look
    /// identical to it (both "the tile is empty"), and only a pin at this
    /// level distinguishes them.
    ///
    /// The no-SRID case is asserted beside the 4326 one deliberately: the
    /// two travel through different arms of `tile_storage_srid`, and a
    /// deployment whose backend reports no SRID is exactly the one with no
    /// way to notice a change of default.
    #[test]
    fn mvt_plan_on_a_crs84_storage_is_byte_for_byte_unchanged_on_both_grids() {
        let mut explicit = collection();
        explicit.srid = Some(4326);
        let unknown = collection();
        assert_eq!(
            unknown.srid, None,
            "the fixture's SRID is genuinely unknown"
        );

        let mercator = TileCoord { z: 10, x: 3, y: 5 };
        let mercator_golden = "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom) SELECT ST_AsMVT(mvt, 'demo', 4096, 'geom') FROM (SELECT \"id\"::text AS id, ST_AsMVTGeom(ST_SimplifyPreserveTopology(ST_Transform(t.\"geom\", 3857), $4), tile_env.geom, 4096, $5, true) AS geom FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && ST_Transform(tile_env.geom, 4326) LIMIT $6) AS mvt";
        for decl in [&explicit, &unknown] {
            let (sql, _) = build_mvt_plan(
                decl,
                TileMatrixSet::WebMercatorQuad,
                mercator,
                9.5,
                256,
                2000,
                None,
            )
            .unwrap();
            assert_eq!(sql, mercator_golden, "srid {:?}", decl.srid);
        }

        let crs84 = TileCoord { z: 2, x: 3, y: 1 };
        let crs84_golden = "WITH tile_env AS (SELECT ST_MakeEnvelope($1, $2, $3, $4, 4326) AS geom) SELECT ST_AsMVT(mvt, 'demo', 4096, 'geom') FROM (SELECT \"id\"::text AS id, ST_AsMVTGeom(ST_SimplifyPreserveTopology(t.\"geom\", $5), tile_env.geom, 4096, $6, true) AS geom FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && tile_env.geom LIMIT $7) AS mvt";
        for decl in [&explicit, &unknown] {
            let (sql, _) = build_mvt_plan(
                decl,
                TileMatrixSet::WorldCrs84Quad,
                crs84,
                0.001,
                256,
                2000,
                None,
            )
            .unwrap();
            assert_eq!(sql, crs84_golden, "srid {:?}", decl.srid);
            assert!(
                !sql.contains("ST_Transform"),
                "the CRS84 grid over a CRS84 storage transforms nothing at all: {sql}"
            );
        }
    }

    /// `#262` on the mercator grid: EPSG:3857 storage IS the grid's own CRS,
    /// so the honest SQL carries no transform on either side — where before
    /// this issue it carried `ST_Transform(tile_env.geom, 4326)`, comparing
    /// the column's metres against a degrees box that can never contain
    /// them, and matching nothing.
    ///
    /// Both sides are asserted, not just the predicate: `ST_AsMVTGeom` clips
    /// against `tile_env.geom` in the grid's CRS and `$4` is a tolerance in
    /// the grid's units, so a fix that transformed only the envelope would
    /// select the right rows and then still be re-projecting geometry that
    /// was already in the right CRS.
    #[test]
    fn mvt_plan_needs_no_transform_when_the_storage_is_the_mercator_grids_own_crs() {
        let mut projected = collection();
        projected.srid = Some(3857);
        let (sql, _) = build_mvt_plan(
            &projected,
            TileMatrixSet::WebMercatorQuad,
            TileCoord { z: 10, x: 3, y: 5 },
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom) SELECT ST_AsMVT(mvt, 'demo', 4096, 'geom') FROM (SELECT \"id\"::text AS id, ST_AsMVTGeom(ST_SimplifyPreserveTopology(t.\"geom\", $4), tile_env.geom, 4096, $5, true) AS geom FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && tile_env.geom LIMIT $6) AS mvt"
        );
    }

    /// `#262` is not a 3857 special case. An arbitrary projected storage —
    /// EPSG:32633, UTM zone 33N, the CRS an Italian deployment would
    /// actually store in — transforms the tile envelope INTO it for the
    /// prune, and transforms the geometry OUT into the grid's CRS for the
    /// clip. Two transforms, in opposite directions, each on the cheap side
    /// of its own comparison.
    #[test]
    fn mvt_plan_transforms_both_sides_for_an_arbitrary_projected_storage() {
        let mut projected = collection();
        projected.srid = Some(32633);
        let (sql, _) = build_mvt_plan(
            &projected,
            TileMatrixSet::WebMercatorQuad,
            TileCoord { z: 10, x: 3, y: 5 },
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();
        assert!(
            sql.contains("ST_SimplifyPreserveTopology(ST_Transform(t.\"geom\", 3857), $4)"),
            "the geometry travels into the grid's CRS: {sql}"
        );
        assert!(
            sql.contains("WHERE t.\"geom\" && ST_Transform(tile_env.geom, 32633)"),
            "and the envelope travels into the storage's: {sql}"
        );
    }

    /// `#262` on the WorldCRS84Quad grid, which `#190` left assuming 4326
    /// storage on BOTH sides — a bare `t.<geom>` handed to `ST_AsMVTGeom`
    /// and a bare `tile_env.geom` in the predicate. Against a 3857 column
    /// both are wrong, and in opposite directions from the mercator arm:
    /// here it is the *envelope* that must travel to 3857 and the
    /// *geometry* that must travel to 4326.
    #[test]
    fn world_crs84_mvt_plan_transforms_both_sides_on_a_projected_storage() {
        let mut projected = collection();
        projected.srid = Some(3857);
        let (sql, _) = build_mvt_plan(
            &projected,
            TileMatrixSet::WorldCrs84Quad,
            TileCoord { z: 2, x: 3, y: 1 },
            0.001,
            256,
            2000,
            None,
        )
        .unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_MakeEnvelope($1, $2, $3, $4, 4326) AS geom) SELECT ST_AsMVT(mvt, 'demo', 4096, 'geom') FROM (SELECT \"id\"::text AS id, ST_AsMVTGeom(ST_SimplifyPreserveTopology(ST_Transform(t.\"geom\", 4326), $5), tile_env.geom, 4096, $6, true) AS geom FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && ST_Transform(tile_env.geom, 3857) LIMIT $7) AS mvt"
        );
    }

    /// `#104`: `build_mvt_plan` embeds whichever column
    /// `resolved_geometry_for_zoom` selects for the tile's own zoom — the
    /// declared `geom_z6` variant at a zoom it covers, the base `geom`
    /// column outside that range — proving the whole `build_mvt_candidate_
    /// fragment`-sharing family (`build_mvt_plan`/`build_mvt_vertex_total_
    /// plan`/`build_mvt_budgeted_plan`) picks up the selection through that
    /// one shared fragment.
    #[test]
    fn mvt_plan_selects_the_zoom_scoped_geometry_variant_and_falls_back_to_the_base_column() {
        let mut variant_collection = collection();
        variant_collection.geometry_variants = vec![tellurion_core::GeometryVariantDecl {
            column: "geom_z6".to_string(),
            minzoom: 0,
            maxzoom: 6,
        }];

        let (in_range_sql, _) = build_mvt_plan(
            &variant_collection,
            TileMatrixSet::WebMercatorQuad,
            TileCoord { z: 3, x: 0, y: 0 },
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();
        assert!(
            in_range_sql.contains("\"geom_z6\""),
            "zoom 3 falls inside the variant's range: {in_range_sql}"
        );
        assert!(!in_range_sql.contains("t.\"geom\""));

        let (out_of_range_sql, _) = build_mvt_plan(
            &variant_collection,
            TileMatrixSet::WebMercatorQuad,
            TileCoord { z: 10, x: 3, y: 5 },
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();
        assert!(
            out_of_range_sql.contains("t.\"geom\""),
            "zoom 10 falls outside the variant's range, so the base column applies: {out_of_range_sql}"
        );
        assert!(!out_of_range_sql.contains("geom_z6"));
    }

    /// `#34`: a grant filter AND-merges into the MVT subquery's own `WHERE`
    /// clause, with its own placeholder numbered `$7` — after the six fixed
    /// params, even though it appears in the SQL text before the `$6` LIMIT.
    #[test]
    fn mvt_plan_with_filter_ands_the_filter_clause_and_numbers_it_after_the_fixed_params() {
        let coord = TileCoord { z: 10, x: 3, y: 5 };
        let filter = tellurion_core::filter::parse_text("org = 'acme'").unwrap();
        let (sql, params) = build_mvt_plan(
            &collection(),
            TileMatrixSet::WebMercatorQuad,
            coord,
            9.5,
            256,
            2000,
            Some(&filter),
        )
        .unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom) SELECT ST_AsMVT(mvt, 'demo', 4096, 'geom') FROM (SELECT \"id\"::text AS id, ST_AsMVTGeom(ST_SimplifyPreserveTopology(ST_Transform(t.\"geom\", 3857), $4), tile_env.geom, 4096, $5, true) AS geom FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && ST_Transform(tile_env.geom, 4326) AND (\"org\"::text = $7) LIMIT $6) AS mvt"
        );
        assert_eq!(params.len(), 7);
        assert!(matches!(&params[6], SqlParam::Text(org) if org == "acme"));
    }

    /// `#90`: the vertex-total probe selects from the exact same candidate
    /// shape `mvt_plan_shape` above proves for `build_mvt_plan` — same six
    /// params, same envelope/simplify/clip/`LIMIT $6` — just aggregated to a
    /// single `SUM(ST_NPoints(geom))` instead of built into MVT bytes.
    #[test]
    fn mvt_vertex_total_plan_shape() {
        let coord = TileCoord { z: 10, x: 3, y: 5 };
        let (sql, params) = build_mvt_vertex_total_plan(
            &collection(),
            TileMatrixSet::WebMercatorQuad,
            coord,
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom) SELECT COALESCE(SUM(ST_NPoints(geom)), 0)::bigint FROM (SELECT \"id\"::text AS id, ST_AsMVTGeom(ST_SimplifyPreserveTopology(ST_Transform(t.\"geom\", 3857), $4), tile_env.geom, 4096, $5, true) AS geom FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && ST_Transform(tile_env.geom, 4326) LIMIT $6) AS budget_probe"
        );
        assert!(matches!(
            params.as_slice(),
            [
                SqlParam::Int4(10),
                SqlParam::Int4(3),
                SqlParam::Int4(5),
                SqlParam::Float8(v),
                SqlParam::Int4(256),
                SqlParam::Bigint(2000),
            ] if (*v - 9.5).abs() < f64::EPSILON
        ));
    }

    /// `#90`: the truncating plan keeps only the prefix of candidate rows
    /// whose cumulative `ST_NPoints` fits `vertex_budget`, numbered `$7`
    /// after the six fixed params (the same numbering convention a filter's
    /// own placeholder uses — see the filter test above).
    #[test]
    fn mvt_budgeted_plan_shape() {
        let coord = TileCoord { z: 10, x: 3, y: 5 };
        let (sql, params) = build_mvt_budgeted_plan(
            &collection(),
            TileMatrixSet::WebMercatorQuad,
            coord,
            9.5,
            256,
            2000,
            750,
            None,
        )
        .unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom), candidate AS (SELECT \"id\"::text AS id, ST_AsMVTGeom(ST_SimplifyPreserveTopology(ST_Transform(t.\"geom\", 3857), $4), tile_env.geom, 4096, $5, true) AS geom FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && ST_Transform(tile_env.geom, 4326) LIMIT $6), budgeted AS (SELECT *, SUM(ST_NPoints(geom)) OVER (ORDER BY id) AS running_vertices FROM candidate) SELECT ST_AsMVT(mvt, 'demo', 4096, 'geom') FROM (SELECT id, geom FROM budgeted WHERE running_vertices <= $7) AS mvt"
        );
        assert_eq!(params.len(), 7);
        assert!(matches!(&params[6], SqlParam::Bigint(750)));
    }

    /// The budgeted plan's final projection must name `id`/`geom`/each
    /// allowlisted property explicitly, never `SELECT *` — `budgeted` also
    /// carries the internal `running_vertices` accounting column, which must
    /// never leak into a feature's attribute table as a bogus property.
    #[test]
    fn mvt_budgeted_plan_widens_the_final_projection_with_allowlisted_properties_and_omits_running_vertices(
    ) {
        let mut collection = collection();
        collection.tile_properties = vec!["name".to_string(), "pop".to_string()];
        let coord = TileCoord { z: 10, x: 3, y: 5 };
        let (sql, _params) = build_mvt_budgeted_plan(
            &collection,
            TileMatrixSet::WebMercatorQuad,
            coord,
            9.5,
            256,
            2000,
            750,
            None,
        )
        .unwrap();
        assert!(
            sql.ends_with(
                "SELECT id, geom, \"name\", \"pop\" FROM budgeted WHERE running_vertices <= $7) AS mvt"
            ),
            "sql was: {sql}"
        );
    }

    /// `#85`: an allowlisted `tile_properties` set widens the MVT subquery's
    /// SELECT list with one whitelist-quoted `t."column"` per entry, each
    /// selected straight off `t` (no casting — PostGIS's own `ST_AsMVT`
    /// aggregate types each attribute from the column's real SQL type, so
    /// string/number/bool all round-trip verbatim with no Rust-side
    /// involvement).
    #[test]
    fn mvt_plan_widens_the_select_list_with_the_allowlisted_properties() {
        let mut collection = collection();
        collection.tile_properties = vec!["name".to_string(), "pop".to_string()];
        let coord = TileCoord { z: 10, x: 3, y: 5 };
        let (sql, _params) = build_mvt_plan(
            &collection,
            TileMatrixSet::WebMercatorQuad,
            coord,
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom) SELECT ST_AsMVT(mvt, 'demo', 4096, 'geom') FROM (SELECT \"id\"::text AS id, ST_AsMVTGeom(ST_SimplifyPreserveTopology(ST_Transform(t.\"geom\", 3857), $4), tile_env.geom, 4096, $5, true) AS geom, t.\"name\", t.\"pop\" FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && ST_Transform(tile_env.geom, 4326) LIMIT $6) AS mvt"
        );
    }

    /// No-regression guard (`#85`): a collection with no `tile_properties`
    /// declared anywhere in the settings chain produces byte-for-byte the
    /// pre-`#85` SQL — `mvt_plan_shape` above already proves this for the
    /// default `collection()` fixture; this proves it's genuinely the empty
    /// allowlist driving that, not some other default.
    #[test]
    fn mvt_plan_pk_only_by_default_when_tile_properties_is_empty() {
        let collection = collection();
        assert!(collection.tile_properties.is_empty());
        let coord = TileCoord { z: 10, x: 3, y: 5 };
        let (sql, _params) = build_mvt_plan(
            &collection,
            TileMatrixSet::WebMercatorQuad,
            coord,
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();
        assert!(
            !sql.contains(", t.\""),
            "no property columns should be selected: {sql}"
        );
    }

    /// `#87`: the tile lane needs zero special-casing for a non-integer pk —
    /// `build_mvt_plan` never sets `ST_AsMVT`'s native (unsigned-integer)
    /// feature id, always exposing the pk as an ordinary `::text` attribute
    /// instead (`build_mvt_plan`'s own doc), so a `Uuid` collection produces
    /// byte-for-byte identical SQL to an `Integer` one — the honest,
    /// documented "omit the native id" behavior applies uniformly, never a
    /// lossy cast attempted and caught.
    #[test]
    fn mvt_plan_is_identical_regardless_of_id_type() {
        let coord = TileCoord { z: 10, x: 3, y: 5 };
        let integer_plan = build_mvt_plan(
            &collection(),
            TileMatrixSet::WebMercatorQuad,
            coord,
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();

        let mut uuid_collection = collection();
        uuid_collection.id_type = IdType::Uuid;
        let uuid_plan = build_mvt_plan(
            &uuid_collection,
            TileMatrixSet::WebMercatorQuad,
            coord,
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();

        assert_eq!(integer_plan.0, uuid_plan.0, "sql must be identical");

        // `#94`: `Text` rides the same `::text` cast the fragment already
        // uses unconditionally — no special-casing needed there either.
        let mut text_collection = collection();
        text_collection.id_type = IdType::Text;
        let text_plan = build_mvt_plan(
            &text_collection,
            TileMatrixSet::WebMercatorQuad,
            coord,
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();
        assert_eq!(integer_plan.0, text_plan.0, "sql must be identical");
    }

    /// `#49`: a collection whose `external_id` differs from its internal
    /// `id` must embed the EXTERNAL id as the MVT layer name — the internal
    /// id is never derivable by a client from anything the API exposes, so
    /// embedding it would make styling this collection's tiles impossible.
    #[test]
    fn mvt_plan_embeds_the_external_id_not_the_internal_id_as_the_layer_name() {
        let collection: CollectionDecl = serde_yaml::from_str(
            r#"
id: internal-only-name
external_id: public-demo
catalog: default
storage: main
table: demo
geometry: geom
pk: id
"#,
        )
        .unwrap();
        let coord = TileCoord { z: 10, x: 3, y: 5 };
        let (sql, _params) = build_mvt_plan(
            &collection,
            TileMatrixSet::WebMercatorQuad,
            coord,
            9.5,
            256,
            2000,
            None,
        )
        .unwrap();
        assert!(
            sql.contains("ST_AsMVT(mvt, 'public-demo', "),
            "sql was: {sql}"
        );
        assert!(
            !sql.contains("internal-only-name"),
            "the internal id must never appear in the generated SQL: {sql}"
        );
    }

    #[test]
    fn mvt_plan_rejects_coordinates_beyond_i32_range() {
        let coord = TileCoord {
            z: 24,
            x: u32::MAX,
            y: 0,
        };
        assert!(matches!(
            build_mvt_plan(
                &collection(),
                TileMatrixSet::WebMercatorQuad,
                coord,
                1.0,
                256,
                100,
                None
            ),
            Err(PostgisError::TileCoordOutOfRange(_))
        ));
    }

    #[test]
    fn volume_plan_shape() {
        let coord = TileCoord { z: 4, x: 2, y: 3 };
        let (sql, params) = build_volume_plan(&collection(), coord, 500, None).unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom) SELECT ST_AsEWKB(ST_Transform(t.\"geom\", 3857)) FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && ST_Transform(tile_env.geom, 4326) LIMIT $4"
        );
        assert!(matches!(
            params.as_slice(),
            [
                SqlParam::Int4(4),
                SqlParam::Int4(2),
                SqlParam::Int4(3),
                SqlParam::Bigint(500),
            ]
        ));
    }

    /// `#70`: a grant filter AND-merges into the volume query's own `WHERE`
    /// clause, with its own placeholder numbered `$5` — after the four fixed
    /// params, even though it appears in the SQL text before the `$4` LIMIT.
    #[test]
    fn volume_plan_with_filter_ands_the_filter_clause_and_numbers_it_after_the_fixed_params() {
        let coord = TileCoord { z: 4, x: 2, y: 3 };
        let filter = tellurion_core::filter::parse_text("org = 'acme'").unwrap();
        let (sql, params) = build_volume_plan(&collection(), coord, 500, Some(&filter)).unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom) SELECT ST_AsEWKB(ST_Transform(t.\"geom\", 3857)) FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && ST_Transform(tile_env.geom, 4326) AND (\"org\"::text = $5) LIMIT $4"
        );
        assert_eq!(params.len(), 5);
        assert!(matches!(&params[4], SqlParam::Text(org) if org == "acme"));
    }

    /// `#262`, the volume lane's own share: a `#41` solid collection stored
    /// in a projected CRS was pruned by the identical degrees-vs-metres
    /// predicate the MVT fragment carried, and returned an empty 3D tile for
    /// the identical reason. Both sides move, and a CRS84-equivalent storage
    /// (4326, or none reported) is pinned character-for-character against
    /// `volume_plan_shape`'s own golden string above.
    #[test]
    fn volume_plan_compares_the_tile_envelope_in_the_storage_crs() {
        let coord = TileCoord { z: 4, x: 2, y: 3 };
        let crs84_golden = "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom) SELECT ST_AsEWKB(ST_Transform(t.\"geom\", 3857)) FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && ST_Transform(tile_env.geom, 4326) LIMIT $4";

        let mut explicit = collection();
        explicit.srid = Some(4326);
        let unknown = collection();
        for decl in [&explicit, &unknown] {
            let (sql, _) = build_volume_plan(decl, coord, 500, None).unwrap();
            assert_eq!(sql, crs84_golden, "srid {:?}", decl.srid);
        }

        let mut projected = collection();
        projected.srid = Some(3857);
        let (sql, _) = build_volume_plan(&projected, coord, 500, None).unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom) SELECT ST_AsEWKB(t.\"geom\") FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && tile_env.geom LIMIT $4"
        );

        let mut utm = collection();
        utm.srid = Some(32633);
        let (sql, _) = build_volume_plan(&utm, coord, 500, None).unwrap();
        assert_eq!(
            sql,
            "WITH tile_env AS (SELECT ST_TileEnvelope($1, $2, $3) AS geom) SELECT ST_AsEWKB(ST_Transform(t.\"geom\", 3857)) FROM \"demo\" AS t, tile_env WHERE t.\"geom\" && ST_Transform(tile_env.geom, 32633) LIMIT $4"
        );
    }

    #[test]
    fn volume_plan_rejects_coordinates_beyond_i32_range() {
        let coord = TileCoord {
            z: 24,
            x: u32::MAX,
            y: 0,
        };
        assert!(matches!(
            build_volume_plan(&collection(), coord, 100, None),
            Err(PostgisError::TileCoordOutOfRange(_))
        ));
    }

    #[test]
    fn invalid_table_identifier_is_rejected() {
        let mut collection = collection();
        collection.table = Some("demo; DROP TABLE x; --".to_string());
        assert!(matches!(
            build_items_plan(&collection, &ItemsQuery::default()),
            Err(PostgisError::InvalidIdentifier(_))
        ));
    }

    #[test]
    fn real_extent_plan_shape() {
        let (sql, params) = build_real_extent_plan("demo", "geom", 4326).unwrap();
        assert_eq!(
            sql,
            "SELECT ST_XMin(t) AS minx, ST_YMin(t) AS miny, ST_XMax(t) AS maxx, ST_YMax(t) AS maxy FROM (SELECT ST_Transform(ST_SetSRID(ST_Extent(\"geom\")::geometry, $1), 4326) AS t FROM \"demo\") sub"
        );
        assert!(matches!(params.as_slice(), [SqlParam::Int4(4326)]));
    }

    #[test]
    fn real_extent_plan_rejects_an_invalid_table_identifier() {
        assert!(matches!(
            build_real_extent_plan("demo; DROP TABLE x; --", "geom", 4326),
            Err(PostgisError::InvalidIdentifier(_))
        ));
    }

    // -- filter compilation (`#33`) ------------------------------------------

    #[test]
    fn items_plan_with_a_text_equality_filter() {
        let query = ItemsQuery {
            filter: Some(Filter::Compare {
                property: "name".to_string(),
                op: CompareOp::Eq,
                value: Literal::Text("a".to_string()),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql.contains("WHERE (\"name\"::text = $1)"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Text(v), SqlParam::Bigint(11)] if v == "a"
        ));
        assert!(
            plan.count.is_none(),
            "a filtered query must never report a cheap unfiltered estimate"
        );
    }

    #[test]
    fn items_plan_with_a_numeric_comparison_filter_casts_to_double_precision() {
        let query = ItemsQuery {
            filter: Some(Filter::Compare {
                property: "population".to_string(),
                op: CompareOp::Gt,
                value: Literal::Number(100.0),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql
                .contains("WHERE (\"population\"::double precision > $1)"),
            "sql was: {}",
            plan.sql
        );
        assert!(
            matches!(plan.params.as_slice(), [SqlParam::Float8(v), ..] if (*v - 100.0).abs() < f64::EPSILON)
        );
    }

    #[test]
    fn items_plan_with_a_boolean_filter_casts_to_boolean() {
        let query = ItemsQuery {
            filter: Some(Filter::Compare {
                property: "active".to_string(),
                op: CompareOp::Eq,
                value: Literal::Bool(true),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql.contains("WHERE (\"active\"::boolean = $1)"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(plan.params.as_slice(), [SqlParam::Bool(true), ..]));
    }

    #[test]
    fn items_plan_with_is_null_filter() {
        let query = ItemsQuery {
            filter: Some(Filter::IsNull {
                property: "name".to_string(),
                negated: true,
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql.contains("WHERE (\"name\" IS NOT NULL)"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(plan.params.as_slice(), [SqlParam::Bigint(11)]));
    }

    #[test]
    fn items_plan_with_and_or_not_filter() {
        let query = ItemsQuery {
            filter: Some(Filter::And(vec![
                Filter::Compare {
                    property: "population".to_string(),
                    op: CompareOp::Gt,
                    value: Literal::Number(0.0),
                },
                Filter::Not(Box::new(Filter::IsNull {
                    property: "name".to_string(),
                    negated: false,
                })),
            ])),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql.contains(
                "WHERE ((\"population\"::double precision > $1) AND (NOT (\"name\" IS NULL)))"
            ),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn items_plan_with_s_intersects_bbox_filter() {
        let query = ItemsQuery {
            filter: Some(Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql
                .contains("WHERE ST_Intersects(\"geom\", ST_MakeEnvelope($1, $2, $3, $4, 4326))"),
            "sql was: {}",
            plan.sql
        );
        assert_eq!(plan.params.len(), 5); // 4 bbox coords + limit
    }

    // -- `filter-crs` (Part 3 Filtering, 19-079r2 Req 7/Req 8, `#217`) ------

    /// **The assertion `#247` replaced, and why the pin was wrong.**
    ///
    /// This slot used to hold
    /// `items_plan_with_no_filter_crs_is_byte_for_byte_unchanged_even_with_a_non_4326_srid`,
    /// which pinned the `Omitted` arm to an untransformed CRS84 literal
    /// *whatever* the storage SRID, in the name of campaign rule 1. That pin
    /// was protecting an error page. Against a 3857 column the SQL it pinned
    /// is a 4326 envelope beside a 3857 geometry, and PostGIS refuses the
    /// comparison outright — "ST_Intersects: Operation on mixed SRID
    /// geometries" — so the "unchanged behaviour" it defended was a `500` for
    /// an ordinary conformant `filter=S_INTERSECTS(geom, BBOX(...))` carrying
    /// no `filter-crs` at all. Requirement 7 (`/req/filter/filter-crs-wgs84`)
    /// says such a request's geometries SHALL be *processed* in CRS84;
    /// erroring is not processing them.
    ///
    /// So the pin is replaced by its positive counterpart: with no
    /// `filter-crs`, a spatial literal is built at CRS84 and then genuinely
    /// reprojected into a projected storage CRS — the same SQL an explicit
    /// `filter-crs=CRS84` already compiled to, because the two say the same
    /// thing about the same numbers.
    ///
    /// Rule 1 keeps the whole of its force one test down
    /// (`items_plan_with_no_filter_crs_is_byte_for_byte_unchanged_on_a_4326_srid`),
    /// which is where every deployment that exists today actually lives.
    #[test]
    fn items_plan_with_no_filter_crs_transforms_the_literal_into_a_non_4326_storage_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let query = ItemsQuery {
            filter: Some(Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql.contains(
                "WHERE ST_Intersects(\"geom\", ST_Transform(ST_MakeEnvelope($1, $2, $3, $4, 4326), 3857))"
            ),
            "sql was: {}",
            plan.sql
        );
    }

    /// **Campaign rule 1, where it still bites: the CRS84 storage.**
    ///
    /// `#247`'s authorised exception covers exactly one case — a projected
    /// storage, where the bytes it changes are an error page. A collection
    /// stored at CRS84 has real, working behaviour to preserve, and every live
    /// Render demo is one. The transform is therefore conditional on the
    /// storage SRID, exactly as the `Crs84` arm's own `match` always was, and
    /// this pins that: with no `filter-crs` and a 4326 storage the compiled
    /// SQL is character-for-character the pre-`#217` statement, with no
    /// `ST_Transform` anywhere in it.
    ///
    /// Deliberately a whole-statement equality plus an absence assertion
    /// rather than a `contains` on the predicate: a `contains` would still
    /// pass if a transform leaked into the `SELECT` list. The Italy contract
    /// gate cannot catch a regression here for the mirror-image reason — its
    /// collection is 4326, so it exercises this path and never the one above.
    #[test]
    fn items_plan_with_no_filter_crs_is_byte_for_byte_unchanged_on_a_4326_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(4326);
        let query = ItemsQuery {
            filter: Some(Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert_eq!(
            plan.sql,
            "SELECT \"id\"::bigint AS pk_value, json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"demo\" AS t WHERE ST_Intersects(\"geom\", ST_MakeEnvelope($1, $2, $3, $4, 4326)) ORDER BY \"id\" ASC LIMIT $5"
        );
        assert!(
            !plan.sql.contains("ST_Transform"),
            "a CRS84 storage must never gain a transform: {}",
            plan.sql
        );

        // ...and identical to the same query against a collection whose
        // storage SRID is not known at all — the other shape a deployment that
        // never declared one presents, and the one `crs::
        // crs84_literals_need_transform` also answers `false` for.
        let unknown = build_items_plan(&collection(), &query).unwrap();
        assert_eq!(plan.sql, unknown.sql);
    }

    /// Requirement 8 (`/req/filter/filter-crs-param`) with `filter-crs`
    /// naming CRS84 against a projected storage: "the server SHALL process
    /// all geometries in the filter expression using the CRS identified by
    /// the URI in `filter-crs`" — which against a 3857 column means a real
    /// `ST_Transform`, not a no-op. Without it the two geometries are in
    /// different SRIDs and PostGIS refuses the comparison outright.
    #[test]
    fn items_plan_with_explicit_filter_crs84_transforms_the_literal_to_a_non_4326_storage_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let query = ItemsQuery {
            filter: Some(Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
            }),
            filter_crs: RequestedCrs::Crs84,
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql.contains(
                "WHERE ST_Intersects(\"geom\", ST_Transform(ST_MakeEnvelope($1, $2, $3, $4, 4326), 3857))"
            ),
            "sql was: {}",
            plan.sql
        );
    }

    /// Requirement 8 with `filter-crs` naming the collection's own (projected)
    /// storage CRS: the numbers are already in that CRS, so the literal is
    /// built there directly and nothing is transformed or flipped.
    #[test]
    fn items_plan_with_storage_filter_crs_builds_the_literal_directly_at_the_storage_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let query = ItemsQuery {
            filter: Some(Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
            }),
            filter_crs: RequestedCrs::Storage,
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql
                .contains("WHERE ST_Intersects(\"geom\", ST_MakeEnvelope($1, $2, $3, $4, 3857))"),
            "sql was: {}",
            plan.sql
        );
    }

    /// The case where honouring `filter-crs` changes which rows match
    /// without changing a single coordinate value: a 4326 storage SRID and a
    /// `filter-crs` naming EPSG:4326 *by authority*, which is
    /// latitude-before-longitude. The literal's own SRID is unchanged (4326
    /// either way) — only the axis order differs, so `ST_FlipCoordinates` is
    /// the entire fix, and its absence is invisible in the SQL's SRIDs.
    #[test]
    fn items_plan_with_storage_filter_crs_flips_axis_order_for_a_4326_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(4326);
        let query = ItemsQuery {
            filter: Some(Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
            }),
            filter_crs: RequestedCrs::Storage,
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql.contains(
                "WHERE ST_Intersects(\"geom\", ST_FlipCoordinates(ST_MakeEnvelope($1, $2, $3, $4, 4326)))"
            ),
            "sql was: {}",
            plan.sql
        );
    }

    /// `filter-crs` reaches every spatial literal shape, not just `BBOX(...)`
    /// — a WKT or GeoJSON literal is exactly as capable of being authored in
    /// the wrong CRS.
    #[test]
    fn filter_crs_applies_to_wkt_and_geojson_literals_too() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        for (geometry, expected) in [
            (
                GeometryLiteral::GeoJson(
                    serde_json::json!({"type":"Point","coordinates":[1.0,2.0]}),
                ),
                "ST_Transform(ST_SetSRID(ST_GeomFromGeoJSON($1), 4326), 3857)",
            ),
            (
                GeometryLiteral::Wkt(WktGeometry::Point([1.0, 2.0])),
                "ST_Transform(ST_SetSRID(ST_GeomFromText($1), 4326), 3857)",
            ),
        ] {
            let query = ItemsQuery {
                filter: Some(Filter::Intersects {
                    property: "geom".to_string(),
                    geometry,
                }),
                filter_crs: RequestedCrs::Crs84,
                ..ItemsQuery::default()
            };
            let plan = build_items_plan(&with_srid, &query).unwrap();
            assert!(plan.sql.contains(expected), "sql was: {}", plan.sql);
        }
    }

    /// `filter-crs` reaches literals nested anywhere in the expression tree,
    /// not just a bare top-level predicate — the recursion carries it.
    #[test]
    fn filter_crs_reaches_a_literal_nested_under_and_not() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let query = ItemsQuery {
            filter: Some(Filter::And(vec![
                Filter::IsNull {
                    property: "name".to_string(),
                    negated: true,
                },
                Filter::Not(Box::new(Filter::Spatial {
                    property: "geom".to_string(),
                    op: SpatialOp::Within,
                    geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
                })),
            ])),
            filter_crs: RequestedCrs::Crs84,
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql.contains(
                "ST_Within(\"geom\", ST_Transform(ST_MakeEnvelope($1, $2, $3, $4, 4326), 3857))"
            ),
            "sql was: {}",
            plan.sql
        );
    }

    /// A `#34` ABAC grant filter is authored by the deployment, not by a
    /// client, so no lane that carries one has a `filter-crs` to honour —
    /// `FilterCrs::grant_only` gives all three of them (single-item, MVT,
    /// MVT-in-a-grid) the `Omitted` reading, and on a CRS84 storage that is
    /// byte-for-byte what they compiled before `#217`.
    ///
    /// `#247` made the same reading transform on a *projected* storage, and
    /// `grant_only` now carries the SRID so these three lanes follow. They
    /// have to: the items lane already compiles an AND-merged grant through
    /// `FilterCrs::requested` with the collection's SRID, so leaving these
    /// behind would make one deployment's own grant transform on `/items` and
    /// raise the mixed-SRID `500` on `/items/{id}` and every tile — the same
    /// defect `#247` removes, reached by a request that names no parameter at
    /// all.
    #[test]
    fn a_grant_filter_follows_the_storage_srid_on_the_single_item_and_tile_lanes() {
        let grant = Filter::Intersects {
            property: "geom".to_string(),
            geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
        };

        let mut crs84 = collection();
        crs84.srid = Some(4326);
        let (crs84_sql, _) = build_item_plan(
            &crs84,
            PkValue::Integer(7),
            Some(&grant),
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert!(
            crs84_sql.contains("ST_Intersects(\"geom\", ST_MakeEnvelope($2, $3, $4, $5, 4326))")
                && !crs84_sql.contains("ST_Transform"),
            "a CRS84 storage keeps the untouched literal: {crs84_sql}"
        );

        let mut projected = collection();
        projected.srid = Some(3857);
        let (projected_sql, _) = build_item_plan(
            &projected,
            PkValue::Integer(7),
            Some(&grant),
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert!(
            projected_sql.contains(
                "ST_Intersects(\"geom\", ST_Transform(ST_MakeEnvelope($2, $3, $4, $5, 4326), 3857))"
            ),
            "sql was: {projected_sql}"
        );

        let (tile_sql, _) = build_mvt_plan(
            &projected,
            TileMatrixSet::WebMercatorQuad,
            TileCoord { z: 1, x: 0, y: 0 },
            9.5,
            256,
            2000,
            Some(&grant),
        )
        .unwrap();
        assert!(
            tile_sql.contains("ST_Transform(ST_MakeEnvelope("),
            "the MVT lane's grant follows the same rule: {tile_sql}"
        );
    }

    #[test]
    fn items_plan_with_s_intersects_geojson_filter() {
        let query = ItemsQuery {
            filter: Some(Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::GeoJson(
                    serde_json::json!({"type":"Point","coordinates":[1.0,2.0]}),
                ),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql.contains(
                "WHERE ST_Intersects(\"geom\", ST_SetSRID(ST_GeomFromGeoJSON($1), 4326))"
            ),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Text(_), SqlParam::Bigint(11)]
        ));
    }

    #[test]
    fn items_plan_with_temporal_after_before_during_filters() {
        let after = ItemsQuery {
            filter: Some(Filter::After {
                property: "observed_at".to_string(),
                instant: "2020-01-01T00:00:00Z".to_string(),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &after).unwrap();
        assert!(plan
            .sql
            .contains("WHERE (\"observed_at\" > $1::text::timestamptz)"));

        let before = ItemsQuery {
            filter: Some(Filter::Before {
                property: "observed_at".to_string(),
                instant: "2020-01-01T00:00:00Z".to_string(),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &before).unwrap();
        assert!(plan
            .sql
            .contains("WHERE (\"observed_at\" < $1::text::timestamptz)"));

        let during = ItemsQuery {
            filter: Some(Filter::During {
                property: "observed_at".to_string(),
                start: "2020-01-01T00:00:00Z".to_string(),
                end: "2021-01-01T00:00:00Z".to_string(),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &during).unwrap();
        assert!(plan.sql.contains(
            "WHERE (\"observed_at\" >= $1::text::timestamptz AND \"observed_at\" <= $2::text::timestamptz)"
        ));
    }

    #[test]
    fn items_plan_rejects_a_filter_property_that_fails_identifier_whitelisting() {
        let query = ItemsQuery {
            filter: Some(Filter::Compare {
                property: "name; DROP TABLE x; --".to_string(),
                op: CompareOp::Eq,
                value: Literal::Text("a".to_string()),
            }),
            ..ItemsQuery::default()
        };
        assert!(matches!(
            build_items_plan(&collection(), &query),
            Err(PostgisError::InvalidIdentifier(_))
        ));
    }

    #[test]
    fn items_plan_combines_filter_with_bbox_and_datetime_via_and() {
        let query = ItemsQuery {
            bbox: Some([1.0, 2.0, 3.0, 4.0]),
            filter: Some(Filter::Compare {
                property: "population".to_string(),
                op: CompareOp::Gt,
                value: Literal::Number(0.0),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql.contains(
                "WHERE \"geom\" && ST_MakeEnvelope($1, $2, $3, $4, 4326) AND (\"population\"::double precision > $5)"
            ),
            "sql was: {}",
            plan.sql
        );
        assert!(plan.count.is_none());
    }

    // -- advanced CQL2 operator compilation ----------------------------------

    #[test]
    fn items_plan_with_a_like_filter_binds_the_pattern_as_a_parameter() {
        let query = ItemsQuery {
            filter: Some(Filter::Like {
                property: "name".to_string(),
                pattern: "Sm%".to_string(),
                negated: false,
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql.contains("WHERE (\"name\"::text LIKE $1)"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Text(p), SqlParam::Bigint(11)] if p == "Sm%"
        ));
    }

    #[test]
    fn items_plan_with_a_not_like_filter() {
        let query = ItemsQuery {
            filter: Some(Filter::Like {
                property: "name".to_string(),
                pattern: "Sm%".to_string(),
                negated: true,
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql.contains("WHERE (\"name\"::text NOT LIKE $1)"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn items_plan_with_a_between_filter_casts_the_column_once_and_binds_both_bounds() {
        let query = ItemsQuery {
            filter: Some(Filter::Between {
                property: "population".to_string(),
                low: Literal::Number(10.0),
                high: Literal::Number(20.0),
                negated: false,
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql
                .contains("WHERE (\"population\"::double precision BETWEEN $1 AND $2)"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Float8(low), SqlParam::Float8(high), SqlParam::Bigint(11)]
                if (*low - 10.0).abs() < f64::EPSILON && (*high - 20.0).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn items_plan_with_a_not_between_filter() {
        let query = ItemsQuery {
            filter: Some(Filter::Between {
                property: "population".to_string(),
                low: Literal::Number(10.0),
                high: Literal::Number(20.0),
                negated: true,
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql
                .contains("WHERE (\"population\"::double precision NOT BETWEEN $1 AND $2)"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn items_plan_with_an_in_filter_binds_every_value() {
        let query = ItemsQuery {
            filter: Some(Filter::In {
                property: "name".to_string(),
                values: vec![
                    Literal::Text("a".to_string()),
                    Literal::Text("b".to_string()),
                    Literal::Text("c".to_string()),
                ],
                negated: false,
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql.contains("WHERE (\"name\"::text IN ($1, $2, $3))"),
            "sql was: {}",
            plan.sql
        );
        assert_eq!(plan.params.len(), 4); // 3 values + limit
    }

    #[test]
    fn items_plan_with_a_not_in_filter() {
        let query = ItemsQuery {
            filter: Some(Filter::In {
                property: "name".to_string(),
                values: vec![Literal::Text("a".to_string())],
                negated: true,
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql.contains("WHERE (\"name\"::text NOT IN ($1))"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn items_plan_with_an_empty_in_list_compiles_to_the_harmless_identity() {
        let matches_nothing = ItemsQuery {
            filter: Some(Filter::In {
                property: "name".to_string(),
                values: vec![],
                negated: false,
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &matches_nothing).unwrap();
        assert!(plan.sql.contains("WHERE FALSE"), "sql was: {}", plan.sql);

        let matches_everything = ItemsQuery {
            filter: Some(Filter::In {
                property: "name".to_string(),
                values: vec![],
                negated: true,
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &matches_everything).unwrap();
        assert!(plan.sql.contains("WHERE TRUE"), "sql was: {}", plan.sql);
    }

    #[test]
    fn items_plan_with_a_casei_filter_lowercases_both_sides() {
        let query = ItemsQuery {
            filter: Some(Filter::CaseInsensitiveCompare {
                property: "name".to_string(),
                op: CaseInsensitiveCompareOp::Eq,
                value: "John".to_string(),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql
                .contains("WHERE (lower(\"name\"::text) = lower($1))"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Text(v), SqlParam::Bigint(11)] if v == "John"
        ));
    }

    #[test]
    fn items_plan_with_a_casei_not_equal_filter() {
        let query = ItemsQuery {
            filter: Some(Filter::CaseInsensitiveCompare {
                property: "name".to_string(),
                op: CaseInsensitiveCompareOp::Ne,
                value: "John".to_string(),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql
                .contains("WHERE (lower(\"name\"::text) <> lower($1))"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn items_plan_with_every_new_spatial_predicate() {
        let cases = [
            (SpatialOp::Within, "ST_Within"),
            (SpatialOp::Contains, "ST_Contains"),
            (SpatialOp::Disjoint, "ST_Disjoint"),
            (SpatialOp::Touches, "ST_Touches"),
            (SpatialOp::Overlaps, "ST_Overlaps"),
            (SpatialOp::Crosses, "ST_Crosses"),
            (SpatialOp::Equals, "ST_Equals"),
        ];
        for (op, expected_fn) in cases {
            let query = ItemsQuery {
                filter: Some(Filter::Spatial {
                    property: "geom".to_string(),
                    op,
                    geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
                }),
                ..ItemsQuery::default()
            };
            let plan = build_items_plan(&collection(), &query).unwrap();
            assert!(
                plan.sql.contains(&format!(
                    "WHERE {expected_fn}(\"geom\", ST_MakeEnvelope($1, $2, $3, $4, 4326))"
                )),
                "sql was: {} (op {op:?})",
                plan.sql
            );
        }
    }

    // -- WKT geometry literal compilation --------------------------------------

    #[test]
    fn items_plan_with_a_wkt_point_intersects_filter_binds_a_single_text_parameter() {
        let query = ItemsQuery {
            filter: Some(Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Wkt(WktGeometry::Point([1.0, 2.0])),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql
                .contains("WHERE ST_Intersects(\"geom\", ST_SetSRID(ST_GeomFromText($1), 4326))"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Text(wkt), SqlParam::Bigint(11)] if wkt == "POINT(1 2)"
        ));
    }

    #[test]
    fn items_plan_with_a_wkt_polygon_s_within_filter() {
        let query = ItemsQuery {
            filter: Some(Filter::Spatial {
                property: "geom".to_string(),
                op: SpatialOp::Within,
                geometry: GeometryLiteral::Wkt(WktGeometry::Polygon(vec![vec![
                    [0.0, 0.0],
                    [1.0, 0.0],
                    [0.0, 1.0],
                    [0.0, 0.0],
                ]])),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql
                .contains("WHERE ST_Within(\"geom\", ST_SetSRID(ST_GeomFromText($1), 4326))"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Text(wkt), SqlParam::Bigint(11)]
                if wkt == "POLYGON((0 0,1 0,0 1,0 0))"
        ));
    }

    // -- temporal operator compilation (Allen relations) -----------------------

    #[test]
    fn items_plan_with_every_new_temporal_predicate_against_an_instant_literal() {
        // Against an instant literal every op still binds one fresh `$N`
        // parameter per *occurrence* in its Allen formula (never reusing a
        // placeholder index, even for a same-bound repeat like `Overlaps`),
        // even though several ops (Overlaps/OverlappedBy/StartedBy/
        // FinishedBy/Contains) can never match any row for an instant-valued
        // column — see `temporal_op_sql`'s own doc.
        let cases: &[(TemporalOp, &str, usize)] = &[
            (
                TemporalOp::Contains,
                "(\"observed_at\" < $1::text::timestamptz AND \"observed_at\" > $2::text::timestamptz)",
                2,
            ),
            (
                TemporalOp::Disjoint,
                "(\"observed_at\" < $1::text::timestamptz OR \"observed_at\" > $2::text::timestamptz)",
                2,
            ),
            (
                TemporalOp::Equals,
                "(\"observed_at\" = $1::text::timestamptz AND \"observed_at\" = $2::text::timestamptz)",
                2,
            ),
            (
                TemporalOp::FinishedBy,
                "(\"observed_at\" = $2::text::timestamptz AND \"observed_at\" < $1::text::timestamptz)",
                2,
            ),
            (
                TemporalOp::Finishes,
                "(\"observed_at\" = $2::text::timestamptz AND \"observed_at\" > $1::text::timestamptz)",
                2,
            ),
            (
                TemporalOp::Intersects,
                "(\"observed_at\" >= $1::text::timestamptz AND \"observed_at\" <= $2::text::timestamptz)",
                2,
            ),
            (TemporalOp::Meets, "(\"observed_at\" = $1::text::timestamptz)", 1),
            (TemporalOp::MetBy, "(\"observed_at\" = $1::text::timestamptz)", 1),
            (
                TemporalOp::OverlappedBy,
                "($1::text::timestamptz < \"observed_at\" AND \"observed_at\" < $2::text::timestamptz AND $3::text::timestamptz < \"observed_at\")",
                3,
            ),
            (
                TemporalOp::Overlaps,
                "(\"observed_at\" < $1::text::timestamptz AND $2::text::timestamptz < \"observed_at\" AND \"observed_at\" < $3::text::timestamptz)",
                3,
            ),
            (
                TemporalOp::StartedBy,
                "(\"observed_at\" = $1::text::timestamptz AND \"observed_at\" > $2::text::timestamptz)",
                2,
            ),
            (
                TemporalOp::Starts,
                "(\"observed_at\" = $1::text::timestamptz AND \"observed_at\" < $2::text::timestamptz)",
                2,
            ),
        ];
        for (op, expected_fragment, expected_text_params) in cases.iter().copied() {
            let query = ItemsQuery {
                filter: Some(Filter::Temporal {
                    property: "observed_at".to_string(),
                    op,
                    value: TemporalValue::Instant("2020-01-01T00:00:00Z".to_string()),
                }),
                ..ItemsQuery::default()
            };
            let plan = build_items_plan(&collection(), &query).unwrap();
            assert!(
                plan.sql.contains(expected_fragment),
                "op {op:?}: sql was: {}",
                plan.sql
            );
            let text_params = plan
                .params
                .iter()
                .filter(|p| matches!(p, SqlParam::Text(t) if t == "2020-01-01T00:00:00Z"))
                .count();
            assert_eq!(
                text_params, expected_text_params,
                "op {op:?}: params were: {:?}",
                plan.params
            );
            assert!(matches!(plan.params.last(), Some(SqlParam::Bigint(11))));
        }
    }

    #[test]
    fn items_plan_with_a_temporal_interval_literal_binds_start_and_end_as_distinct_parameters() {
        let query = ItemsQuery {
            filter: Some(Filter::Temporal {
                property: "observed_at".to_string(),
                op: TemporalOp::Overlaps,
                value: TemporalValue::Interval(
                    "2020-01-01T00:00:00Z".to_string(),
                    "2020-12-31T00:00:00Z".to_string(),
                ),
            }),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&collection(), &query).unwrap();
        assert!(
            plan.sql.contains(
                "WHERE (\"observed_at\" < $1::text::timestamptz AND $2::text::timestamptz < \"observed_at\" AND \"observed_at\" < $3::text::timestamptz)"
            ),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [
                SqlParam::Text(a),
                SqlParam::Text(b),
                SqlParam::Text(c),
                SqlParam::Bigint(11)
            ] if a == "2020-01-01T00:00:00Z" && b == "2020-01-01T00:00:00Z" && c == "2020-12-31T00:00:00Z"
        ));
    }

    // -- CRS reprojection (`crs`/`bbox-crs`, OGC API Features Part 2) --------

    #[test]
    fn items_plan_with_no_crs_parameter_is_byte_for_byte_unchanged_even_with_a_non_4326_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let plan = build_items_plan(&with_srid, &ItemsQuery::default()).unwrap();
        assert_eq!(
            plan.sql,
            build_items_plan(&collection(), &ItemsQuery::default())
                .unwrap()
                .sql,
            "an omitted crs parameter must produce identical SQL regardless of storage_srid"
        );
        assert!(
            !plan.sql.contains("ST_Transform") && !plan.sql.contains("ST_FlipCoordinates"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn items_plan_with_explicit_crs84_transforms_a_non_4326_storage_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let query = ItemsQuery {
            crs: RequestedCrs::Crs84,
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql
                .contains("'geometry',ST_AsGeoJSON(ST_Transform(\"geom\", 4326))::json"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn items_plan_with_explicit_crs84_against_a_4326_storage_srid_needs_no_transform() {
        let mut with_srid = collection();
        with_srid.srid = Some(4326);
        let query = ItemsQuery {
            crs: RequestedCrs::Crs84,
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql.contains("'geometry',ST_AsGeoJSON(\"geom\")::json"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn items_plan_with_storage_crs_against_a_4326_srid_flips_coordinates() {
        let mut with_srid = collection();
        with_srid.srid = Some(4326);
        let query = ItemsQuery {
            crs: RequestedCrs::Storage,
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql
                .contains("'geometry',ST_AsGeoJSON(ST_FlipCoordinates(\"geom\"))::json"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn items_plan_with_storage_crs_against_a_non_4326_srid_needs_no_flip_or_transform() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let query = ItemsQuery {
            crs: RequestedCrs::Storage,
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql.contains("'geometry',ST_AsGeoJSON(\"geom\")::json"),
            "sql was: {}",
            plan.sql
        );
    }

    /// **The assertion `#255` replaced, and why the pin was wrong.**
    ///
    /// This slot used to hold
    /// `items_plan_with_no_bbox_crs_is_byte_for_byte_unchanged_even_with_a_non_4326_srid`,
    /// the `bbox` twin of the pin `#247` replaced one function over: it pinned
    /// the `Omitted` arm to an untransformed CRS84 envelope *whatever* the
    /// storage SRID, in the name of campaign rule 1.
    ///
    /// That pin was defending a worse thing than `#247`'s was. There the
    /// untransformed literal reached `ST_Intersects`, which refuses a
    /// mixed-SRID comparison, so the preserved behaviour was at least a loud
    /// `500`. `bbox` compiles to the `&&` operator instead, and `&&` does not
    /// raise — `SELECT ST_SetSRID(ST_MakePoint(1,1),3857) &&
    /// ST_MakeEnvelope(0,0,2,2,4326)` answers `t` on a live PostGIS 3.4 — so
    /// what this pinned was a `200` comparing degrees against metres and
    /// returning rows that are simply not the ones asked for. Part 1
    /// Requirement 24 (`/req/core/fc-bbox-response`) clause A: "Only features
    /// that have a spatial geometry that intersects the bounding box SHALL be
    /// part of the result set". There is no working behaviour on that side of
    /// the branch to keep byte-for-byte.
    ///
    /// So the pin is replaced by its positive counterpart: with no `bbox-crs`,
    /// the envelope is built at CRS84 (Part 1 Requirement 23 clause C, Part 2
    /// Requirement 8) and then genuinely reprojected into a projected storage
    /// CRS — character-for-character the SQL an explicit `bbox-crs=CRS84`
    /// already compiled to, which is Part 2's own Abstract Test 10
    /// (`/conf/crs/bbox-crs-parameter-default`) expressed as a golden.
    ///
    /// Rule 1 keeps the whole of its force one test down
    /// (`items_plan_with_no_bbox_crs_is_byte_for_byte_unchanged_on_a_4326_srid`),
    /// which is where every deployment that exists today actually lives.
    #[test]
    fn items_plan_with_no_bbox_crs_transforms_the_envelope_into_a_non_4326_storage_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let query = ItemsQuery {
            bbox: Some([1.0, 2.0, 3.0, 4.0]),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql
                .contains("\"geom\" && ST_Transform(ST_MakeEnvelope($1, $2, $3, $4, 4326), 3857)"),
            "sql was: {}",
            plan.sql
        );

        // Abstract Test 10 as an identity between the two readings: an omitted
        // `bbox-crs` and an explicit CRS84 one are the same request, so they
        // must compile to the same statement with the same bound values.
        let explicit = build_items_plan(
            &with_srid,
            &ItemsQuery {
                bbox: Some([1.0, 2.0, 3.0, 4.0]),
                bbox_crs: RequestedCrs::Crs84,
                ..ItemsQuery::default()
            },
        )
        .unwrap();
        assert_eq!(plan.sql, explicit.sql);
        assert_eq!(plan.params, explicit.params);
    }

    /// **Campaign rule 1, where it still bites: the CRS84 storage.**
    ///
    /// `#255`'s authorised exception covers exactly one case — a projected
    /// storage, where the bytes it changes are wrong rows. A collection stored
    /// at CRS84 has real, working behaviour to preserve, and every live Render
    /// demo is one: `&&` against a 4326 envelope on a 4326 column was always
    /// right, and stays byte-for-byte what it was. The transform is therefore
    /// conditional on the storage SRID, exactly as the `Crs84` arm's own
    /// `match` always was, and this pins that.
    ///
    /// Deliberately a whole-statement equality plus an absence assertion
    /// rather than a `contains` on the predicate: a `contains` would still
    /// pass if a transform leaked into the `SELECT` list. The Italy contract
    /// gate exercises this path and never the one above (its one collection is
    /// 4326), which is why that gate cannot catch a regression in the
    /// projected case — and why this pair, not that gate, is where both halves
    /// are held.
    #[test]
    fn items_plan_with_no_bbox_crs_is_byte_for_byte_unchanged_on_a_4326_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(4326);
        let query = ItemsQuery {
            bbox: Some([1.0, 2.0, 3.0, 4.0]),
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert_eq!(
            plan.sql,
            "SELECT \"id\"::bigint AS pk_value, json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',to_jsonb(t) - 'geom' - 'id') AS feature FROM \"demo\" AS t WHERE \"geom\" && ST_MakeEnvelope($1, $2, $3, $4, 4326) ORDER BY \"id\" ASC LIMIT $5"
        );
        assert!(
            !plan.sql.contains("ST_Transform"),
            "a CRS84 storage must never gain a transform: {}",
            plan.sql
        );

        // ...and identical to the same query against a collection whose
        // storage SRID is not known at all — the other shape a deployment that
        // never declared one presents, and the one
        // `crs::crs84_literals_need_transform` also answers `false` for.
        let mut no_srid = collection();
        no_srid.srid = None;
        assert_eq!(build_items_plan(&no_srid, &query).unwrap().sql, plan.sql);
    }

    #[test]
    fn items_plan_with_explicit_bbox_crs84_transforms_the_envelope_to_a_non_4326_storage_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let query = ItemsQuery {
            bbox: Some([1.0, 2.0, 3.0, 4.0]),
            bbox_crs: RequestedCrs::Crs84,
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql
                .contains("\"geom\" && ST_Transform(ST_MakeEnvelope($1, $2, $3, $4, 4326), 3857)"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn items_plan_with_storage_bbox_crs_builds_the_envelope_directly_at_the_storage_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let query = ItemsQuery {
            bbox: Some([1.0, 2.0, 3.0, 4.0]),
            bbox_crs: RequestedCrs::Storage,
            ..ItemsQuery::default()
        };
        let plan = build_items_plan(&with_srid, &query).unwrap();
        assert!(
            plan.sql
                .contains("\"geom\" && ST_MakeEnvelope($1, $2, $3, $4, 3857)"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn item_plan_with_no_crs_is_byte_for_byte_unchanged() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let (sql, _params) = build_item_plan(
            &with_srid,
            PkValue::Integer(42),
            None,
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert!(
            sql.contains("'geometry',ST_AsGeoJSON(\"geom\")::json"),
            "sql was: {sql}"
        );
    }

    #[test]
    fn item_plan_with_explicit_crs84_transforms_a_non_4326_storage_srid() {
        let mut with_srid = collection();
        with_srid.srid = Some(3857);
        let (sql, _params) =
            build_item_plan(&with_srid, PkValue::Integer(42), None, RequestedCrs::Crs84).unwrap();
        assert!(
            sql.contains("'geometry',ST_AsGeoJSON(ST_Transform(\"geom\", 4326))::json"),
            "sql was: {sql}"
        );
    }

    #[test]
    fn item_plan_with_storage_crs_against_a_4326_srid_flips_coordinates() {
        let mut with_srid = collection();
        with_srid.srid = Some(4326);
        let (sql, _params) = build_item_plan(
            &with_srid,
            PkValue::Integer(42),
            None,
            RequestedCrs::Storage,
        )
        .unwrap();
        assert!(
            sql.contains("'geometry',ST_AsGeoJSON(ST_FlipCoordinates(\"geom\"))::json"),
            "sql was: {sql}"
        );
    }

    // -- geometry profile (`#101`) -------------------------------------------

    #[test]
    fn sample_percentage_targets_the_configured_row_count_for_a_large_table() {
        let pct = sample_percentage(Some(1_000_000));
        // 2_000 / 1_000_000 * 100 = 0.2%, clamped up to the 1% floor.
        assert_eq!(pct, 1.0);
    }

    #[test]
    fn sample_percentage_clamps_to_one_hundred_for_a_small_table() {
        let pct = sample_percentage(Some(10));
        assert_eq!(pct, 100.0);
    }

    #[test]
    fn sample_percentage_never_drops_below_the_floor_regardless_of_table_size() {
        assert_eq!(sample_percentage(Some(u64::MAX)), 1.0);
    }

    #[test]
    fn sample_percentage_uses_a_small_fixed_fallback_when_the_estimate_is_unknown() {
        assert_eq!(sample_percentage(None), 5.0);
    }

    #[test]
    fn sample_percentage_uses_the_same_fallback_when_the_estimate_is_the_never_analyzed_zero_sentinel(
    ) {
        assert_eq!(sample_percentage(Some(0)), 5.0);
    }

    #[test]
    fn geometry_profile_plan_uses_tablesample_system_with_a_bound_percentage_parameter() {
        let (sql, params) =
            build_geometry_profile_plan("demo", "geom", Some("POLYGON"), 2.5).unwrap();
        assert!(
            sql.contains("TABLESAMPLE SYSTEM ($1::double precision::real)"),
            "sql was: {sql}"
        );
        assert!(
            matches!(params.as_slice(), [SqlParam::Float8(pct)] if (*pct - 2.5).abs() < f64::EPSILON)
        );
    }

    #[test]
    fn geometry_profile_plan_quotes_the_table_and_geometry_identifiers() {
        let (sql, _params) =
            build_geometry_profile_plan("demo", "geom", Some("POLYGON"), 10.0).unwrap();
        assert!(sql.contains("FROM \"demo\""), "sql was: {sql}");
        assert!(sql.contains("\"geom\" AS geom"), "sql was: {sql}");
    }

    #[test]
    fn geometry_profile_plan_uses_area_for_a_polygon_type() {
        let (sql, _params) =
            build_geometry_profile_plan("demo", "geom", Some("MULTIPOLYGON"), 10.0).unwrap();
        assert!(
            sql.contains("ST_Area(geom) AS feature_size"),
            "sql was: {sql}"
        );
    }

    #[test]
    fn geometry_profile_plan_uses_the_sample_alias_for_a_nonstandard_polygon_column() {
        let (sql, _params) =
            build_geometry_profile_plan("demo", "the_geo", Some("MULTIPOLYGON"), 10.0).unwrap();
        assert!(sql.contains("\"the_geo\" AS geom"), "sql was: {sql}");
        assert!(
            sql.contains("ST_Area(geom) AS feature_size"),
            "the metrics CTE must read its sampled alias: {sql}"
        );
        assert!(
            !sql.contains("ST_Area(\"the_geo\")"),
            "the physical column is not visible inside the metrics CTE: {sql}"
        );
    }

    #[test]
    fn geometry_profile_plan_uses_length_for_a_line_type() {
        let (sql, _params) =
            build_geometry_profile_plan("demo", "geom", Some("MULTILINESTRING"), 10.0).unwrap();
        assert!(
            sql.contains("ST_Length(geom) AS feature_size"),
            "sql was: {sql}"
        );
    }

    #[test]
    fn geometry_profile_plan_has_no_feature_size_metric_for_a_point_type() {
        let (sql, _params) =
            build_geometry_profile_plan("demo", "geom", Some("MULTIPOINT"), 10.0).unwrap();
        assert!(
            sql.contains("NULL::double precision AS feature_size"),
            "sql was: {sql}"
        );
    }

    #[test]
    fn geometry_profile_plan_has_no_feature_size_metric_for_an_unknown_or_mixed_type() {
        let (sql, _params) = build_geometry_profile_plan("demo", "geom", None, 10.0).unwrap();
        assert!(
            sql.contains("NULL::double precision AS feature_size"),
            "sql was: {sql}"
        );
        let (sql, _params) =
            build_geometry_profile_plan("demo", "geom", Some("GEOMETRY"), 10.0).unwrap();
        assert!(
            sql.contains("NULL::double precision AS feature_size"),
            "sql was: {sql}"
        );
    }

    #[test]
    fn geometry_profile_plan_enumerates_every_part_for_ring_count_on_a_polygon_type() {
        let (sql, _params) =
            build_geometry_profile_plan("demo", "geom", Some("MULTIPOLYGON"), 10.0).unwrap();
        assert!(
            sql.contains("CROSS JOIN LATERAL"),
            "ring enumeration must join laterally per sampled row, not scan separately: {sql}"
        );
        assert!(
            sql.contains("generate_series(1, ST_NumGeometries(geom))"),
            "ring enumeration must walk every part ST_NumGeometries reports, not just the first: {sql}"
        );
        assert!(
            sql.contains("ST_NumInteriorRings(ST_GeometryN(geom, part.n))"),
            "each part's own interior-ring count must be read via the enumerated part index: {sql}"
        );
        assert!(
            !sql.contains("ST_GeometryN(geom, 1))"),
            "the first-part-only proxy must be gone: {sql}"
        );
    }

    #[test]
    fn geometry_profile_plan_has_no_ring_metric_for_a_line_or_point_type() {
        let (sql, _params) =
            build_geometry_profile_plan("demo", "geom", Some("MULTILINESTRING"), 10.0).unwrap();
        assert!(
            sql.contains("NULL::double precision AS ring_count"),
            "a line-typed collection has no ring concept: {sql}"
        );
        assert!(!sql.contains("CROSS JOIN LATERAL"), "sql was: {sql}");

        let (sql, _params) =
            build_geometry_profile_plan("demo", "geom", Some("MULTIPOINT"), 10.0).unwrap();
        assert!(
            sql.contains("NULL::double precision AS ring_count"),
            "a point-typed collection has no ring concept: {sql}"
        );
    }

    #[test]
    fn geometry_profile_plan_has_no_ring_metric_for_an_unknown_or_mixed_type() {
        let (sql, _params) = build_geometry_profile_plan("demo", "geom", None, 10.0).unwrap();
        assert!(
            sql.contains("NULL::double precision AS ring_count"),
            "sql was: {sql}"
        );
        let (sql, _params) =
            build_geometry_profile_plan("demo", "geom", Some("GEOMETRY"), 10.0).unwrap();
        assert!(
            sql.contains("NULL::double precision AS ring_count"),
            "sql was: {sql}"
        );
    }

    #[test]
    fn geometry_profile_plan_excludes_null_geometries_from_the_sample() {
        let (sql, _params) =
            build_geometry_profile_plan("demo", "geom", Some("POLYGON"), 10.0).unwrap();
        assert!(sql.contains("WHERE \"geom\" IS NOT NULL"), "sql was: {sql}");
    }

    #[test]
    fn geometry_profile_plan_rejects_an_invalid_table_identifier() {
        assert!(build_geometry_profile_plan("bad; drop table x", "geom", None, 10.0).is_err());
    }

    // ---------------------------------------------------------------
    // `#278`: `jsonb_build_object` over the backend-derived column list
    // ---------------------------------------------------------------

    fn attribute(name: &str, sql_type: &str) -> AttributeColumn {
        AttributeColumn {
            name: name.to_string(),
            sql_type: sql_type.to_string(),
        }
    }

    /// [`collection`] plus the backend-derived attribute list `Router::
    /// effective_decl` fills `CollectionDecl::attribute_columns` with
    /// (`#278`): `information_schema` ordinal order, the geometry column
    /// already excluded by `catalog::ATTRIBUTE_SCHEMA_SQL`, the pk still
    /// present.
    fn collection_with_derived_columns() -> CollectionDecl {
        let mut collection = collection();
        collection.attribute_columns = Some(vec![
            attribute("id", "bigint"),
            attribute("observed_at", "timestamp with time zone"),
            attribute("name", "text"),
        ]);
        collection
    }

    #[test]
    fn items_plan_names_the_derived_property_columns_instead_of_rendering_the_whole_row() {
        let plan =
            build_items_plan(&collection_with_derived_columns(), &ItemsQuery::default()).unwrap();
        assert_eq!(
            plan.sql,
            "SELECT \"id\"::bigint AS pk_value, json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',jsonb_build_object('observed_at',t.\"observed_at\",'name',t.\"name\")) AS feature FROM \"demo\" AS t ORDER BY \"id\" ASC LIMIT $1"
        );
        // The whole point: the geometry column is never rendered at all, so
        // there is no hex WKB to build and discard.
        assert!(
            !plan.sql.contains("to_jsonb"),
            "a derived column list must not fall back to the whole-row render: {}",
            plan.sql
        );
    }

    #[test]
    fn items_plan_keeps_the_whole_row_render_when_no_column_list_was_derived() {
        // `Router::effective_decl`'s fully-pinned fast path derives no
        // descriptor, so it carries no attribute list — that collection's
        // SQL must be byte for byte what it was before `#278`.
        let collection = collection();
        assert!(collection.attribute_columns.is_none());
        let plan = build_items_plan(&collection, &ItemsQuery::default()).unwrap();
        assert!(plan
            .sql
            .contains("'properties',to_jsonb(t) - 'geom' - 'id')"));
        assert!(!plan.sql.contains("jsonb_build_object"));
    }

    #[test]
    fn budgeted_items_plan_names_property_columns_off_the_carried_row_composite() {
        let mut collection = collection_with_derived_columns();
        collection.settings.items_vertex_budget = Some(50_000);
        let plan = build_items_plan(&collection, &ItemsQuery::default()).unwrap();
        assert!(plan.sql.contains(
            "'properties',jsonb_build_object('observed_at',(source_row).\"observed_at\",'name',(source_row).\"name\")"
        ), "sql was: {}", plan.sql);
        // Still behind the budget's own `CASE`, exactly as `#1` left it.
        let candidate_scan = plan.sql.split("LIMIT $1").next().unwrap();
        assert!(!candidate_scan.contains("jsonb_build_object"));
        assert!(plan.sql.contains("t AS source_row"));
    }

    #[test]
    fn item_plan_names_the_derived_property_columns() {
        let (sql, _params) = build_item_plan(
            &collection_with_derived_columns(),
            PkValue::Integer(7),
            None,
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT json_build_object('type','Feature','id',\"id\"::text,'geometry',ST_AsGeoJSON(\"geom\")::json,'properties',jsonb_build_object('observed_at',t.\"observed_at\",'name',t.\"name\")) AS feature FROM \"demo\" AS t WHERE \"id\" = $1::bigint"
        );
    }

    #[test]
    fn the_named_projection_excludes_the_pk_and_the_geometry_by_name() {
        let mut collection = collection();
        // A descriptor derived against a different geometry column would
        // still leave `geom` in the list; both are excluded by name here
        // regardless, matching the `- 'geom' - 'id'` this replaces.
        collection.attribute_columns = Some(vec![
            attribute("id", "bigint"),
            attribute("geom", "USER-DEFINED"),
            attribute("name", "text"),
        ]);
        let plan = build_items_plan(&collection, &ItemsQuery::default()).unwrap();
        assert!(plan
            .sql
            .contains("'properties',jsonb_build_object('name',t.\"name\"))"));
        assert!(!plan
            .sql
            .contains("\"geom\")::json,'properties',jsonb_build_object('id'"));
        assert!(!plan.sql.contains("'geom',t.\"geom\""));
    }

    #[test]
    fn a_table_with_no_property_columns_projects_an_empty_object() {
        let mut collection = collection();
        collection.attribute_columns = Some(vec![attribute("id", "bigint")]);
        let plan = build_items_plan(&collection, &ItemsQuery::default()).unwrap();
        // `jsonb_build_object()` is `{}` — exactly what
        // `to_jsonb(t) - 'geom' - 'id'` answers for that same table.
        assert!(plan.sql.contains("'properties',jsonb_build_object())"));
    }

    #[test]
    fn more_property_columns_than_one_call_can_take_are_concatenated_not_truncated() {
        // `jsonb_build_object` is variadic over `"any"`, so `FUNC_MAX_ARGS`
        // (100) caps one call at 50 key/value pairs; a 51st fails the whole
        // statement with `cannot pass more than 100 arguments to a
        // function`. Chunking with `||` merges the objects key-wise.
        let mut columns = vec![attribute("id", "bigint")];
        for n in 0..120 {
            columns.push(attribute(&format!("c{n}"), "text"));
        }
        let mut collection = collection();
        collection.attribute_columns = Some(columns);
        let plan = build_items_plan(&collection, &ItemsQuery::default()).unwrap();
        assert_eq!(
            plan.sql.matches("jsonb_build_object(").count(),
            3,
            "120 property columns need three calls of at most 50 pairs: {}",
            plan.sql
        );
        assert_eq!(plan.sql.matches(" || ").count(), 2);
        for n in 0..120 {
            assert!(
                plan.sql.contains(&format!("'c{n}',t.\"c{n}\"")),
                "column c{n} must survive chunking"
            );
        }
        // No call may exceed the cap.
        for call in plan.sql.split("jsonb_build_object(").skip(1) {
            let args = call.split(')').next().unwrap();
            assert!(
                args.split(',').count() <= MAX_JSONB_BUILD_OBJECT_PAIRS * 2,
                "one call carried more pairs than PostgreSQL accepts: {args}"
            );
        }
    }

    #[test]
    fn a_column_name_the_identifier_whitelist_rejects_falls_back_to_the_whole_row_render() {
        // `"my-col"` is a perfectly legal PostgreSQL column name that this
        // crate's identifier whitelist refuses, and `to_jsonb` serves it
        // correctly today because it never spells a column name in SQL text.
        // Refusing the request, or quietly dropping the column from
        // `properties`, would both be regressions — so the whole projection
        // falls back instead, keeping the bytes and losing only the speedup.
        let mut collection = collection();
        collection.attribute_columns = Some(vec![
            attribute("id", "bigint"),
            attribute("name", "text"),
            attribute("my-col", "text"),
        ]);
        let plan = build_items_plan(&collection, &ItemsQuery::default()).unwrap();
        assert!(plan
            .sql
            .contains("'properties',to_jsonb(t) - 'geom' - 'id')"));
        assert!(!plan.sql.contains("jsonb_build_object"));
    }

    /// `#278`'s decisive proof: the `jsonb_build_object` projection and the
    /// `to_jsonb` whole-row projection must produce **byte-identical**
    /// GeoJSON for the same request — across every property type a
    /// collection's schema can carry, across the vertex-budgeted and
    /// unbudgeted plan shapes, across `crs=` variants, and on both the page
    /// (`build_items_plan`) and single-item (`build_item_plan`) lanes.
    ///
    /// Compared as the database's own `json` text, not as parsed values: a
    /// `numeric`'s trailing zeros, a `float8`'s shortest round-trip form and
    /// a `timestamptz`'s offset all survive to the wire, and a parsed
    /// comparison would let two different renderings of the same number
    /// through. Skipped gracefully unless `TELLURION_TEST_DATABASE_URL` is
    /// set, like every other live test in this workspace.
    #[cfg(feature = "test-support")]
    mod jsonb_parity {
        use super::*;
        use crate::catalog::ATTRIBUTE_SCHEMA_SQL;
        use crate::test_harness;
        use tokio_postgres::types::ToSql;
        use tokio_postgres::Client;

        const TABLE_4326: &str = "tellurion_postgis_jsonb_parity_4326";
        const TABLE_3857: &str = "tellurion_postgis_jsonb_parity_3857";

        /// Every property type the derived schema can carry — integer,
        /// float, text, boolean, timestamp, `NULL`, and nested `json`/
        /// `jsonb` — plus the shapes most likely to render differently
        /// between the two projections if they were not the same rendering
        /// function underneath (`numeric` keeps trailing zeros, `text[]`
        /// becomes a JSON array, `bytea` becomes a hex string, and a second
        /// `geometry` column becomes hex WKB in *both* projections).
        fn ddl(table: &str, srid: i32, point: &str) -> String {
            format!(
                "DROP TABLE IF EXISTS {table};
                 CREATE TABLE {table} (
                     id bigserial PRIMARY KEY,
                     p_int integer,
                     p_bigint bigint,
                     p_float double precision,
                     p_numeric numeric(12,3),
                     p_text text,
                     p_bool boolean,
                     p_ts timestamp,
                     p_tstz timestamptz,
                     p_date date,
                     p_uuid uuid,
                     p_json json,
                     p_jsonb jsonb,
                     p_array text[],
                     p_bytea bytea,
                     p_null text,
                     p_other_geom geometry(Point, 4326),
                     geom geometry(Point, {srid}) NOT NULL
                 );
                 INSERT INTO {table}
                     (p_int, p_bigint, p_float, p_numeric, p_text, p_bool, p_ts, p_tstz,
                      p_date, p_uuid, p_json, p_jsonb, p_array, p_bytea, p_null,
                      p_other_geom, geom)
                 VALUES
                     (42, 9007199254740993, 3.14159265358979, 1.500,
                      'h\u{00e9}llo \"quoted\" backslash', true,
                      '2021-01-02T03:04:05.123456', '2021-01-02T03:04:05.123456+02',
                      '2021-06-01', '11111111-2222-3333-4444-555555555555',
                      '{{\"a\":[1,2],\"b\":null}}', '{{\"a\":[1,2],\"b\":null}}',
                      ARRAY['x','y'], '\\xdeadbeef', NULL,
                      ST_SetSRID(ST_MakePoint(5, 6), 4326), {point}),
                     (NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
                      NULL, NULL, NULL, NULL, NULL, NULL, {point});
                 ANALYZE {table};"
            )
        }

        fn parity_collection(table: &str, srid: i32) -> CollectionDecl {
            let mut collection: CollectionDecl = serde_yaml::from_str(&format!(
                "id: parity\ncatalog: default\nstorage: main\ntable: {table}\ngeometry: geom\npk: id\n"
            ))
            .unwrap();
            collection.srid = Some(srid);
            collection
        }

        /// The attribute list exactly as `PostgisBackend::attribute_schema`
        /// derives it — the same query, so this test cannot pass against a
        /// column list the real derivation would never produce.
        async fn derived_columns(client: &Client, table: &str) -> Vec<AttributeColumn> {
            let rows = client
                .query(ATTRIBUTE_SCHEMA_SQL, &[&table, &"geom"])
                .await
                .expect("derives the attribute schema");
            rows.iter()
                .map(|row| AttributeColumn {
                    name: row.get("column_name"),
                    sql_type: row.get("data_type"),
                })
                .collect()
        }

        async fn feature_texts(
            client: &Client,
            plan_sql: &str,
            params: &[SqlParam],
        ) -> Vec<Option<String>> {
            let boxed: Vec<Box<dyn ToSql + Sync + Send>> =
                params.iter().map(SqlParam::boxed).collect();
            let refs: Vec<&(dyn ToSql + Sync)> = boxed
                .iter()
                .map(|param| param.as_ref() as &(dyn ToSql + Sync))
                .collect();
            let wrapped = format!("SELECT q.feature::text AS feature_text FROM ({plan_sql}) AS q");
            let rows = client
                .query(&wrapped, &refs)
                .await
                .unwrap_or_else(|error| panic!("running {wrapped}: {error:?}"));
            rows.iter()
                .map(|row| row.get::<_, Option<String>>("feature_text"))
                .collect()
        }

        #[tokio::test]
        async fn the_named_projection_is_byte_identical_to_the_whole_row_render() {
            let Ok(database_url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
                eprintln!("skipping the_named_projection_is_byte_identical_to_the_whole_row_render: TELLURION_TEST_DATABASE_URL not set");
                return;
            };
            let client = test_harness::connect(&database_url).await;
            for (table, srid, point) in [
                (TABLE_4326, 4326, "ST_SetSRID(ST_MakePoint(1, 2), 4326)"),
                (
                    TABLE_3857,
                    3857,
                    "ST_Transform(ST_SetSRID(ST_MakePoint(10, 20), 4326), 3857)",
                ),
            ] {
                test_harness::apply_fixture_ddl(&client, table, &ddl(table, srid, point))
                    .await
                    .expect("seeds the parity fixture");

                let columns = derived_columns(&client, table).await;
                assert!(
                    columns.iter().any(|column| column.name == "p_jsonb"),
                    "the derivation must report the nested-json column: {columns:?}"
                );
                assert!(
                    columns.iter().all(|column| column.name != "geom"),
                    "the derivation already excludes the geometry column: {columns:?}"
                );

                for crs in [
                    RequestedCrs::Omitted,
                    RequestedCrs::Crs84,
                    RequestedCrs::Storage,
                ] {
                    for budget in [None, Some(1_000_000_u64)] {
                        let mut whole_row = parity_collection(table, srid);
                        whole_row.settings.items_vertex_budget = budget;
                        let mut named = whole_row.clone();
                        named.attribute_columns = Some(columns.clone());

                        let context = format!("table={table} crs={crs:?} budget={budget:?}");

                        let query = ItemsQuery {
                            limit: 10,
                            crs,
                            ..ItemsQuery::default()
                        };
                        let whole_row_plan = build_items_plan(&whole_row, &query).unwrap();
                        let named_plan = build_items_plan(&named, &query).unwrap();
                        assert!(
                            whole_row_plan.sql.contains("to_jsonb")
                                && !whole_row_plan.sql.contains("jsonb_build_object"),
                            "the control side must be the pre-#278 projection ({context})"
                        );
                        assert!(
                            named_plan.sql.contains("jsonb_build_object")
                                && !named_plan.sql.contains("to_jsonb"),
                            "the subject side must be the named projection ({context})"
                        );

                        let expected =
                            feature_texts(&client, &whole_row_plan.sql, &whole_row_plan.params)
                                .await;
                        let actual =
                            feature_texts(&client, &named_plan.sql, &named_plan.params).await;
                        assert_eq!(expected.len(), 2, "both seeded rows are served ({context})");
                        assert!(
                            expected.iter().all(Option::is_some),
                            "the budget must not refuse either row ({context})"
                        );
                        assert_eq!(actual, expected, "items parity diverged ({context})");

                        // Single-item lane, same comparison.
                        let (whole_row_sql, whole_row_params) =
                            build_item_plan(&whole_row, PkValue::Integer(1), None, crs).unwrap();
                        let (named_sql, named_params) =
                            build_item_plan(&named, PkValue::Integer(1), None, crs).unwrap();
                        let expected_item =
                            feature_texts(&client, &whole_row_sql, &whole_row_params).await;
                        let actual_item = feature_texts(&client, &named_sql, &named_params).await;
                        assert_eq!(expected_item.len(), 1, "one item ({context})");
                        assert_eq!(
                            actual_item, expected_item,
                            "item parity diverged ({context})"
                        );
                    }
                }
            }
        }

        /// Parity alone proves the two projections agree; it cannot prove
        /// what they agree *on*, because a change that broke reprojection
        /// would break both sides identically. This pins the coordinates the
        /// named projection actually emits per `crs=`: untouched for
        /// `Omitted`, axis-swapped for a 4326 storage asked for its own CRS
        /// by authority, and genuinely reprojected to degrees for a
        /// projected storage asked for CRS84.
        #[tokio::test]
        async fn the_named_projection_honours_crs_axis_order_and_reprojection() {
            let Ok(database_url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
                eprintln!("skipping the_named_projection_honours_crs_axis_order_and_reprojection: TELLURION_TEST_DATABASE_URL not set");
                return;
            };
            let client = test_harness::connect(&database_url).await;
            for (table, srid, point) in [
                (TABLE_4326, 4326, "ST_SetSRID(ST_MakePoint(1, 2), 4326)"),
                (
                    TABLE_3857,
                    3857,
                    "ST_Transform(ST_SetSRID(ST_MakePoint(10, 20), 4326), 3857)",
                ),
            ] {
                test_harness::apply_fixture_ddl(&client, table, &ddl(table, srid, point))
                    .await
                    .expect("seeds the parity fixture");
            }

            let cases = [
                // A 4326 storage with no `crs` at all: x,y exactly as stored.
                (TABLE_4326, 4326, RequestedCrs::Omitted, "[1,2]"),
                // CRS84 against a 4326 storage is a no-op, not a flip.
                (TABLE_4326, 4326, RequestedCrs::Crs84, "[1,2]"),
                // The storage CRS *by authority* is latitude-first.
                (TABLE_4326, 4326, RequestedCrs::Storage, "[2,1]"),
                // A projected storage asked for CRS84 reprojects to degrees.
                (TABLE_3857, 3857, RequestedCrs::Crs84, "[10,20]"),
            ];
            for (table, srid, crs, coordinates) in cases {
                let columns = derived_columns(&client, table).await;
                let mut named = parity_collection(table, srid);
                named.attribute_columns = Some(columns);
                let plan = build_items_plan(
                    &named,
                    &ItemsQuery {
                        limit: 1,
                        crs,
                        ..ItemsQuery::default()
                    },
                )
                .unwrap();
                assert!(plan.sql.contains("jsonb_build_object"));
                let texts = feature_texts(&client, &plan.sql, &plan.params).await;
                let feature = texts[0].as_deref().expect("a feature");
                assert!(
                    feature.contains(&format!("\"coordinates\":{coordinates}")),
                    "table={table} crs={crs:?} expected coordinates {coordinates} in {feature}"
                );
            }
        }
    }
}
