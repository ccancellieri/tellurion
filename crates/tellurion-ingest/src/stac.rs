//! Per-item STAC metadata sidecar DDL (`#202`, third slice of the sidecar
//! line of work). The server never creates this table — see
//! `tellurion-postgis::driver`'s `StacTableMissing` error, which is exactly
//! what a STAC request against a collection declaring `stac_metadata: true`
//! whose sidecar was never provisioned gets instead. This module is the
//! only place the table comes from, the same "ingest owns all DDL" rule
//! `index.rs`/`assets.rs` already follow for their own tables.
//!
//! One table per collection, named `"<table>_stac"` — the same
//! per-collection (never global, never cross-tenant) naming convention
//! `outbox.rs`/`index.rs`/`assets.rs` use for `"<table>_outbox"`,
//! `"<table>_index"` and `"<table>_assets"`.
//! `tellurion-postgis::stac_sql`'s own doc comment carries the matching
//! half of this convention; the two crates never depend on each other (this
//! crate never depends on a driver crate — see this crate's own top-level
//! doc), so the name and column shape below must stay in sync with that
//! module's SQL text by hand, the same arrangement `index.rs` documents.
//!
//! Populated out-of-band, like `#201`'s geometry variants: maintaining the
//! sidecar on write (an applier with a pluggable derivation over the
//! existing outbox) is a later slice, so nothing in the server ever writes
//! a row here.

use anyhow::Context;

/// Whitelist-validates and double-quotes `name` for use as a SQL
/// identifier — the same rules `index.rs::quote_table_ident` applies (kept
/// as a local copy rather than a shared helper for the same reason that
/// module gives: this crate has no driver-crate dependency to share
/// `tellurion-postgis::ident::quote_ident` with).
fn quote_table_ident(name: &str) -> anyhow::Result<String> {
    let mut chars = name.chars();
    let first = chars
        .next()
        .filter(|c| c.is_ascii_alphabetic() || *c == '_');
    if first.is_none() || name.len() > 63 || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        anyhow::bail!(
            "'{name}' is not a valid Postgres identifier: only ASCII letters, digits, and '_' are allowed, it may not start with a digit, and it may not exceed 63 bytes"
        );
    }
    Ok(format!("\"{name}\""))
}

/// Structurally the derived index's twin (`index.rs`): `feature_id` is the
/// primary key — one row per item, upserted in place — and `doc` is the
/// versioned payload. Two deliberate differences from `"<table>_index"`,
/// both because this slice has no write path at all (see this module's own
/// doc):
///
/// - No `kind` column and a `NOT NULL` `doc`: a tombstone only exists to
///   let a version-guarded applier reject a replayed delete, and there is
///   no applier here yet. Absence of a row IS "this item has no sidecar
///   metadata", which the read side already treats as the ordinary answer.
/// - `version` is carried and indexed exactly as the index table carries
///   it, even though this slice's read path ignores it: it is the
///   dedup/ordering stamp a later applier's `ON CONFLICT ... WHERE version
///   < EXCLUDED.version` guard needs, and adding the column later would
///   mean rewriting every provisioned sidecar rather than an `ALTER TABLE`.
///   An out-of-band populator with no sequence of its own can write any
///   monotonic stamp it likes (`0` for a one-shot load).
///
/// Everything is `IF NOT EXISTS`-idempotent, same as the rest of this
/// crate's DDL.
fn create_stac_table_sql(table: &str) -> anyhow::Result<String> {
    let stac_table = quote_table_ident(&format!("{table}_stac"))?;
    let version_index = quote_table_ident(&format!("{table}_stac_version_idx"))?;
    Ok(format!(
        "CREATE TABLE IF NOT EXISTS {stac_table} (
    feature_id text PRIMARY KEY,
    version bigint NOT NULL DEFAULT 0,
    doc jsonb NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS {version_index} ON {stac_table} (version);"
    ))
}

pub struct CreateTablesArgs {
    pub table: String,
    pub database_url_env: String,
    /// Print the DDL without connecting to a database at all — same escape
    /// hatch `outbox::create_tables`/`index::create_tables` offer an
    /// operator with no direct CLI database access.
    pub dry_run: bool,
}

pub async fn create_tables(args: CreateTablesArgs) -> anyhow::Result<()> {
    let sql = create_stac_table_sql(&args.table)?;
    // Always printed, dry run or not — same requirement `outbox::
    // create_tables` already follows.
    println!("{sql}");
    if args.dry_run {
        return Ok(());
    }

    let client = crate::db::connect(&args.database_url_env).await?;
    // `#272`: locked on the collection's own table name, the same name the
    // outbox/index/assets commands for this collection take.
    crate::provision::apply_ddl(&client, &args.table, &sql)
        .await
        .with_context(|| {
            format!(
                "creating the STAC metadata sidecar table for collection table '{}'",
                args.table
            )
        })?;
    tracing::info!(table = %args.table, "created (or confirmed existing) the STAC metadata sidecar table");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_is_idempotent_and_names_the_table_after_its_data_table() {
        let sql = create_stac_table_sql("demo").unwrap();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"demo_stac\""));
        assert!(sql.contains("feature_id text PRIMARY KEY"));
        assert!(sql.contains("version bigint NOT NULL DEFAULT 0"));
        assert!(sql.contains("doc jsonb NOT NULL"));
        assert!(sql.contains(
            "CREATE INDEX IF NOT EXISTS \"demo_stac_version_idx\" ON \"demo_stac\" (version)"
        ));
    }

    #[test]
    fn rejects_a_table_name_that_fails_identifier_whitelisting() {
        assert!(create_stac_table_sql("demo; DROP TABLE x; --").is_err());
    }

    /// Live-database test: `create_tables` against a real Postgres
    /// instance, twice (proving `CREATE TABLE IF NOT EXISTS`/`CREATE INDEX
    /// IF NOT EXISTS` make a rerun a no-op, not an error), then an `INSERT`
    /// + a batched `feature_id = ANY(...)` read proving the shape matches
    /// what `tellurion-postgis::stac_sql`'s own lookup expects. Skips
    /// gracefully unless `TELLURION_TEST_DATABASE_URL` is set, matching
    /// every other live test in this workspace.
    #[tokio::test]
    async fn create_tables_is_idempotent_and_matches_the_driver_side_table_shape() {
        if std::env::var("TELLURION_TEST_DATABASE_URL").is_err() {
            eprintln!(
                "skipping create_tables_is_idempotent_and_matches_the_driver_side_table_shape: TELLURION_TEST_DATABASE_URL not set"
            );
            return;
        }
        let Ok(url) = std::env::var("DATABASE_URL") else {
            eprintln!(
                "skipping create_tables_is_idempotent_and_matches_the_driver_side_table_shape: DATABASE_URL not set"
            );
            return;
        };

        let table = "tellurion_ingest_stac_test_table";
        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to the test database");
        client
            .batch_execute(&format!("DROP TABLE IF EXISTS {table}_stac"))
            .await
            .expect("drop any leftover sidecar table from a previous run");

        create_tables(CreateTablesArgs {
            table: table.to_string(),
            database_url_env: "DATABASE_URL".to_string(),
            dry_run: false,
        })
        .await
        .expect("first create_tables call succeeds");

        // Rerun: `IF NOT EXISTS` makes this a no-op, not an error.
        create_tables(CreateTablesArgs {
            table: table.to_string(),
            database_url_env: "DATABASE_URL".to_string(),
            dry_run: false,
        })
        .await
        .expect("rerunning create_tables is idempotent");

        client
            .execute(
                &format!("INSERT INTO {table}_stac (feature_id, version, doc) VALUES ($1, $2, $3)"),
                &[
                    &"1",
                    &7i64,
                    &serde_json::json!({"properties": {"eo:cloud_cover": 12}}),
                ],
            )
            .await
            .expect("the created table accepts the shape an out-of-band populator writes");

        let ids = vec!["1".to_string(), "2".to_string()];
        let rows = client
            .query(
                &format!("SELECT feature_id, doc FROM {table}_stac WHERE feature_id = ANY($1)"),
                &[&ids],
            )
            .await
            .expect("the batched lookup the driver compiles reads the row back");
        assert_eq!(rows.len(), 1);
        let feature_id: String = rows[0].get(0);
        assert_eq!(feature_id, "1");

        client
            .batch_execute(&format!("DROP TABLE {table}_stac"))
            .await
            .expect("clean up the test table");
    }
}
