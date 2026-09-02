//! Pure builders for the durable job ledger (`#182`,
//! [`tellurion_core::job::JobStore`]): a submission/claim/outcome in, the SQL
//! text and its binds out — the same "no I/O, no state, fully unit-testable"
//! discipline `index_sql.rs`/`write_sql.rs`/`asset_sql.rs` follow.
//!
//! ## One table, named once, created elsewhere
//!
//! The ledger is deployment-wide, not per collection: `#182`'s whole
//! heterogeneous-runner design is several replicas claiming from **one**
//! ledger, so there is one fixed table name rather than a `"<table>_jobs"`
//! convention. That name is [`JOBS_TABLE`], and this crate never creates it —
//! `tellurion-ingest processes create-tables` owns the DDL, exactly as
//! `outbox`/`index`/`assets`/`stac` already do. The two crates never depend on
//! each other, so the name and column shape below must stay in sync with that
//! module's SQL text by hand, the same arrangement `index_sql.rs` documents
//! for its own table.
//!
//! ## Why `FOR UPDATE SKIP LOCKED` plus a visibility timeout, and not a lease
//!
//! [`build_claim_plan`] takes one row under a row lock other claimants skip
//! rather than block on, and stamps it with a `locked_until` in the future.
//! Those two mechanisms answer two different failure modes and neither
//! replaces the other: `SKIP LOCKED` stops two live replicas taking the same
//! row *in the same instant*, `locked_until` stops a row being stranded
//! forever by a replica that was `SIGKILL`ed between the claim and the
//! outcome. A [`tellurion_core::lease::Lease`] would answer a third question
//! — "who is the single leader?" — which is deliberately NOT asked here: a
//! single leader would funnel every job in the deployment through one replica
//! and defeat the heterogeneous-runner design the claim filter exists for.
//!
//! ## Identifiers
//!
//! Nothing here is caller-controlled: the table name is a constant and every
//! caller value is a bind. There is consequently no `quote_ident` call in this
//! module — unlike its siblings, which interpolate a collection's own
//! configured table name.

use serde_json::Value;

use crate::sql::SqlParam;

/// The one ledger table. Spelled here and in `tellurion-ingest`'s
/// `processes.rs` — see this module's own doc for why those two spellings are
/// a hand-kept convention rather than a shared constant.
pub(crate) const JOBS_TABLE: &str = "tellurion_jobs";

/// Every column a [`tellurion_core::JobRecord`] is read back from, in the
/// order [`row_to_job_record`] reads them. One constant so a `RETURNING`
/// clause and a `SELECT` can never disagree about the shape they hand the
/// same decoder.
const JOB_COLUMNS: &str = "job_id, process_id, tenant, catalog, status, message, \
     inputs, results, created, started, finished, updated, attempts";

/// The statuses a job can still be acted on from. Written once, because the
/// claim query, the dedup index predicate and the dismiss guard must all mean
/// the same thing by "still in play" — and because
/// `tellurion_core::JobStatus::is_terminal` is the Rust-side statement of the
/// identical fact.
const NON_TERMINAL: &str = "('accepted', 'running')";

pub(crate) struct Plan {
    pub(crate) sql: String,
    pub(crate) params: Vec<SqlParam>,
}

/// A JSON value bound as `text` and cast to `jsonb` in the statement — this
/// crate's own `$N::text::<cast>` idiom (see `write_sql.rs`'s doc), which
/// keeps the bind types this driver sends to a fixed, small set rather than
/// growing `SqlParam` a variant for every column type in the workspace.
fn push_json(params: &mut Vec<SqlParam>, value: &Value) -> String {
    params.push(SqlParam::Text(value.to_string()));
    format!("${}::jsonb", params.len())
}

fn push_text(params: &mut Vec<SqlParam>, value: &str) -> String {
    params.push(SqlParam::Text(value.to_string()));
    format!("${}", params.len())
}

/// An optional text bind, rendered as the literal `NULL` when absent —
/// `asset_sql.rs`'s own `push_opt_text` convention, kept local for the same
/// reason it is local there.
fn push_opt_text(params: &mut Vec<SqlParam>, value: Option<&str>) -> String {
    match value {
        Some(v) => push_text(params, v),
        None => "NULL".to_string(),
    }
}

/// `INSERT ... ON CONFLICT DO NOTHING RETURNING` for a new job.
///
/// `ON CONFLICT DO NOTHING` with no conflict target on purpose: the only two
/// unique constraints on this table are the `job_id` primary key and the
/// partial dedup index, and either one firing means the same thing to the
/// caller — "somebody already has this". Naming the partial index's own
/// predicate as a conflict target would couple this statement to the exact
/// text `tellurion-ingest` wrote it with, which is the one thing this module
/// cannot verify.
///
/// Returns zero rows when a conflict fired; the caller then reads the
/// incumbent back with [`build_dedup_lookup_plan`], which is what makes
/// enqueue idempotent rather than merely non-duplicating.
pub(crate) fn build_enqueue_plan(
    job_id: &str,
    process_id: &str,
    tenant: &str,
    catalog: &str,
    inputs: &Value,
    dedup_key: Option<&str>,
) -> Plan {
    let mut params = Vec::new();
    let job_id = push_text(&mut params, job_id);
    let process_id = push_text(&mut params, process_id);
    let tenant = push_text(&mut params, tenant);
    let catalog = push_text(&mut params, catalog);
    let inputs = push_json(&mut params, inputs);
    let dedup_key = push_opt_text(&mut params, dedup_key);
    Plan {
        sql: format!(
            "INSERT INTO {JOBS_TABLE} \
             (job_id, process_id, tenant, catalog, status, inputs, dedup_key) \
             VALUES ({job_id}, {process_id}, {tenant}, {catalog}, 'accepted', {inputs}, {dedup_key}) \
             ON CONFLICT DO NOTHING \
             RETURNING {JOB_COLUMNS}"
        ),
        params,
    }
}

/// Reads back the non-terminal job that already holds `dedup_key` in this
/// scope — the second half of an idempotent enqueue. Scoped by
/// `(tenant, catalog, process_id)` as well as the key, because the dedup index
/// is: one client's idempotency key must not collide with another catalog's.
pub(crate) fn build_dedup_lookup_plan(
    tenant: &str,
    catalog: &str,
    process_id: &str,
    dedup_key: &str,
) -> Plan {
    let mut params = Vec::new();
    let tenant = push_text(&mut params, tenant);
    let catalog = push_text(&mut params, catalog);
    let process_id = push_text(&mut params, process_id);
    let dedup_key = push_text(&mut params, dedup_key);
    Plan {
        sql: format!(
            "SELECT {JOB_COLUMNS} FROM {JOBS_TABLE} \
             WHERE tenant = {tenant} AND catalog = {catalog} \
               AND process_id = {process_id} AND dedup_key = {dedup_key} \
               AND status IN {NON_TERMINAL} \
             LIMIT 1"
        ),
        params,
    }
}

/// One job by id, scoped to the catalog asking for it.
///
/// The scope predicate is not decoration: a job id is a bare UUID on the wire,
/// and without it a client of catalog A could read — or dismiss — catalog B's
/// job by guessing or by having once been told an id. A job outside the scope
/// answers exactly like a job that never existed.
pub(crate) fn build_get_plan(tenant: &str, catalog: &str, job_id: &str) -> Plan {
    let mut params = Vec::new();
    let job_id = push_text(&mut params, job_id);
    let tenant = push_text(&mut params, tenant);
    let catalog = push_text(&mut params, catalog);
    Plan {
        sql: format!(
            "SELECT {JOB_COLUMNS} FROM {JOBS_TABLE} \
             WHERE job_id = {job_id} AND tenant = {tenant} AND catalog = {catalog}"
        ),
        params,
    }
}

/// Takes the oldest claimable job whose process this replica can run.
///
/// Claimable means either never started (`accepted`) or started by a claimant
/// whose visibility window has lapsed (`running` with `locked_until < now()`)
/// — the second arm is the whole recovery story for a replica that died
/// mid-job, and it is why this is at-least-once rather than at-most-once.
///
/// `process_ids` is bound as one `text[]` (`= ANY($1)`) rather than an
/// interpolated `IN (...)` list, for the reason `SqlParam::TextArray`'s own
/// doc gives: one statement shape regardless of how many runners a build
/// registers. An empty array matches nothing, which is exactly what a binary
/// with no runners should claim.
pub(crate) fn build_claim_plan(process_ids: &[String], visibility_secs: f64) -> Plan {
    let mut params: Vec<SqlParam> = vec![
        SqlParam::TextArray(process_ids.to_vec()),
        SqlParam::Float8(visibility_secs),
    ];
    // Kept explicit rather than derived from `params.len()` so the statement
    // below reads as the fixed two-bind statement it is.
    debug_assert_eq!(params.len(), 2);
    params.shrink_to_fit();
    Plan {
        sql: format!(
            "UPDATE {JOBS_TABLE} SET \
                status = 'running', \
                attempts = attempts + 1, \
                started = COALESCE(started, now()), \
                updated = now(), \
                locked_until = now() + make_interval(secs => $2::double precision) \
             WHERE job_id = ( \
                SELECT job_id FROM {JOBS_TABLE} \
                WHERE process_id = ANY($1::text[]) \
                  AND (status = 'accepted' \
                       OR (status = 'running' AND locked_until IS NOT NULL AND locked_until < now())) \
                ORDER BY created \
                FOR UPDATE SKIP LOCKED \
                LIMIT 1 \
             ) \
             RETURNING {JOB_COLUMNS}"
        ),
        params,
    }
}

/// Records a claimed job's terminal outcome.
///
/// Guarded on `status = 'running'`, so a job dismissed while it ran, or one
/// whose visibility lapsed and was re-claimed elsewhere, updates zero rows and
/// the caller learns it no longer owns the outcome. Deliberately not guarded
/// on `locked_until`: a runner that overran its window by a second should
/// still be allowed to record what it found if nobody else took the job, and a
/// second claimant that DID take it has already moved `started`/`attempts`
/// on — the `running` guard alone cannot tell those apart, which is precisely
/// why this contract is at-least-once and says so.
pub(crate) fn build_finish_plan(
    job_id: &str,
    status: &str,
    message: Option<&str>,
    results: Option<&Value>,
) -> Plan {
    let mut params = Vec::new();
    let job_id = push_text(&mut params, job_id);
    let status = push_text(&mut params, status);
    let message = push_opt_text(&mut params, message);
    let results = match results {
        Some(value) => push_json(&mut params, value),
        None => "NULL".to_string(),
    };
    Plan {
        sql: format!(
            "UPDATE {JOBS_TABLE} SET \
                status = {status}, \
                message = {message}, \
                results = {results}, \
                finished = now(), \
                updated = now(), \
                locked_until = NULL \
             WHERE job_id = {job_id} AND status = 'running' \
             RETURNING {JOB_COLUMNS}"
        ),
        params,
    }
}

/// Moves a still-in-play job to `dismissed`.
///
/// Updates zero rows for a job that is already terminal — the caller reads it
/// back and reports it unchanged rather than rewriting a `successful` job's
/// recorded status to satisfy a dismissal response. `finished` is stamped
/// because a dismissed job has, in fact, stopped.
pub(crate) fn build_dismiss_plan(tenant: &str, catalog: &str, job_id: &str, message: &str) -> Plan {
    let mut params = Vec::new();
    let job_id = push_text(&mut params, job_id);
    let tenant = push_text(&mut params, tenant);
    let catalog = push_text(&mut params, catalog);
    let message = push_text(&mut params, message);
    Plan {
        sql: format!(
            "UPDATE {JOBS_TABLE} SET \
                status = 'dismissed', \
                message = {message}, \
                finished = now(), \
                updated = now(), \
                locked_until = NULL \
             WHERE job_id = {job_id} AND tenant = {tenant} AND catalog = {catalog} \
               AND status IN {NON_TERMINAL} \
             RETURNING {JOB_COLUMNS}"
        ),
        params,
    }
}

/// Decodes one ledger row. A `status` outside the closed vocabulary is a
/// storage anomaly (a hand-edited row, a schema drift), refused by name rather
/// than coerced onto a neighbouring state — the same treatment
/// `MalformedAssetRow`/`MalformedStacRow` already get.
pub(crate) fn row_to_job_record(row: &tokio_postgres::Row) -> crate::error::Result<JobRow> {
    use crate::error::PostgisError;
    let status_text: String = row.try_get("status")?;
    let status = tellurion_core::JobStatus::parse(&status_text)
        .ok_or_else(|| PostgisError::MalformedJobRow(format!("unknown status '{status_text}'")))?;
    Ok(JobRow {
        record: tellurion_core::JobRecord {
            job_id: row.try_get("job_id")?,
            process_id: row.try_get("process_id")?,
            scope: tellurion_core::JobScope::new(
                row.try_get::<_, String>("tenant")?,
                row.try_get::<_, String>("catalog")?,
            ),
            status,
            message: row.try_get("message")?,
            inputs: row.try_get("inputs")?,
            results: row.try_get("results")?,
            created: row.try_get("created")?,
            started: row.try_get("started")?,
            finished: row.try_get("finished")?,
            updated: row.try_get("updated")?,
            attempts: row.try_get("attempts")?,
        },
    })
}

/// Newtype so [`row_to_job_record`] can live in this crate without this module
/// having to name `tellurion_core::JobRecord` in its return position twice.
pub(crate) struct JobRow {
    pub(crate) record: tellurion_core::JobRecord,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every statement reads its rows back through the same column list, so a
    /// column added to one and forgotten in another cannot compile past this.
    #[test]
    fn every_statement_projects_the_same_column_list() {
        let plans = [
            build_enqueue_plan("j", "p", "t", "c", &json!({}), None).sql,
            build_dedup_lookup_plan("t", "c", "p", "k").sql,
            build_get_plan("t", "c", "j").sql,
            build_claim_plan(&["p".to_string()], 30.0).sql,
            build_finish_plan("j", "successful", None, Some(&json!({}))).sql,
            build_dismiss_plan("t", "c", "j", "Job dismissed").sql,
        ];
        for sql in plans {
            assert!(sql.contains(JOB_COLUMNS), "missing column list in: {sql}");
        }
    }

    /// The claim query is the whole concurrency story: it must skip rows
    /// another claimant holds, order oldest-first, take exactly one, filter on
    /// the caller's own runner set, and re-offer a job whose visibility
    /// lapsed. Each of those is a separate way to get at-least-once wrong.
    #[test]
    fn the_claim_query_skips_locked_rows_and_reclaims_lapsed_ones() {
        let plan = build_claim_plan(&["index-rebuild".to_string()], 300.0);
        assert!(plan.sql.contains("FOR UPDATE SKIP LOCKED"));
        assert!(plan.sql.contains("ORDER BY created"));
        assert!(plan.sql.contains("LIMIT 1"));
        assert!(plan.sql.contains("process_id = ANY($1::text[])"));
        assert!(plan.sql.contains("locked_until < now()"));
        assert!(plan.sql.contains("attempts = attempts + 1"));
        assert_eq!(plan.params.len(), 2);
        assert!(matches!(
            &plan.params[0],
            SqlParam::TextArray(ids) if ids == &vec!["index-rebuild".to_string()]
        ));
        assert!(
            matches!(&plan.params[1], SqlParam::Float8(v) if (*v - 300.0).abs() < f64::EPSILON)
        );
    }

    /// A binary with no runners binds an empty array, which matches no row —
    /// the SQL-side half of "a replica claims only what it can execute".
    #[test]
    fn a_claim_with_no_registered_runner_binds_an_empty_array() {
        let plan = build_claim_plan(&[], 30.0);
        assert!(matches!(&plan.params[0], SqlParam::TextArray(ids) if ids.is_empty()));
    }

    /// Enqueue is idempotent by conflict, not by pre-check: a racing pair of
    /// submissions must both reach the database and let one lose.
    #[test]
    fn enqueue_lets_the_database_settle_a_duplicate() {
        let plan = build_enqueue_plan("j", "p", "t", "c", &json!({"a": 1}), Some("key"));
        assert!(plan.sql.contains("ON CONFLICT DO NOTHING"));
        assert!(plan.sql.contains("'accepted'"));
        assert_eq!(plan.params.len(), 6);
        assert!(matches!(&plan.params[4], SqlParam::Text(v) if v == "{\"a\":1}"));
        assert!(matches!(&plan.params[5], SqlParam::Text(v) if v == "key"));
    }

    /// No dedup key means no dedup: the column is bound `NULL`, which the
    /// partial unique index deliberately does not cover, so two identical
    /// submissions stay two jobs.
    #[test]
    fn a_submission_with_no_dedup_key_binds_null_and_dedups_nothing() {
        let plan = build_enqueue_plan("j", "p", "t", "c", &json!({}), None);
        assert!(plan.sql.contains(", NULL)"));
        assert_eq!(plan.params.len(), 5);
    }

    /// Reads and dismissals are scoped to the catalog that asked, so a job id
    /// leaked across catalogs discloses (and destroys) nothing.
    #[test]
    fn reads_and_dismissals_are_scoped_to_the_asking_catalog() {
        for sql in [
            build_get_plan("t", "c", "j").sql,
            build_dismiss_plan("t", "c", "j", "Job dismissed").sql,
        ] {
            assert!(sql.contains("tenant = $"), "unscoped statement: {sql}");
            assert!(sql.contains("catalog = $"), "unscoped statement: {sql}");
        }
    }

    /// An outcome may only be recorded for a job this claimant still owns.
    #[test]
    fn an_outcome_is_only_recorded_for_a_still_running_job() {
        let plan = build_finish_plan("j", "failed", Some("boom"), None);
        assert!(plan.sql.contains("AND status = 'running'"));
        assert!(plan.sql.contains("results = NULL"));
        assert!(plan.sql.contains("locked_until = NULL"));
    }

    /// Dismissal never rewrites a terminal job's recorded status.
    #[test]
    fn dismissal_only_touches_a_job_still_in_play() {
        let plan = build_dismiss_plan("t", "c", "j", "Job dismissed");
        assert!(plan.sql.contains("status IN ('accepted', 'running')"));
    }
}
