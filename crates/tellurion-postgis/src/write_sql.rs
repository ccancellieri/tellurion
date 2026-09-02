//! Pure SQL builders for the write lane (`#25`, the transactional-outbox
//! design): a `CollectionDecl` + mutation in, SQL text + typed params out —
//! no I/O, the same discipline `sql.rs` follows for reads. Every identifier
//! is whitelist-quoted (`ident.rs`); every value is bound as a parameter,
//! never interpolated.
//!
//! ## The per-collection outbox table
//!
//! `outbox_table_name` derives `"<table>_outbox"` from a collection's
//! physical table name — the per-collection obligation log the design doc's
//! section 2 (invariant 2) calls for, never a global cross-tenant table.
//! Kept in sync by hand with `tellurion-ingest::outbox`'s own DDL, the same
//! arrangement `tellurion-postgis::registry`'s own doc describes for the
//! relational registry tables: the two crates never depend on each other, so
//! the name is a documented convention rather than a shared constant.
//!
//! ## Binding a scalar property value without knowing its exact wire type
//!
//! A property's real column type (`int4` vs `int8`, `text`, `timestamptz`,
//! ...) isn't always known in a Rust-binding-safe sense — see `sql.rs`'s own
//! note on `ST_TileEnvelope`'s `int4` arguments for a concrete case where
//! guessing wrong makes `tokio_postgres` reject the bind client-side
//! ("WrongType") before the query ever reaches the server. This module
//! sidesteps that the same way `sql.rs`'s own datetime filter already does
//! (`$N::text::timestamptz`): every scalar is bound as `SqlParam::Text` and
//! coerced with an explicit `$N::text::<pg_type>` cast in the SQL text.
//! Postgres resolves a parameter cast straight to `text` first (matching the
//! client's own `text`-typed bind), then applies the second cast using that
//! type's ordinary text-input parser — exactly as if the value had arrived
//! as a plain SQL string literal.

use std::collections::HashMap;

use serde_json::{Map, Value};
use tellurion_core::{
    crs::{crs84_literals_need_transform, is_lat_lon_order},
    locking::RowVersion,
    CollectionDecl, IdType, ObligationExtent, PropertyType, RequestedCrs,
};

use crate::error::{PostgisError, Result};
use crate::ident::quote_ident;
use crate::sql::{pk_sql_cast, PkValue, SqlParam};

/// PostgreSQL's own per-row version witness (`#150`,
/// [`tellurion_core::locking::RowVersion`]): the id of the transaction that
/// last inserted or updated the row. Every `INSERT`/`UPDATE` touching a row
/// necessarily writes a new one, so comparing it answers exactly "has this
/// row changed since I read it?" — and the database answers it in the same
/// statement as the write, which is what a content hash computed in Rust can
/// never be.
///
/// Rendered `::text` on both sides rather than compared as a bare `xid`:
/// `xid` has no equality operator against an ordinary bound parameter, and
/// this comparison always sits on an already-pk-narrowed single row, so
/// nothing is lost by comparing the decimal text. A user table can never
/// shadow the name — PostgreSQL reserves its system column names and rejects
/// a `CREATE TABLE` that declares one.
const ROW_VERSION_EXPR: &str = "xmin::text";

/// The `(predicate, parameter suffix)` pair every pk equality in this module
/// binds through — factored out of `build_delete_plan` so the conditional
/// `UPDATE`/`DELETE` and the witness `SELECT` narrow to a row exactly the way
/// the unconditional path already did, rather than three hand-written
/// near-copies. Keeping an `Integer` pk column bare lets PostgreSQL use its
/// btree index even when the physical type is `int4`; the `::bigint` on the
/// PARAMETER side is what keeps the `i64` this crate always binds acceptable
/// against such a column (see `sql.rs`'s own note).
fn pk_equality(collection: &CollectionDecl) -> Result<(String, &'static str)> {
    let pk = quote_ident(collection.resolved_pk())?;
    Ok(match collection.id_type {
        IdType::Integer => (pk, "::bigint"),
        IdType::Uuid | IdType::Text => {
            let cast = pk_sql_cast(collection.id_type);
            (format!("{pk}::{cast}"), "")
        }
    })
}

/// `SELECT xmin::text FROM <table> WHERE <pk> = $1` — the capture half of
/// `WriteSink::row_version` (`#150`). No row matched means the feature does
/// not exist, which the caller reports as `Ok(None)` rather than as a
/// witness.
pub(crate) fn build_row_version_plan(
    collection: &CollectionDecl,
    pk_value: PkValue,
) -> Result<(String, Vec<SqlParam>)> {
    let table = quote_ident(collection.resolved_table())?;
    let (pk_predicate, param_suffix) = pk_equality(collection)?;
    let sql =
        format!("SELECT {ROW_VERSION_EXPR} FROM {table} WHERE {pk_predicate} = $1{param_suffix}");
    Ok((sql, vec![pk_value.as_sql_param()]))
}

/// `"<table>_outbox"` — see this module's own doc for why the name is a
/// hand-kept convention rather than a shared constant.
pub(crate) fn outbox_table_name(table: &str) -> String {
    format!("{table}_outbox")
}

/// The outbox column `#141`/`#142` added — kept in sync by hand with
/// `tellurion-ingest::outbox`'s own `ALTER TABLE ... ADD COLUMN IF NOT
/// EXISTS`, the same arrangement the rest of this module's outbox SQL
/// already follows.
pub(crate) const OUTBOX_EXTENT_COLUMN: &str = "extent_crs84";

/// How many `double precision` columns a CRS84 extent occupies in a
/// `RETURNING`/`SELECT` list ([`crs84_extent_select_list`]).
pub(crate) const CRS84_EXTENT_COLUMNS: usize = 4;

/// The four `double precision` expressions — `minlon, minlat, maxlon,
/// maxlat` in that order — giving `geom_column`'s extent **in CRS84**
/// (`#142`).
///
/// Whether a transform is emitted at all is
/// [`crs84_literals_need_transform`]'s decision, not a fourth
/// CRS-comparison rule invented here (`#227`/`#247` ask that same question
/// on the filter-input and response-output lanes; this is the identical
/// question about the identical pair of CRSs, so it is the identical
/// predicate). Two consequences worth stating outright:
///
/// - A collection whose storage is CRS84-equivalent (SRID 4326, or unknown)
///   gets `ST_XMin("geom"), ...` — no `ST_Transform` anywhere, the extent
///   PostGIS already holds, and byte-for-byte the numbers such a deployment
///   was already invalidating by before this issue.
/// - A projected collection gets `ST_XMin(ST_Transform("geom", 4326)), ...`
///   — a real reprojection, done by the storage that knows the storage CRS,
///   so no consumer has to (and `tellurion-core` has no projection
///   dependency to do it with).
///
/// A `NULL` geometry yields four `NULL`s, which the driver reads back as
/// "this feature has no extent" — distinct from "no extent was recorded",
/// which is the absence of the whole column value (see
/// [`ObligationExtent`]).
///
/// Note the axis order: PostGIS stores and returns 4326 geometry
/// longitude-first internally regardless of EPSG:4326's authority
/// latitude-first axis order (see `tellurion_core::crs`'s "Axis order"
/// section), so `ST_XMin` is a longitude on every branch here. That is
/// exactly what makes this expression trustworthy where the obligation's own
/// payload is not: a write submitted under `Content-Crs: .../EPSG/0/4326`
/// arrives latitude-first and is flipped on the way in, and no amount of
/// inspecting the payload afterwards could tell you so.
fn crs84_extent_select_list(geom_column: &str, storage_srid: Option<i32>) -> String {
    let geom = if crs84_literals_need_transform(storage_srid) {
        format!("ST_Transform({geom_column}, 4326)")
    } else {
        geom_column.to_string()
    };
    format!("ST_XMin({geom}), ST_YMin({geom}), ST_XMax({geom}), ST_YMax({geom})")
}

/// `SELECT <crs84 extent> FROM <table> WHERE <pk> = $1` — the "where was
/// this feature BEFORE the mutation" half of `#141`, run inside the same
/// transaction as (and immediately before) the data statement, so no
/// concurrent writer can move the row between the two.
///
/// Only the upsert lane needs this: a `DELETE`'s own `RETURNING` already
/// hands back the row it removed, and a server-assigned `create` has no
/// prior row by construction. No rows back means the feature did not exist
/// yet — a recorded answer (`prior: None`), not an unknown one.
pub(crate) fn build_prior_extent_plan(
    collection: &CollectionDecl,
    pk_value: PkValue,
) -> Result<(String, Vec<SqlParam>)> {
    let table = quote_ident(collection.resolved_table())?;
    let geom = quote_ident(collection.resolved_geometry())?;
    let (pk_predicate, param_suffix) = pk_equality(collection)?;
    let extent = crs84_extent_select_list(&geom, collection.srid);
    let sql = format!("SELECT {extent} FROM {table} WHERE {pk_predicate} = $1{param_suffix}");
    Ok((sql, vec![pk_value.as_sql_param()]))
}

fn pg_cast(type_: PropertyType) -> &'static str {
    match type_ {
        PropertyType::String => "text",
        PropertyType::Integer => "bigint",
        PropertyType::Number => "double precision",
        PropertyType::Boolean => "boolean",
        PropertyType::Date => "date",
        PropertyType::DateTime => "timestamptz",
    }
}

/// A scalar JSON value's plain-text form for the `$N::text::<cast>` bind
/// idiom (see this module's own doc). `Ok(None)` for `Value::Null` — the
/// caller writes a literal `NULL` instead of binding one; `Err` for an
/// array/object, outside this write path's flat, one-column-per-property
/// model.
fn scalar_as_text(key: &str, value: &Value) -> Result<Option<String>> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        Value::Number(n) => Ok(Some(n.to_string())),
        Value::Bool(b) => Ok(Some(b.to_string())),
        Value::Array(_) | Value::Object(_) => {
            Err(PostgisError::UnsupportedPropertyValue(key.to_string()))
        }
    }
}

/// The `ST_SetSRID(ST_GeomFromGeoJSON($n), <srid>)`-shaped SQL fragment
/// (optionally `ST_Transform`/`ST_FlipCoordinates`-wrapped) that turns a
/// bound GeoJSON geometry parameter at placeholder `$n` into the value an
/// `INSERT` should write — the write-lane mirror of `sql.rs`'s own
/// `reprojected_geom_expr`, built from the identical
/// `ST_Transform`/`ST_SetSRID`/`ST_FlipCoordinates` primitives that function
/// already uses for the opposite (storage-to-response) direction (OGC API
/// Features Part 4, Requirements 40-42: `/req/features/content-crs-header`,
/// `/req/features/default-crs`, `/req/features/crs-other-crs`):
///
/// - `RequestedCrs::Omitted`/`::Crs84`: an absent or explicit-CRS84
///   `Content-Crs` means "interpret the request body as CRS84" (Requirements
///   39/41) — the coordinates genuinely are CRS84, so this must convert them
///   into the collection's own storage representation, not merely relabel
///   them:
///   - `storage_srid` unknown or already `4326`: `ST_SetSRID(ST_GeomFromGeoJSON($n),
///     4326)`, byte-for-byte the SQL this module always produced — a no-op
///     tag is the correct, cheapest-possible conversion when the storage CRS
///     and CRS84 share the same (PostGIS-internal) coordinate
///     representation.
///   - any other known `storage_srid`: `ST_Transform(ST_SetSRID(ST_GeomFromGeoJSON($n),
///     4326), <srid>)` — a real reprojection, closing the silent-corruption
///     bug this lane exists to fix (a collection stored in a non-4326 SRID
///     was previously having its CRS84 input tagged 4326 instead of
///     converted). `ST_FlipCoordinates` additionally wraps the transformed
///     output when `storage_srid` is itself authority-ordered
///     latitude-before-longitude (`crs::is_lat_lon_order`), mirroring
///     `reprojected_geom_expr`'s and this function's own `RequestedCrs::
///     Storage` arm's use of the same primitive for the same reason. Narrowly
///     unreachable today — `is_lat_lon_order` only ever recognizes SRID 4326,
///     which the arm above already short-circuits — but kept so this stays
///     correct if that recognition ever widens to another lat-lon-ordered
///     authority.
/// - `RequestedCrs::Storage`: the caller (`tellurion-features`'s write
///   handlers) only ever resolves this when the declared CRS is this
///   collection's own storage CRS (`crs::resolve`'s own contract) *and* the
///   driver's `WriteSink::crs_capable` is `true` — so the incoming
///   coordinates are already expressed in that CRS's own representation and
///   need no `ST_Transform`, only the correct SRID tag.
///   `ST_FlipCoordinates` additionally undoes the axis-order swap the same
///   way `reprojected_geom_expr`'s `RequestedCrs::Storage` arm applies it on
///   read, exactly when `storage_srid` is authority-ordered
///   latitude-before-longitude (`crs::is_lat_lon_order`, narrowly SRID 4326
///   — see that function's own doc for why).
fn input_geom_expr(
    placeholder: usize,
    requested_crs: RequestedCrs,
    storage_srid: Option<i32>,
) -> String {
    match requested_crs {
        RequestedCrs::Omitted | RequestedCrs::Crs84 => match storage_srid {
            None | Some(4326) => {
                format!("ST_SetSRID(ST_GeomFromGeoJSON(${placeholder}), 4326)")
            }
            Some(srid) if is_lat_lon_order(srid) => format!(
                "ST_FlipCoordinates(ST_Transform(ST_SetSRID(ST_GeomFromGeoJSON(${placeholder}), 4326), {srid}))"
            ),
            Some(srid) => format!(
                "ST_Transform(ST_SetSRID(ST_GeomFromGeoJSON(${placeholder}), 4326), {srid})"
            ),
        },
        RequestedCrs::Storage => {
            // `storage_srid` is always `Some` here in practice — `crs::
            // resolve` can only ever produce `RequestedCrs::Storage` when a
            // storage SRID is known (see its own doc) — but this falls back
            // to the CRS84 SQL rather than panicking on the unreachable
            // `None` case, the same defensive-fallback idiom
            // `crs::content_crs_uri` already uses.
            match storage_srid {
                Some(srid) if is_lat_lon_order(srid) => {
                    format!("ST_SetSRID(ST_FlipCoordinates(ST_GeomFromGeoJSON(${placeholder})), {srid})")
                }
                Some(srid) => format!("ST_SetSRID(ST_GeomFromGeoJSON(${placeholder}), {srid})"),
                None => format!("ST_SetSRID(ST_GeomFromGeoJSON(${placeholder}), 4326)"),
            }
        }
    }
}

pub(crate) struct UpsertPlan {
    pub(crate) sql: String,
    pub(crate) params: Vec<SqlParam>,
}

/// Builds the data mutation half of `WriteSink::apply`/`apply_conditional`
/// for one feature (the outbox insert is a separate statement,
/// `build_outbox_insert_plan`, committed in the same transaction by the
/// caller). `types` resolves every incoming property's [`PropertyType`]; the
/// caller (`driver.rs`'s `resolve_property_types`) builds it from the
/// collection's declared schema where one exists, and a live catalog lookup
/// for anything undeclared — this function never touches the database
/// itself. A property with no entry in `types` fails with
/// `UnwritableProperty` before any SQL is built.
///
/// `expected` is the `#150` optimistic-locking guard, and it changes the
/// SHAPE of the statement rather than adding a clause to it:
///
/// - `None` — byte-for-byte the `INSERT ... ON CONFLICT (pk) DO UPDATE` this
///   function always emitted. A `PUT` against an id that does not exist yet
///   is an upsert (`WriteSink::apply`'s own contract), so the insert arm has
///   to be there.
/// - `Some(version)` — a plain `UPDATE ... WHERE <pk> = $1 AND xmin::text =
///   $N`. The caller only ever passes `Some` once it has evaluated a
///   precondition against an EXISTING representation, so there is nothing
///   left for an insert arm to do — and keeping one would reintroduce the
///   very bug this closes: a concurrent `DELETE` between the check and the
///   apply would let `ON CONFLICT` fall through to a fresh insert, silently
///   resurrecting a row the caller's `If-Match` was never matched against.
///   A row whose `xmin` moved (or which is gone entirely) simply matches
///   zero rows, and the caller reads that as "somebody else got there
///   first".
///
/// The guard is a predicate the DATABASE evaluates as part of the write, so
/// under `READ COMMITTED` a concurrent writer that commits first makes
/// PostgreSQL re-evaluate this `WHERE` against the row version that writer
/// left behind — which no longer carries `expected`. That is the whole
/// point: the comparison and the write are one indivisible step, with no
/// window between them for anyone to slip through.
pub(crate) fn build_upsert_plan(
    collection: &CollectionDecl,
    pk_value: PkValue,
    geometry: Option<&Value>,
    properties: &Map<String, Value>,
    types: &HashMap<String, PropertyType>,
    requested_crs: RequestedCrs,
    expected: Option<&RowVersion>,
) -> Result<UpsertPlan> {
    let table = quote_ident(collection.resolved_table())?;
    let pk = quote_ident(collection.resolved_pk())?;
    let geom = quote_ident(collection.resolved_geometry())?;

    // `$1` is the pk in either shape — the `INSERT`'s first value, or the
    // `UPDATE`'s `WHERE` term — so every value expression below numbers from
    // `$2` regardless of which statement is emitted.
    let mut columns = vec![geom.clone()];
    let mut values = Vec::new();
    let mut params = vec![pk_value.as_sql_param()];

    match geometry {
        Some(value) if !value.is_null() => {
            params.push(SqlParam::Text(value.to_string()));
            values.push(input_geom_expr(
                params.len(),
                requested_crs,
                collection.srid,
            ));
        }
        _ => values.push("NULL".to_string()),
    }

    for (key, value) in properties {
        let column = quote_ident(key)?;
        let type_ = *types
            .get(key.as_str())
            .ok_or_else(|| PostgisError::UnwritableProperty(key.clone()))?;
        match scalar_as_text(key, value)? {
            Some(text) => {
                params.push(SqlParam::Text(text));
                values.push(format!("${}::text::{}", params.len(), pg_cast(type_)));
            }
            None => values.push("NULL".to_string()),
        }
        columns.push(column);
    }

    // `#142`: the row's own post-mutation extent, in CRS84, on the same
    // statement that writes it — no extra round trip, and no chance of a
    // concurrent writer moving it between the write and the reading of it.
    let returning = crs84_extent_select_list(&geom, collection.srid);
    let sql = match expected {
        None => {
            let sets = columns
                .iter()
                .map(|column| format!("{column} = EXCLUDED.{column}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "INSERT INTO {table} ({pk}, {cols}) VALUES ($1, {vals}) ON CONFLICT ({pk}) DO UPDATE SET {sets} RETURNING {returning}",
                cols = columns.join(", "),
                vals = values.join(", "),
            )
        }
        Some(version) => {
            let (pk_predicate, param_suffix) = pk_equality(collection)?;
            params.push(SqlParam::Text(version.as_str().to_string()));
            let version_placeholder = params.len();
            let sets = columns
                .iter()
                .zip(values.iter())
                .map(|(column, value)| format!("{column} = {value}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "UPDATE {table} SET {sets} WHERE {pk_predicate} = $1{param_suffix} \
                 AND {ROW_VERSION_EXPR} = ${version_placeholder} RETURNING {returning}"
            )
        }
    };
    Ok(UpsertPlan { sql, params })
}

pub(crate) struct InsertPlan {
    pub(crate) sql: String,
    pub(crate) params: Vec<SqlParam>,
}

/// Builds `INSERT INTO table (geom, ...) VALUES (...) RETURNING pk` for a
/// server-assigned create (`#88`, `WriteSink::create`). `caller_pk` is the
/// seam `#94` added for `Text` id-type collections — `Integer`/`Uuid` always
/// pass `None`, which omits the pk column from both the column list and the
/// values (byte-for-byte the pre-`#94` SQL), so Postgres applies the
/// column's own `DEFAULT` (a `bigserial`'s `nextval(...)` for an `Integer`
/// collection, or a `uuid` column's `DEFAULT gen_random_uuid()` for a `Uuid`
/// one, `#87`) rather than this driver guessing an id itself. `Text` passes
/// `Some(pk_value)` instead — a `Text` pk has no server-side generator, so
/// `driver.rs`'s `create_inner` reads the id straight out of the caller's
/// own feature body and binds it here as an ordinary column value, the same
/// way `build_upsert_plan` always does. Either way the minted-or-supplied
/// value comes back in the SAME statement via `RETURNING` rather than a
/// separate round trip, so the id a caller sees is always exactly what the
/// database stored. The data-mutation half of `WriteSink::create`; the
/// outbox insert is a separate statement (`build_outbox_insert_plan`, reused
/// unchanged once the caller has the pk in hand), committed in the same
/// transaction by the caller. A pk column with no `DEFAULT` (the `caller_pk:
/// None` case) fails the resulting `INSERT` with a `NOT NULL` violation on
/// exactly that column — `run_create_transaction` turns that into a named
/// refusal rather than a raw SQL error; a `caller_pk: Some(_)` id already
/// claimed by another row instead hits a `UNIQUE` violation, which
/// `run_create_transaction` maps to a named `409` the same way.
pub(crate) fn build_insert_plan(
    collection: &CollectionDecl,
    caller_pk: Option<PkValue>,
    geometry: Option<&Value>,
    properties: &Map<String, Value>,
    types: &HashMap<String, PropertyType>,
    requested_crs: RequestedCrs,
) -> Result<InsertPlan> {
    let table = quote_ident(collection.resolved_table())?;
    let geom = quote_ident(collection.resolved_geometry())?;

    let mut columns = Vec::new();
    let mut placeholders = Vec::new();
    let mut params = Vec::new();

    if let Some(pk_value) = caller_pk {
        columns.push(quote_ident(collection.resolved_pk())?);
        params.push(pk_value.as_sql_param());
        placeholders.push(format!("${}", params.len()));
    }

    columns.push(geom.clone());
    match geometry {
        Some(value) if !value.is_null() => {
            params.push(SqlParam::Text(value.to_string()));
            placeholders.push(input_geom_expr(
                params.len(),
                requested_crs,
                collection.srid,
            ));
        }
        _ => placeholders.push("NULL".to_string()),
    }

    for (key, value) in properties {
        let column = quote_ident(key)?;
        let type_ = *types
            .get(key.as_str())
            .ok_or_else(|| PostgisError::UnwritableProperty(key.clone()))?;
        match scalar_as_text(key, value)? {
            Some(text) => {
                params.push(SqlParam::Text(text));
                placeholders.push(format!("${}::text::{}", params.len(), pg_cast(type_)));
            }
            None => placeholders.push("NULL".to_string()),
        }
        columns.push(column);
    }

    let pk = quote_ident(collection.resolved_pk())?;
    // The minted pk, then this row's own CRS84 extent (`#142`) — one
    // statement, so the id a caller sees and the extent the outbox records
    // are read off the very row that was written.
    let extent = crs84_extent_select_list(&geom, collection.srid);
    let sql = format!(
        "INSERT INTO {table} ({cols}) VALUES ({vals}) RETURNING {pk}, {extent}",
        cols = columns.join(", "),
        vals = placeholders.join(", "),
    );
    Ok(InsertPlan { sql, params })
}

/// `DELETE FROM table WHERE pk = $1::bigint` (for `Integer`) or `pk::<cast>
/// = $1` (for other id types) — the data mutation half of a
/// `MutationKind::Delete` apply. Keeping an integer pk column bare lets
/// PostgreSQL use its btree index even when the physical type is `int4`;
/// `Uuid` and `Text` retain their existing column casts.
///
/// `expected` is `#150`'s optimistic-locking guard, ANDed straight into the
/// same `WHERE` the pk already narrows — see `build_upsert_plan`'s own doc
/// for why a predicate the database evaluates as part of the write is the
/// only sound place for it. `None` emits byte-for-byte the SQL this function
/// always did.
///
/// `RETURNING` carries `#141`'s whole point: a `Delete` obligation has no
/// geometry of its own and, by the time any consumer drains it, the row is
/// gone — so the ONE moment the feature's prior extent is still knowable is
/// the statement that removes it. It is read there, in CRS84, and travels on
/// the obligation. No rows back means the feature was not there to begin
/// with, which is a recorded `prior: None`, not an unknown.
pub(crate) fn build_delete_plan(
    collection: &CollectionDecl,
    pk_value: PkValue,
    expected: Option<&RowVersion>,
) -> Result<(String, Vec<SqlParam>)> {
    let table = quote_ident(collection.resolved_table())?;
    let geom = quote_ident(collection.resolved_geometry())?;
    let (pk_predicate, param_suffix) = pk_equality(collection)?;
    let mut params = vec![pk_value.as_sql_param()];
    let guard = match expected {
        None => String::new(),
        Some(version) => {
            params.push(SqlParam::Text(version.as_str().to_string()));
            format!(" AND {ROW_VERSION_EXPR} = ${}", params.len())
        }
    };
    let returning = crs84_extent_select_list(&geom, collection.srid);
    let sql = format!(
        "DELETE FROM {table} WHERE {pk_predicate} = $1{param_suffix}{guard} RETURNING {returning}"
    );
    Ok((sql, params))
}

/// Appends one obligation to `table`'s outbox, returning the sequence it
/// committed at (`bigserial PRIMARY KEY`, see `tellurion-ingest::outbox`'s
/// DDL) — the other half of the same transaction `build_upsert_plan`/
/// `build_delete_plan`'s statement runs in. `payload` is `None` for a
/// `Delete` obligation (a tombstone carries no feature body) and `Some` for
/// an `Upsert` (the whole GeoJSON Feature, so a later index-apply step has
/// something to derive from).
/// `extent` is `#141`/`#142`'s CRS84 record of where the feature was and
/// where it now is, written into the same row as one `jsonb` object so a
/// consumer can tell "the storage says: nowhere" apart from "the storage
/// said nothing" — a `NULL` column is exactly
/// [`ObligationExtent::Unrecorded`], which is what every outbox row written
/// before the column existed carries, and what the consumer degrades
/// conservatively on.
pub(crate) fn build_outbox_insert_plan(
    table: &str,
    feature_id: &str,
    kind: &str,
    payload: Option<&Value>,
    extent: ObligationExtent,
) -> Result<(String, Vec<SqlParam>)> {
    let outbox_table = quote_ident(&outbox_table_name(table))?;
    let mut params = vec![
        SqlParam::Text(feature_id.to_string()),
        SqlParam::Text(kind.to_string()),
    ];
    let payload_placeholder = match payload {
        Some(value) => {
            params.push(SqlParam::Text(value.to_string()));
            format!("${}::text::jsonb", params.len())
        }
        None => "NULL".to_string(),
    };
    let extent_placeholder = match encode_extent(extent) {
        Some(json) => {
            params.push(SqlParam::Text(json.to_string()));
            format!("${}::text::jsonb", params.len())
        }
        None => "NULL".to_string(),
    };
    let sql = format!(
        "INSERT INTO {outbox_table} (feature_id, kind, payload, {OUTBOX_EXTENT_COLUMN}) VALUES ($1, $2, {payload_placeholder}, {extent_placeholder}) RETURNING sequence"
    );
    Ok((sql, params))
}

/// The `jsonb` shape [`OUTBOX_EXTENT_COLUMN`] stores, or `None` for
/// [`ObligationExtent::Unrecorded`] (a literal SQL `NULL`, which is what
/// "the storage recorded nothing" has to look like on the wire so that
/// re-reading it cannot be confused with "the feature has no geometry").
/// Kept beside [`decode_extent`], which is its exact inverse.
pub(crate) fn encode_extent(extent: ObligationExtent) -> Option<Value> {
    match extent {
        ObligationExtent::Unrecorded => None,
        ObligationExtent::Crs84 { prior, current } => Some(serde_json::json!({
            "prior": prior.map(Vec::from),
            "current": current.map(Vec::from),
        })),
    }
}

/// [`encode_extent`]'s inverse, applied to whatever came back out of
/// [`OUTBOX_EXTENT_COLUMN`]. Anything this cannot make sense of — a `NULL`
/// column, an object with the wrong shape — reads as
/// [`ObligationExtent::Unrecorded`], i.e. *unknown*, so a malformed value can
/// only ever cost a conservative over-invalidation, never a wrong one.
pub(crate) fn decode_extent(value: Option<&Value>) -> ObligationExtent {
    fn bbox(value: Option<&Value>) -> Option<[f64; 4]> {
        let numbers: Vec<f64> = value?
            .as_array()?
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        <[f64; 4]>::try_from(numbers).ok()
    }
    let Some(object) = value.and_then(Value::as_object) else {
        return ObligationExtent::Unrecorded;
    };
    if !object.contains_key("prior") || !object.contains_key("current") {
        return ObligationExtent::Unrecorded;
    }
    ObligationExtent::Crs84 {
        prior: bbox(object.get("prior")),
        current: bbox(object.get("current")),
    }
}

/// Obligations with `sequence > after`, ascending, at most `limit` — see
/// `tellurion_core::OutboxSource::read_after`'s own contract (never skips or
/// reorders). `committed_at` (`timestamptz`, read back as `SystemTime` —
/// `postgres-types`' own built-in conversion, no `chrono`/`time` dependency
/// needed) rides along so `Obligation::committed_at` (`#115`) needs no
/// second query to fill in.
pub(crate) fn build_read_after_plan(
    table: &str,
    after: u64,
    limit: u32,
) -> Result<(String, Vec<SqlParam>)> {
    let outbox_table = quote_ident(&outbox_table_name(table))?;
    let sql = format!(
        "SELECT sequence, feature_id, kind, payload, committed_at, {OUTBOX_EXTENT_COLUMN} FROM {outbox_table} WHERE sequence > $1 ORDER BY sequence ASC LIMIT $2"
    );
    let after = i64::try_from(after).unwrap_or(i64::MAX);
    let limit = i64::from(limit);
    Ok((sql, vec![SqlParam::Bigint(after), SqlParam::Bigint(limit)]))
}

/// The highest sequence committed to `table`'s outbox — `0` (never `NULL`)
/// for an outbox with no rows yet, matching `Sequence`'s own "gaps allowed,
/// starts nowhere in particular" contract.
pub(crate) fn build_primary_high_water_plan(table: &str) -> Result<String> {
    let outbox_table = quote_ident(&outbox_table_name(table))?;
    Ok(format!(
        "SELECT COALESCE(MAX(sequence), 0)::bigint AS high_water FROM {outbox_table}"
    ))
}

/// Removes at most `batch_size` obligations with `sequence <= floor` from
/// `table`'s outbox — `crate::driver`'s `OutboxSource::prune_before` (`#115`)
/// own SQL half. The inner `SELECT ... LIMIT` bounds one call's own row
/// count regardless of how far the floor has advanced since the last prune,
/// the same "bounded per-pass work" discipline `build_read_after_plan`'s own
/// `LIMIT $2` already applies to reads.
pub(crate) fn build_prune_before_plan(
    table: &str,
    floor: u64,
    batch_size: u32,
) -> Result<(String, Vec<SqlParam>)> {
    let outbox_table = quote_ident(&outbox_table_name(table))?;
    let sql = format!(
        "DELETE FROM {outbox_table} WHERE sequence IN (SELECT sequence FROM {outbox_table} WHERE sequence <= $1 ORDER BY sequence ASC LIMIT $2)"
    );
    let floor = i64::try_from(floor).unwrap_or(i64::MAX);
    let batch_size = i64::from(batch_size);
    Ok((
        sql,
        vec![SqlParam::Bigint(floor), SqlParam::Bigint(batch_size)],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn collection() -> CollectionDecl {
        serde_yaml::from_str(
            r#"
id: demo
catalog: default
storage: main
table: demo
geometry: geom
pk: id
"#,
        )
        .unwrap()
    }

    #[test]
    fn outbox_table_name_appends_the_suffix() {
        assert_eq!(outbox_table_name("demo"), "demo_outbox");
    }

    /// The four CRS84 extent expressions `crs84_extent_select_list` emits
    /// (`#142`), spelled out by hand rather than derived by calling it — so
    /// the assertions below pin the SQL text a real PostGIS server will see
    /// instead of restating whatever the code under test happens to build.
    ///
    /// `EXTENT_CRS84_STORAGE` is the CRS84-equivalent case (SRID 4326, or
    /// unknown): no `ST_Transform` anywhere, which is what keeps such a
    /// deployment's invalidation extents the numbers it already stored.
    const EXTENT_CRS84_STORAGE: &str =
        "ST_XMin(\"geom\"), ST_YMin(\"geom\"), ST_XMax(\"geom\"), ST_YMax(\"geom\")";
    /// And the projected case, where the storage does the reprojection so no
    /// consumer has to guess.
    const EXTENT_PROJECTED_STORAGE: &str = "ST_XMin(ST_Transform(\"geom\", 4326)), \
         ST_YMin(ST_Transform(\"geom\", 4326)), ST_XMax(ST_Transform(\"geom\", 4326)), \
         ST_YMax(ST_Transform(\"geom\", 4326))";

    #[test]
    fn upsert_plan_with_no_properties_writes_pk_and_geometry_only() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [1.0, 2.0]});
        let plan = build_upsert_plan(
            &collection(),
            PkValue::Integer(42),
            Some(&geometry),
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Omitted,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            format!("INSERT INTO \"demo\" (\"id\", \"geom\") VALUES ($1, ST_SetSRID(ST_GeomFromGeoJSON($2), 4326)) ON CONFLICT (\"id\") DO UPDATE SET \"geom\" = EXCLUDED.\"geom\" RETURNING {EXTENT_CRS84_STORAGE}")
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Bigint(42), SqlParam::Text(_)]
        ));
    }

    #[test]
    fn upsert_plan_with_a_null_geometry_writes_a_literal_null() {
        let plan = build_upsert_plan(
            &collection(),
            PkValue::Integer(1),
            None,
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Omitted,
            None,
        )
        .unwrap();
        assert!(
            plan.sql.contains("VALUES ($1, NULL)"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(plan.params.as_slice(), [SqlParam::Bigint(1)]));
    }

    #[test]
    fn upsert_plan_casts_a_declared_property_through_text() {
        let mut properties = Map::new();
        properties.insert("population".to_string(), serde_json::json!(42));
        let mut types = HashMap::new();
        types.insert("population".to_string(), PropertyType::Integer);

        let plan = build_upsert_plan(
            &collection(),
            PkValue::Integer(1),
            None,
            &properties,
            &types,
            RequestedCrs::Omitted,
            None,
        )
        .unwrap();
        assert!(
            plan.sql
                .contains("\"population\" = EXCLUDED.\"population\""),
            "sql was: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("$2::text::bigint"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Bigint(1), SqlParam::Text(v)] if v == "42"
        ));
    }

    #[test]
    fn upsert_plan_writes_a_literal_null_for_a_null_property_value() {
        let mut properties = Map::new();
        properties.insert("name".to_string(), Value::Null);
        let mut types = HashMap::new();
        types.insert("name".to_string(), PropertyType::String);

        let plan = build_upsert_plan(
            &collection(),
            PkValue::Integer(1),
            None,
            &properties,
            &types,
            RequestedCrs::Omitted,
            None,
        )
        .unwrap();
        assert!(
            plan.sql.contains("NULL) ON CONFLICT"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn upsert_plan_rejects_a_property_with_no_resolved_type() {
        let mut properties = Map::new();
        properties.insert("mystery".to_string(), serde_json::json!("x"));
        assert!(matches!(
            build_upsert_plan(
                &collection(),
                PkValue::Integer(1),
                None,
                &properties,
                &HashMap::new(),
                RequestedCrs::Omitted,
                None,
            ),
            Err(PostgisError::UnwritableProperty(key)) if key == "mystery"
        ));
    }

    #[test]
    fn upsert_plan_rejects_an_array_property_value() {
        let mut properties = Map::new();
        properties.insert("tags".to_string(), serde_json::json!(["a", "b"]));
        let mut types = HashMap::new();
        types.insert("tags".to_string(), PropertyType::String);
        assert!(matches!(
            build_upsert_plan(
                &collection(),
                PkValue::Integer(1),
                None,
                &properties,
                &types,
                RequestedCrs::Omitted,
                None,
            ),
            Err(PostgisError::UnsupportedPropertyValue(key)) if key == "tags"
        ));
    }

    #[test]
    fn upsert_plan_rejects_an_invalid_property_key_identifier() {
        let mut properties = Map::new();
        properties.insert("name; DROP TABLE x; --".to_string(), serde_json::json!("x"));
        let mut types = HashMap::new();
        types.insert("name; DROP TABLE x; --".to_string(), PropertyType::String);
        assert!(matches!(
            build_upsert_plan(
                &collection(),
                PkValue::Integer(1),
                None,
                &properties,
                &types,
                RequestedCrs::Omitted,
                None,
            ),
            Err(PostgisError::InvalidIdentifier(_))
        ));
    }

    #[test]
    fn insert_plan_with_no_properties_omits_the_pk_column_and_writes_geometry_only() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [1.0, 2.0]});
        let plan = build_insert_plan(
            &collection(),
            None,
            Some(&geometry),
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            format!("INSERT INTO \"demo\" (\"geom\") VALUES (ST_SetSRID(ST_GeomFromGeoJSON($1), 4326)) RETURNING \"id\", {EXTENT_CRS84_STORAGE}")
        );
        assert!(matches!(plan.params.as_slice(), [SqlParam::Text(_)]));
    }

    #[test]
    fn insert_plan_with_a_null_geometry_writes_a_literal_null() {
        let plan = build_insert_plan(
            &collection(),
            None,
            None,
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            format!("INSERT INTO \"demo\" (\"geom\") VALUES (NULL) RETURNING \"id\", {EXTENT_CRS84_STORAGE}")
        );
        assert!(plan.params.is_empty());
    }

    #[test]
    fn insert_plan_casts_a_declared_property_through_text() {
        let mut properties = Map::new();
        properties.insert("population".to_string(), serde_json::json!(42));
        let mut types = HashMap::new();
        types.insert("population".to_string(), PropertyType::Integer);

        let plan = build_insert_plan(
            &collection(),
            None,
            None,
            &properties,
            &types,
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert!(
            plan.sql
                .contains("\"population\") VALUES (NULL, $1::text::bigint)"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(plan.params.as_slice(), [SqlParam::Text(v)] if v == "42"));
    }

    #[test]
    fn insert_plan_rejects_a_property_with_no_resolved_type() {
        let mut properties = Map::new();
        properties.insert("mystery".to_string(), serde_json::json!("x"));
        assert!(matches!(
            build_insert_plan(
                &collection(),
                None,
                None,
                &properties,
                &HashMap::new(),
                RequestedCrs::Omitted
            ),
            Err(PostgisError::UnwritableProperty(key)) if key == "mystery"
        ));
    }

    /// `#94`: a `Text` id-type collection's create binds the caller-supplied
    /// pk directly, as the INSERT's first column — unlike `Integer`/`Uuid`,
    /// which always omit the pk column so the database mints it.
    #[test]
    fn insert_plan_with_a_caller_supplied_text_pk_binds_it_as_the_first_column() {
        let mut text_collection = collection();
        text_collection.id_type = tellurion_core::IdType::Text;
        let geometry = serde_json::json!({"type": "Point", "coordinates": [1.0, 2.0]});
        let plan = build_insert_plan(
            &text_collection,
            Some(PkValue::Text("acme-1".to_string())),
            Some(&geometry),
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            format!("INSERT INTO \"demo\" (\"id\", \"geom\") VALUES ($1, ST_SetSRID(ST_GeomFromGeoJSON($2), 4326)) RETURNING \"id\", {EXTENT_CRS84_STORAGE}")
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Text(id), SqlParam::Text(_)] if id == "acme-1"
        ));
    }

    #[test]
    fn delete_plan_keeps_an_integer_pk_column_bare_and_casts_the_parameter() {
        let (sql, params) = build_delete_plan(&collection(), PkValue::Integer(7), None).unwrap();
        assert_eq!(
            sql,
            format!(
                "DELETE FROM \"demo\" WHERE \"id\" = $1::bigint RETURNING {EXTENT_CRS84_STORAGE}"
            )
        );
        assert!(matches!(params.as_slice(), [SqlParam::Bigint(7)]));
    }

    /// `#87`: a `Uuid` id-type collection's upsert binds the pk as a native
    /// `uuid` parameter and never casts it through `bigint`.
    #[test]
    fn upsert_plan_binds_a_uuid_pk_natively_for_a_uuid_id_type_collection() {
        let mut uuid_collection = collection();
        uuid_collection.id_type = tellurion_core::IdType::Uuid;
        let id = uuid::Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
        let plan = build_upsert_plan(
            &uuid_collection,
            PkValue::Uuid(id),
            None,
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Omitted,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            format!("INSERT INTO \"demo\" (\"id\", \"geom\") VALUES ($1, NULL) ON CONFLICT (\"id\") DO UPDATE SET \"geom\" = EXCLUDED.\"geom\" RETURNING {EXTENT_CRS84_STORAGE}")
        );
        assert!(matches!(plan.params.as_slice(), [SqlParam::Uuid(v)] if *v == id));
    }

    /// `#87`: a `Uuid` id-type collection's delete casts the pk column to
    /// `uuid`, not `bigint`.
    #[test]
    fn delete_plan_casts_uuid_for_a_uuid_id_type_collection() {
        let mut uuid_collection = collection();
        uuid_collection.id_type = tellurion_core::IdType::Uuid;
        let id = uuid::Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap();
        let (sql, params) = build_delete_plan(&uuid_collection, PkValue::Uuid(id), None).unwrap();
        assert_eq!(
            sql,
            format!(
                "DELETE FROM \"demo\" WHERE \"id\"::uuid = $1 RETURNING {EXTENT_CRS84_STORAGE}"
            )
        );
        assert!(matches!(params.as_slice(), [SqlParam::Uuid(v)] if *v == id));
    }

    /// `#94`: a `Text` id-type collection's delete casts the pk column to
    /// `text` and binds the id as a native text parameter, mirroring the
    /// `Uuid` case above.
    #[test]
    fn delete_plan_casts_text_for_a_text_id_type_collection() {
        let mut text_collection = collection();
        text_collection.id_type = tellurion_core::IdType::Text;
        let (sql, params) =
            build_delete_plan(&text_collection, PkValue::Text("acme-1".to_string()), None).unwrap();
        assert_eq!(
            sql,
            format!(
                "DELETE FROM \"demo\" WHERE \"id\"::text = $1 RETURNING {EXTENT_CRS84_STORAGE}"
            )
        );
        assert!(matches!(params.as_slice(), [SqlParam::Text(id)] if id == "acme-1"));
    }

    // -- `#150`: the optimistic-locking guard lives in the statement -------

    /// The load-bearing assertion of `#150`, at the SQL level: the witness
    /// is a predicate on the mutating statement itself — evaluated by the
    /// database, atomically with the write — not a check some caller ran
    /// beforehand. And it is BOUND, never interpolated, like every other
    /// value this module emits.
    #[test]
    fn a_conditional_upsert_carries_the_row_version_predicate_in_its_own_where() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [1.0, 2.0]});
        let mut properties = Map::new();
        properties.insert("name".to_string(), serde_json::json!("alpha"));
        let mut types = HashMap::new();
        types.insert("name".to_string(), PropertyType::String);

        let plan = build_upsert_plan(
            &collection(),
            PkValue::Integer(42),
            Some(&geometry),
            &properties,
            &types,
            RequestedCrs::Omitted,
            Some(&RowVersion::new("991")),
        )
        .unwrap();

        assert_eq!(
            plan.sql,
            format!(
                "UPDATE \"demo\" SET \"geom\" = ST_SetSRID(ST_GeomFromGeoJSON($2), 4326), \
                 \"name\" = $3::text::text WHERE \"id\" = $1::bigint AND xmin::text = $4 \
                 RETURNING {EXTENT_CRS84_STORAGE}"
            )
        );
        assert!(
            matches!(plan.params.last(), Some(SqlParam::Text(token)) if token == "991"),
            "the witness must be a bound parameter, not interpolated: {:?}",
            plan.params
        );
    }

    /// A conditional upsert must NOT keep an insert arm. The caller only
    /// ever conditions on a target it already found, so an `ON CONFLICT`
    /// fallthrough could only fire when a concurrent `DELETE` removed that
    /// target — resurrecting a row whose `If-Match` was never matched
    /// against anything, which is the same lost-update class this closes.
    #[test]
    fn a_conditional_upsert_never_falls_through_to_an_insert() {
        let plan = build_upsert_plan(
            &collection(),
            PkValue::Integer(42),
            None,
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Omitted,
            Some(&RowVersion::new("7")),
        )
        .unwrap();
        assert!(
            !plan.sql.contains("INSERT") && !plan.sql.contains("ON CONFLICT"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn a_conditional_delete_ands_the_row_version_into_the_same_where() {
        let (sql, params) = build_delete_plan(
            &collection(),
            PkValue::Integer(7),
            Some(&RowVersion::new("1234")),
        )
        .unwrap();
        assert_eq!(
            sql,
            format!("DELETE FROM \"demo\" WHERE \"id\" = $1::bigint AND xmin::text = $2 RETURNING {EXTENT_CRS84_STORAGE}")
        );
        assert!(
            matches!(params.as_slice(), [SqlParam::Bigint(7), SqlParam::Text(token)] if token == "1234")
        );
    }

    /// Rule 1 of this slice, pinned: a request carrying no precondition
    /// produces exactly the SQL it always did — no stray predicate, no
    /// extra bound parameter.
    #[test]
    fn no_precondition_leaves_the_statement_byte_for_byte_unchanged() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [1.0, 2.0]});
        let plan = build_upsert_plan(
            &collection(),
            PkValue::Integer(42),
            Some(&geometry),
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Omitted,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            format!("INSERT INTO \"demo\" (\"id\", \"geom\") VALUES ($1, ST_SetSRID(ST_GeomFromGeoJSON($2), 4326)) ON CONFLICT (\"id\") DO UPDATE SET \"geom\" = EXCLUDED.\"geom\" RETURNING {EXTENT_CRS84_STORAGE}")
        );
        assert!(!plan.sql.contains("xmin"));

        let (sql, params) = build_delete_plan(&collection(), PkValue::Integer(7), None).unwrap();
        assert_eq!(
            sql,
            format!(
                "DELETE FROM \"demo\" WHERE \"id\" = $1::bigint RETURNING {EXTENT_CRS84_STORAGE}"
            )
        );
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn row_version_plan_reads_xmin_for_one_pk() {
        let (sql, params) = build_row_version_plan(&collection(), PkValue::Integer(42)).unwrap();
        assert_eq!(
            sql,
            "SELECT xmin::text FROM \"demo\" WHERE \"id\" = $1::bigint"
        );
        assert!(matches!(params.as_slice(), [SqlParam::Bigint(42)]));
    }

    /// `#87`/`#94`: the witness `SELECT` narrows through the same pk cast
    /// every other statement in this module uses, so a `Uuid`/`Text`
    /// collection is never silently unable to mint a witness.
    #[test]
    fn row_version_plan_casts_a_non_integer_pk_the_same_way_every_other_plan_does() {
        let mut text_collection = collection();
        text_collection.id_type = IdType::Text;
        let (sql, _) =
            build_row_version_plan(&text_collection, PkValue::Text("acme-1".to_string())).unwrap();
        assert_eq!(
            sql,
            "SELECT xmin::text FROM \"demo\" WHERE \"id\"::text = $1"
        );
    }

    // -- `Content-Crs`-declared write CRS (OGC API Features Part 4,
    // `/req/features/content-crs-header` and `/req/features/crs-other-crs`)
    // --------------------------------------------------------------------

    fn collection_with_srid(srid: i32) -> CollectionDecl {
        let mut collection = collection();
        collection.srid = Some(srid);
        collection
    }

    /// An explicit `RequestedCrs::Crs84` must produce byte-for-byte the same
    /// SQL as `RequestedCrs::Omitted` (Requirement 41, `/req/features/
    /// default-crs`) — both mean "interpret the body as CRS84."
    #[test]
    fn input_geom_expr_is_identical_for_omitted_and_explicit_crs84() {
        assert_eq!(
            input_geom_expr(2, RequestedCrs::Omitted, Some(3857)),
            input_geom_expr(2, RequestedCrs::Crs84, Some(3857)),
        );
        assert_eq!(
            input_geom_expr(2, RequestedCrs::Omitted, None),
            "ST_SetSRID(ST_GeomFromGeoJSON($2), 4326)"
        );
    }

    /// `RequestedCrs::Storage` against a non-lat/lon-ordered storage SRID
    /// (anything but 4326) tags the geometry with that SRID directly — no
    /// axis flip, no transform, since the caller only ever resolves
    /// `Storage` when the coordinates already arrived in that CRS's own
    /// representation.
    #[test]
    fn input_geom_expr_for_storage_tags_a_non_lat_lon_srid_directly() {
        assert_eq!(
            input_geom_expr(1, RequestedCrs::Storage, Some(3857)),
            "ST_SetSRID(ST_GeomFromGeoJSON($1), 3857)"
        );
    }

    /// `RequestedCrs::Storage` against storage SRID 4326 — authority-ordered
    /// latitude-before-longitude (`crs::is_lat_lon_order`) — additionally
    /// undoes that axis swap with `ST_FlipCoordinates`, the write-side
    /// mirror of `sql.rs::reprojected_geom_expr`'s own read-side use of the
    /// same function.
    #[test]
    fn input_geom_expr_for_storage_flips_coordinates_for_srid_4326() {
        assert_eq!(
            input_geom_expr(1, RequestedCrs::Storage, Some(4326)),
            "ST_SetSRID(ST_FlipCoordinates(ST_GeomFromGeoJSON($1)), 4326)"
        );
    }

    /// A declared storage CRS actually changes the `INSERT`'s SQL — the
    /// concrete fix for the silent-corruption bug this lane exists to close:
    /// before this, every write tagged its geometry SRID 4326 regardless of
    /// what the collection's own storage CRS was or what the caller
    /// declared.
    #[test]
    fn upsert_plan_with_requested_crs_storage_tags_the_storage_srid_not_4326() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [500000.0, 4649776.0]});
        let plan = build_upsert_plan(
            &collection_with_srid(3857),
            PkValue::Integer(1),
            Some(&geometry),
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Storage,
            None,
        )
        .unwrap();
        assert!(
            plan.sql
                .contains("ST_SetSRID(ST_GeomFromGeoJSON($2), 3857)"),
            "sql was: {}",
            plan.sql
        );
        // The INPUT half must name no 4326 at all — that is what "tags the
        // storage SRID, not 4326" means. Scoped to the statement before its
        // `RETURNING`, because `#142`'s extent expression there legitimately
        // transforms the STORED geometry INTO 4326 on its way to the outbox:
        // the opposite direction, and a different question.
        let written = plan
            .sql
            .split_once(" RETURNING ")
            .expect("every upsert plan carries its CRS84 extent in RETURNING")
            .0;
        assert!(!written.contains("4326"), "sql was: {}", plan.sql);
    }

    #[test]
    fn insert_plan_with_requested_crs_storage_tags_the_storage_srid_not_4326() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [500000.0, 4649776.0]});
        let plan = build_insert_plan(
            &collection_with_srid(3857),
            None,
            Some(&geometry),
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Storage,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            format!("INSERT INTO \"demo\" (\"geom\") VALUES (ST_SetSRID(ST_GeomFromGeoJSON($1), 3857)) RETURNING \"id\", {EXTENT_PROJECTED_STORAGE}")
        );
    }

    // -- `#116`: the default write path (`Content-Crs` absent, or explicit
    // CRS84) must transform into a non-4326 storage SRID rather than tag it
    // 4326 --------------------------------------------------------------

    /// The regression pin from `#116`: before the fix, this SQL was produced
    /// unconditionally regardless of the collection's own storage SRID. A
    /// collection genuinely stored in 4326 must keep getting exactly this —
    /// a bare tag is already the correct, cheapest conversion when the
    /// storage CRS and CRS84 share the same representation.
    #[test]
    fn input_geom_expr_default_path_stays_byte_for_byte_when_storage_srid_is_4326() {
        assert_eq!(
            input_geom_expr(2, RequestedCrs::Omitted, Some(4326)),
            "ST_SetSRID(ST_GeomFromGeoJSON($2), 4326)"
        );
        assert_eq!(
            input_geom_expr(2, RequestedCrs::Crs84, Some(4326)),
            "ST_SetSRID(ST_GeomFromGeoJSON($2), 4326)"
        );
    }

    /// Same pin, for a collection whose storage SRID is unknown entirely —
    /// there is nothing to transform into, so the pre-fix tag-only SQL is
    /// still the only sensible output.
    #[test]
    fn input_geom_expr_default_path_stays_byte_for_byte_when_storage_srid_is_unknown() {
        assert_eq!(
            input_geom_expr(2, RequestedCrs::Omitted, None),
            "ST_SetSRID(ST_GeomFromGeoJSON($2), 4326)"
        );
    }

    /// The actual fix: a collection stored in any SRID other than 4326 gets
    /// its CRS84 input reprojected, not merely relabeled.
    #[test]
    fn input_geom_expr_default_path_transforms_into_a_non_4326_storage_srid() {
        assert_eq!(
            input_geom_expr(2, RequestedCrs::Omitted, Some(3857)),
            "ST_Transform(ST_SetSRID(ST_GeomFromGeoJSON($2), 4326), 3857)"
        );
        assert_eq!(
            input_geom_expr(2, RequestedCrs::Crs84, Some(3857)),
            "ST_Transform(ST_SetSRID(ST_GeomFromGeoJSON($2), 4326), 3857)"
        );
    }

    /// `is_lat_lon_order` only ever recognizes SRID 4326 (see its own doc),
    /// and SRID 4326 is exactly the storage SRID the arm above already
    /// short-circuits away from a transform — so no real SRID this crate can
    /// transform into today ever triggers the `ST_FlipCoordinates`-wrapped
    /// branch. This pins that today's reachable non-4326 SRIDs (a projected
    /// CRS like 3857, and a geographic-but-not-narrowly-recognized one like
    /// 4269/NAD83) never grow an unwarranted flip.
    #[test]
    fn input_geom_expr_default_path_never_flips_for_todays_recognized_srids() {
        for srid in [3857, 2154, 4269] {
            let sql = input_geom_expr(2, RequestedCrs::Omitted, Some(srid));
            assert!(
                !sql.contains("FlipCoordinates"),
                "srid {srid} unexpectedly flipped: {sql}"
            );
        }
    }

    /// `build_upsert_plan` (the `PUT`/replace path) end-to-end: a non-4326
    /// collection's default-path write now transforms.
    #[test]
    fn upsert_plan_with_omitted_crs_transforms_a_non_4326_collection() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [10.0, 45.0]});
        let plan = build_upsert_plan(
            &collection_with_srid(3857),
            PkValue::Integer(1),
            Some(&geometry),
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Omitted,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            format!("INSERT INTO \"demo\" (\"id\", \"geom\") VALUES ($1, ST_Transform(ST_SetSRID(ST_GeomFromGeoJSON($2), 4326), 3857)) ON CONFLICT (\"id\") DO UPDATE SET \"geom\" = EXCLUDED.\"geom\" RETURNING {EXTENT_PROJECTED_STORAGE}")
        );
    }

    /// `build_insert_plan` (the `POST`/create path) end-to-end: same fix,
    /// same collection shape.
    #[test]
    fn insert_plan_with_omitted_crs_transforms_a_non_4326_collection() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [10.0, 45.0]});
        let plan = build_insert_plan(
            &collection_with_srid(3857),
            None,
            Some(&geometry),
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Omitted,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            format!("INSERT INTO \"demo\" (\"geom\") VALUES (ST_Transform(ST_SetSRID(ST_GeomFromGeoJSON($1), 4326), 3857)) RETURNING \"id\", {EXTENT_PROJECTED_STORAGE}")
        );
    }

    /// `build_upsert_plan`'s own byte-for-byte pin (`#116`): a collection
    /// whose storage SRID IS 4326 must produce the exact pre-fix SQL on the
    /// default path, same as a collection with no known SRID at all
    /// (`upsert_plan_with_no_properties_writes_pk_and_geometry_only` already
    /// covers that case).
    #[test]
    fn upsert_plan_with_omitted_crs_is_unchanged_when_storage_srid_is_4326() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [1.0, 2.0]});
        let plan = build_upsert_plan(
            &collection_with_srid(4326),
            PkValue::Integer(42),
            Some(&geometry),
            &Map::new(),
            &HashMap::new(),
            RequestedCrs::Omitted,
            None,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            format!("INSERT INTO \"demo\" (\"id\", \"geom\") VALUES ($1, ST_SetSRID(ST_GeomFromGeoJSON($2), 4326)) ON CONFLICT (\"id\") DO UPDATE SET \"geom\" = EXCLUDED.\"geom\" RETURNING {EXTENT_CRS84_STORAGE}")
        );
    }

    #[test]
    fn outbox_insert_plan_with_a_payload() {
        let payload = serde_json::json!({"type": "Feature"});
        let (sql, params) = build_outbox_insert_plan(
            "demo",
            "1",
            "upsert",
            Some(&payload),
            ObligationExtent::Crs84 {
                prior: None,
                current: Some([1.0, 2.0, 3.0, 4.0]),
            },
        )
        .unwrap();
        assert_eq!(
            sql,
            "INSERT INTO \"demo_outbox\" (feature_id, kind, payload, extent_crs84) VALUES ($1, $2, $3::text::jsonb, $4::text::jsonb) RETURNING sequence"
        );
        assert!(matches!(
            params.as_slice(),
            [SqlParam::Text(a), SqlParam::Text(b), SqlParam::Text(_), SqlParam::Text(extent)]
            if a == "1" && b == "upsert"
                && extent.contains("\"current\":[1.0,2.0,3.0,4.0]")
                && extent.contains("\"prior\":null")
        ));
    }

    #[test]
    fn outbox_insert_plan_without_a_payload_writes_a_literal_null() {
        let (sql, params) =
            build_outbox_insert_plan("demo", "1", "delete", None, ObligationExtent::Unrecorded)
                .unwrap();
        assert_eq!(
            sql,
            "INSERT INTO \"demo_outbox\" (feature_id, kind, payload, extent_crs84) VALUES ($1, $2, NULL, NULL) RETURNING sequence"
        );
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn read_after_plan_shape() {
        let (sql, params) = build_read_after_plan("demo", 5, 100).unwrap();
        assert_eq!(
            sql,
            "SELECT sequence, feature_id, kind, payload, committed_at, extent_crs84 FROM \"demo_outbox\" WHERE sequence > $1 ORDER BY sequence ASC LIMIT $2"
        );
        assert!(matches!(
            params.as_slice(),
            [SqlParam::Bigint(5), SqlParam::Bigint(100)]
        ));
    }

    #[test]
    fn primary_high_water_plan_shape() {
        let sql = build_primary_high_water_plan("demo").unwrap();
        assert_eq!(
            sql,
            "SELECT COALESCE(MAX(sequence), 0)::bigint AS high_water FROM \"demo_outbox\""
        );
    }

    #[test]
    fn prune_before_plan_shape() {
        let (sql, params) = build_prune_before_plan("demo", 42, 500).unwrap();
        assert_eq!(
            sql,
            "DELETE FROM \"demo_outbox\" WHERE sequence IN (SELECT sequence FROM \"demo_outbox\" WHERE sequence <= $1 ORDER BY sequence ASC LIMIT $2)"
        );
        assert!(matches!(
            params.as_slice(),
            [SqlParam::Bigint(42), SqlParam::Bigint(500)]
        ));
    }
}
