//! Asset-records table DDL (assets-and-object-storage proposal, first
//! slice). The server never creates this table — see
//! `tellurion-postgis::driver`'s `AssetsTableMissing` error, which is
//! exactly what an asset operation against a collection whose asset-records
//! table was never provisioned gets instead. This module is the only place
//! the table comes from, the same "ingest owns all DDL" rule `main.rs`'s
//! own module doc states for physical collection tables.
//!
//! One table per collection, named `"<table>_assets"` — the same
//! per-collection (never global, never cross-tenant) naming convention
//! `outbox.rs` already uses for `"<table>_outbox"`.
//! `tellurion-postgis::asset_sql`'s own doc comment carries the matching
//! half of this convention; the two crates never depend on each other (this
//! crate never depends on a driver crate — see this crate's own top-level
//! doc), so the name and column shape below must stay in sync with that
//! module's SQL text by hand, the same arrangement `outbox.rs` already
//! documents.
//!
//! Collection-level and item-level assets share this one table:
//! `item_id` is `''` (never SQL `NULL`) for a collection-level asset, so
//! `UNIQUE (item_id, asset_key)` enforces per-parent key uniqueness
//! correctly — two `NULL`s never collide in a Postgres unique index, which
//! `''` sidesteps entirely.
//!
//! That unique index is also what makes `#221`'s STAC Item projection
//! cheap: its batched `item_id = ANY($1)` read (`tellurion-postgis::
//! asset_sql::build_item_lookup_plan`) is served by the index's leading
//! column, so a collection opting into `stac_item_assets` needs no
//! additional DDL beyond this command — the same table, provisioned once,
//! serves both the assets API and the Item projection.

use anyhow::Context;

/// Whitelist-validates and double-quotes `name` for use as a SQL identifier
/// — this crate's own small counterpart to `tellurion-postgis::ident::
/// quote_ident` (that crate is a driver this one never depends on, see this
/// module's own doc). Unlike `sanitize::sanitize_identifier` (which always
/// succeeds by transforming its input), this rejects outright: the assets
/// table name must exactly match `"<table>_assets"` for a real,
/// already-named data table, never a mangled variant of whatever the
/// operator typed.
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

fn create_assets_table_sql(table: &str) -> anyhow::Result<String> {
    let assets_table = quote_table_ident(&format!("{table}_assets"))?;
    Ok(format!(
        "CREATE TABLE IF NOT EXISTS {assets_table} (
    id uuid PRIMARY KEY,
    item_id text NOT NULL DEFAULT '',
    asset_key text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('managed', 'remote')),
    state text NOT NULL CHECK (state IN ('pending', 'available', 'failed')),
    href text,
    media_type text,
    title text,
    description text,
    roles jsonb NOT NULL DEFAULT '[]',
    declared_size bigint,
    digest_algorithm text,
    digest_value text,
    failure_reason text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (item_id, asset_key)
);"
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
    let sql = create_assets_table_sql(&args.table)?;
    // Always printed, dry run or not — the same "hand it to an operator
    // without CLI database access" requirement `outbox::create_tables`
    // already follows.
    println!("{sql}");
    if args.dry_run {
        return Ok(());
    }

    let client = crate::db::connect(&args.database_url_env).await?;
    // `#272`: locked on the collection's own table name, the same name a
    // live test seeding this collection's fixtures takes.
    crate::provision::apply_ddl(&client, &args.table, &sql)
        .await
        .with_context(|| {
            format!(
                "creating the asset-records table for collection table '{}'",
                args.table
            )
        })?;
    tracing::info!(table = %args.table, "created (or confirmed existing) the asset-records table");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_is_idempotent_and_names_the_table_after_its_data_table() {
        let sql = create_assets_table_sql("demo").unwrap();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"demo_assets\""));
        assert!(sql.contains("id uuid PRIMARY KEY"));
        assert!(sql.contains("item_id text NOT NULL DEFAULT ''"));
        assert!(sql.contains("kind text NOT NULL CHECK (kind IN ('managed', 'remote'))"));
        assert!(
            sql.contains("state text NOT NULL CHECK (state IN ('pending', 'available', 'failed'))")
        );
        assert!(sql.contains("UNIQUE (item_id, asset_key)"));
    }

    #[test]
    fn rejects_a_table_name_that_fails_identifier_whitelisting() {
        assert!(create_assets_table_sql("demo; DROP TABLE x; --").is_err());
    }

    /// Live-database test: `create_tables` against a real Postgres instance,
    /// twice (proving `CREATE TABLE IF NOT EXISTS` makes a rerun a no-op,
    /// not an error), then a plain `INSERT` proving the shape matches what
    /// `tellurion-postgis::asset_sql`'s own write path expects. Skips
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

        let table = "tellurion_ingest_assets_test_table";
        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to the test database");
        client
            .batch_execute(&format!("DROP TABLE IF EXISTS {table}_assets"))
            .await
            .expect("drop any leftover assets table from a previous run");

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
                    "INSERT INTO {table}_assets (id, item_id, asset_key, kind, state, href) \
                     VALUES (gen_random_uuid(), '', 'thumb', 'remote', 'available', 'https://example.test/x')"
                ),
                &[],
            )
            .await
            .expect("the created table accepts the shape the driver's asset_sql writes");

        let count: i64 = client
            .query_one(
                &format!("SELECT count(*) FROM {table}_assets WHERE asset_key = 'thumb'"),
                &[],
            )
            .await
            .expect("the inserted row is readable")
            .get(0);
        assert_eq!(count, 1);

        client
            .batch_execute(&format!("DROP TABLE {table}_assets"))
            .await
            .expect("clean up the test table");
    }
}
