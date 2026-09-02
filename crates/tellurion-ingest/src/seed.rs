//! `seed`: creates a demo table and populates it with a deterministic grid
//! of synthetic features spanning the globe, alternating points and small
//! polygons. Deterministic so reruns are reproducible for benchmarking. The
//! grid itself (name, timestamp, position, point/polygon parity) comes from
//! `synthetic::grid`, shared with `geopackage seed`'s own writer — see that
//! module's own doc for why only the row shape is shared, not the writing.
//!
//! The physical table name is caller-supplied (`--table`, defaulting to
//! `demo`) rather than a literal, so more than one demo collection can live
//! in the same database. Because `DROP TABLE ... CASCADE` runs against that
//! name on every seed, `create_demo_table` first checks a `COMMENT ON
//! TABLE` marker this module stamps on every table it creates, and refuses
//! to touch an existing table that doesn't carry it — an operator's own
//! table happening to share the name is not this seeder's to drop. `--force`
//! bypasses that check for the rare case an operator wants the old
//! unconditional-overwrite behavior back.

use std::collections::BTreeMap;
use std::time::SystemTime;

use anyhow::Context;
use tellurion_core::{CollectionDecl, RoutingDecl, StyleConf, TilesConf, ZoomCaps};
use tokio_postgres::Client;

use crate::synthetic;

const BATCH_SIZE: usize = 100;
const HALF_EXTENT_DEG: f64 = 0.25;

/// Stamped via `COMMENT ON TABLE` on every table this module creates, and
/// read back before a future run's `DROP TABLE ... CASCADE` — the cheap
/// ownership check that keeps this seeder from destroying a table it did
/// not create.
const OWNERSHIP_COMMENT: &str = "created by tellurion-ingest seed";

pub struct SeedArgs {
    pub database_url_env: String,
    pub catalog: String,
    pub storage: String,
    /// Physical table name to create and seed. Defaults to `demo` so every
    /// existing invocation is unchanged.
    pub table: String,
    /// Drops and recreates `table` even if it doesn't carry this seeder's
    /// own ownership marker. Off by default.
    pub force: bool,
}

pub async fn run(args: SeedArgs) -> anyhow::Result<()> {
    let client = crate::db::connect(&args.database_url_env).await?;

    create_demo_table(&client, &args.table, args.force).await?;
    let inserted = seed_features(&client, &args.table).await?;
    tracing::info!(count = inserted, table = %args.table, "seeded demo features");

    let decl = demo_collection_decl(&args.catalog, &args.storage, &args.table);
    println!("{}", crate::yaml_snippet::render_collection_snippet(decl)?);
    Ok(())
}

/// Whitelist-validates and double-quotes `name` for use as a SQL
/// identifier — this module's own local copy of the same rule every other
/// DDL module in this crate hand-keeps (`index.rs`, `outbox.rs`,
/// `geopackage.rs`); see any of those for why it stays a local copy rather
/// than a shared helper.
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

/// Builds the DDL text `create_demo_table` executes: drop-if-exists,
/// create, spatial index, and stamp the ownership marker. Kept pure (no
/// database access) so the shape — including the quoting — can be asserted
/// directly, the same split `index.rs::create_index_table_sql` uses for its
/// own DDL.
fn create_table_sql(table: &str) -> anyhow::Result<String> {
    let ident = quote_table_ident(table)?;
    let index_ident = quote_table_ident(&format!("{table}_geom_gix"))?;
    Ok(format!(
        "DROP TABLE IF EXISTS {ident} CASCADE;
         CREATE TABLE {ident} (
             id bigserial PRIMARY KEY,
             name text,
             observed_at timestamptz,
             geom geometry(Geometry,4326)
         );
         CREATE INDEX {index_ident} ON {ident} USING GIST (geom);
         COMMENT ON TABLE {ident} IS '{OWNERSHIP_COMMENT}';"
    ))
}

/// Looks up the OID of an already-existing table via `to_regclass`, which
/// returns `NULL` rather than erroring when nothing matches. `ident` is the
/// already-quoted identifier (quotes included) so a mixed-case name is
/// looked up with the same case sensitivity `CREATE TABLE` gave it, not
/// folded to lowercase the way an unquoted name would be.
async fn existing_table_oid(client: &Client, ident: &str) -> anyhow::Result<Option<u32>> {
    let row = client
        .query_one("SELECT to_regclass($1)::oid", &[&ident])
        .await
        .context("checking whether the target table already exists")?;
    Ok(row.get(0))
}

/// Reads the `COMMENT ON TABLE` this module stamps on every table it
/// creates. A table with no comment (or a different one) was not created by
/// this seeder, regardless of its name.
async fn is_owned_by_this_seeder(client: &Client, oid: u32) -> anyhow::Result<bool> {
    let row = client
        .query_one("SELECT obj_description($1, 'pg_class')", &[&oid])
        .await
        .context("reading the existing table's ownership marker")?;
    let comment: Option<String> = row.get(0);
    Ok(comment.as_deref() == Some(OWNERSHIP_COMMENT))
}

/// `#272`: the ownership check and the `DROP`/`CREATE` it authorises happen
/// inside one advisory-locked transaction, not one after the other.
///
/// Both halves need it and for different reasons. The `CREATE TABLE` races
/// the same way every other DDL command in this crate does (a composite type
/// of the same name, plus the `bigserial` id's implicit sequence and the
/// GiST index — three `pg_type`/`pg_class` rows to collide on). And the
/// check is a check-then-act: two seeds of the same table could both read
/// this seeder's own ownership marker, both conclude the table is theirs to
/// replace, and interleave a `DROP ... CASCADE` with the other's `CREATE`.
/// A lock that spanned only the DDL would leave that window open, which is
/// why this path uses `begin_locked` rather than `apply_ddl`.
async fn create_demo_table(client: &Client, table: &str, force: bool) -> anyhow::Result<()> {
    let sql = create_table_sql(table)?;
    crate::provision::refuse_overlong_identifiers(&sql)?;
    crate::provision::begin_locked(client, table).await?;
    match check_ownership_and_create(client, table, force, &sql).await {
        Ok(()) => crate::provision::commit(client).await,
        Err(error) => {
            crate::provision::rollback(client).await;
            Err(error)
        }
    }
}

/// The body of [`create_demo_table`], unchanged from before the lock existed
/// — split out only so every way of leaving it, refusal included, passes
/// through the one `commit`/`rollback` above.
async fn check_ownership_and_create(
    client: &Client,
    table: &str,
    force: bool,
    sql: &str,
) -> anyhow::Result<()> {
    if !force {
        let ident = quote_table_ident(table)?;
        if let Some(oid) = existing_table_oid(client, &ident).await? {
            if !is_owned_by_this_seeder(client, oid).await? {
                anyhow::bail!(
                    "table '{table}' already exists and does not carry this seeder's ownership marker; refusing to drop it with CASCADE (pass --force to override)"
                );
            }
        }
    }

    client
        .batch_execute(sql)
        .await
        .with_context(|| format!("creating table '{table}'"))?;
    Ok(())
}

/// Builds the deterministic grid and inserts it in bounded-size batches via
/// `unnest`, never materializing the full 500-row set as SQL text.
async fn seed_features(client: &Client, table: &str) -> anyhow::Result<usize> {
    let ident = quote_table_ident(table)?;
    let mut names = Vec::with_capacity(BATCH_SIZE);
    let mut timestamps = Vec::with_capacity(BATCH_SIZE);
    let mut wkts = Vec::with_capacity(BATCH_SIZE);
    let mut total = 0usize;

    for feature in synthetic::grid() {
        let lon = -180.0 + feature.u * 360.0;
        let lat = -80.0 + feature.v * 160.0;

        let wkt = if feature.is_polygon {
            square_wkt(lon, lat)
        } else {
            format!("POINT({lon} {lat})")
        };

        names.push(feature.name);
        timestamps.push(feature.observed_at);
        wkts.push(wkt);
        total += 1;

        if names.len() == BATCH_SIZE {
            insert_batch(client, &ident, &names, &timestamps, &wkts).await?;
            names.clear();
            timestamps.clear();
            wkts.clear();
        }
    }

    if !names.is_empty() {
        insert_batch(client, &ident, &names, &timestamps, &wkts).await?;
    }

    Ok(total)
}

fn square_wkt(center_lon: f64, center_lat: f64) -> String {
    let (w, e) = (center_lon - HALF_EXTENT_DEG, center_lon + HALF_EXTENT_DEG);
    let (s, n) = (center_lat - HALF_EXTENT_DEG, center_lat + HALF_EXTENT_DEG);
    format!("POLYGON(({w} {s}, {e} {s}, {e} {n}, {w} {n}, {w} {s}))")
}

async fn insert_batch(
    client: &Client,
    ident: &str,
    names: &[String],
    timestamps: &[SystemTime],
    wkts: &[String],
) -> anyhow::Result<()> {
    client
        .execute(
            &format!(
                "INSERT INTO {ident} (name, observed_at, geom)
                 SELECT name, observed_at, ST_SetSRID(ST_GeomFromText(wkt), 4326)
                 FROM unnest($1::text[], $2::timestamptz[], $3::text[]) AS t(name, observed_at, wkt)"
            ),
            &[&names, &timestamps, &wkts],
        )
        .await
        .context("inserting demo batch")?;
    Ok(())
}

fn demo_collection_decl(catalog: &str, storage: &str, table: &str) -> CollectionDecl {
    let mut caps = BTreeMap::new();
    caps.insert(0u8, 2000u64);
    caps.insert(10u8, 20000u64);

    CollectionDecl {
        id: table.to_string(),
        kind: tellurion_core::CollectionKind::Vector,
        external_id: None,
        catalog: catalog.to_string(),
        storage: storage.to_string(),
        routing: RoutingDecl::default(),
        table: Some(table.to_string()),
        geometry: Some("geom".to_string()),
        pk: Some("id".to_string()),
        id_type: tellurion_core::IdType::default(),
        datetime: Some("observed_at".to_string()),
        modified_column: None,
        row_estimate: None,
        srid: None,
        projection: None,
        geometry_profile: None,
        tiles: TilesConf {
            minzoom: 0,
            maxzoom: 14,
            caps: ZoomCaps(caps),
        },
        geometry_variants: Vec::new(),
        style: StyleConf::default(),
        places3d: None,
        schema: None,
        search: tellurion_core::SearchConf::default(),
        tile_invalidation: false,
        settings: tellurion_core::SettingsDecl::default(),
        attribute_columns: None,
        tile_properties: Vec::new(),
        visibility: tellurion_core::VisibilityDecl::default(),
        object_store: None,
        stac_metadata: false,
        stac_item_assets: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_size_matches_spec_target() {
        assert_eq!(synthetic::grid().len(), 500);
    }

    #[test]
    fn square_wkt_is_a_closed_ring() {
        let wkt = square_wkt(10.0, 20.0);
        assert!(wkt.starts_with("POLYGON(("));
        assert!(wkt.ends_with("))"));
    }

    #[test]
    fn demo_collection_decl_matches_seeded_table() {
        let decl = demo_collection_decl("default", "main", "demo");
        assert_eq!(decl.id, "demo");
        assert_eq!(decl.table.as_deref(), Some("demo"));
        assert_eq!(decl.pk.as_deref(), Some("id"));
        assert_eq!(decl.datetime.as_deref(), Some("observed_at"));
    }

    #[test]
    fn demo_collection_decl_honors_a_custom_table_name() {
        let decl = demo_collection_decl("default", "main", "second_demo");
        assert_eq!(decl.id, "second_demo");
        assert_eq!(decl.table.as_deref(), Some("second_demo"));
    }

    #[test]
    fn create_table_sql_default_name_is_unchanged() {
        let sql = create_table_sql("demo").unwrap();
        assert!(sql.contains("DROP TABLE IF EXISTS \"demo\" CASCADE"));
        assert!(sql.contains("CREATE TABLE \"demo\" ("));
        assert!(sql.contains("CREATE INDEX \"demo_geom_gix\" ON \"demo\" USING GIST (geom)"));
        assert!(sql.contains("COMMENT ON TABLE \"demo\" IS"));
    }

    #[test]
    fn create_table_sql_honors_a_custom_table_name() {
        let sql = create_table_sql("second_demo").unwrap();
        assert!(sql.contains("DROP TABLE IF EXISTS \"second_demo\" CASCADE"));
        assert!(sql.contains("CREATE TABLE \"second_demo\" ("));
        assert!(sql.contains(
            "CREATE INDEX \"second_demo_geom_gix\" ON \"second_demo\" USING GIST (geom)"
        ));
    }

    #[test]
    fn create_table_sql_quotes_an_identifier_that_needs_it() {
        // Unquoted, Postgres would fold this to lowercase `mydemo` and it
        // would no longer name the table that was actually asked for.
        let sql = create_table_sql("MyDemo").unwrap();
        assert!(sql.contains("DROP TABLE IF EXISTS \"MyDemo\" CASCADE"));
        assert!(sql.contains("CREATE TABLE \"MyDemo\" ("));
        assert!(sql.contains("CREATE INDEX \"MyDemo_geom_gix\" ON \"MyDemo\" USING GIST (geom)"));
    }

    #[test]
    fn create_table_sql_rejects_a_name_that_fails_identifier_whitelisting() {
        assert!(create_table_sql("demo; DROP TABLE x; --").is_err());
    }

    /// Live-database test: seeds against a real Postgres/PostGIS instance.
    /// Skips gracefully unless `TELLURION_TEST_DATABASE_URL` is set. Forces
    /// the create so this smoke test never trips on the ownership check
    /// (that refusal has its own dedicated tests below) if a `demo` table
    /// happens to already exist in the test database.
    #[tokio::test]
    async fn seeds_against_live_database() {
        let Ok(url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
            eprintln!("skipping: TELLURION_TEST_DATABASE_URL not set");
            return;
        };

        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to test database");

        create_demo_table(&client, "demo", true)
            .await
            .expect("create demo table");
        let inserted = seed_features(&client, "demo").await.expect("seed features");
        assert_eq!(inserted, 500);

        let row: i64 = client
            .query_one("SELECT count(*) FROM demo", &[])
            .await
            .expect("count demo rows")
            .get(0);
        assert_eq!(row, 500);
    }

    /// Live-database test: a non-default table name creates its own table,
    /// independent of `demo` — the fix that makes seeding a second demo
    /// collection into one database possible. Skips gracefully unless
    /// `TELLURION_TEST_DATABASE_URL` is set.
    #[tokio::test]
    async fn seeds_a_custom_table_name_independent_of_demo() {
        let Ok(url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
            eprintln!("skipping: TELLURION_TEST_DATABASE_URL not set");
            return;
        };

        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to test database");

        let table = "tellurion_ingest_seed_test_custom";
        create_demo_table(&client, table, false)
            .await
            .expect("create the custom-named table");
        let inserted = seed_features(&client, table)
            .await
            .expect("seed the custom-named table");
        assert_eq!(inserted, 500);

        let row: i64 = client
            .query_one(&format!("SELECT count(*) FROM {table}"), &[])
            .await
            .expect("count rows in the custom table")
            .get(0);
        assert_eq!(row, 500);

        client
            .batch_execute(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
            .await
            .expect("clean up the test table");
    }

    /// Live-database test: `create_demo_table` must refuse to drop a table
    /// that already exists but carries none of this seeder's ownership
    /// marker — an operator's own table that happens to share the name.
    /// Skips gracefully unless `TELLURION_TEST_DATABASE_URL` is set.
    #[tokio::test]
    async fn refuses_to_drop_an_existing_table_it_did_not_create() {
        let Ok(url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
            eprintln!("skipping: TELLURION_TEST_DATABASE_URL not set");
            return;
        };

        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to test database");

        let table = "tellurion_ingest_seed_test_unowned";
        client
            .batch_execute(&format!(
                "DROP TABLE IF EXISTS {table} CASCADE; CREATE TABLE {table} (id bigserial PRIMARY KEY)"
            ))
            .await
            .expect("create a plain table this seeder did not create");

        let err = create_demo_table(&client, table, false)
            .await
            .expect_err("must refuse to drop a table without its ownership marker");
        assert!(
            err.to_string().contains("ownership marker"),
            "error should explain the refusal: {err}"
        );

        // The refused table must still be there, untouched.
        let row: i64 = client
            .query_one(&format!("SELECT count(*) FROM {table}"), &[])
            .await
            .expect("the unowned table was not dropped")
            .get(0);
        assert_eq!(row, 0);

        client
            .batch_execute(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
            .await
            .expect("clean up the test table");
    }

    /// Live-database test: `--force` bypasses the ownership refusal and
    /// drops/recreates the table anyway. Skips gracefully unless
    /// `TELLURION_TEST_DATABASE_URL` is set.
    #[tokio::test]
    async fn force_overrides_the_ownership_refusal() {
        let Ok(url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
            eprintln!("skipping: TELLURION_TEST_DATABASE_URL not set");
            return;
        };

        let client = crate::db::connect_url(&url)
            .await
            .expect("connect to test database");

        let table = "tellurion_ingest_seed_test_force";
        client
            .batch_execute(&format!(
                "DROP TABLE IF EXISTS {table} CASCADE; CREATE TABLE {table} (id bigserial PRIMARY KEY)"
            ))
            .await
            .expect("create a plain table this seeder did not create");

        create_demo_table(&client, table, true)
            .await
            .expect("force bypasses the ownership refusal");

        // The table now carries this seeder's own marker, so an unforced
        // rerun no longer needs --force.
        create_demo_table(&client, table, false)
            .await
            .expect("a table this seeder created is its own to recreate without --force");

        client
            .batch_execute(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
            .await
            .expect("clean up the test table");
    }
}
