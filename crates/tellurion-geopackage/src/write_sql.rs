//! Pure SQL builders for the write lane (the transactional-outbox design,
//! `#25`): a table/pk/geometry shape + mutation in, SQL text + typed params
//! out — no I/O, mirroring `tellurion-postgis::write_sql`'s own discipline.
//! Every identifier is whitelist-quoted (`ident.rs`); every value is bound
//! as a numbered `?N` parameter, never interpolated.
//!
//! Unlike PostGIS's write path, no property-type resolution/cast is needed
//! here at all: SQLite is dynamically typed, so a scalar JSON value binds
//! directly as its own native SQLite storage class (`SqlParam::{Int,Real,
//! Text}`) with no `$N::text::<pg_type>` cast trick to reach for. The only
//! thing this module still needs from the caller is *which* columns exist
//! (`driver.rs`'s own `attribute_columns` lookup) — an unknown property name
//! still fails with `UnwritableProperty` before any SQL is built, the same
//! contract PostGIS's write path holds.
//!
//! ## The per-collection outbox table
//!
//! `outbox_table_name` derives `"<table>_outbox"`, the same per-collection
//! obligation-log naming convention `tellurion-postgis::write_sql` and this
//! driver's own `tellurion-ingest` provisioning module share by hand (the
//! two crates never depend on each other — see that crate's own doc for why).

use std::collections::HashSet;

use serde_json::{Map, Value};
use tellurion_core::ObligationExtent;

use crate::error::{GeopackageError, Result};
use crate::gpb;
use crate::ident::quote_ident;
use crate::sql::SqlParam;

/// `"<table>_outbox"` — see this module's own doc for why the name is a
/// hand-kept convention rather than a shared constant.
pub(crate) fn outbox_table_name(table: &str) -> String {
    format!("{table}_outbox")
}

/// The outbox column `#141`/`#142` added — kept in sync by hand with
/// `tellurion-ingest::geopackage`'s own provisioning, the same arrangement
/// the rest of this module's outbox SQL already follows.
pub(crate) const OUTBOX_EXTENT_COLUMN: &str = "extent_crs84";

/// The JSON shape [`OUTBOX_EXTENT_COLUMN`] stores, or `None` for
/// [`ObligationExtent::Unrecorded`] (a literal SQL `NULL` — "the storage
/// recorded nothing", which must stay distinguishable from "the feature has
/// no geometry"). Byte-identical to `tellurion-postgis::write_sql`'s own
/// encoding: the two drivers never depend on each other, but a consumer
/// reads both through the same [`ObligationExtent`], so the wire shape is a
/// hand-kept convention exactly like the outbox table name itself.
pub(crate) fn encode_extent(extent: ObligationExtent) -> Option<Value> {
    match extent {
        ObligationExtent::Unrecorded => None,
        ObligationExtent::Crs84 { prior, current } => Some(serde_json::json!({
            "prior": prior.map(Vec::from),
            "current": current.map(Vec::from),
        })),
    }
}

/// [`encode_extent`]'s inverse. Anything this cannot make sense of — a
/// `NULL` column, unparseable text, an object with the wrong shape — reads
/// as [`ObligationExtent::Unrecorded`], i.e. *unknown*, so a malformed value
/// can only ever cost a conservative over-invalidation, never a wrong one.
pub(crate) fn decode_extent(text: Option<&str>) -> ObligationExtent {
    fn bbox(value: Option<&Value>) -> Option<[f64; 4]> {
        let numbers: Vec<f64> = value?
            .as_array()?
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        <[f64; 4]>::try_from(numbers).ok()
    }
    let Some(value) = text.and_then(|text| serde_json::from_str::<Value>(text).ok()) else {
        return ObligationExtent::Unrecorded;
    };
    let Some(object) = value.as_object() else {
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

/// A scalar JSON value's [`SqlParam`]. `Err` for an array/object, outside
/// this write path's flat, one-column-per-property model — mirrors
/// `tellurion-postgis::write_sql::scalar_as_text`'s own refusal.
fn scalar_param(key: &str, value: &Value) -> Result<SqlParam> {
    match value {
        Value::Null => Ok(SqlParam::Null),
        Value::String(s) => Ok(SqlParam::Text(s.clone())),
        Value::Bool(b) => Ok(SqlParam::Int(if *b { 1 } else { 0 })),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(SqlParam::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(SqlParam::Real(f))
            } else {
                Err(GeopackageError::UnsupportedPropertyValue(key.to_string()))
            }
        }
        Value::Array(_) | Value::Object(_) => {
            Err(GeopackageError::UnsupportedPropertyValue(key.to_string()))
        }
    }
}

pub(crate) struct UpsertPlan {
    pub(crate) sql: String,
    pub(crate) params: Vec<SqlParam>,
}

/// Builds `INSERT ... ON CONFLICT (pk) DO UPDATE` for one feature (SQLite's
/// upsert clause, available since 3.24 — always present in this crate's
/// bundled SQLite; see `Cargo.toml`) — the data-mutation half of
/// `WriteSink::apply` (the outbox insert is a separate statement,
/// `build_outbox_insert_plan`, committed in the same transaction by the
/// caller). `known_columns` is every real, non-geometry, non-pk column this
/// collection's table actually has (`driver.rs`'s own catalog lookup) — a
/// property naming anything else fails with `UnwritableProperty` before any
/// SQL is built, without needing to resolve a type the way PostGIS's write
/// path does (see this module's own top-level doc).
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_upsert_plan(
    table: &str,
    pk: &str,
    geometry_column: &str,
    srid: i32,
    pk_value: i64,
    geometry: Option<&Value>,
    properties: &Map<String, Value>,
    known_columns: &HashSet<String>,
    requested_crs: tellurion_core::RequestedCrs,
) -> Result<UpsertPlan> {
    let table_ident = quote_ident(table)?;
    let pk_ident = quote_ident(pk)?;
    let geom_ident = quote_ident(geometry_column)?;

    let mut columns = vec![pk_ident.clone(), geom_ident.clone()];
    let mut placeholders = vec!["?1".to_string()];
    let mut params = vec![SqlParam::Int(pk_value)];
    let mut set_clauses = vec![format!("{geom_ident} = excluded.{geom_ident}")];

    match geometry {
        Some(value) if !value.is_null() => {
            let blob = gpb::encode_from_geojson_geometry(srid, value, requested_crs)?;
            params.push(SqlParam::Blob(blob));
        }
        _ => params.push(SqlParam::Null),
    }
    placeholders.push(format!("?{}", params.len()));

    for (key, value) in properties {
        if !known_columns.contains(key.as_str()) {
            return Err(GeopackageError::UnwritableProperty(key.clone()));
        }
        let column = quote_ident(key)?;
        params.push(scalar_param(key, value)?);
        placeholders.push(format!("?{}", params.len()));
        set_clauses.push(format!("{column} = excluded.{column}"));
        columns.push(column);
    }

    let sql = format!(
        "INSERT INTO {table_ident} ({cols}) VALUES ({vals}) ON CONFLICT ({pk_ident}) DO UPDATE SET {sets}",
        cols = columns.join(", "),
        vals = placeholders.join(", "),
        sets = set_clauses.join(", "),
    );
    Ok(UpsertPlan { sql, params })
}

/// `SELECT <geom> FROM <table> WHERE <pk> = ?1` — the stored geometry blob
/// for one feature, read inside the write transaction so `#141`'s prior
/// extent (before the mutation) and `#142`'s current extent (after it) both
/// come from what the file actually holds rather than from the request body,
/// whose CRS the outbox never records.
///
/// SQLite is in-process, so these reads cost a page lookup on an already-open
/// connection, not a round trip — which is why this driver reads the geometry
/// twice for an upsert rather than reaching for the `RETURNING`-shaped
/// contortion `tellurion-postgis` needs to avoid a real network hop.
pub(crate) fn build_stored_geometry_plan(
    table: &str,
    pk: &str,
    geometry_column: &str,
    pk_value: i64,
) -> Result<(String, Vec<SqlParam>)> {
    let table_ident = quote_ident(table)?;
    let pk_ident = quote_ident(pk)?;
    let geom_ident = quote_ident(geometry_column)?;
    let sql = format!("SELECT {geom_ident} FROM {table_ident} WHERE {pk_ident} = ?1");
    Ok((sql, vec![SqlParam::Int(pk_value)]))
}

/// `DELETE FROM table WHERE pk = ?1` — the data-mutation half of a
/// `MutationKind::Delete` apply.
pub(crate) fn build_delete_plan(
    table: &str,
    pk: &str,
    pk_value: i64,
) -> Result<(String, Vec<SqlParam>)> {
    let table_ident = quote_ident(table)?;
    let pk_ident = quote_ident(pk)?;
    let sql = format!("DELETE FROM {table_ident} WHERE {pk_ident} = ?1");
    Ok((sql, vec![SqlParam::Int(pk_value)]))
}

/// Appends one obligation to `table`'s outbox. `sequence` is `INTEGER
/// PRIMARY KEY AUTOINCREMENT` (see `tellurion-ingest`'s own provisioning
/// module doc for why `AUTOINCREMENT`, not a plain rowid alias, matters
/// here) — the caller reads it back via `last_insert_rowid()` rather than a
/// SQL `RETURNING` clause (portable across the SQLite versions this crate's
/// bundled build might resolve to; `RETURNING` itself is available too, but
/// `last_insert_rowid()` needs no extra parsing of the result). `payload` is
/// `None` for a `Delete` obligation (a tombstone carries no feature body)
/// and `Some` for an `Upsert` (serialized to a JSON text column — SQLite has
/// no native JSONB storage class).
///
/// `extent` is `#141`/`#142`'s CRS84 record of where the feature was and
/// where it now is — a JSON text column (SQLite has no native JSON storage
/// class), or SQL `NULL` for `ObligationExtent::Unrecorded`, which is both
/// what every outbox row written before the column existed carries and what
/// this driver honestly writes for a storage CRS it cannot express in CRS84
/// (`crs::bbox_to_crs84`).
pub(crate) fn build_outbox_insert_plan(
    table: &str,
    feature_id: &str,
    kind: &str,
    payload: Option<&Value>,
    extent: ObligationExtent,
) -> Result<(String, Vec<SqlParam>)> {
    let outbox_ident = quote_ident(&outbox_table_name(table))?;
    let payload_param = match payload {
        Some(value) => SqlParam::Text(value.to_string()),
        None => SqlParam::Null,
    };
    let extent_param = match encode_extent(extent) {
        Some(value) => SqlParam::Text(value.to_string()),
        None => SqlParam::Null,
    };
    let sql = format!(
        "INSERT INTO {outbox_ident} (feature_id, kind, payload, {OUTBOX_EXTENT_COLUMN}) VALUES (?1, ?2, ?3, ?4)"
    );
    Ok((
        sql,
        vec![
            SqlParam::Text(feature_id.to_string()),
            SqlParam::Text(kind.to_string()),
            payload_param,
            extent_param,
        ],
    ))
}

/// Obligations with `sequence > after`, ascending, at most `limit` — see
/// `tellurion_core::OutboxSource::read_after`'s own contract (never skips or
/// reorders). `committed_at` (this driver's own `strftime('%Y-%m-%dT%H:%M:%fZ',
/// 'now')` text column) rides along so `Obligation::committed_at` (`#115`)
/// needs no second query — `tellurion_core::parse_utc_datetime_text` is this
/// fixed shape's own parser back into a `SystemTime`.
pub(crate) fn build_read_after_plan(
    table: &str,
    after: u64,
    limit: u32,
) -> Result<(String, Vec<SqlParam>)> {
    let outbox_ident = quote_ident(&outbox_table_name(table))?;
    let sql = format!(
        "SELECT sequence, feature_id, kind, payload, committed_at, {OUTBOX_EXTENT_COLUMN} FROM {outbox_ident} WHERE sequence > ?1 ORDER BY sequence ASC LIMIT ?2"
    );
    let after = i64::try_from(after).unwrap_or(i64::MAX);
    Ok((
        sql,
        vec![SqlParam::Int(after), SqlParam::Int(i64::from(limit))],
    ))
}

/// The highest sequence committed to `table`'s outbox — `0` (never `NULL`)
/// for an outbox with no rows yet, matching `Sequence`'s own "gaps allowed,
/// starts nowhere in particular" contract.
pub(crate) fn build_primary_high_water_plan(table: &str) -> Result<String> {
    let outbox_ident = quote_ident(&outbox_table_name(table))?;
    Ok(format!(
        "SELECT COALESCE(MAX(sequence), 0) FROM {outbox_ident}"
    ))
}

/// Removes at most `batch_size` obligations with `sequence <= floor` from
/// `table`'s outbox. The nested ordered selection keeps one retention pass
/// bounded even when a previously unavailable consumer releases a long
/// prefix at once; `floor` remains caller-owned, as required by
/// [`tellurion_core::OutboxSource::prune_before`].
pub(crate) fn build_prune_before_plan(
    table: &str,
    floor: u64,
    batch_size: u32,
) -> Result<(String, Vec<SqlParam>)> {
    let outbox_ident = quote_ident(&outbox_table_name(table))?;
    let sql = format!(
        "DELETE FROM {outbox_ident} WHERE sequence IN (SELECT sequence FROM {outbox_ident} WHERE sequence <= ?1 ORDER BY sequence ASC LIMIT ?2)"
    );
    let floor = i64::try_from(floor).unwrap_or(i64::MAX);
    Ok((
        sql,
        vec![SqlParam::Int(floor), SqlParam::Int(i64::from(batch_size))],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> HashSet<String> {
        ["name".to_string(), "population".to_string()]
            .into_iter()
            .collect()
    }

    #[test]
    fn outbox_table_name_appends_the_suffix() {
        assert_eq!(outbox_table_name("demo"), "demo_outbox");
    }

    #[test]
    fn upsert_plan_with_no_properties_writes_pk_and_geometry_only() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [1.0, 2.0]});
        let plan = build_upsert_plan(
            "demo",
            "id",
            "geom",
            4326,
            42,
            Some(&geometry),
            &Map::new(),
            &known(),
            tellurion_core::RequestedCrs::Omitted,
        )
        .unwrap();
        assert_eq!(
            plan.sql,
            "INSERT INTO \"demo\" (\"id\", \"geom\") VALUES (?1, ?2) ON CONFLICT (\"id\") DO UPDATE SET \"geom\" = excluded.\"geom\""
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Int(42), SqlParam::Blob(_)]
        ));
    }

    #[test]
    fn upsert_plan_with_a_null_geometry_binds_sql_null() {
        let plan = build_upsert_plan(
            "demo",
            "id",
            "geom",
            4326,
            1,
            None,
            &Map::new(),
            &known(),
            tellurion_core::RequestedCrs::Omitted,
        )
        .unwrap();
        assert_eq!(plan.params, vec![SqlParam::Int(1), SqlParam::Null]);
    }

    #[test]
    fn upsert_plan_binds_a_known_property_directly() {
        let mut properties = Map::new();
        properties.insert("population".to_string(), serde_json::json!(42));
        let plan = build_upsert_plan(
            "demo",
            "id",
            "geom",
            4326,
            1,
            None,
            &properties,
            &known(),
            tellurion_core::RequestedCrs::Omitted,
        )
        .unwrap();
        assert!(
            plan.sql
                .contains("\"population\" = excluded.\"population\""),
            "sql was: {}",
            plan.sql
        );
        assert_eq!(
            plan.params,
            vec![SqlParam::Int(1), SqlParam::Null, SqlParam::Int(42)]
        );
    }

    #[test]
    fn upsert_plan_rejects_an_unknown_property() {
        let mut properties = Map::new();
        properties.insert("mystery".to_string(), serde_json::json!("x"));
        assert!(matches!(
            build_upsert_plan(
                "demo",
                "id",
                "geom",
                4326,
                1,
                None,
                &properties,
                &known(),
                tellurion_core::RequestedCrs::Omitted,
            ),
            Err(GeopackageError::UnwritableProperty(key)) if key == "mystery"
        ));
    }

    #[test]
    fn upsert_plan_rejects_an_array_property_value() {
        let mut properties = Map::new();
        properties.insert("name".to_string(), serde_json::json!(["a", "b"]));
        assert!(matches!(
            build_upsert_plan(
                "demo",
                "id",
                "geom",
                4326,
                1,
                None,
                &properties,
                &known(),
                tellurion_core::RequestedCrs::Omitted,
            ),
            Err(GeopackageError::UnsupportedPropertyValue(key)) if key == "name"
        ));
    }

    #[test]
    fn delete_plan_shape() {
        let (sql, params) = build_delete_plan("demo", "id", 7).unwrap();
        assert_eq!(sql, "DELETE FROM \"demo\" WHERE \"id\" = ?1");
        assert_eq!(params, vec![SqlParam::Int(7)]);
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
                prior: Some([1.0, 2.0, 3.0, 4.0]),
                current: None,
            },
        )
        .unwrap();
        assert_eq!(
            sql,
            "INSERT INTO \"demo_outbox\" (feature_id, kind, payload, extent_crs84) VALUES (?1, ?2, ?3, ?4)"
        );
        assert!(matches!(
            params.as_slice(),
            [SqlParam::Text(a), SqlParam::Text(b), SqlParam::Text(_), SqlParam::Text(extent)]
            if a == "1" && b == "upsert"
                && extent.contains("\"prior\":[1.0,2.0,3.0,4.0]")
                && extent.contains("\"current\":null")
        ));
    }

    #[test]
    fn outbox_insert_plan_without_a_payload_binds_null() {
        let (_sql, params) =
            build_outbox_insert_plan("demo", "1", "delete", None, ObligationExtent::Unrecorded)
                .unwrap();
        assert_eq!(params.len(), 4);
        assert_eq!(params[2], SqlParam::Null);
        assert_eq!(
            params[3],
            SqlParam::Null,
            "an unrecorded extent binds SQL NULL, which is what a consumer reads as UNKNOWN"
        );
    }

    #[test]
    fn read_after_plan_shape() {
        let (sql, params) = build_read_after_plan("demo", 5, 100).unwrap();
        assert_eq!(
            sql,
            "SELECT sequence, feature_id, kind, payload, committed_at, extent_crs84 FROM \"demo_outbox\" WHERE sequence > ?1 ORDER BY sequence ASC LIMIT ?2"
        );
        assert_eq!(params, vec![SqlParam::Int(5), SqlParam::Int(100)]);
    }

    #[test]
    fn primary_high_water_plan_shape() {
        let sql = build_primary_high_water_plan("demo").unwrap();
        assert_eq!(
            sql,
            "SELECT COALESCE(MAX(sequence), 0) FROM \"demo_outbox\""
        );
    }
}
