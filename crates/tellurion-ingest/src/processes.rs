//! Durable job ledger DDL (`#182`, the Processes lane). The server never
//! creates this table — see `tellurion-postgis`'s `JobsTableMissing` error,
//! which is exactly what a job submission against an unprovisioned ledger gets
//! instead. This module is the only place the table comes from, the same
//! "ingest owns all DDL" rule `outbox.rs`/`index.rs`/`assets.rs`/`stac.rs`
//! already follow.
//!
//! **One table for the whole deployment**, unlike every other DDL command in
//! this crate. The outbox, derived index, asset records and STAC sidecar are
//! all per-collection (`"<table>_outbox"`, …); a job is not a collection's, it
//! belongs to a process and to a `(tenant, catalog)`, and `#182`'s
//! heterogeneous-runner design is several replicas claiming from **one**
//! ledger. So the name is fixed rather than derived, and this command takes no
//! `--table`.
//!
//! The name and column shape below must stay in sync with
//! `tellurion-postgis::job_sql` by hand: that crate and this one never depend
//! on each other (see this crate's own top-level doc), the same arrangement
//! `index.rs` documents for its own table.

use anyhow::Context;

/// The one ledger table — spelled here and in `tellurion-postgis::job_sql`.
const JOBS_TABLE: &str = "tellurion_jobs";

/// The DDL, in full.
///
/// - `job_id` is the primary key and is a server-minted UUID, never anything
///   a client chose: a client-chosen id in a deployment-wide table would let
///   one catalog's submission collide with another's.
/// - `status` is `CHECK`-constrained to the five values of OGC API —
///   Processes — Part 1: Core's `statusCode.yaml` (Figure 21), which is the
///   same closed vocabulary `tellurion_core::JobStatus` enumerates. The
///   constraint is the point: a status the server cannot parse is a job
///   nothing can ever read back, so the database refuses to store one.
/// - `locked_until` is the visibility timeout a claimant stamps. A claim takes
///   a row whose status is `accepted`, or one that is `running` with a
///   `locked_until` in the past — that second arm is what returns a job
///   stranded by a `SIGKILL`ed replica to the pool, and what makes the
///   contract at-least-once.
/// - `tellurion_jobs_claim_idx` is the index the claim query rides:
///   `(process_id, created)` restricted to non-terminal rows, so the ordered
///   `FOR UPDATE SKIP LOCKED` scan never walks a ledger's completed history.
///   Partial, so a ledger that accumulates a million finished jobs costs a
///   claim exactly as much as an empty one.
/// - `tellurion_jobs_dedup_idx` is the idempotency mechanism: a UNIQUE partial
///   index over `(tenant, catalog, process_id, dedup_key)` covering only rows
///   that both carry a key and are still in play. `NULL` keys are not covered
///   (a partial index's predicate excludes them, and Postgres treats NULLs as
///   distinct regardless), so a submission with no `Idempotency-Key` dedups
///   nothing — two identical submissions stay two jobs. Terminal rows are not
///   covered either, so a key becomes reusable once the job it named has
///   finished.
///
/// Everything is `IF NOT EXISTS`-idempotent, same as the rest of this crate's
/// DDL: rerunning the command is always safe.
fn create_jobs_table_sql() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {JOBS_TABLE} (
    job_id text PRIMARY KEY,
    process_id text NOT NULL,
    tenant text NOT NULL,
    catalog text NOT NULL,
    status text NOT NULL CHECK (status IN ('accepted', 'running', 'successful', 'failed', 'dismissed')),
    message text,
    inputs jsonb NOT NULL,
    results jsonb,
    dedup_key text,
    attempts integer NOT NULL DEFAULT 0,
    locked_until timestamptz,
    created timestamptz NOT NULL DEFAULT now(),
    started timestamptz,
    finished timestamptz,
    updated timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS {JOBS_TABLE}_claim_idx ON {JOBS_TABLE} (process_id, created) WHERE status IN ('accepted', 'running');
CREATE UNIQUE INDEX IF NOT EXISTS {JOBS_TABLE}_dedup_idx ON {JOBS_TABLE} (tenant, catalog, process_id, dedup_key) WHERE dedup_key IS NOT NULL AND status IN ('accepted', 'running');"
    )
}

pub struct CreateTablesArgs {
    pub database_url_env: String,
    /// Print the DDL without connecting to a database at all — same escape
    /// hatch `outbox::create_tables`/`index::create_tables` offer an operator
    /// with no direct CLI database access.
    pub dry_run: bool,
}

pub async fn create_tables(args: CreateTablesArgs) -> anyhow::Result<()> {
    let sql = create_jobs_table_sql();
    // Always printed, dry run or not — same requirement `outbox::
    // create_tables` already follows.
    println!("{sql}");
    if args.dry_run {
        return Ok(());
    }

    let client = crate::db::connect(&args.database_url_env).await?;
    // `#272`: the ledger is deployment-wide and its name is fixed, so every
    // operator who runs this command runs it against the *same* three
    // objects — the most likely of this crate's DDL commands to be issued
    // twice at once, and the one where a second `--table` cannot separate
    // two runs. Locked on the ledger's own name.
    crate::provision::apply_ddl(&client, JOBS_TABLE, &sql)
        .await
        .with_context(|| format!("creating the durable job ledger table '{JOBS_TABLE}'"))?;
    tracing::info!(table = %JOBS_TABLE, "created (or confirmed existing) the durable job ledger");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_is_idempotent_and_names_the_one_deployment_wide_ledger() {
        let sql = create_jobs_table_sql();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS tellurion_jobs"));
        assert!(sql.contains("job_id text PRIMARY KEY"));
        assert!(sql.contains("CREATE INDEX IF NOT EXISTS tellurion_jobs_claim_idx"));
        assert!(sql.contains("CREATE UNIQUE INDEX IF NOT EXISTS tellurion_jobs_dedup_idx"));
    }

    /// The `CHECK` constraint must name exactly the closed status vocabulary
    /// the server parses, in the server's own spelling. A drift here is a row
    /// the ledger accepts and `tellurion_core::JobStatus::parse` then refuses
    /// as a malformed row — which is why the assertion is written against
    /// that type rather than against a second copy of the strings.
    #[test]
    fn the_status_check_constraint_is_exactly_the_servers_closed_vocabulary() {
        let sql = create_jobs_table_sql();
        let rendered: Vec<String> = tellurion_core::JobStatus::ALL
            .into_iter()
            .map(|status| format!("'{}'", status.as_str()))
            .collect();
        assert!(
            sql.contains(&format!("CHECK (status IN ({}))", rendered.join(", "))),
            "the CHECK constraint does not match JobStatus::ALL: {sql}"
        );
    }

    /// The dedup index must cover only keyed, non-terminal rows. Both halves
    /// are load-bearing: without the `dedup_key IS NOT NULL` predicate an
    /// unkeyed submission would still be deduped, and without the status
    /// predicate an idempotency key could never be reused after its job
    /// finished.
    #[test]
    fn the_dedup_index_covers_only_keyed_jobs_still_in_play() {
        let sql = create_jobs_table_sql();
        assert!(sql.contains("WHERE dedup_key IS NOT NULL AND status IN ('accepted', 'running')"));
    }

    /// The claim index is partial too, so a ledger's finished history never
    /// slows a claim down.
    #[test]
    fn the_claim_index_skips_terminal_history() {
        let sql = create_jobs_table_sql();
        assert!(sql.contains(
            "ON tellurion_jobs (process_id, created) WHERE status IN ('accepted', 'running')"
        ));
    }
}
