//! Derived-index table DDL (`#67`, the derived-index half of the
//! transactional-outbox design). The server never creates this table — see
//! `tellurion-postgis::driver`'s `IndexTableMissing` error, which is exactly
//! what an apply against a collection whose index was never provisioned
//! gets instead. This module is the only place the table comes from, the
//! same "ingest owns all DDL" rule `outbox.rs` already follows for the
//! outbox table.
//!
//! One table per collection, named `"<table>_index"` — `tellurion-postgis::
//! index_sql`'s own doc comment carries the matching half of this
//! convention; the two crates never depend on each other (this crate never
//! depends on a driver crate — see this crate's own top-level doc), so the
//! name and column shape below must stay in sync with that module's SQL
//! text by hand, the same arrangement `outbox.rs` documents for its own
//! table.

use anyhow::Context;

/// Whitelist-validates and double-quotes `name` for use as a SQL
/// identifier — the same rules `outbox.rs::quote_table_ident` applies (kept
/// as a local copy rather than a shared helper for the same reason
/// `outbox.rs` gives: this crate has no driver-crate dependency to share
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

/// `feature_id` is the primary key (one row per item, upserted in place —
/// never physically deleted, since a `Delete` obligation is applied as a
/// versioned tombstone; see `tellurion-postgis::index_sql`'s own doc for
/// why). `version` is the dedup/ordering stamp `IndexSink::apply`'s
/// `ON CONFLICT ... WHERE` guard compares against, and the column
/// `applied_high_water` reads `MAX(version)` off of — indexed for that
/// reason. `kind` mirrors the outbox table's own `'upsert'`/`'delete'`
/// check. `doc` is nullable: `NULL` for a delete tombstone, the whole
/// obligation payload for an upsert.
///
/// `search_text` (`#181`) is the free-text half of the same table: a stored
/// generated `tsvector` over every text-typed value under the stored
/// GeoJSON document's `properties` (`jsonb_to_tsvector`'s `'["string"]'`
/// filter — which is exactly where a STAC Item carries its
/// title/description/keywords, the latter's array elements included), GIN-
/// indexed for `tellurion-postgis::index_sql::build_search_plan`'s
/// `websearch_to_tsquery` predicate. The `'simple'` configuration is
/// load-bearing on BOTH sides of that hand-kept convention — a deliberate
/// no-stemming, no-language-guess choice; changing it means changing the
/// query side in lockstep and reprovisioning. Added via `ALTER TABLE ...
/// ADD COLUMN IF NOT EXISTS` *after* the `CREATE TABLE` (rather than inside
/// it) so rerunning this command upgrades a pre-`#181` table in place — the
/// server itself still never does DDL, it refuses a `q` against the missing
/// column by name (`tellurion-postgis`'s `SearchColumnMissing`) and points
/// back here. A tombstone's `NULL` `doc` coalesces to an empty vector, so
/// deletes never match free text. Everything is `IF NOT EXISTS`-idempotent,
/// same as the rest of this module's DDL.
fn create_index_table_sql(table: &str) -> anyhow::Result<String> {
    let index_table = quote_table_ident(&format!("{table}_index"))?;
    let version_index = quote_table_ident(&format!("{table}_index_version_idx"))?;
    let search_index = quote_table_ident(&format!("{table}_index_search_idx"))?;
    Ok(format!(
        "CREATE TABLE IF NOT EXISTS {index_table} (
    feature_id text PRIMARY KEY,
    version bigint NOT NULL,
    kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
    doc jsonb,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS {version_index} ON {index_table} (version);
ALTER TABLE {index_table} ADD COLUMN IF NOT EXISTS search_text tsvector GENERATED ALWAYS AS (jsonb_to_tsvector('simple', coalesce(doc -> 'properties', '{{}}'::jsonb), '[\"string\"]')) STORED;
CREATE INDEX IF NOT EXISTS {search_index} ON {index_table} USING GIN (search_text);"
    ))
}

pub struct CreateTablesArgs {
    pub table: String,
    pub database_url_env: String,
    /// Print the DDL without connecting to a database at all — same escape
    /// hatch `outbox::create_tables`/`registry::create_tables` offer an
    /// operator with no direct CLI database access.
    pub dry_run: bool,
}

pub async fn create_tables(args: CreateTablesArgs) -> anyhow::Result<()> {
    let sql = create_index_table_sql(&args.table)?;
    // Always printed, dry run or not — same requirement `outbox::
    // create_tables` already follows.
    println!("{sql}");
    if args.dry_run {
        return Ok(());
    }

    let client = crate::db::connect(&args.database_url_env).await?;
    // `#272`: locked on the collection's own table name. Both the `CREATE
    // TABLE` and the two `CREATE INDEX`es race — an index is a `pg_class`
    // row, and `CREATE INDEX` holds only a `ShareLock` on the table, which
    // is compatible with itself, so two sessions pass the `IF NOT EXISTS`
    // check together and the loser fails on `pg_class_relname_nsp_index`.
    // Measured on its own against an already-created table, six sessions
    // over fifteen rounds: `CREATE INDEX IF NOT EXISTS` failed 2 of 15
    // rounds with exactly that `23505`. The `ALTER TABLE ... ADD COLUMN IF
    // NOT EXISTS` between them is the one statement here that is safe
    // unaided (0 of 15) — it holds an `AccessExclusiveLock` and re-reads the
    // catalog under it — but it shares a batch with two that are not.
    crate::provision::apply_ddl(&client, &args.table, &sql)
        .await
        .with_context(|| {
            format!(
                "creating the derived-index table for collection table '{}'",
                args.table
            )
        })?;
    tracing::info!(table = %args.table, "created (or confirmed existing) the derived-index table");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_is_idempotent_and_names_the_table_after_its_data_table() {
        let sql = create_index_table_sql("demo").unwrap();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"demo_index\""));
        assert!(sql.contains("feature_id text PRIMARY KEY"));
        assert!(sql.contains("kind text NOT NULL CHECK (kind IN ('upsert', 'delete'))"));
        assert!(sql.contains("doc jsonb"));
        assert!(sql.contains(
            "CREATE INDEX IF NOT EXISTS \"demo_index_version_idx\" ON \"demo_index\" (version)"
        ));
    }

    /// `#181`: the free-text half of the DDL — the generated `search_text`
    /// column is added via `ALTER TABLE ... IF NOT EXISTS` (so a rerun
    /// upgrades a pre-`#181` table in place), its expression uses the same
    /// `'simple'` configuration `tellurion-postgis::index_sql`'s query side
    /// hardcodes, and the GIN index backing it is `IF NOT EXISTS`-idempotent
    /// like everything else here.
    #[test]
    fn ddl_provisions_the_generated_tsvector_column_and_its_gin_index() {
        let sql = create_index_table_sql("demo").unwrap();
        assert!(
            sql.contains(
                "ALTER TABLE \"demo_index\" ADD COLUMN IF NOT EXISTS search_text tsvector \
                 GENERATED ALWAYS AS (jsonb_to_tsvector('simple', \
                 coalesce(doc -> 'properties', '{}'::jsonb), '[\"string\"]')) STORED"
            ),
            "sql was: {sql}"
        );
        assert!(
            sql.contains(
                "CREATE INDEX IF NOT EXISTS \"demo_index_search_idx\" ON \"demo_index\" USING GIN (search_text)"
            ),
            "sql was: {sql}"
        );
    }

    #[test]
    fn rejects_a_table_name_that_fails_identifier_whitelisting() {
        assert!(create_index_table_sql("demo; DROP TABLE x; --").is_err());
    }

    /// Live-database test: `create_tables` against a real Postgres instance,
    /// twice (proving `CREATE TABLE IF NOT EXISTS`/`CREATE INDEX IF NOT
    /// EXISTS` make a rerun a no-op, not an error), then an `INSERT` +
    /// version-guarded upsert proving the shape matches what
    /// `tellurion-postgis`'s own `IndexSink::apply` expects. Skips
    /// gracefully unless `TELLURION_TEST_DATABASE_URL` is set, matching
    /// `outbox.rs`'s own live test.
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

        let table = "tellurion_ingest_index_test_table";
        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to the test database");
        client
            .batch_execute(&format!(
                "DROP TABLE IF EXISTS {table}_index; DROP INDEX IF EXISTS {table}_index_version_idx"
            ))
            .await
            .expect("drop any leftover index table from a previous run");

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
                &format!(
                    "INSERT INTO {table}_index (feature_id, version, kind, doc) VALUES ($1, $2, $3, $4)"
                ),
                &[&"1", &5i64, &"upsert", &serde_json::json!({"type": "Feature"})],
            )
            .await
            .expect("the created table accepts the shape the index-sink write path writes");

        let version: i64 = client
            .query_one(
                &format!("SELECT version FROM {table}_index WHERE feature_id = '1'"),
                &[],
            )
            .await
            .expect("the inserted row is readable")
            .get(0);
        assert_eq!(version, 5);

        // `#181`: the generated `search_text` column exists and indexes the
        // stored document's text-typed properties — a row carrying
        // `properties.name = 'acme harbour'` matches the same
        // `websearch_to_tsquery('simple', ...)` predicate the driver's
        // search plan compiles.
        client
            .execute(
                &format!(
                    "INSERT INTO {table}_index (feature_id, version, kind, doc) VALUES ($1, $2, $3, $4)"
                ),
                &[
                    &"2",
                    &6i64,
                    &"upsert",
                    &serde_json::json!({"type": "Feature", "properties": {"name": "acme harbour"}}),
                ],
            )
            .await
            .expect("a properties-bearing row inserts");
        let matched: String = client
            .query_one(
                &format!(
                    "SELECT feature_id FROM {table}_index WHERE search_text @@ websearch_to_tsquery('simple', 'acme')"
                ),
                &[],
            )
            .await
            .expect("exactly the properties-bearing row matches the free-text predicate")
            .get(0);
        assert_eq!(matched, "2");

        client
            .batch_execute(&format!(
                "DROP TABLE {table}_index; DROP INDEX IF EXISTS {table}_index_version_idx"
            ))
            .await
            .expect("clean up the test table");
    }
}
