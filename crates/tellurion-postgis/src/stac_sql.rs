//! Pure SQL builders for the per-item STAC metadata sidecar (`#202`) — same
//! "no I/O, every identifier whitelist-quoted, every value bound as a
//! parameter" discipline `write_sql.rs`/`index_sql.rs` follow.
//!
//! ## The per-collection sidecar table
//!
//! `stac_table_name` derives `"<table>_stac"` from a collection's physical
//! table name, mirroring `index_sql::index_table_name`'s own convention —
//! kept in sync by hand with `tellurion-ingest::stac`'s DDL, the same
//! arrangement documented there (the two crates never depend on each
//! other).
//!
//! ## One round trip per page, never one per item
//!
//! [`build_lookup_plan`] compiles the whole page's feature ids into a
//! single `feature_id = ANY($1)` predicate against a `text[]` bind
//! (`sql::SqlParam::TextArray`) rather than an `IN ($1, $2, ...)` list: one
//! statement text regardless of page size (so the plan cache sees one
//! entry, not one per distinct page size), one round trip, and the primary
//! key index serves it directly. This is the whole cost model
//! `StacMetadataSource`'s own contract states — a page of N items adds one
//! query, not N.
//!
//! The result set is deliberately sparse: ids with no sidecar row simply
//! come back missing, which the STAC lane treats as "this item has no
//! sidecar metadata", the ordinary case.

use crate::error::Result;
use crate::ident::quote_ident;
use crate::sql::SqlParam;

/// `"<table>_stac"` — see this module's own doc for why the name is a
/// hand-kept convention rather than a shared constant.
pub(crate) fn stac_table_name(table: &str) -> String {
    format!("{table}_stac")
}

/// `SELECT feature_id, doc FROM "<table>_stac" WHERE feature_id = ANY($1)`
/// — see this module's own doc for why `ANY` over an array bind rather than
/// a generated `IN` list. `version`/`updated_at` are deliberately not
/// projected: this slice's read path merges the stored document and nothing
/// else (the version stamp exists for a future applier's own conflict
/// guard, `tellurion-ingest::stac`'s DDL doc).
pub(crate) fn build_lookup_plan(
    table: &str,
    feature_ids: &[String],
) -> Result<(String, Vec<SqlParam>)> {
    let stac_table = quote_ident(&stac_table_name(table))?;
    let sql = format!(
        "SELECT feature_id, doc FROM {stac_table} WHERE feature_id = ANY($1) ORDER BY feature_id"
    );
    Ok((sql, vec![SqlParam::TextArray(feature_ids.to_vec())]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_name_follows_the_per_collection_convention() {
        assert_eq!(stac_table_name("demo"), "demo_stac");
    }

    #[test]
    fn lookup_batches_every_id_into_one_array_bind() {
        let (sql, params) =
            build_lookup_plan("demo", &["a".to_string(), "b".to_string(), "c".to_string()])
                .unwrap();
        assert!(
            sql.contains("FROM \"demo_stac\" WHERE feature_id = ANY($1)"),
            "sql was: {sql}"
        );
        // One placeholder for the whole page, not one per id.
        assert!(!sql.contains("$2"), "sql was: {sql}");
        assert_eq!(
            params,
            vec![SqlParam::TextArray(vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string()
            ])]
        );
    }

    #[test]
    fn rejects_a_table_name_that_fails_identifier_whitelisting() {
        assert!(build_lookup_plan("demo; DROP TABLE x; --", &["a".to_string()]).is_err());
    }
}
