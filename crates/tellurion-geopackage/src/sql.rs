//! Pure SQL builders: table/column names + a query in, SQL text + typed
//! params out. No I/O, no `rusqlite::Connection` — mirrors
//! `tellurion-postgis::sql`'s own discipline exactly, adapted to the SQLite
//! dialect. Every identifier is whitelist-quoted (`ident.rs`); every value
//! is bound as a numbered `?N` parameter, never interpolated.
//!
//! Unlike PostGIS's own compiler, a bound parameter here never needs a text-
//! cast trick (`$N::text::<pg_type>`): SQLite's manifest typing plus its
//! comparison-affinity rules compare a REAL/INTEGER column against a bound
//! value of either numeric Rust type correctly regardless of the column's
//! own storage class, so [`SqlParam`] carries the literal's own native type
//! straight through.
//!
//! v0.1 assumes the primary key is a single-column `INTEGER PRIMARY KEY` —
//! the same assumption `tellurion-postgis::sql`'s own module doc states for
//! its `int4`/`int8` primary key. Keyset tokens and item ids are parsed to
//! `i64`.
//!
//! ## bbox pushdown
//!
//! A `bbox` items-query parameter and this driver's tile envelope both
//! compile to a `pk IN (SELECT id FROM "rtree_<table>_<geom>" WHERE ...)`
//! subquery against the GeoPackage spec's own R*Tree spatial-index virtual
//! table (Annex L) — a coarse bounding-box test, exactly what the OGC API
//! Features `bbox` parameter itself is defined to mean (bbox-vs-bbox
//! overlap, not exact geometry intersection).
//!
//! `compile_filter`'s `Spatial` arm (the six `S_*` predicates beyond
//! `S_INTERSECTS`) still refuses by name — this SQLite dialect has no
//! geometry engine loaded to evaluate them, and reusing the R*Tree's own
//! coarse bounding-box test as if it answered the exact question would be
//! silently wrong for anything but a bbox-shaped literal.
//!
//! `S_INTERSECTS` (`Filter::Intersects`) is different: this driver answers
//! it *exactly*, by combining the same R*Tree bbox prune above (a sound
//! candidate pre-filter — any row whose bbox doesn't overlap the query
//! geometry's own bbox can never intersect it) with an in-process exact
//! geometry test on each candidate's decoded row, via `crate::intersects`
//! (`geo_types`/`geo::Intersects`, no new heavyweight dependency — see that
//! module's own doc). `compile_filter`'s own `Intersects` arm below
//! contributes only a cheap SQL-level guard (`column IS NOT NULL`); the
//! rest of the work — the R*Tree bbox clause and the exact per-row test —
//! is `collect_intersects_check`'s and each builder function's job below,
//! and `driver.rs`'s job for the actual per-row decode+test. This is
//! un-refused only for the 2D geometry classes `crate::intersects` covers,
//! and only when `S_INTERSECTS` sits in an AND-only position (never beneath
//! `OR`/`NOT` — see `collect_intersects_check`'s own doc for why): anything
//! outside that stays refused by name, the same honest-refusal posture the
//! `Spatial` arm keeps for its own six predicates unconditionally.

use tellurion_core::{
    CaseInsensitiveCompareOp, CompareOp, Filter, Literal, SpatialOp, TemporalOp, TemporalValue,
};

use crate::error::{GeopackageError, Result};
use crate::ident::quote_ident;
use crate::intersects::{self, IntersectsCheck};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SqlParam {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    /// A full GeoPackage Binary geometry blob (`gpb::encode_from_geojson_
    /// geometry`'s output) — used only by the write lane (`write_sql.rs`),
    /// never by a read-side filter/query builder in this module.
    Blob(Vec<u8>),
}

impl rusqlite::ToSql for SqlParam {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        match self {
            SqlParam::Null => Ok(rusqlite::types::ToSqlOutput::Owned(
                rusqlite::types::Value::Null,
            )),
            SqlParam::Int(v) => v.to_sql(),
            SqlParam::Real(v) => v.to_sql(),
            SqlParam::Text(v) => v.to_sql(),
            SqlParam::Blob(v) => v.to_sql(),
        }
    }
}

/// `"rtree_<table>_<geometry_column>"`, quoted — the GeoPackage spec's fixed
/// naming convention (Annex L.2) for the R*Tree spatial-index virtual table
/// this driver's own provisioning subcommand creates. Kept in sync by hand
/// with `tellurion-ingest`'s provisioning module, the same arrangement
/// `tellurion-postgis::write_sql`'s own doc describes for its outbox table
/// name.
fn rtree_table_ident(table: &str, geometry_column: &str) -> Result<String> {
    quote_ident(&format!("rtree_{table}_{geometry_column}"))
}

/// The `pk IN (SELECT id FROM rtree WHERE ...)` bbox-pushdown clause shared
/// by items paging and tile production — see this module's own "bbox
/// pushdown" doc.
/// Standard rectangle-overlap test between the query envelope
/// `[minx,miny,maxx,maxy]` and each `rtree` row's own stored
/// `(minx,maxx,miny,maxy)` bounds: two axis-aligned rectangles A (the
/// query) and B (a stored row) overlap iff `A.minx <= B.maxx AND A.maxx >=
/// B.minx AND A.miny <= B.maxy AND A.maxy >= B.miny` — written below with
/// the stored column on the left, matching every other comparison in this
/// module's own style (column first).
fn bbox_clause(
    pk: &str,
    table: &str,
    geometry_column: &str,
    bbox: [f64; 4],
    params: &mut Vec<SqlParam>,
) -> Result<String> {
    let rtree = rtree_table_ident(table, geometry_column)?;
    let [minx, miny, maxx, maxy] = bbox;
    params.push(SqlParam::Real(minx));
    params.push(SqlParam::Real(maxx));
    params.push(SqlParam::Real(miny));
    params.push(SqlParam::Real(maxy));
    let n = params.len();
    Ok(format!(
        "{pk} IN (SELECT id FROM {rtree} WHERE maxx >= ?{} AND minx <= ?{} AND maxy >= ?{} AND miny <= ?{})",
        n - 3,
        n - 2,
        n - 1,
        n
    ))
}

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

/// A scalar `Literal`'s bound [`SqlParam`] — SQLite compares a bound value
/// against a column of any storage class using its own affinity rules, so
/// (unlike PostGIS's `sql.rs`) no per-comparison cast text is needed here at
/// all; see this module's own top-level doc.
fn literal_param(value: &Literal) -> SqlParam {
    match value {
        Literal::Text(s) => SqlParam::Text(s.clone()),
        Literal::Number(n) => SqlParam::Real(*n),
        Literal::Bool(b) => SqlParam::Int(if *b { 1 } else { 0 }),
    }
}

fn compile_bool_chain(
    items: &[Filter],
    joiner: &str,
    params: &mut Vec<SqlParam>,
) -> Result<String> {
    if items.is_empty() {
        return Ok(if joiner == "AND" {
            "1".to_string()
        } else {
            "0".to_string()
        });
    }
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        parts.push(compile_filter(item, params)?);
    }
    Ok(format!("({})", parts.join(&format!(" {joiner} "))))
}

/// Compiles one of the twelve `TemporalOp` variants ([`Filter::Temporal`]) to
/// a bound-parameter boolean expression, mirroring `tellurion-postgis::
/// sql::temporal_op_sql`'s own Allen-interval formulas exactly (see that
/// function's doc for the derivation of each arm) — except every comparison
/// here is a plain lexicographic text comparison, never a `::timestamptz`
/// cast: this driver stores a datetime column as the GeoPackage spec's own
/// recommended ISO 8601 text (Annex on GeoPackage Data Types), and
/// lexicographic order over zero-padded, `Z`-suffixed ISO 8601 UTC text
/// agrees with chronological order.
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
        format!("?{}", params.len())
    };
    match op {
        TemporalOp::Equals => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} = {b1} AND {column} = {b2})")
        }
        TemporalOp::Disjoint => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} < {b1} OR {column} > {b2})")
        }
        TemporalOp::Intersects => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} >= {b1} AND {column} <= {b2})")
        }
        TemporalOp::Meets => {
            let b1 = bind(start);
            format!("({column} = {b1})")
        }
        TemporalOp::MetBy => {
            let b2 = bind(end);
            format!("({column} = {b2})")
        }
        TemporalOp::Starts => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} = {b1} AND {column} < {b2})")
        }
        TemporalOp::StartedBy => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} = {b1} AND {column} > {b2})")
        }
        TemporalOp::Finishes => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} = {b2} AND {column} > {b1})")
        }
        TemporalOp::FinishedBy => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} = {b2} AND {column} < {b1})")
        }
        TemporalOp::Overlaps => {
            let (b1_lo, b1_hi, b2) = (bind(start), bind(start), bind(end));
            format!("({column} < {b1_lo} AND {b1_hi} < {column} AND {column} < {b2})")
        }
        TemporalOp::OverlappedBy => {
            let (b1, b2_lo, b2_hi) = (bind(start), bind(end), bind(end));
            format!("({b1} < {column} AND {column} < {b2_lo} AND {b2_hi} < {column})")
        }
        TemporalOp::Contains => {
            let (b1, b2) = (bind(start), bind(end));
            format!("({column} < {b1} AND {column} > {b2})")
        }
    }
}

fn spatial_op_name(op: SpatialOp) -> &'static str {
    match op {
        SpatialOp::Within => "S_WITHIN",
        SpatialOp::Contains => "S_CONTAINS",
        SpatialOp::Disjoint => "S_DISJOINT",
        SpatialOp::Touches => "S_TOUCHES",
        SpatialOp::Overlaps => "S_OVERLAPS",
        SpatialOp::Crosses => "S_CROSSES",
        SpatialOp::Equals => "S_EQUALS",
    }
}

/// Compiles a `tellurion_core::Filter` (`#33`) to a parenthesized boolean
/// SQL expression, pushing every literal onto `params` and referencing it
/// only by `?N` placeholder — never string-interpolated. Every property name
/// reaching here has already passed `tellurion_core::filter::validate`
/// against the collection's descriptor, but `quote_ident` still
/// whitelist-validates it again here as defense in depth, exactly as
/// `tellurion-postgis::sql`'s own doc describes for every identifier that
/// ends up in SQL text.
pub(crate) fn compile_filter(filter: &Filter, params: &mut Vec<SqlParam>) -> Result<String> {
    match filter {
        Filter::Compare {
            property,
            op,
            value,
        } => {
            let column = quote_ident(property)?;
            let op_sql = compare_op_sql(*op);
            params.push(literal_param(value));
            Ok(format!("({column} {op_sql} ?{})", params.len()))
        }
        Filter::IsNull { property, negated } => {
            let column = quote_ident(property)?;
            let not = if *negated { " NOT" } else { "" };
            Ok(format!("({column} IS{not} NULL)"))
        }
        // `ESCAPE '\'` makes SQLite's `LIKE` honor CQL2's own backslash
        // escape convention (SQLite's `LIKE` has no escape character at all
        // unless one is named explicitly, unlike Postgres's default). Every
        // connection this driver opens also sets `PRAGMA case_sensitive_like
        // = ON` (see `pool.rs`) so this stays case-sensitive, matching
        // Postgres's own default `LIKE` behavior CQL2's grammar assumes —
        // `CaseInsensitiveCompare` below is the only intentionally
        // case-folding comparison this compiler produces.
        Filter::Like {
            property,
            pattern,
            negated,
        } => {
            let column = quote_ident(property)?;
            let not = if *negated { " NOT" } else { "" };
            params.push(SqlParam::Text(pattern.clone()));
            Ok(format!(
                "({column}{not} LIKE ?{} ESCAPE '\\')",
                params.len()
            ))
        }
        Filter::Between {
            property,
            low,
            high,
            negated,
        } => {
            let column = quote_ident(property)?;
            let not = if *negated { "NOT " } else { "" };
            params.push(literal_param(low));
            let low_idx = params.len();
            params.push(literal_param(high));
            let high_idx = params.len();
            Ok(format!(
                "({column} {not}BETWEEN ?{low_idx} AND ?{high_idx})"
            ))
        }
        Filter::In {
            property,
            values,
            negated,
        } => {
            let column = quote_ident(property)?;
            let not = if *negated { "NOT " } else { "" };
            // `IN ()` is unreachable through either CQL2 parser but a
            // hand-built `Filter` could still carry an empty list — same
            // harmless-identity resolution `compile_bool_chain` uses for an
            // empty AND/OR.
            if values.is_empty() {
                return Ok(if *negated {
                    "1".to_string()
                } else {
                    "0".to_string()
                });
            }
            let mut placeholders = Vec::with_capacity(values.len());
            for value in values {
                params.push(literal_param(value));
                placeholders.push(format!("?{}", params.len()));
            }
            Ok(format!("({column} {not}IN ({}))", placeholders.join(", ")))
        }
        // `LOWER(...)` on both sides — SQLite's built-in `lower()` is
        // ASCII-only by default (no ICU/Unicode case-folding extension
        // loaded), a narrower fold than CQL2's full Unicode requirement;
        // same documented narrowing `tellurion-postgis::sql`'s own
        // `CaseInsensitiveCompare` arm takes for Postgres's locale-aware
        // (but still not full Unicode) `lower()`. That gap in every
        // filter-capable driver is why `case-insensitive-comparison` stays
        // out of `tellurion_core::filter::CQL2_CONFORMANCE_CLASSES`.
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
                "(LOWER({column}) {op_sql} LOWER(?{}))",
                params.len()
            ))
        }
        Filter::And(items) => compile_bool_chain(items, "AND", params),
        Filter::Or(items) => compile_bool_chain(items, "OR", params),
        Filter::Not(inner) => Ok(format!("(NOT {})", compile_filter(inner, params)?)),
        // Un-refused (see this module's own "bbox pushdown" doc for the
        // boundary): the rest of the exact-intersection work — the R*Tree
        // bbox clause and the in-process geometry test — is
        // `collect_intersects_check`'s and each builder function's job, and
        // this arm's only contribution to the SQL text is a cheap, always-
        // sound guard: no row with a NULL geometry can ever match
        // `S_INTERSECTS`, so ruling those out here costs nothing and can
        // never wrongly exclude a real match.
        Filter::Intersects { property, .. } => {
            let column = quote_ident(property)?;
            Ok(format!("({column} IS NOT NULL)"))
        }
        Filter::Spatial { op, .. } => Err(GeopackageError::SpatialPredicateUnsupported(
            spatial_op_name(*op),
        )),
        Filter::After { property, instant } => {
            let column = quote_ident(property)?;
            params.push(SqlParam::Text(instant.clone()));
            Ok(format!("({column} > ?{})", params.len()))
        }
        Filter::Before { property, instant } => {
            let column = quote_ident(property)?;
            params.push(SqlParam::Text(instant.clone()));
            Ok(format!("({column} < ?{})", params.len()))
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
                "({column} >= ?{start_idx} AND {column} <= ?{end_idx})"
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

/// Walks `filter` for the single `S_INTERSECTS` predicate (if any) this
/// driver's exact evaluator must apply after SQL returns candidate rows —
/// called alongside `compile_filter` by every builder function below, never
/// on its own (see `compile_filter`'s own `Intersects` arm doc for why the
/// two must always run together).
///
/// Refuses by name — rather than silently ignoring the problem and
/// answering with the coarse bbox test alone — in two cases:
///
/// - **More than one `S_INTERSECTS` node.** Each would need its own,
///   independently AND'd R*Tree bbox subquery to stay sound: merging two
///   different query geometries' bounding boxes into one rectangle before
///   testing a row's own bbox against it can wrongly exclude a row whose
///   bbox legitimately overlaps *each* query bbox individually, just not
///   their mutual intersection. Supporting that soundly needs a builder API
///   that ANDs an arbitrary number of R*Tree subqueries, which this v0.1
///   slice doesn't have; one `S_INTERSECTS` per filter is what every builder
///   function below implements.
/// - **A node beneath `OR`/`NOT`.** The R*Tree bbox clause this driver ANDs
///   into the SQL text is a sound *pre-filter*, valid only because the
///   overall row set it narrows is later refined by the same AND'd exact
///   test — safe exactly because `AND` only ever narrows. Under `OR`, a row
///   this predicate alone would match could still be excluded by the SQL
///   over-approximation of some *other* disjunct never getting a chance to
///   include it; under `NOT`, pruning bbox-disjoint rows would wrongly
///   exclude rows the negation should keep. Both need the whole filter
///   re-evaluated per row to answer correctly, which this slice's SQL-first,
///   exact-only-for-the-narrowed-candidate-set design doesn't do.
pub(crate) fn collect_intersects_check(filter: &Filter) -> Result<Option<IntersectsCheck>> {
    let mut found = None;
    collect_intersects_into(filter, false, &mut found)?;
    Ok(found)
}

fn collect_intersects_into(
    filter: &Filter,
    under_or_not: bool,
    found: &mut Option<IntersectsCheck>,
) -> Result<()> {
    match filter {
        Filter::Intersects { geometry, .. } => {
            if under_or_not {
                return Err(GeopackageError::IntersectsUnsupported(
                    "S_INTERSECTS beneath OR/NOT can't be pruned exactly by this driver's bbox-first evaluator".to_string(),
                ));
            }
            if found.is_some() {
                return Err(GeopackageError::IntersectsUnsupported(
                    "more than one S_INTERSECTS predicate in one filter isn't supported"
                        .to_string(),
                ));
            }
            *found = Some(intersects::literal_to_check(geometry)?);
            Ok(())
        }
        Filter::And(items) => items
            .iter()
            .try_for_each(|item| collect_intersects_into(item, under_or_not, found)),
        Filter::Or(items) => items
            .iter()
            .try_for_each(|item| collect_intersects_into(item, true, found)),
        Filter::Not(inner) => collect_intersects_into(inner, true, found),
        _ => Ok(()),
    }
}

pub(crate) struct ItemsPlan {
    pub(crate) sql: String,
    pub(crate) params: Vec<SqlParam>,
    /// `Some("SELECT COUNT(*) FROM ...")` only when the query carries no
    /// bbox/datetime/filter — an exact count of a filtered result would need
    /// the same unbounded scan the design forbids running twice; see
    /// `catalog::row_estimate`'s own doc for why this is an exact count
    /// rather than PostGIS's cheap `reltuples` estimate.
    pub(crate) count_sql: Option<String>,
    /// `Some` when `filter` carries an `S_INTERSECTS` predicate this SQL
    /// only narrowed to a bbox-overlapping candidate set — `driver.rs`'s
    /// `items_inner` must still exact-test (and, unlike every other filter
    /// shape, re-batch across multiple queries to fill a page — see its own
    /// doc) every row this plan's `sql` returns before it can honestly land
    /// on a page. `None` for every other query shape, including one with no
    /// `S_INTERSECTS` at all.
    pub(crate) intersects_check: Option<IntersectsCheck>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_items_plan(
    table: &str,
    pk: &str,
    geometry_column: &str,
    datetime_column: Option<&str>,
    collection_id: &str,
    limit: u32,
    token: Option<&str>,
    bbox: Option<[f64; 4]>,
    datetime: Option<(&Option<String>, &Option<String>)>,
    filter: Option<&Filter>,
) -> Result<ItemsPlan> {
    let table_ident = quote_ident(table)?;
    let pk_ident = quote_ident(pk)?;

    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<SqlParam> = Vec::new();

    if let Some(token) = token {
        let parsed: i64 = token
            .parse()
            .map_err(|_| GeopackageError::InvalidToken(token.to_string()))?;
        params.push(SqlParam::Int(parsed));
        clauses.push(format!("{pk_ident} > ?{}", params.len()));
    }

    if let Some(bbox) = bbox {
        clauses.push(bbox_clause(
            &pk_ident,
            table,
            geometry_column,
            bbox,
            &mut params,
        )?);
    }

    if let Some((start, end)) = datetime {
        let dt_col = datetime_column
            .ok_or_else(|| GeopackageError::NoDatetimeColumn(collection_id.to_string()))?;
        let dt_ident = quote_ident(dt_col)?;
        if let Some(start) = start {
            params.push(SqlParam::Text(start.clone()));
            clauses.push(format!("{dt_ident} >= ?{}", params.len()));
        }
        if let Some(end) = end {
            params.push(SqlParam::Text(end.clone()));
            clauses.push(format!("{dt_ident} <= ?{}", params.len()));
        }
    }

    let mut intersects_check = None;
    if let Some(filter) = filter {
        intersects_check = collect_intersects_check(filter)?;
        clauses.push(compile_filter(filter, &mut params)?);
    }
    // AND'd separately from `bbox` above rather than merged into one
    // rectangle — see `collect_intersects_check`'s own doc for why merging
    // two different query geometries' bboxes into one can wrongly exclude a
    // valid candidate.
    if let Some(check) = &intersects_check {
        clauses.push(bbox_clause(
            &pk_ident,
            table,
            geometry_column,
            check.needle_bbox,
            &mut params,
        )?);
    }

    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };

    // One extra row so the caller can detect a next page without a second
    // round trip; never OFFSET.
    params.push(SqlParam::Int(i64::from(limit).saturating_add(1)));
    let limit_idx = params.len();

    let sql = format!(
        "SELECT * FROM {table_ident} AS t{where_sql} ORDER BY {pk_ident} ASC LIMIT ?{limit_idx}"
    );

    let count_sql = if bbox.is_none() && datetime.is_none() && filter.is_none() {
        Some(format!("SELECT COUNT(*) FROM {table_ident}"))
    } else {
        None
    };

    Ok(ItemsPlan {
        sql,
        params,
        count_sql,
        intersects_check,
    })
}

/// A single-row-by-pk lookup gets no R*Tree bbox pushdown for its own
/// `S_INTERSECTS` clause (a `pk = ?1` lookup is already as narrow as SQL can
/// make it — an R*Tree subquery would prune nothing an index seek doesn't
/// already), but the caller still needs `Option<IntersectsCheck>` to
/// exact-test the one row this returns, honoring the same "excluded row
/// looks exactly like a missing one" contract `FeatureSource::item`'s own
/// doc states.
pub(crate) fn build_item_plan(
    table: &str,
    pk: &str,
    pk_value: i64,
    filter: Option<&Filter>,
) -> Result<(String, Vec<SqlParam>, Option<IntersectsCheck>)> {
    let table_ident = quote_ident(table)?;
    let pk_ident = quote_ident(pk)?;

    let mut params = vec![SqlParam::Int(pk_value)];
    let mut intersects_check = None;
    let filter_clause = match filter {
        Some(filter) => {
            intersects_check = collect_intersects_check(filter)?;
            format!(" AND {}", compile_filter(filter, &mut params)?)
        }
        None => String::new(),
    };
    let sql = format!("SELECT * FROM {table_ident} AS t WHERE {pk_ident} = ?1{filter_clause}");
    Ok((sql, params, intersects_check))
}

/// The tile lane's fetch: only `pk`/`geometry_column` are selected (this
/// driver embeds just the id as an MVT tag, mirroring
/// `tellurion-postgis::sql::build_mvt_plan`'s own `SELECT pk::text AS id,
/// geom` shape — see that function's own doc, its `ST_AsMVT` also never
/// includes any other column). `envelope` is the R*Tree bbox pushdown's own
/// query window, in the collection's own storage CRS units — `driver.rs`'s
/// `mvt_tile_inner` passes its EPSG:3857-meters tile bounds directly when
/// storage is 3857, or the same bounds reprojected into EPSG:4326 degrees
/// (`#89`) when storage is 4326, so this always compares like units against
/// the R*Tree's own stored coordinates regardless of which.
///
/// ## Why the read column and the indexed column are separate (`#104`)
///
/// `geometry_column` is the column whose bytes this tile reads — whichever
/// `geometry_variants` entry covers the tile's own zoom, else the base
/// column (`CollectionDecl::resolved_geometry_for_zoom`). `index_column` is
/// the column whose `rtree_<table>_<column>` index every bbox clause below
/// prunes against, and it is always the collection's *base* geometry column,
/// even when a variant is being read. Two reasons, both about not inventing
/// an obligation:
///
/// - A GeoPackage R*Tree index is optional per the spec (Annex L, an
///   extension a table opts into per geometry column) and only ever
///   provisioned for the base column by `tellurion-ingest geopackage
///   create-tables`. Naming `rtree_<table>_<variant>` here would turn a
///   config that boots clean (`Router::validate_catalog` checks the
///   variant's existence, SRID and geometry type — never an index) into a
///   `no such table` failure on every tile request.
/// - The prune stays sound: a variant is a *pre-generalized* rendering of
///   the very same feature (`GeometryVariantDecl`'s own contract), so the
///   base geometry's envelope — what the R*Tree stores — contains the
///   variant's, and a candidate the base envelope rejects could not have
///   been served from the variant either.
///
/// The two are the same column for every collection that declares no
/// variants, which is why the SQL text is byte-for-byte what it was before
/// `#104` reached this lane.
///
/// The tile lane needs no batching for its own `S_INTERSECTS` post-filter
/// the way `build_items_plan`'s caller does (`driver.rs`'s own doc): a tile
/// has no page to fill exactly, only a best-effort `cap` ceiling that was
/// already a heuristic before this predicate existed, so `driver.rs`'s
/// `mvt_tile_inner` exact-tests the one batch this returns and simply omits
/// whatever doesn't pass, same as it already does for an empty geometry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_tile_plan(
    table: &str,
    pk: &str,
    geometry_column: &str,
    index_column: &str,
    tile_properties: &[String],
    envelope: [f64; 4],
    cap: u64,
    filter: Option<&Filter>,
) -> Result<(String, Vec<SqlParam>, Option<IntersectsCheck>)> {
    let table_ident = quote_ident(table)?;
    let pk_ident = quote_ident(pk)?;
    let geom_ident = quote_ident(geometry_column)?;
    // `#85`: each allowlisted column, whitelist-quoted, appended to the
    // SELECT list in declaration order — `driver.rs`'s own row reader relies
    // on that same order to read each value back out by position (columns
    // 0/1 are always `pk`/`geom`; every property after that lands at index
    // `2 + i` for `tile_properties[i]`). Every entry has already been
    // reconciled against this collection's derived attribute schema at
    // boot-or-first-touch (`descriptor::reconcile_tile_properties`), so this
    // never has to re-check existence — it only has to quote and select.
    let mut property_idents = Vec::with_capacity(tile_properties.len());
    for property in tile_properties {
        property_idents.push(quote_ident(property)?);
    }
    let property_columns: String = property_idents
        .iter()
        .map(|ident| format!(", {ident}"))
        .collect();

    let mut params = Vec::new();
    let envelope_bbox = bbox_clause(&pk_ident, table, index_column, envelope, &mut params)?;
    let mut intersects_check = None;
    let filter_clause = match filter {
        Some(filter) => {
            intersects_check = collect_intersects_check(filter)?;
            format!(" AND {}", compile_filter(filter, &mut params)?)
        }
        None => String::new(),
    };
    // AND'd separately from `envelope_bbox` above — see
    // `collect_intersects_check`'s own doc for why merging two different
    // query geometries' bboxes into one rectangle can wrongly exclude a
    // valid candidate.
    let needle_clause = match &intersects_check {
        Some(check) => format!(
            " AND {}",
            bbox_clause(
                &pk_ident,
                table,
                index_column,
                check.needle_bbox,
                &mut params
            )?
        ),
        None => String::new(),
    };
    params.push(SqlParam::Int(i64::try_from(cap).unwrap_or(i64::MAX)));
    let limit_idx = params.len();

    let sql = format!(
        "SELECT {pk_ident}, {geom_ident}{property_columns} FROM {table_ident} WHERE {envelope_bbox}{filter_clause}{needle_clause} LIMIT ?{limit_idx}"
    );
    Ok((sql, params, intersects_check))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tellurion_core::{GeometryLiteral, WktGeometry};

    #[test]
    fn items_plan_with_no_filters() {
        let plan = build_items_plan(
            "demo", "id", "geom", None, "demo", 10, None, None, None, None,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            "SELECT * FROM \"demo\" AS t ORDER BY \"id\" ASC LIMIT ?1"
        );
        assert_eq!(plan.params, vec![SqlParam::Int(11)]);
        assert_eq!(
            plan.count_sql.as_deref(),
            Some("SELECT COUNT(*) FROM \"demo\"")
        );
    }

    #[test]
    fn items_plan_with_token() {
        let plan = build_items_plan(
            "demo",
            "id",
            "geom",
            None,
            "demo",
            10,
            Some("5"),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            "SELECT * FROM \"demo\" AS t WHERE \"id\" > ?1 ORDER BY \"id\" ASC LIMIT ?2"
        );
        assert_eq!(plan.params, vec![SqlParam::Int(5), SqlParam::Int(11)]);
    }

    #[test]
    fn items_plan_rejects_unparsable_token() {
        assert!(matches!(
            build_items_plan(
                "demo",
                "id",
                "geom",
                None,
                "demo",
                10,
                Some("nope"),
                None,
                None,
                None
            ),
            Err(GeopackageError::InvalidToken(_))
        ));
    }

    #[test]
    fn items_plan_with_bbox_uses_the_rtree_subquery_and_disables_the_count() {
        let plan = build_items_plan(
            "demo",
            "id",
            "geom",
            None,
            "demo",
            10,
            None,
            Some([1.0, 2.0, 3.0, 4.0]),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            "SELECT * FROM \"demo\" AS t WHERE \"id\" IN (SELECT id FROM \"rtree_demo_geom\" WHERE maxx >= ?1 AND minx <= ?2 AND maxy >= ?3 AND miny <= ?4) ORDER BY \"id\" ASC LIMIT ?5"
        );
        assert!(plan.count_sql.is_none());
    }

    #[test]
    fn items_plan_with_intersects_filter_ands_the_needle_bbox_and_reports_the_check() {
        let filter = Filter::Intersects {
            property: "geom".to_string(),
            geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
        };
        let plan = build_items_plan(
            "demo",
            "id",
            "geom",
            None,
            "demo",
            10,
            None,
            None,
            None,
            Some(&filter),
        )
        .unwrap();
        assert!(
            plan.sql.contains("(\"geom\" IS NOT NULL)"),
            "sql was: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("rtree_demo_geom"),
            "sql was: {}",
            plan.sql
        );
        assert!(plan.count_sql.is_none());
        assert_eq!(
            plan.intersects_check.unwrap().needle_bbox,
            [1.0, 2.0, 3.0, 4.0]
        );
    }

    #[test]
    fn items_plan_rejects_datetime_without_a_configured_column() {
        let start = Some("2020-01-01T00:00:00Z".to_string());
        assert!(matches!(
            build_items_plan(
                "demo",
                "id",
                "geom",
                None,
                "demo",
                10,
                None,
                None,
                Some((&start, &None)),
                None
            ),
            Err(GeopackageError::NoDatetimeColumn(_))
        ));
    }

    #[test]
    fn items_plan_with_datetime_range() {
        let start = Some("2020-01-01T00:00:00Z".to_string());
        let end = Some("2020-12-31T00:00:00Z".to_string());
        let plan = build_items_plan(
            "demo",
            "id",
            "geom",
            Some("observed_at"),
            "demo",
            10,
            None,
            None,
            Some((&start, &end)),
            None,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            "SELECT * FROM \"demo\" AS t WHERE \"observed_at\" >= ?1 AND \"observed_at\" <= ?2 ORDER BY \"id\" ASC LIMIT ?3"
        );
    }

    #[test]
    fn item_plan_by_pk() {
        let (sql, params, check) = build_item_plan("demo", "id", 42, None).unwrap();
        assert_eq!(sql, "SELECT * FROM \"demo\" AS t WHERE \"id\" = ?1");
        assert_eq!(params, vec![SqlParam::Int(42)]);
        assert!(check.is_none());
    }

    #[test]
    fn item_plan_with_filter_ands_the_clause() {
        let filter = tellurion_core::filter::parse_text("name = 'acme'").unwrap();
        let (sql, params, check) = build_item_plan("demo", "id", 42, Some(&filter)).unwrap();
        assert_eq!(
            sql,
            "SELECT * FROM \"demo\" AS t WHERE \"id\" = ?1 AND (\"name\" = ?2)"
        );
        assert_eq!(
            params,
            vec![SqlParam::Int(42), SqlParam::Text("acme".to_string())]
        );
        assert!(check.is_none());
    }

    #[test]
    fn compile_filter_text_equality() {
        let filter = Filter::Compare {
            property: "name".to_string(),
            op: CompareOp::Eq,
            value: Literal::Text("a".to_string()),
        };
        let mut params = Vec::new();
        let sql = compile_filter(&filter, &mut params).unwrap();
        assert_eq!(sql, "(\"name\" = ?1)");
        assert_eq!(params, vec![SqlParam::Text("a".to_string())]);
    }

    #[test]
    fn compile_filter_like_appends_escape_clause() {
        let filter = Filter::Like {
            property: "name".to_string(),
            pattern: "a%".to_string(),
            negated: false,
        };
        let mut params = Vec::new();
        let sql = compile_filter(&filter, &mut params).unwrap();
        assert_eq!(sql, "(\"name\" LIKE ?1 ESCAPE '\\')");
    }

    #[test]
    fn compile_filter_case_insensitive_uses_lower() {
        let filter = Filter::CaseInsensitiveCompare {
            property: "name".to_string(),
            op: CaseInsensitiveCompareOp::Eq,
            value: "Acme".to_string(),
        };
        let mut params = Vec::new();
        let sql = compile_filter(&filter, &mut params).unwrap();
        assert_eq!(sql, "(LOWER(\"name\") = LOWER(?1))");
    }

    #[test]
    fn compile_filter_intersects_compiles_a_not_null_guard() {
        let filter = Filter::Intersects {
            property: "geom".to_string(),
            geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
        };
        let mut params = Vec::new();
        let sql = compile_filter(&filter, &mut params).unwrap();
        assert_eq!(sql, "(\"geom\" IS NOT NULL)");
        assert!(params.is_empty());
    }

    #[test]
    fn collect_intersects_check_accepts_a_bare_intersects_filter() {
        let filter = Filter::Intersects {
            property: "geom".to_string(),
            geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
        };
        let check = collect_intersects_check(&filter).unwrap();
        assert_eq!(check.unwrap().needle_bbox, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn collect_intersects_check_accepts_intersects_and_ed_with_other_conditions() {
        let filter = Filter::And(vec![
            Filter::Compare {
                property: "population".to_string(),
                op: CompareOp::Gt,
                value: Literal::Number(0.0),
            },
            Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
            },
        ]);
        assert!(collect_intersects_check(&filter).unwrap().is_some());
    }

    #[test]
    fn collect_intersects_check_refuses_intersects_beneath_or_by_name() {
        let filter = Filter::Or(vec![
            Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
            },
            Filter::Compare {
                property: "population".to_string(),
                op: CompareOp::Gt,
                value: Literal::Number(0.0),
            },
        ]);
        assert!(matches!(
            collect_intersects_check(&filter),
            Err(GeopackageError::IntersectsUnsupported(_))
        ));
    }

    #[test]
    fn collect_intersects_check_refuses_intersects_beneath_not_by_name() {
        let filter = Filter::Not(Box::new(Filter::Intersects {
            property: "geom".to_string(),
            geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
        }));
        assert!(matches!(
            collect_intersects_check(&filter),
            Err(GeopackageError::IntersectsUnsupported(_))
        ));
    }

    #[test]
    fn collect_intersects_check_refuses_a_second_intersects_predicate_by_name() {
        let filter = Filter::And(vec![
            Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([1.0, 2.0, 3.0, 4.0]),
            },
            Filter::Intersects {
                property: "geom".to_string(),
                geometry: GeometryLiteral::Bbox([5.0, 6.0, 7.0, 8.0]),
            },
        ]);
        assert!(matches!(
            collect_intersects_check(&filter),
            Err(GeopackageError::IntersectsUnsupported(_))
        ));
    }

    #[test]
    fn collect_intersects_check_refuses_a_3d_geojson_literal_by_name() {
        let filter = Filter::Intersects {
            property: "geom".to_string(),
            geometry: GeometryLiteral::GeoJson(
                serde_json::json!({"type": "Point", "coordinates": [1.0, 2.0, 3.0]}),
            ),
        };
        assert!(matches!(
            collect_intersects_check(&filter),
            Err(GeopackageError::IntersectsUnsupported(_))
        ));
    }

    #[test]
    fn compile_filter_spatial_within_is_refused_by_name() {
        let filter = Filter::Spatial {
            property: "geom".to_string(),
            op: SpatialOp::Within,
            geometry: GeometryLiteral::Wkt(WktGeometry::Point([1.0, 2.0])),
        };
        let mut params = Vec::new();
        assert!(matches!(
            compile_filter(&filter, &mut params),
            Err(GeopackageError::SpatialPredicateUnsupported("S_WITHIN"))
        ));
    }

    #[test]
    fn compile_filter_and_or_not() {
        let filter = Filter::And(vec![
            Filter::Compare {
                property: "population".to_string(),
                op: CompareOp::Gt,
                value: Literal::Number(0.0),
            },
            Filter::Not(Box::new(Filter::IsNull {
                property: "name".to_string(),
                negated: false,
            })),
        ]);
        let mut params = Vec::new();
        let sql = compile_filter(&filter, &mut params).unwrap();
        assert_eq!(sql, "((\"population\" > ?1) AND (NOT (\"name\" IS NULL)))");
    }

    #[test]
    fn tile_plan_shape() {
        let (sql, params, check) = build_tile_plan(
            "demo",
            "id",
            "geom",
            "geom",
            &[],
            [1.0, 2.0, 3.0, 4.0],
            2000,
            None,
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT \"id\", \"geom\" FROM \"demo\" WHERE \"id\" IN (SELECT id FROM \"rtree_demo_geom\" WHERE maxx >= ?1 AND minx <= ?2 AND maxy >= ?3 AND miny <= ?4) LIMIT ?5"
        );
        assert_eq!(
            params,
            vec![
                SqlParam::Real(1.0),
                SqlParam::Real(3.0),
                SqlParam::Real(2.0),
                SqlParam::Real(4.0),
                SqlParam::Int(2000),
            ]
        );
        assert!(check.is_none());
    }

    /// `#104`: a variant column is read by the SELECT list, while the bbox
    /// pushdown keeps naming the *base* column's R*Tree — see
    /// `build_tile_plan`'s own doc for why the two are allowed to differ.
    /// Only the projected column changes; every param stays exactly where it
    /// was.
    #[test]
    fn tile_plan_reads_the_variant_column_but_prunes_on_the_base_columns_rtree() {
        let (sql, params, _check) = build_tile_plan(
            "demo",
            "id",
            "geom_z6",
            "geom",
            &[],
            [1.0, 2.0, 3.0, 4.0],
            2000,
            None,
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT \"id\", \"geom_z6\" FROM \"demo\" WHERE \"id\" IN (SELECT id FROM \"rtree_demo_geom\" WHERE maxx >= ?1 AND minx <= ?2 AND maxy >= ?3 AND miny <= ?4) LIMIT ?5"
        );
        assert!(
            !sql.contains("rtree_demo_geom_z6"),
            "the variant column's own R*Tree is never assumed to exist: {sql}"
        );
        assert_eq!(
            params,
            vec![
                SqlParam::Real(1.0),
                SqlParam::Real(3.0),
                SqlParam::Real(2.0),
                SqlParam::Real(4.0),
                SqlParam::Int(2000),
            ]
        );
    }

    /// The needle's own R*Tree subquery (`S_INTERSECTS`) follows the same
    /// rule as the envelope's: base column's index, even when a variant is
    /// the column being read.
    #[test]
    fn tile_plan_needle_bbox_also_prunes_on_the_base_columns_rtree_under_a_variant() {
        let filter = Filter::Intersects {
            property: "geom".to_string(),
            geometry: GeometryLiteral::Bbox([10.0, 20.0, 30.0, 40.0]),
        };
        let (sql, _params, check) = build_tile_plan(
            "demo",
            "id",
            "geom_z6",
            "geom",
            &[],
            [1.0, 2.0, 3.0, 4.0],
            2000,
            Some(&filter),
        )
        .unwrap();
        assert_eq!(sql.matches("\"rtree_demo_geom\"").count(), 2);
        assert!(!sql.contains("rtree_demo_geom_z6"), "sql was: {sql}");
        assert!(check.is_some());
    }

    /// A variant column name that isn't a plain identifier is refused by the
    /// same whitelist every other column name goes through — never
    /// interpolated raw into the SELECT list.
    #[test]
    fn tile_plan_refuses_a_non_identifier_variant_column() {
        assert!(build_tile_plan(
            "demo",
            "id",
            "geom-z6",
            "geom",
            &[],
            [1.0, 2.0, 3.0, 4.0],
            2000,
            None,
        )
        .is_err());
    }

    /// `#85`: an allowlisted `tile_properties` set widens the SELECT list
    /// with one whitelist-quoted column per entry, in declaration order,
    /// after `pk`/`geom` — `driver.rs`'s own row reader relies on that exact
    /// order (see `build_tile_plan`'s own doc).
    #[test]
    fn tile_plan_widens_the_select_list_with_the_allowlisted_properties() {
        let tile_properties = vec!["name".to_string(), "pop".to_string()];
        let (sql, _params, _check) = build_tile_plan(
            "demo",
            "id",
            "geom",
            "geom",
            &tile_properties,
            [1.0, 2.0, 3.0, 4.0],
            2000,
            None,
        )
        .unwrap();
        assert!(
            sql.starts_with("SELECT \"id\", \"geom\", \"name\", \"pop\" FROM"),
            "sql was: {sql}"
        );
    }

    #[test]
    fn tile_plan_with_filter_ands_the_clause_after_the_bbox() {
        let filter = tellurion_core::filter::parse_text("name = 'acme'").unwrap();
        let (sql, params, check) = build_tile_plan(
            "demo",
            "id",
            "geom",
            "geom",
            &[],
            [1.0, 2.0, 3.0, 4.0],
            2000,
            Some(&filter),
        )
        .unwrap();
        assert!(sql.contains("AND (\"name\" = ?5)"), "sql was: {sql}");
        assert_eq!(params.len(), 6);
        assert!(check.is_none());
    }

    #[test]
    fn tile_plan_with_intersects_filter_ands_a_second_bbox_clause_for_the_needle() {
        let filter = Filter::Intersects {
            property: "geom".to_string(),
            geometry: GeometryLiteral::Bbox([10.0, 20.0, 30.0, 40.0]),
        };
        let (sql, params, check) = build_tile_plan(
            "demo",
            "id",
            "geom",
            "geom",
            &[],
            [1.0, 2.0, 3.0, 4.0],
            2000,
            Some(&filter),
        )
        .unwrap();
        // Two independent R*Tree subqueries AND'd — the tile envelope's own
        // and the needle's own — never one merged rectangle (see
        // `collect_intersects_check`'s own doc for why merging would be
        // unsound).
        assert_eq!(sql.matches("rtree_demo_geom").count(), 2);
        assert_eq!(check.unwrap().needle_bbox, [10.0, 20.0, 30.0, 40.0]);
        // 4 params for the envelope bbox, 0 for the `IS NOT NULL` guard
        // `compile_filter` compiles `S_INTERSECTS` to, 4 for the needle
        // bbox, 1 for `cap`.
        assert_eq!(params.len(), 9);
    }
}
