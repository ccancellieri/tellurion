//! Pure SQL builders for the database-backed `AssetRecordStore` capability
//! (assets-and-object-storage proposal, first slice) — same "no I/O, every
//! identifier whitelist-quoted, every value bound as a parameter" discipline
//! `write_sql.rs` follows for the write lane.
//!
//! ## The per-collection asset-records table
//!
//! `assets_table_name` derives `"<table>_assets"` from a collection's
//! physical table name, the identical per-collection (never global,
//! never cross-tenant) naming convention `write_sql::outbox_table_name`
//! already uses for `"<table>_outbox"`. Kept in sync by hand with
//! `tellurion-ingest::assets`'s own DDL — the two crates never depend on
//! each other, see that module's own doc.
//!
//! Item-level and collection-level assets share one table per collection:
//! `item_id` is `''` (never SQL `NULL`) for a collection-level asset, so a
//! plain `UNIQUE (item_id, asset_key)` constraint enforces per-parent key
//! uniqueness correctly — Postgres treats two `NULL`s in a unique index as
//! distinct, which `''` sidesteps entirely.

use serde_json::Value;
use tellurion_core::{decode_base64, encode_base64, AssetKind, AssetRecord, AssetState, Digest};

use crate::error::{PostgisError, Result};
use crate::ident::quote_ident;
use crate::sql::SqlParam;

/// `"<table>_assets"` — see this module's own doc for why the name is a
/// hand-kept convention rather than a shared constant.
pub(crate) fn assets_table_name(table: &str) -> String {
    format!("{table}_assets")
}

/// SQL `''` sentinel for "no item" (collection-level) — never `NULL`, see
/// this module's own doc.
pub(crate) fn item_scope(item_id: Option<&str>) -> &str {
    item_id.unwrap_or("")
}

/// `id::text AS id`: the `id` column is a real Postgres `uuid`, cast to
/// `text` on the wire rather than binding it as `uuid::Uuid` on the Rust
/// side — the same "`$N::text::<cast>`" idiom `write_sql.rs`'s own module
/// doc describes for the opposite direction (Rust `text` bind, Postgres
/// column cast), applied here to a read: it avoids this crate taking on
/// `tokio-postgres`'s `with-uuid-1` feature for one column.
const SELECT_COLUMNS: &str = "id::text AS id, item_id, asset_key, kind, state, href, media_type, \
     title, description, roles, declared_size, digest_algorithm, digest_value, failure_reason";

fn push_text(params: &mut Vec<SqlParam>, value: &str) -> String {
    params.push(SqlParam::Text(value.to_string()));
    format!("${}", params.len())
}

fn push_opt_text(params: &mut Vec<SqlParam>, value: Option<&str>) -> String {
    match value {
        Some(v) => {
            params.push(SqlParam::Text(v.to_string()));
            format!("${}", params.len())
        }
        None => "NULL".to_string(),
    }
}

fn push_opt_bigint(params: &mut Vec<SqlParam>, value: Option<i64>) -> String {
    match value {
        Some(v) => {
            params.push(SqlParam::Bigint(v));
            format!("${}", params.len())
        }
        None => "NULL".to_string(),
    }
}

pub(crate) struct Plan {
    pub(crate) sql: String,
    pub(crate) params: Vec<SqlParam>,
}

/// One INSERT covering every column, `NULL` literal for whichever ones a
/// given `new_record.kind` doesn't use — see this module's own doc for why
/// a fixed column list (rather than two differently-shaped `INSERT`
/// statements) keeps this simple. A `UNIQUE (item_id, asset_key)` violation
/// surfaces to the caller as a raw Postgres error; `driver.rs` rewrites it
/// into the named `AssetKeyConflict` the same way it rewrites an
/// undefined-relation error into `AssetsTableMissing`.
pub(crate) fn build_register_plan(
    table: &str,
    item_id: Option<&str>,
    key: &str,
    id: uuid::Uuid,
    new_record: &tellurion_core::NewAssetRecord,
) -> Result<Plan> {
    let assets_table = quote_ident(&assets_table_name(table))?;
    let mut params = Vec::new();
    let id_ph = push_text(&mut params, &id.to_string());
    let item_ph = push_text(&mut params, item_scope(item_id));
    let key_ph = push_text(&mut params, key);

    let (kind, state, href, media_type, title, description, roles, declared_size, digest) =
        match &new_record.kind {
            tellurion_core::NewAssetKind::Managed {
                media_type,
                title,
                description,
                roles,
                declared_size,
                digest,
            } => (
                "managed",
                "pending",
                None,
                media_type.as_deref(),
                title.as_deref(),
                description.as_deref(),
                roles.as_slice(),
                Some(*declared_size as i64),
                Some(digest),
            ),
            tellurion_core::NewAssetKind::Remote {
                href,
                media_type,
                title,
                description,
                roles,
            } => (
                "remote",
                "available",
                Some(href.as_str()),
                media_type.as_deref(),
                title.as_deref(),
                description.as_deref(),
                roles.as_slice(),
                None,
                None,
            ),
        };

    let kind_ph = push_text(&mut params, kind);
    let state_ph = push_text(&mut params, state);
    let href_ph = push_opt_text(&mut params, href);
    let media_type_ph = push_opt_text(&mut params, media_type);
    let title_ph = push_opt_text(&mut params, title);
    let description_ph = push_opt_text(&mut params, description);
    let roles_json = serde_json::to_string(
        &roles
            .iter()
            .map(|r| Value::String(r.clone()))
            .collect::<Vec<_>>(),
    )
    .expect("Vec<Value::String> always serializes");
    let roles_ph = format!("{}::text::jsonb", push_text(&mut params, &roles_json));
    let declared_size_ph = push_opt_bigint(&mut params, declared_size);
    let digest_algorithm_ph = push_opt_text(&mut params, digest.map(|_| "sha-256"));
    let digest_value_ph = push_opt_text(
        &mut params,
        digest.map(|d| encode_base64(&d.value)).as_deref(),
    );

    let sql = format!(
        "INSERT INTO {assets_table} \
         (id, item_id, asset_key, kind, state, href, media_type, title, description, roles, declared_size, digest_algorithm, digest_value) \
         VALUES ({id_ph}::text::uuid, {item_ph}, {key_ph}, {kind_ph}, {state_ph}, {href_ph}, {media_type_ph}, {title_ph}, {description_ph}, {roles_ph}, {declared_size_ph}, {digest_algorithm_ph}, {digest_value_ph}) \
         RETURNING {SELECT_COLUMNS}"
    );
    Ok(Plan { sql, params })
}

pub(crate) fn build_get_plan(table: &str, item_id: Option<&str>, key: &str) -> Result<Plan> {
    let assets_table = quote_ident(&assets_table_name(table))?;
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM {assets_table} WHERE item_id = $1 AND asset_key = $2"
    );
    Ok(Plan {
        sql,
        params: vec![
            SqlParam::Text(item_scope(item_id).to_string()),
            SqlParam::Text(key.to_string()),
        ],
    })
}

pub(crate) fn build_finalize_plan(
    table: &str,
    item_id: Option<&str>,
    key: &str,
    outcome: &tellurion_core::FinalizeOutcome,
) -> Result<Plan> {
    let assets_table = quote_ident(&assets_table_name(table))?;
    let (state, reason) = match outcome {
        tellurion_core::FinalizeOutcome::Available => ("available", None),
        tellurion_core::FinalizeOutcome::Failed { reason } => ("failed", Some(reason.as_str())),
    };
    let mut params = Vec::new();
    let state_ph = push_text(&mut params, state);
    let reason_ph = push_opt_text(&mut params, reason);
    let item_ph = push_text(&mut params, item_scope(item_id));
    let key_ph = push_text(&mut params, key);
    let sql = format!(
        "UPDATE {assets_table} SET state = {state_ph}, failure_reason = {reason_ph}, updated_at = now() \
         WHERE item_id = {item_ph} AND asset_key = {key_ph} RETURNING {SELECT_COLUMNS}"
    );
    Ok(Plan { sql, params })
}

pub(crate) fn build_delete_plan(table: &str, item_id: Option<&str>, key: &str) -> Result<Plan> {
    let assets_table = quote_ident(&assets_table_name(table))?;
    let sql =
        format!("DELETE FROM {assets_table} WHERE item_id = $1 AND asset_key = $2 RETURNING {SELECT_COLUMNS}");
    Ok(Plan {
        sql,
        params: vec![
            SqlParam::Text(item_scope(item_id).to_string()),
            SqlParam::Text(key.to_string()),
        ],
    })
}

/// `AssetRecordStore::list` (reconcile surface, `#93`): every row in this
/// collection's own assets table, `item_id`/`asset_key` selected alongside
/// the standard [`SELECT_COLUMNS`] set — reconcile needs both to name a
/// drift by scope, not just by internal id. No `WHERE` clause: unlike every
/// other query in this module, this one is unscoped by design (the whole
/// table IS the collection's own scope, `asset_sql.rs`'s own module doc).
pub(crate) fn build_list_plan(table: &str) -> Result<Plan> {
    let assets_table = quote_ident(&assets_table_name(table))?;
    let sql = format!(
        "SELECT item_id, asset_key, {SELECT_COLUMNS} FROM {assets_table} \
         ORDER BY item_id, asset_key"
    );
    Ok(Plan {
        sql,
        params: Vec::new(),
    })
}

/// `AssetRecordStore::item_assets` (`#221`): every item-scoped record for a
/// whole page of item ids in ONE round trip.
///
/// `item_id = ANY($1)` over a single `text[]` bind (`SqlParam::TextArray`),
/// never a generated `IN ($1, $2, ...)` list — the identical batching
/// `stac_sql::build_lookup_plan` documents for the metadata sidecar: one
/// statement text regardless of page size (so the plan cache sees one entry
/// rather than one per distinct page size), one round trip, and the leading
/// column of the `UNIQUE (item_id, asset_key)` index this table already
/// carries (`tellurion-ingest::assets`'s DDL) serves the predicate
/// directly — no new index and no DDL change.
///
/// `item_id <> ''` is a fixed predicate, not a filter on the caller's
/// input: `''` is this module's collection-level sentinel (see the module
/// doc), and the STAC lane derives its ids from feature documents, where a
/// missing `id` member degrades to `""`. Excluding the sentinel in the SQL
/// itself means no caller can pull a collection-level asset into an Item,
/// which is exactly the scope boundary `#221` turns on.
///
/// Ordered by `(item_id, asset_key)` so the per-item asset map is built
/// from a deterministic sequence rather than whatever order the heap
/// happens to yield.
pub(crate) fn build_item_lookup_plan(table: &str, item_ids: &[String]) -> Result<Plan> {
    let assets_table = quote_ident(&assets_table_name(table))?;
    let sql = format!(
        "SELECT item_id, asset_key, {SELECT_COLUMNS} FROM {assets_table} \
         WHERE item_id = ANY($1) AND item_id <> '' ORDER BY item_id, asset_key"
    );
    Ok(Plan {
        sql,
        params: vec![SqlParam::TextArray(item_ids.to_vec())],
    })
}

/// Maps one [`build_list_plan`]-shaped row into the domain
/// `AssetRecordEntry` — `item_id`/`asset_key` plus everything
/// [`row_to_asset_record`] already extracts from the shared column set.
pub(crate) fn row_to_asset_record_entry(
    row: &tokio_postgres::Row,
) -> Result<tellurion_core::AssetRecordEntry> {
    let item_id: String = row.try_get("item_id").map_err(PostgisError::from)?;
    let key: String = row.try_get("asset_key").map_err(PostgisError::from)?;
    let record = row_to_asset_record(row)?;
    Ok(tellurion_core::AssetRecordEntry {
        item_id: (!item_id.is_empty()).then_some(item_id),
        key,
        record,
    })
}

/// Maps one `SELECT_COLUMNS`-shaped row into the domain [`AssetRecord`] —
/// shared by every query above (`register`/`get`/`finalize`/`delete` all
/// `RETURNING`/`SELECT` the identical column list).
pub(crate) fn row_to_asset_record(row: &tokio_postgres::Row) -> Result<AssetRecord> {
    let id_text: String = row.try_get("id").map_err(PostgisError::from)?;
    let id = uuid::Uuid::parse_str(&id_text).map_err(|_| {
        PostgisError::MalformedAssetRow(format!("id column '{id_text}' is not a valid uuid"))
    })?;
    let kind_text: String = row.try_get("kind").map_err(PostgisError::from)?;
    let kind = match kind_text.as_str() {
        "managed" => AssetKind::Managed,
        "remote" => AssetKind::Remote,
        other => {
            return Err(PostgisError::MalformedAssetRow(format!(
                "kind column '{other}' is neither 'managed' nor 'remote'"
            )))
        }
    };
    let state_text: String = row.try_get("state").map_err(PostgisError::from)?;
    let state = match state_text.as_str() {
        "pending" => AssetState::Pending,
        "available" => AssetState::Available,
        "failed" => AssetState::Failed,
        other => {
            return Err(PostgisError::MalformedAssetRow(format!(
                "state column '{other}' is not a recognized asset state"
            )))
        }
    };
    let href: Option<String> = row.try_get("href").map_err(PostgisError::from)?;
    let media_type: Option<String> = row.try_get("media_type").map_err(PostgisError::from)?;
    let title: Option<String> = row.try_get("title").map_err(PostgisError::from)?;
    let description: Option<String> = row.try_get("description").map_err(PostgisError::from)?;
    let roles_json: Value = row.try_get("roles").map_err(PostgisError::from)?;
    let roles = roles_json
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let declared_size: Option<i64> = row.try_get("declared_size").map_err(PostgisError::from)?;
    let digest_algorithm: Option<String> = row
        .try_get("digest_algorithm")
        .map_err(PostgisError::from)?;
    let digest_value: Option<String> = row.try_get("digest_value").map_err(PostgisError::from)?;
    let digest = match (digest_algorithm.as_deref(), digest_value) {
        (Some("sha-256"), Some(value)) => {
            let bytes = decode_base64(&value).ok_or_else(|| {
                PostgisError::MalformedAssetRow(
                    "digest_value column is not valid base64".to_string(),
                )
            })?;
            let value: [u8; 32] = bytes.try_into().map_err(|_| {
                PostgisError::MalformedAssetRow(
                    "digest_value column did not decode to 32 bytes".to_string(),
                )
            })?;
            Some(Digest::from_sha256_bytes(value))
        }
        (None, _) => None,
        (Some(other), _) => {
            return Err(PostgisError::MalformedAssetRow(format!(
                "digest_algorithm column '{other}' is not 'sha-256'"
            )))
        }
    };
    let failure_reason: Option<String> =
        row.try_get("failure_reason").map_err(PostgisError::from)?;

    Ok(AssetRecord {
        id,
        kind,
        state,
        href,
        media_type,
        title,
        description,
        roles,
        declared_size: declared_size.map(|v| v as u64),
        digest,
        failure_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tellurion_core::{Digest as CoreDigest, NewAssetKind, NewAssetRecord};

    #[test]
    fn assets_table_name_appends_the_suffix() {
        assert_eq!(assets_table_name("demo"), "demo_assets");
    }

    #[test]
    fn item_scope_defaults_absent_to_the_empty_sentinel() {
        assert_eq!(item_scope(None), "");
        assert_eq!(item_scope(Some("feature-1")), "feature-1");
    }

    #[test]
    fn register_plan_for_a_managed_asset_carries_size_and_digest() {
        let digest = CoreDigest::from_sha256_bytes([7u8; 32]);
        let new_record = NewAssetRecord {
            id: uuid::Uuid::nil(),
            kind: NewAssetKind::Managed {
                media_type: Some("image/png".to_string()),
                title: None,
                description: None,
                roles: vec!["thumbnail".to_string()],
                declared_size: 42,
                digest: digest.clone(),
            },
        };
        let plan = build_register_plan("demo", None, "thumb", new_record.id, &new_record).unwrap();
        assert!(plan.sql.contains("INSERT INTO \"demo_assets\""));
        assert!(plan.sql.contains("RETURNING"));
        // item_id, kind, state land as literal bound params, not NULLs.
        assert!(matches!(&plan.params[1], SqlParam::Text(v) if v.is_empty()));
    }

    #[test]
    fn register_plan_for_a_remote_asset_has_no_declared_size_or_digest_params() {
        let new_record = NewAssetRecord {
            id: uuid::Uuid::nil(),
            kind: NewAssetKind::Remote {
                href: "https://example.test/x".to_string(),
                media_type: None,
                title: None,
                description: None,
                roles: vec![],
            },
        };
        let plan =
            build_register_plan("demo", Some("feature-1"), "ext", new_record.id, &new_record)
                .unwrap();
        assert!(plan.sql.contains("NULL")); // declared_size/digest columns
        assert!(plan.sql.contains("$"));
    }

    #[test]
    fn get_plan_scopes_by_item_and_key() {
        let plan = build_get_plan("demo", Some("feature-1"), "thumb").unwrap();
        assert!(plan.sql.contains("WHERE item_id = $1 AND asset_key = $2"));
        assert!(matches!(&plan.params[0], SqlParam::Text(v) if v == "feature-1"));
    }

    #[test]
    fn delete_plan_returns_the_deleted_row() {
        let plan = build_delete_plan("demo", None, "thumb").unwrap();
        assert!(plan.sql.starts_with("DELETE FROM \"demo_assets\""));
        assert!(plan.sql.contains("RETURNING"));
    }

    #[test]
    fn item_lookup_batches_every_id_into_one_array_bind() {
        let plan =
            build_item_lookup_plan("demo", &["a".to_string(), "b".to_string(), "c".to_string()])
                .unwrap();
        assert!(
            plan.sql
                .contains("FROM \"demo_assets\" WHERE item_id = ANY($1)"),
            "sql was: {}",
            plan.sql
        );
        // One placeholder for the whole page, not one per id — the N+1
        // guard at the SQL layer.
        assert!(!plan.sql.contains("$2"), "sql was: {}", plan.sql);
        assert_eq!(
            plan.params,
            vec![SqlParam::TextArray(vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string()
            ])]
        );
    }

    /// The collection-level sentinel is excluded by the statement itself,
    /// so no caller — including one that passed the `""` a feature with no
    /// `id` member degrades to — can pull a collection-level asset into an
    /// Item.
    #[test]
    fn item_lookup_never_matches_the_collection_level_sentinel() {
        let plan = build_item_lookup_plan("demo", &["".to_string(), "a".to_string()]).unwrap();
        assert!(
            plan.sql.contains("AND item_id <> ''"),
            "sql was: {}",
            plan.sql
        );
    }

    #[test]
    fn item_lookup_rejects_a_table_name_that_fails_identifier_whitelisting() {
        assert!(build_item_lookup_plan("demo; DROP TABLE x; --", &["a".to_string()]).is_err());
    }

    #[test]
    fn list_plan_selects_every_row_unscoped() {
        let plan = build_list_plan("demo").unwrap();
        assert!(plan.sql.starts_with("SELECT item_id, asset_key"));
        assert!(plan.sql.contains("FROM \"demo_assets\""));
        assert!(!plan.sql.contains("WHERE"));
        assert!(plan.params.is_empty());
    }
}
