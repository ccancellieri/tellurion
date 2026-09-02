//! Outbox table DDL (`#25`, the transactional-outbox design). The server
//! never creates this table — see `tellurion-postgis::driver`'s
//! `OutboxTableMissing` error, which is exactly what a write against a
//! collection whose outbox was never provisioned gets instead. This module
//! is the only place the table comes from, the same "ingest owns all DDL"
//! rule `main.rs`'s own module doc states for physical collection tables.
//!
//! One table per collection, named `"<table>_outbox"` — the transactional
//! outbox design doc's invariant 2 (per collection, never a global
//! cross-tenant obligation table). `tellurion-postgis::write_sql`'s own doc
//! comment carries the matching half of this convention; the two crates
//! never depend on each other (this crate never depends on a driver crate —
//! see this crate's own top-level doc), so the name and column shape below
//! must stay in sync with that module's SQL text by hand, the same
//! arrangement `registry.rs` already documents for the relational registry
//! tables.

use anyhow::Context;

/// Whitelist-validates and double-quotes `name` for use as a SQL identifier —
/// this crate's own small counterpart to `tellurion-postgis::ident::
/// quote_ident` (that crate is a driver this one never depends on, see this
/// module's own doc). Unlike `sanitize::sanitize_identifier` (which always
/// succeeds by transforming its input), this rejects outright: the outbox
/// table name must exactly match `"<table>_outbox"` for a real, already-named
/// data table, never a mangled variant of whatever the operator typed.
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

/// `sequence` is `bigserial PRIMARY KEY` — the per-collection monotonic
/// commit order the design doc's section 8 calls for directly from a
/// Postgres sequence, no separate counter table. `kind` is `'upsert'` or
/// `'delete'`, matching `tellurion_core::MutationKind`'s two variants
/// one-for-one. `payload` is nullable: an `Upsert` obligation carries the
/// whole GeoJSON Feature to derive from; a `Delete` tombstone carries none.
///
/// `extent_crs84` (`#141`, `#142`) is the write path's own record of where
/// the mutated feature was and where it now is, in CRS84 —
/// `{"prior": [minlon, minlat, maxlon, maxlat] | null, "current": ... }` —
/// computed by the storage inside the same transaction as the mutation.
/// The consumer that maps a write to tile-cache buckets reads this and only
/// this: the `payload` is the client's feature verbatim, in whatever CRS
/// its `Content-Crs` declared, so reading ITS coordinates as CRS84 is a
/// guess that silently invalidates the wrong buckets when it is wrong, and
/// a `Delete`'s payload is `NULL` besides.
///
/// Added via `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` *after* the `CREATE
/// TABLE` (rather than inside it) so rerunning this command upgrades a
/// pre-`#141` table in place — the same idiom `index::create_index_table_sql`
/// already uses for its own grown column. The server itself still never does
/// DDL: a write against an outbox table that lacks this column refuses by
/// name (`tellurion-postgis`'s `OutboxExtentColumnMissing`) and points back
/// here. Rows written before the upgrade keep a `NULL` here, which the
/// consumer reads as "unknown" and degrades conservatively on rather than
/// mistaking for "nothing moved".
fn create_outbox_table_sql(table: &str) -> anyhow::Result<String> {
    let outbox_table = quote_table_ident(&format!("{table}_outbox"))?;
    Ok(format!(
        "CREATE TABLE IF NOT EXISTS {outbox_table} (
    sequence bigserial PRIMARY KEY,
    feature_id text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('upsert', 'delete')),
    payload jsonb,
    committed_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE {outbox_table} ADD COLUMN IF NOT EXISTS extent_crs84 jsonb;"
    ))
}

pub struct CreateTablesArgs {
    pub table: String,
    pub database_url_env: String,
    /// Print the DDL without connecting to a database at all — same escape
    /// hatch `registry::create_tables` offers for an operator with no direct
    /// CLI database access.
    pub dry_run: bool,
}

pub async fn create_tables(args: CreateTablesArgs) -> anyhow::Result<()> {
    let sql = create_outbox_table_sql(&args.table)?;
    // Always printed, dry run or not — the same "hand it to an operator
    // without CLI database access" requirement `registry::create_tables`
    // already follows.
    println!("{sql}");
    if args.dry_run {
        return Ok(());
    }

    let client = crate::db::connect(&args.database_url_env).await?;
    // `#272`: locked on the collection's own table name, which is also what
    // a live test seeding the same collection locks — the `_outbox`
    // companion this batch creates is covered by the same lock.
    crate::provision::apply_ddl(&client, &args.table, &sql)
        .await
        .with_context(|| {
            format!(
                "creating the outbox table for collection table '{}'",
                args.table
            )
        })?;
    tracing::info!(table = %args.table, "created (or confirmed existing) the outbox table");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_is_idempotent_and_names_the_table_after_its_data_table() {
        let sql = create_outbox_table_sql("demo").unwrap();
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS \"demo_outbox\""));
        assert!(sql.contains("sequence bigserial PRIMARY KEY"));
        assert!(sql.contains("kind text NOT NULL CHECK (kind IN ('upsert', 'delete'))"));
        assert!(sql.contains("payload jsonb"));
    }

    /// `#141`/`#142`: the extent column arrives through an `ALTER TABLE ...
    /// ADD COLUMN IF NOT EXISTS`, not inside the `CREATE TABLE`, so that
    /// rerunning this command upgrades an outbox table provisioned before it
    /// existed instead of silently leaving it behind.
    #[test]
    fn the_extent_column_is_added_idempotently_so_a_rerun_upgrades_an_existing_table() {
        let sql = create_outbox_table_sql("demo").unwrap();
        assert!(sql
            .contains("ALTER TABLE \"demo_outbox\" ADD COLUMN IF NOT EXISTS extent_crs84 jsonb;"));
        let create = sql
            .split_once("ALTER TABLE")
            .expect("the ALTER follows the CREATE")
            .0;
        assert!(
            !create.contains("extent_crs84"),
            "the column must NOT be inside the CREATE TABLE, or a pre-existing table never gains it"
        );
    }

    #[test]
    fn rejects_a_table_name_that_fails_identifier_whitelisting() {
        assert!(create_outbox_table_sql("demo; DROP TABLE x; --").is_err());
    }

    /// Live-database test: `create_tables` against a real Postgres instance,
    /// twice (proving `CREATE TABLE IF NOT EXISTS` makes a rerun a no-op,
    /// not an error), then a plain `INSERT` proving the shape matches what
    /// `tellurion-postgis`'s own write path expects (`sequence` assigns
    /// itself, `kind` is checked, `payload` is nullable). Skips gracefully
    /// unless `TELLURION_TEST_DATABASE_URL` is set, matching every other
    /// live test in this workspace. Uses `DATABASE_URL` (not a second env
    /// var) as `database_url_env` — both name the same connection string in
    /// this workspace's test setup.
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

        let table = "tellurion_ingest_outbox_test_table";
        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to the test database");
        client
            .batch_execute(&format!("DROP TABLE IF EXISTS {table}_outbox"))
            .await
            .expect("drop any leftover outbox table from a previous run");

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
                    "INSERT INTO {table}_outbox (feature_id, kind, payload) VALUES ($1, $2, $3)"
                ),
                &[&"1", &"upsert", &serde_json::json!({"type": "Feature"})],
            )
            .await
            .expect("the created table accepts the shape the write path writes");

        let sequence: i64 = client
            .query_one(
                &format!("SELECT sequence FROM {table}_outbox WHERE feature_id = '1'"),
                &[],
            )
            .await
            .expect("the inserted row is readable")
            .get(0);
        assert_eq!(sequence, 1, "sequence assigns itself via bigserial");

        client
            .batch_execute(&format!("DROP TABLE {table}_outbox"))
            .await
            .expect("clean up the test table");
    }

    /// `#272`, the decisive one: several concurrent connections issue the
    /// *same* provisioning DDL, and every one of them must succeed.
    ///
    /// This is the outbox DDL rather than the registry's because it is the
    /// worst case in this crate and the safest table to churn. Worst case
    /// because `sequence bigserial PRIMARY KEY` makes each `CREATE TABLE`
    /// three catalog insertions, not one — the table, its composite type
    /// (`pg_type_typname_nsp_index`), and the sequence
    /// (`pg_class_relname_nsp_index`) — so it reproduces both signatures
    /// `#138` and `#272` report. Safest because the table is this test's
    /// own: the registry tables are shared with several live suites that
    /// scope themselves by id prefix and must never be dropped.
    ///
    /// Measured on this workspace's cluster before the fix, with this exact
    /// shape driving a bare `batch_execute` instead of `provision::
    /// apply_ddl`: 6 sessions × 20 rounds failed in 5 rounds of 20 (17 of
    /// 120 sessions), every failure a `23505` naming one of those two
    /// catalog indexes. After: 0 of 20 rounds, 0 of 120 sessions. Removing
    /// the `apply_ddl` call and putting `batch_execute` back is the mutation
    /// check — this test fails within a few rounds.
    ///
    /// A single serial run proves nothing here, which is why this one is
    /// worth its seconds: `CREATE TABLE IF NOT EXISTS` passes that every
    /// time and passed it for as long as this command has existed.
    #[tokio::test]
    async fn concurrent_create_tables_all_succeed() {
        let Some(url) = tellurion_postgis::test_harness::require_database_url(
            "concurrent_create_tables_all_succeed",
        ) else {
            return;
        };

        // Rounds and sessions both matter: sessions widen the window, rounds
        // give it enough chances to be hit. These are the numbers the
        // pre-fix control was measured at, so "0 failures" here is
        // comparable to "5 of 20" there rather than to nothing.
        const SESSIONS: usize = 6;
        const ROUNDS: usize = 20;

        let table = "tellurion_ingest_provision_race";
        let sql = create_outbox_table_sql(table).expect("the DDL builds");
        let control = crate::db::connect_url(&url)
            .await
            .expect("connect to the test database");

        for round in 0..ROUNDS {
            // Each round starts from "absent", which is the only state the
            // race exists in: once the table is there, `IF NOT EXISTS` short
            // -circuits before any catalog write and nothing collides.
            control
                .batch_execute(&format!("DROP TABLE IF EXISTS {table}_outbox"))
                .await
                .expect("clear the table between rounds");

            let mut sessions = Vec::with_capacity(SESSIONS);
            for session in 0..SESSIONS {
                let url = url.clone();
                let sql = sql.clone();
                sessions.push(tokio::spawn(async move {
                    let client = crate::db::connect_url(&url)
                        .await
                        .unwrap_or_else(|error| panic!("session {session} connects: {error}"));
                    crate::provision::apply_ddl(&client, table, &sql).await
                }));
            }

            for (session, handle) in sessions.into_iter().enumerate() {
                let outcome = handle.await.expect("the session task itself did not panic");
                if let Err(error) = outcome {
                    panic!(
                        "round {round}, session {session} of {SESSIONS} failed to provision \
                         '{table}_outbox': {error:#}. Every concurrent caller of an idempotent \
                         create-tables must succeed — see #272."
                    );
                }
            }
        }

        control
            .batch_execute(&format!("DROP TABLE IF EXISTS {table}_outbox"))
            .await
            .expect("clean up the test table");
    }

    /// `#272` must not change what a single operator gets. The lock is
    /// mutual exclusion, not a different schema — so the table
    /// `provision::apply_ddl` leaves behind has to be indistinguishable, in
    /// the catalog, from the one the bare `batch_execute` this replaced left
    /// behind.
    ///
    /// Compared against the catalog rather than asserted in a comment,
    /// because "it is the same SQL" is exactly the kind of claim that stops
    /// being true quietly. Columns with their types, nullability and
    /// defaults; indexes with their definitions; and the sequence
    /// `bigserial` mints, which is the piece a wrapper transaction could
    /// most plausibly have disturbed.
    #[tokio::test]
    async fn provisioning_under_the_lock_builds_the_same_table_as_an_unlocked_batch_execute() {
        let Some(url) = tellurion_postgis::test_harness::require_database_url(
            "provisioning_under_the_lock_builds_the_same_table_as_an_unlocked_batch_execute",
        ) else {
            return;
        };
        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to the test database");

        let unlocked = "tellurion_ingest_provision_unlocked";
        let locked = "tellurion_ingest_provision_locked";
        for table in [unlocked, locked] {
            client
                .batch_execute(&format!("DROP TABLE IF EXISTS {table}_outbox"))
                .await
                .expect("clear any leftover table");
        }

        // The old code path, verbatim: one `batch_execute` of the DDL.
        client
            .batch_execute(&create_outbox_table_sql(unlocked).expect("the DDL builds"))
            .await
            .expect("the unlocked provisioning succeeds");
        // The new one.
        crate::provision::apply_ddl(
            &client,
            locked,
            &create_outbox_table_sql(locked).expect("the DDL builds"),
        )
        .await
        .expect("the locked provisioning succeeds");

        let columns = |table: &str| {
            let table = table.to_string();
            let client = &client;
            async move {
                client
                    .query(
                        "SELECT column_name, data_type, is_nullable, column_default \
                         FROM information_schema.columns WHERE table_name = $1 \
                         ORDER BY ordinal_position",
                        &[&format!("{table}_outbox")],
                    )
                    .await
                    .expect("reads the column catalog")
                    .iter()
                    .map(|row| {
                        format!(
                            "{}|{}|{}|{}",
                            row.get::<_, String>(0),
                            row.get::<_, String>(1),
                            row.get::<_, String>(2),
                            // The default names the sequence, which is named
                            // after the table — normalised away so the
                            // comparison is of shape, not of name.
                            row.get::<_, Option<String>>(3)
                                .unwrap_or_default()
                                .replace(&table, "TABLE")
                        )
                    })
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(
            columns(unlocked).await,
            columns(locked).await,
            "the advisory lock changed the columns the DDL produces"
        );

        let indexes = |table: &str| {
            let table = table.to_string();
            let client = &client;
            async move {
                client
                    .query(
                        "SELECT indexdef FROM pg_indexes WHERE tablename = $1 ORDER BY indexname",
                        &[&format!("{table}_outbox")],
                    )
                    .await
                    .expect("reads the index catalog")
                    .iter()
                    .map(|row| row.get::<_, String>(0).replace(&table, "TABLE"))
                    .collect::<Vec<_>>()
            }
        };
        let unlocked_indexes = indexes(unlocked).await;
        assert!(
            !unlocked_indexes.is_empty(),
            "the primary key index must exist, or this comparison proves nothing"
        );
        assert_eq!(
            unlocked_indexes,
            indexes(locked).await,
            "the advisory lock changed the indexes the DDL produces"
        );

        // `bigserial`'s implicit sequence is the object a wrapper
        // transaction is most likely to have disturbed, and the one whose
        // catalog row races on `pg_class_relname_nsp_index`.
        for table in [unlocked, locked] {
            let sequence: Option<String> = client
                .query_one(
                    "SELECT pg_get_serial_sequence($1, 'sequence')",
                    &[&format!("{table}_outbox")],
                )
                .await
                .expect("reads the sequence")
                .get(0);
            assert_eq!(
                sequence,
                Some(format!("public.{table}_outbox_sequence_seq")),
                "'{table}_outbox' did not get its bigserial sequence"
            );
        }

        for table in [unlocked, locked] {
            client
                .batch_execute(&format!("DROP TABLE {table}_outbox"))
                .await
                .expect("clean up the test table");
        }
    }
}
