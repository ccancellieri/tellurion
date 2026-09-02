//! Pure SQL builders for the derived-index lane (`#67`, the derived-index
//! half of the transactional-outbox design): a `CollectionDecl` + obligation
//! in, SQL text + typed params out — same discipline `write_sql.rs` follows.
//!
//! ## The per-collection index table
//!
//! `index_table_name` derives `"<table>_index"` from a collection's physical
//! table name, mirroring `write_sql::outbox_table_name`'s own convention —
//! kept in sync by hand with `tellurion-ingest::index`'s DDL, the same
//! arrangement documented there. Since `#181` that DDL also provisions a
//! generated `search_text` `tsvector` column (GIN-backed) over the stored
//! document's text-typed properties; [`build_search_plan`]'s free-text
//! predicate is the query half of that hand-kept pairing.
//!
//! One row per `feature_id`, never physically deleted: a `Delete` obligation
//! stores a versioned tombstone (`kind = 'delete'`, `doc = NULL`) rather
//! than removing the row, so [`build_apply_plan`]'s `ON CONFLICT ... DO
//! UPDATE ... WHERE` version guard has something to compare against on a
//! replayed or out-of-order delete, and so [`build_high_water_plan`] can
//! read the applied high-water mark straight off the table's own data —
//! `MAX(version)` across every row IS the highest primary sequence this
//! index has durably applied, with no separate cursor row to keep
//! consistent with it (a second, independently-written cursor is exactly
//! the kind of "two stores that can disagree" shape this design doc rules
//! out — see its own section 1).

use serde_json::Value;
use tellurion_core::{MutationKind, Obligation};

use crate::error::Result;
use crate::ident::quote_ident;
use crate::sql::SqlParam;

/// `"<table>_index"` — see this module's own doc for why the name is a
/// hand-kept convention rather than a shared constant.
pub(crate) fn index_table_name(table: &str) -> String {
    format!("{table}_index")
}

pub(crate) struct ApplyPlan {
    pub(crate) sql: String,
    pub(crate) params: Vec<SqlParam>,
}

/// `INSERT ... ON CONFLICT (feature_id) DO UPDATE ... WHERE version <
/// EXCLUDED.version` — the whole idempotency mechanism `IndexSink::apply`'s
/// contract calls for (`tellurion_core::outbox`'s own doc): a replayed or
/// out-of-order obligation whose version does not exceed what's already
/// stored fails the `WHERE` guard, so `ON CONFLICT DO UPDATE` leaves the row
/// untouched instead of overwriting it — a genuine no-op, not a "same value
/// written again" one. `doc` is `NULL` for a `Delete` tombstone (see this
/// module's own doc for why the row still exists) and the whole obligation
/// payload for an `Upsert`.
pub(crate) fn build_apply_plan(table: &str, obligation: &Obligation) -> Result<ApplyPlan> {
    let index_table = quote_ident(&index_table_name(table))?;
    let (kind, doc): (&str, Option<&Value>) = match &obligation.kind {
        MutationKind::Upsert(value) => ("upsert", Some(value)),
        MutationKind::Delete => ("delete", None),
    };
    let mut params = vec![
        SqlParam::Text(obligation.feature_id.clone()),
        SqlParam::Bigint(i64::try_from(obligation.version.0).unwrap_or(i64::MAX)),
        SqlParam::Text(kind.to_string()),
    ];
    let doc_placeholder = match doc {
        Some(value) => {
            params.push(SqlParam::Text(value.to_string()));
            format!("${}::text::jsonb", params.len())
        }
        None => "NULL".to_string(),
    };
    let sql = format!(
        "INSERT INTO {index_table} (feature_id, version, kind, doc) VALUES ($1, $2, $3, {doc_placeholder}) \
         ON CONFLICT (feature_id) DO UPDATE SET version = EXCLUDED.version, kind = EXCLUDED.kind, doc = EXCLUDED.doc, updated_at = now() \
         WHERE {index_table}.version < EXCLUDED.version"
    );
    Ok(ApplyPlan { sql, params })
}

/// The highest `version` durably stored in `table`'s index — `0` (never
/// `NULL`) for an index with no rows yet, matching `Sequence`'s own "gaps
/// allowed, starts nowhere in particular" contract (same convention
/// `write_sql::build_primary_high_water_plan` already uses for the outbox).
pub(crate) fn build_high_water_plan(table: &str) -> Result<String> {
    let index_table = quote_ident(&index_table_name(table))?;
    Ok(format!(
        "SELECT COALESCE(MAX(version), 0)::bigint AS high_water FROM {index_table}"
    ))
}

/// `SearchSource::search`'s query (`#67`, free text `#181`): every
/// non-tombstoned document in `table`'s index, ordered by `feature_id` for
/// a stable read, bounded by `limit`. Deliberately as plain as
/// `SearchQuery` itself (see that type's own doc for why it carries nothing
/// richer yet) — no filter, no bbox, no paging token; widening this needs
/// `SearchQuery` to grow first, not a change here alone. A `Delete`
/// tombstone's `doc` is `NULL` (see this module's own doc), so filtering on
/// `kind = 'upsert'` alone would already exclude it, but the query also
/// excludes a `NULL` `doc` explicitly in case that invariant is ever
/// violated by a hand-run migration.
///
/// `q` (`#181`) compiles to `search_text @@ websearch_to_tsquery('simple',
/// $2)` against the GIN-backed generated `tsvector` column
/// `tellurion-ingest index create-tables` provisions (that module's DDL is
/// the other half of this hand-kept convention, same as the table name
/// itself). `websearch_to_tsquery` is deliberately the forgiving parser:
/// it never errors on raw user input, so a garbled `q` narrows to nothing
/// rather than surfacing a SQL error. Its `'simple'` configuration is
/// load-bearing and must match the DDL's generated-column expression
/// exactly — mismatched configurations would make the predicate and the
/// stored vectors tokenize differently and silently miss matches. Results
/// stay `feature_id`-ordered: there is no relevance ranking in this slice
/// (`ts_rank` is future work, not something to half-do now).
pub(crate) fn build_search_plan(
    table: &str,
    limit: u32,
    q: Option<&str>,
) -> Result<(String, Vec<SqlParam>)> {
    let index_table = quote_ident(&index_table_name(table))?;
    let mut params = vec![SqlParam::Bigint(i64::from(limit))];
    let text_predicate = match q {
        Some(q) => {
            params.push(SqlParam::Text(q.to_string()));
            format!(
                " AND search_text @@ websearch_to_tsquery('simple', ${})",
                params.len()
            )
        }
        None => String::new(),
    };
    let sql = format!(
        "SELECT doc FROM {index_table} WHERE kind = 'upsert' AND doc IS NOT NULL{text_predicate} ORDER BY feature_id ASC LIMIT $1"
    );
    Ok((sql, params))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tellurion_core::Sequence;

    fn upsert_obligation() -> Obligation {
        Obligation {
            sequence: Sequence(5),
            feature_id: "1".to_string(),
            kind: MutationKind::Upsert(serde_json::json!({"type": "Feature"})),
            version: Sequence(5),
            committed_at: std::time::SystemTime::UNIX_EPOCH,
            extent: tellurion_core::ObligationExtent::Unrecorded,
        }
    }

    fn delete_obligation() -> Obligation {
        Obligation {
            sequence: Sequence(6),
            feature_id: "1".to_string(),
            kind: MutationKind::Delete,
            version: Sequence(6),
            committed_at: std::time::SystemTime::UNIX_EPOCH,
            extent: tellurion_core::ObligationExtent::Unrecorded,
        }
    }

    #[test]
    fn index_table_name_appends_the_suffix() {
        assert_eq!(index_table_name("demo"), "demo_index");
    }

    #[test]
    fn apply_plan_for_an_upsert_carries_the_payload() {
        let plan = build_apply_plan("demo", &upsert_obligation()).unwrap();
        assert!(
            plan.sql.starts_with(
                "INSERT INTO \"demo_index\" (feature_id, version, kind, doc) VALUES ($1, $2, $3, $4::text::jsonb)"
            ),
            "sql was: {}",
            plan.sql
        );
        assert!(
            plan.sql.contains("ON CONFLICT (feature_id) DO UPDATE"),
            "sql was: {}",
            plan.sql
        );
        assert!(
            plan.sql
                .contains("WHERE \"demo_index\".version < EXCLUDED.version"),
            "sql was: {}",
            plan.sql
        );
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Text(a), SqlParam::Bigint(5), SqlParam::Text(b), SqlParam::Text(_)]
            if a == "1" && b == "upsert"
        ));
    }

    #[test]
    fn apply_plan_for_a_delete_writes_a_versioned_tombstone_with_no_row_removal() {
        let plan = build_apply_plan("demo", &delete_obligation()).unwrap();
        assert!(
            plan.sql.contains("VALUES ($1, $2, $3, NULL) ON CONFLICT"),
            "sql was: {}",
            plan.sql
        );
        assert!(!plan.sql.to_uppercase().contains("DELETE FROM"));
        assert!(matches!(
            plan.params.as_slice(),
            [SqlParam::Text(a), SqlParam::Bigint(6), SqlParam::Text(b)]
            if a == "1" && b == "delete"
        ));
    }

    #[test]
    fn high_water_plan_shape() {
        let sql = build_high_water_plan("demo").unwrap();
        assert_eq!(
            sql,
            "SELECT COALESCE(MAX(version), 0)::bigint AS high_water FROM \"demo_index\""
        );
    }

    #[test]
    fn search_plan_excludes_tombstones_and_binds_the_limit() {
        let (sql, params) = build_search_plan("demo", 25, None).unwrap();
        assert_eq!(
            sql,
            "SELECT doc FROM \"demo_index\" WHERE kind = 'upsert' AND doc IS NOT NULL ORDER BY feature_id ASC LIMIT $1"
        );
        assert!(matches!(params.as_slice(), [SqlParam::Bigint(25)]));
    }

    /// `#181`: the free-text predicate binds `q` as a parameter (never
    /// interpolated) through the forgiving `websearch_to_tsquery` parser,
    /// against the same `'simple'` configuration the DDL's generated column
    /// uses — see `build_search_plan`'s own doc for why that pairing is
    /// load-bearing.
    #[test]
    fn search_plan_with_q_compiles_a_bound_websearch_tsquery_predicate() {
        let (sql, params) = build_search_plan("demo", 25, Some("acme harbour")).unwrap();
        assert_eq!(
            sql,
            "SELECT doc FROM \"demo_index\" WHERE kind = 'upsert' AND doc IS NOT NULL \
             AND search_text @@ websearch_to_tsquery('simple', $2) \
             ORDER BY feature_id ASC LIMIT $1"
        );
        assert!(matches!(
            params.as_slice(),
            [SqlParam::Bigint(25), SqlParam::Text(q)] if q == "acme harbour"
        ));
    }

    /// A `q` full of tsquery syntax stays an inert bound parameter — the
    /// plan text never changes shape with `q`'s content.
    #[test]
    fn search_plan_never_interpolates_q_into_the_sql_text() {
        let hostile = "'; DROP TABLE demo_index; --";
        let (sql, params) = build_search_plan("demo", 5, Some(hostile)).unwrap();
        assert!(!sql.contains("DROP TABLE"), "sql was: {sql}");
        assert!(matches!(
            params.as_slice(),
            [SqlParam::Bigint(5), SqlParam::Text(q)] if q == hostile
        ));
    }
}
