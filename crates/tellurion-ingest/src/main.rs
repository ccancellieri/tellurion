//! Tellurion ingest CLI: owns all DDL for physical collection tables. The
//! server never creates or alters tables; `seed` and `load` are the only
//! places physical schema comes from.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

mod assets;
mod batch_apply;
mod cog;
mod db;
mod demo;
mod geopackage;
mod geopackage_load;
mod geopackage_seed;
mod harvest;
mod index;
mod load;
mod ogr2ogr_loader;
mod operator;
mod outbox;
mod postgis_load;
mod processes;
/// Serialised DDL (`#272`): the one place this crate takes the advisory lock
/// that keeps two operators running `create-tables` at the same moment from
/// racing each other into a PostgreSQL catalog unique violation.
mod provision;
mod registry;
mod sanitize;
mod seed;
mod source;
mod stac;
mod synthetic;
mod touch_trigger;
mod variants;
mod yaml_snippet;

#[derive(Parser)]
#[command(
    name = "tellurion-ingest",
    about = "Tellurion dataset ingest CLI: seeding and loading physical collection tables."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// One-command demo: provisions and seeds a `.gpkg` file exactly like
    /// `geopackage create-tables` + `geopackage seed`, then serves it by
    /// handing off to the `tellurion` binary built alongside this one.
    Demo(DemoCli),
    /// Create a demo table (`demo` by default, `--table` to override) and
    /// seed it with ~500 deterministic synthetic features.
    Seed(SeedCli),
    /// Load a vector dataset (local path or http(s) URL) into a new physical table.
    Load(LoadCli),
    /// Safely update a Tellurion YAML configuration for an on-prem deployment.
    Operator(operator::OperatorCli),
    /// Manage the relational registry backend's tables (`#42`): DDL and
    /// publishing catalog/collection declarations.
    Registry(RegistryCli),
    /// Manage a collection's outbox table (`#25`): DDL for the transactional
    /// outbox the write path requires before it will serve a write.
    Outbox(OutboxCli),
    /// Manage a collection's derived-index table (`#67`): DDL for the index
    /// the applier writes to and the index lane reads from.
    Index(IndexCli),
    /// Manage a collection's asset-records table (assets-and-object-storage
    /// proposal, first slice): DDL the database-backed `AssetRecordStore`
    /// capability requires before it will serve an asset registration —
    /// and, since `#221`, the same one table the STAC lane's
    /// `stac_item_assets: true` opt-in reads to project item-scoped records
    /// into Items. One table, one DDL command, both surfaces.
    Assets(AssetsCli),
    /// Manage a collection's per-item STAC metadata sidecar table (`#202`):
    /// DDL the STAC lane's `stac_metadata: true` opt-in requires before it
    /// will serve an Item enriched from `"<table>_stac"`.
    Stac(StacCli),
    /// Manage the deployment-wide durable job ledger (`#182`): DDL the
    /// Processes lane requires before the server will accept a single job
    /// submission. Unlike every other DDL command here this one is not
    /// per-collection — there is exactly one ledger per deployment, which is
    /// what lets heterogeneous runner builds claim from it.
    Processes(ProcessesCli),
    /// Manage a GeoPackage (SQLite) storage's schema (`#73`): DDL for the
    /// feature table, its GeoPackage metadata rows, its R*Tree spatial
    /// index and maintenance triggers, and its outbox table.
    Geopackage(GeopackageCli),
    /// The raster authoring lane: `cog author` produces a tiled,
    /// Deflate-compressed, overview-pyramid COG the `cog` driver can serve
    /// from a plain single-resolution GeoTIFF; `cog mosaic` authors the
    /// measured manifest sidecar a `cog-mosaic` storage serves from.
    Cog(CogCli),
    /// Batch-apply a dataset into an already-provisioned PostGIS collection
    /// (`#114`) — the chunked `WriteSink::apply_batch` write path, never a
    /// second one; see `postgis_load.rs`'s own doc for how this differs
    /// from the top-level `load` subcommand above.
    Postgis(PostgisCli),
    /// Harvest a remote catalog into already-published local collections
    /// (`#191`) — a replay through the canonical write path, so every
    /// outbox obligation, derived-index apply and invalidation fires
    /// exactly as it would for any other write. Creates nothing: see
    /// `harvest.rs`'s own doc, including why a Tellurion deployment's own
    /// STAC surface is a valid source and how that makes a harvest the
    /// supported derived-index rebuild.
    Harvest(HarvestCli),
    /// Materialize a collection's declared `geometry_variants` columns
    /// (`#104`/`#201`) — the producer half of the pre-generalized-geometry
    /// story whose reader half the tile lanes already have. Adds the
    /// column, populates it by simplifying the base geometry with a
    /// tolerance derived from the variant's own zoom range, and indexes it
    /// the way the tiles lane actually prunes; see `variants.rs`'s own doc,
    /// including why PostGIS gets a GiST index and GeoPackage deliberately
    /// gets no second R*Tree.
    Variants(VariantsCli),
    /// Provision the maintenance a declared `modified_column` needs (`#151`)
    /// — the Optimistic Locking Timestamps class (`#107`/`#149`) is gated on
    /// an operator-declared column, and nothing in this workspace writes it.
    /// Strictly opt-in: a deployment that never runs this is byte-for-byte
    /// what it is today. See `touch_trigger.rs`'s own doc, including why the
    /// trigger fires on INSERT as well as UPDATE, why it carries no `WHEN`
    /// guard, and why every driver but PostGIS is refused by name.
    Locking(LockingCli),
}

#[derive(Args)]
struct LockingCli {
    #[command(subcommand)]
    command: LockingCommand,
}

#[derive(Subcommand)]
enum LockingCommand {
    /// Install (or replace) the `BEFORE INSERT OR UPDATE ... SET
    /// <modified_column> = now()` trigger on a collection's physical table.
    /// Idempotent: rerunning replaces the trigger in place and never leaves
    /// the table momentarily untriggered.
    InstallTouchTrigger(InstallTouchTriggerCli),
}

#[derive(Args)]
struct InstallTouchTriggerCli {
    /// Tellurion config YAML declaring the collection. Read only, never
    /// written — its `modified_column` is the source of truth for which
    /// column the trigger maintains, so there is no `--column` flag that
    /// could disagree with the declaration.
    #[arg(long)]
    config: PathBuf,
    /// Internal id of the collection whose declared `modified_column` to
    /// maintain.
    #[arg(long)]
    collection: String,
    /// Consent to installing alongside another row-level trigger on the same
    /// table whose function body mentions the declared column — refused by
    /// name without this, since two triggers assigning one column run in
    /// trigger-name order.
    #[arg(long)]
    allow_existing_trigger: bool,
    /// Print the DDL without connecting to a database at all.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct VariantsCli {
    #[command(subcommand)]
    command: VariantsCommand,
}

#[derive(Subcommand)]
enum VariantsCommand {
    /// Add, populate and index a collection's declared variant columns.
    /// Idempotent: rerunning repopulates from the current base geometry
    /// rather than duplicating anything. This is a batch backfill — nothing
    /// keeps the column in step with later writes.
    Materialize(VariantsMaterializeCli),
}

#[derive(Args)]
struct VariantsMaterializeCli {
    /// Tellurion config YAML declaring the collection. Read only, never
    /// written — the `geometry_variants` entries in it are the source of
    /// truth for which columns exist and which zoom ranges they serve.
    #[arg(long)]
    config: PathBuf,
    /// Internal id of the collection whose variants to materialize.
    #[arg(long)]
    collection: String,
    /// Materialize only this declared variant column. Omitted materializes
    /// every variant the collection declares.
    #[arg(long)]
    variant: Option<String>,
    /// Simplification tolerance in the storage CRS's own units, overriding
    /// the zoom-derived one. Required for a storage SRID whose units this
    /// command cannot reach from a tile pixel without a projection engine
    /// (anything but 3857 and 4326) — refused by name rather than guessed.
    #[arg(long)]
    tolerance: Option<f64>,
    /// GeoPackage only: consent to rebuilding `gpkg_geometry_columns`
    /// without its spec-mandated `uk_gc_table_name` unique constraint, which
    /// otherwise forbids registering a second geometry column on a feature
    /// table — and an unregistered variant column stays invisible to the
    /// boot-time check. Every other constraint and every existing row is
    /// preserved; the file stays readable, but is no longer strictly
    /// conformant on this point.
    #[arg(long)]
    allow_second_geometry_column: bool,
    /// Print the resolved plan and its SQL without touching the backend.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct HarvestCli {
    #[command(subcommand)]
    command: HarvestCommand,
}

#[derive(Subcommand)]
enum HarvestCommand {
    /// Walk a STAC API (`GET /collections`, then each collection's items
    /// with `rel=next` pagination) and upsert every item idempotently
    /// through `WriteSink::apply_batch`.
    Stac(HarvestStacCli),
}

#[derive(Args)]
struct HarvestStacCli {
    /// STAC API root URL, e.g. `https://example.test/stac`. Tellurion's own
    /// STAC root is a valid source: harvesting a catalog from itself is the
    /// supported way to rebuild a derived index against the current DDL.
    source: String,
    /// Tenant external id, resolved through `registry_tenants`.
    #[arg(long)]
    tenant: String,
    /// Catalog external id, resolved through `registry_catalogs` under
    /// `--tenant`.
    #[arg(long)]
    catalog: String,
    /// Remote collection ids to harvest, comma-separated. Omitted harvests
    /// every collection the source advertises; a named id the source does
    /// not advertise is refused rather than skipped.
    #[arg(long, value_delimiter = ',')]
    collections: Vec<String>,
    /// Rename one collection on the way in, as `remote-id=local-id`
    /// (repeatable). An unmapped remote id harvests into the local
    /// collection of the same external id.
    #[arg(long = "map", value_name = "REMOTE=LOCAL")]
    map: Vec<String>,
    /// Stop after this many items per collection, counted cumulatively
    /// across resumed runs at page boundaries. Omitted harvests every item.
    #[arg(long)]
    max_items: Option<u64>,
    /// Resume file, rewritten after every fully-applied page. A bookmark
    /// written for a different source/tenant/catalog is refused, never
    /// silently resumed.
    #[arg(long)]
    bookmark: Option<PathBuf>,
    /// Name of the environment variable holding the Postgres connection
    /// string — both the registry this resolves targets through and the
    /// storage it writes into.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
    /// How many harvested items to commit per backend transaction.
    #[arg(long, default_value_t = tellurion_core::DEFAULT_BATCH_CHUNK_ITEMS)]
    chunk_items: u32,
    /// Stop at the first refused item rather than continuing through the
    /// rest of the page. The bookmark stays on the page that carried it.
    #[arg(long)]
    strict: bool,
    /// Resolve every target and print the id-mapping report without
    /// fetching or writing a single item.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct GeopackageCli {
    #[command(subcommand)]
    command: GeopackageCommand,
}

#[derive(Subcommand)]
enum GeopackageCommand {
    /// Create (or confirm) a feature table, its GeoPackage metadata rows,
    /// its R*Tree spatial index and maintenance triggers, and its outbox
    /// table, in a `.gpkg` file (created if it doesn't exist yet).
    CreateTables(GeopackageCreateTablesCli),
    /// Seed a feature table already provisioned by `create-tables` with the
    /// same deterministic synthetic grid the top-level `seed` subcommand
    /// writes into PostGIS. Writes through this driver's own transactional
    /// outbox+R*Tree machinery (`WriteSink`), never raw SQL; runs no DDL.
    Seed(GeopackageSeedCli),
    /// Batch-apply a real (non-synthetic) dataset into a feature table
    /// already provisioned by `create-tables` (`#114`) — the same chunked
    /// `WriteSink::apply_batch` write path the HTTP batch route drives, in
    /// process, against a `.gpkg` file. Closes the gap `Seed`'s own doc
    /// names: no more "one feature per request" for a real dataset.
    Load(GeopackageLoadCli),
}

#[derive(Args)]
struct GeopackageCreateTablesCli {
    /// Local filesystem path to the `.gpkg` file.
    #[arg(long)]
    path: PathBuf,
    /// The feature table's name — also this collection's config `table`.
    #[arg(long)]
    table: String,
    /// The geometry column name.
    #[arg(long, default_value = "geom")]
    geometry: String,
    /// EPSG SRID the geometry column is registered under. `3857` serves
    /// tiles natively; `4326` (the default) also serves tiles, reprojected
    /// to Web Mercator at tile-encode time — any other SRID stays
    /// features-only, refused by name on the tiles lane.
    #[arg(long, default_value_t = 4326)]
    srid: i32,
    /// The GeoPackage `geometry_type_name` (e.g. `POINT`, `LINESTRING`,
    /// `POLYGON`, `GEOMETRY`).
    #[arg(long, default_value = "GEOMETRY")]
    geometry_type: String,
    /// Extra attribute columns as `name:TYPE` pairs, comma-separated
    /// (`TYPE` is one of TEXT, INTEGER, REAL, BOOLEAN, DATE, DATETIME).
    #[arg(long, value_delimiter = ',')]
    columns: Vec<String>,
    /// Print the DDL without touching the file at all.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct GeopackageSeedCli {
    /// Local filesystem path to the `.gpkg` file — must already be
    /// provisioned by `geopackage create-tables`.
    #[arg(long)]
    path: PathBuf,
    /// The feature table to seed — the same table name given to
    /// `geopackage create-tables --table`.
    #[arg(long)]
    table: String,
    /// Catalog id to print in the generated collection snippet.
    #[arg(long, default_value = "default")]
    catalog: String,
    /// Storage id to print in the generated collection snippet.
    #[arg(long, default_value = "main")]
    storage: String,
}

#[derive(Args)]
struct GeopackageLoadCli {
    /// Local filesystem path to the `.gpkg` file — must already be
    /// provisioned by `geopackage create-tables`.
    #[arg(long)]
    path: PathBuf,
    /// The feature table to load into — the same table name given to
    /// `geopackage create-tables --table`.
    #[arg(long)]
    table: String,
    /// Local file path or http(s) URL of the source dataset: an RFC 8142
    /// GeoJSON Text Sequence, or a plain GeoJSON `FeatureCollection`. Every
    /// feature must carry its own top-level `id`.
    source: String,
    /// How many features to commit per backend transaction.
    #[arg(long, default_value_t = tellurion_core::DEFAULT_BATCH_CHUNK_ITEMS)]
    chunk_items: u32,
    /// Stop at the first refused feature rather than continuing through
    /// the rest of the dataset.
    #[arg(long)]
    strict: bool,
}

#[derive(Args)]
struct CogCli {
    #[command(subcommand)]
    command: CogCommand,
}

#[derive(Subcommand)]
enum CogCommand {
    /// Convert a plain single-resolution GeoTIFF into a serving-optimized
    /// COG: tiled, Deflate-compressed, with a power-of-two overview
    /// pyramid down to a level that fits one tile.
    Author(CogAuthorCli),
    /// Author the manifest sidecar a `cog-mosaic` storage serves from
    /// (`#254`): scans the given COGs and MEASURES each one's bbox, byte
    /// length and SHA-256 into a YAML document the server validates but
    /// never writes. One to 32 sources, recorded in ascending id order —
    /// which is also the composition order.
    Mosaic(CogMosaicCli),
}

#[derive(Args)]
struct CogMosaicCli {
    /// A constituent COG, repeated once per source: 1..=32 of them, each a
    /// local file this command opens, hashes and georeferences itself. The
    /// source id is the file stem, so two inputs sharing a stem are refused
    /// rather than silently deduplicated.
    #[arg(long = "source", required = true)]
    sources: Vec<PathBuf>,
    /// Manifest output path. Source paths are recorded relative to this
    /// file's own directory when they sit under it, so a manifest written
    /// beside its COGs stays relocatable as one directory.
    #[arg(long)]
    output: PathBuf,
    /// Collection id to print in the generated config snippet.
    #[arg(long)]
    collection: String,
    /// Catalog id to print in the generated config snippet.
    #[arg(long, default_value = "default")]
    catalog: String,
    /// Storage id to print in the generated config snippet.
    #[arg(long, default_value = "main")]
    storage: String,
}

#[derive(Args)]
struct CogAuthorCli {
    /// Input GeoTIFF: single-IFD (no existing overviews), 8-bit
    /// grayscale/RGB/RGBA (stripped or tiled) or 8-bit paletted/categorical
    /// (`PhotometricInterpretation` = Palette, tiled only), uncompressed or
    /// Deflate compression, with EPSG:4326 (WGS84 geographic)
    /// georeferencing. A paletted source always downsamples with
    /// nearest-neighbor (see `--resample`) and carries its own `ColorMap`
    /// tag through to the output unchanged.
    #[arg(long)]
    input: PathBuf,
    /// Output COG path.
    #[arg(long)]
    output: PathBuf,
    /// Output tile width/height, pixels — also the size of the source
    /// band streamed into memory at a time (bounded-memory streaming, see
    /// `tellurion-cog::author`'s own doc).
    #[arg(long, default_value_t = tellurion_cog::AuthorOptions::default().tile_size)]
    tile_size: u32,
    /// Overview downsampling kernel: `auto` (default) box-averages a
    /// continuous source and nearest-neighbors a paletted one; `nearest`
    /// forces nearest-neighbor even for a non-paletted source (e.g. a
    /// single-band class raster with no `ColorMap` tag); `average` forces
    /// box-average and is refused outright against a paletted source,
    /// since averaging class indices has no correct meaning.
    #[arg(long, value_enum, default_value_t = ResampleArg::Auto)]
    resample: ResampleArg,
    /// Collection id to print in the generated config snippet.
    #[arg(long)]
    collection: String,
    /// Catalog id to print in the generated config snippet.
    #[arg(long, default_value = "default")]
    catalog: String,
    /// Storage id to print in the generated config snippet.
    #[arg(long, default_value = "main")]
    storage: String,
}

/// `--resample`'s own CLI-facing shape — converted to
/// `tellurion_cog::ResampleMode` before reaching the library, so the
/// library crate itself never depends on `clap`.
#[derive(Clone, Copy, clap::ValueEnum)]
enum ResampleArg {
    Auto,
    Nearest,
    Average,
}

impl From<ResampleArg> for tellurion_cog::ResampleMode {
    fn from(arg: ResampleArg) -> Self {
        match arg {
            ResampleArg::Auto => tellurion_cog::ResampleMode::Auto,
            ResampleArg::Nearest => tellurion_cog::ResampleMode::NearestNeighbor,
            ResampleArg::Average => tellurion_cog::ResampleMode::BoxAverage,
        }
    }
}

#[derive(Args)]
struct PostgisCli {
    #[command(subcommand)]
    command: PostgisCommand,
}

#[derive(Subcommand)]
enum PostgisCommand {
    /// Batch-apply a dataset into an EXISTING, already outbox-provisioned
    /// PostGIS table (`#114`) — through the real `WriteSink::apply_batch`,
    /// never a raw-SQL shortcut. See `postgis_load.rs`'s own doc for how
    /// this differs from the top-level `load` subcommand (which creates a
    /// brand-new table via `ogr2ogr` and bypasses the outbox entirely).
    Load(PostgisLoadCli),
}

#[derive(Args)]
struct PostgisLoadCli {
    /// Local file path or http(s) URL of the source dataset: an RFC 8142
    /// GeoJSON Text Sequence, or a plain GeoJSON `FeatureCollection`. Every
    /// feature must carry its own top-level `id`.
    source: String,
    /// The physical table to load into — must already exist and already
    /// have its outbox table provisioned (`tellurion-ingest outbox
    /// create-tables`).
    #[arg(long)]
    table: String,
    /// The geometry column name.
    #[arg(long, default_value = "geom")]
    geometry: String,
    /// The primary key column name.
    #[arg(long, default_value = "id")]
    pk: String,
    /// The primary key's declared value space: `integer`, `uuid`, or
    /// `text`. Every batch item is caller-supplied-id regardless (`#114`),
    /// so this only decides how each id is parsed and bound.
    #[arg(long, default_value = "integer")]
    id_type: String,
    /// EPSG SRID incoming geometries are tagged under. Omitted (the
    /// default) tags every geometry 4326, the same default `WriteSink::
    /// apply`'s own single-item write path assumes for an unset
    /// `CollectionDecl::srid`.
    #[arg(long)]
    srid: Option<i32>,
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
    /// How many features to commit per backend transaction.
    #[arg(long, default_value_t = tellurion_core::DEFAULT_BATCH_CHUNK_ITEMS)]
    chunk_items: u32,
    /// Stop at the first refused feature rather than continuing through
    /// the rest of the dataset.
    #[arg(long)]
    strict: bool,
}

#[derive(Args)]
struct OutboxCli {
    #[command(subcommand)]
    command: OutboxCommand,
}

#[derive(Subcommand)]
enum OutboxCommand {
    /// Create (or confirm) a collection's `"<table>_outbox"` table.
    CreateTables(OutboxCreateTablesCli),
}

#[derive(Args)]
struct OutboxCreateTablesCli {
    /// The collection's physical table name (not its config `id`) — the
    /// same name `CollectionDecl::table` resolves to.
    #[arg(long)]
    table: String,
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
    /// Print the DDL without connecting to a database at all.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct IndexCli {
    #[command(subcommand)]
    command: IndexCommand,
}

#[derive(Subcommand)]
enum IndexCommand {
    /// Create (or confirm) a collection's `"<table>_index"` derived-index table.
    CreateTables(IndexCreateTablesCli),
}

#[derive(Args)]
struct IndexCreateTablesCli {
    /// The collection's physical table name (not its config `id`) — the
    /// same name `CollectionDecl::table` resolves to.
    #[arg(long)]
    table: String,
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
    /// Print the DDL without connecting to a database at all.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct AssetsCli {
    #[command(subcommand)]
    command: AssetsCommand,
}

#[derive(Subcommand)]
enum AssetsCommand {
    /// Create (or confirm) a collection's `"<table>_assets"` asset-records table.
    CreateTables(AssetsCreateTablesCli),
}

#[derive(Args)]
struct AssetsCreateTablesCli {
    /// The collection's physical table name (not its config `id`) — the
    /// same name `CollectionDecl::table` resolves to.
    #[arg(long)]
    table: String,
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
    /// Print the DDL without connecting to a database at all.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct StacCli {
    #[command(subcommand)]
    command: StacCommand,
}

#[derive(Subcommand)]
enum StacCommand {
    /// Create (or confirm) a collection's `"<table>_stac"` metadata sidecar table.
    CreateTables(StacCreateTablesCli),
}

#[derive(Args)]
struct StacCreateTablesCli {
    /// The collection's physical table name (not its config `id`) — the
    /// same name `CollectionDecl::table` resolves to.
    #[arg(long)]
    table: String,
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
    /// Print the DDL without connecting to a database at all.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct ProcessesCli {
    #[command(subcommand)]
    command: ProcessesCommand,
}

#[derive(Subcommand)]
enum ProcessesCommand {
    /// Create (or confirm) the deployment-wide `tellurion_jobs` ledger.
    CreateTables(ProcessesCreateTablesCli),
}

#[derive(Args)]
struct ProcessesCreateTablesCli {
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
    /// Print the DDL without connecting to a database at all.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct RegistryCli {
    #[command(subcommand)]
    command: RegistryCommand,
}

#[derive(Subcommand)]
enum RegistryCommand {
    /// Create (or confirm) the `registry_tenants`/`registry_catalogs`/
    /// `registry_collections` tables.
    CreateTables(CreateTablesCli),
    /// Upsert one `TenantDecl` (a YAML file) into `registry_tenants`.
    PublishTenant(PublishTenantCli),
    /// Upsert one `CatalogDecl` (a YAML file) into `registry_catalogs`.
    PublishCatalog(PublishCatalogCli),
    /// Upsert one `CollectionDecl` (a YAML file) into `registry_collections`.
    PublishCollection(PublishCollectionCli),
}

#[derive(Args)]
struct CreateTablesCli {
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
    /// Print the DDL without connecting to a database at all.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct PublishTenantCli {
    /// Path to a YAML file containing exactly one TenantDecl.
    path: PathBuf,
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
}

#[derive(Args)]
struct PublishCatalogCli {
    /// Path to a YAML file containing exactly one CatalogDecl.
    path: PathBuf,
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
}

#[derive(Args)]
struct PublishCollectionCli {
    /// Path to a YAML file containing exactly one CollectionDecl.
    path: PathBuf,
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
}

#[derive(Args)]
struct DemoCli {
    /// Local filesystem path to the `.gpkg` file — created if it doesn't
    /// exist yet, confirmed (not re-created) if it does.
    #[arg(long, default_value = "demo.gpkg")]
    path: PathBuf,
    /// Port to serve on — passed through to the `tellurion` binary as
    /// `PORT`. Defaults to whatever `config/example-geopackage.yaml` itself
    /// declares (8080) when omitted.
    #[arg(long)]
    port: Option<u16>,
}

#[derive(Args)]
struct SeedCli {
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
    /// Catalog id to print in the generated collection snippet.
    #[arg(long, default_value = "default")]
    catalog: String,
    /// Storage id to print in the generated collection snippet.
    #[arg(long, default_value = "main")]
    storage: String,
    /// Physical table name to create and seed.
    #[arg(long, default_value = "demo")]
    table: String,
    /// Drop and recreate `table` even if it wasn't created by a previous
    /// run of this seeder. Without this, `seed` refuses to touch an
    /// existing table with no ownership marker on it.
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct LoadCli {
    /// Local file path or http(s) URL of the source dataset.
    source: String,
    /// Collection id; also sanitized into the physical table name.
    #[arg(long)]
    collection: String,
    /// Name of the environment variable holding the Postgres connection string.
    #[arg(long, default_value = "DATABASE_URL")]
    database_url_env: String,
    /// Source layer name, for multi-layer datasets. Defaults to the first layer.
    #[arg(long)]
    layer: Option<String>,
    /// Catalog id to print in the generated collection snippet.
    #[arg(long, default_value = "default")]
    catalog: String,
    /// Storage id to print in the generated collection snippet.
    #[arg(long, default_value = "main")]
    storage: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Demo(args) => {
            demo::run(demo::DemoArgs {
                path: args.path,
                port: args.port,
            })
            .await
        }
        Command::Seed(args) => {
            seed::run(seed::SeedArgs {
                database_url_env: args.database_url_env,
                catalog: args.catalog,
                storage: args.storage,
                table: args.table,
                force: args.force,
            })
            .await
        }
        Command::Load(args) => {
            load::run(load::LoadArgs {
                source: args.source,
                collection: args.collection,
                database_url_env: args.database_url_env,
                layer: args.layer,
                catalog: args.catalog,
                storage: args.storage,
            })
            .await
        }
        Command::Operator(args) => operator::run(args).await,
        Command::Registry(args) => match args.command {
            RegistryCommand::CreateTables(args) => {
                registry::create_tables(registry::CreateTablesArgs {
                    database_url_env: args.database_url_env,
                    dry_run: args.dry_run,
                })
                .await
            }
            RegistryCommand::PublishTenant(args) => {
                registry::publish_tenant(registry::PublishTenantArgs {
                    path: args.path,
                    database_url_env: args.database_url_env,
                })
                .await
            }
            RegistryCommand::PublishCatalog(args) => {
                registry::publish_catalog(registry::PublishCatalogArgs {
                    path: args.path,
                    database_url_env: args.database_url_env,
                })
                .await
            }
            RegistryCommand::PublishCollection(args) => {
                registry::publish_collection(registry::PublishCollectionArgs {
                    path: args.path,
                    database_url_env: args.database_url_env,
                })
                .await
            }
        },
        Command::Outbox(args) => match args.command {
            OutboxCommand::CreateTables(args) => {
                outbox::create_tables(outbox::CreateTablesArgs {
                    table: args.table,
                    database_url_env: args.database_url_env,
                    dry_run: args.dry_run,
                })
                .await
            }
        },
        Command::Index(args) => match args.command {
            IndexCommand::CreateTables(args) => {
                index::create_tables(index::CreateTablesArgs {
                    table: args.table,
                    database_url_env: args.database_url_env,
                    dry_run: args.dry_run,
                })
                .await
            }
        },
        Command::Assets(args) => match args.command {
            AssetsCommand::CreateTables(args) => {
                assets::create_tables(assets::CreateTablesArgs {
                    table: args.table,
                    database_url_env: args.database_url_env,
                    dry_run: args.dry_run,
                })
                .await
            }
        },
        Command::Stac(args) => match args.command {
            StacCommand::CreateTables(args) => {
                stac::create_tables(stac::CreateTablesArgs {
                    table: args.table,
                    database_url_env: args.database_url_env,
                    dry_run: args.dry_run,
                })
                .await
            }
        },
        Command::Processes(args) => match args.command {
            ProcessesCommand::CreateTables(args) => {
                processes::create_tables(processes::CreateTablesArgs {
                    database_url_env: args.database_url_env,
                    dry_run: args.dry_run,
                })
                .await
            }
        },
        Command::Geopackage(args) => match args.command {
            GeopackageCommand::CreateTables(args) => {
                let columns = geopackage::parse_columns(&args.columns)?;
                geopackage::create_tables(geopackage::CreateTablesArgs {
                    path: args.path,
                    table: args.table,
                    geometry: args.geometry,
                    srid: args.srid,
                    geometry_type: args.geometry_type,
                    columns,
                    dry_run: args.dry_run,
                })
                .await
            }
            GeopackageCommand::Seed(args) => {
                geopackage_seed::run(geopackage_seed::SeedArgs {
                    path: args.path,
                    table: args.table,
                    catalog: args.catalog,
                    storage: args.storage,
                })
                .await
            }
            GeopackageCommand::Load(args) => {
                geopackage_load::run(geopackage_load::LoadArgs {
                    path: args.path,
                    table: args.table,
                    source: args.source,
                    chunk_items: args.chunk_items as usize,
                    strict: args.strict,
                })
                .await
            }
        },
        Command::Cog(args) => match args.command {
            CogCommand::Author(args) => {
                cog::author(cog::AuthorArgs {
                    input: args.input,
                    output: args.output,
                    tile_size: args.tile_size,
                    resample: args.resample.into(),
                    collection: args.collection,
                    catalog: args.catalog,
                    storage: args.storage,
                })
                .await
            }
            CogCommand::Mosaic(args) => {
                cog::mosaic(cog::MosaicArgs {
                    sources: args.sources,
                    output: args.output,
                    collection: args.collection,
                    catalog: args.catalog,
                    storage: args.storage,
                })
                .await
            }
        },
        Command::Harvest(args) => match args.command {
            HarvestCommand::Stac(args) => {
                harvest::run(harvest::HarvestArgs {
                    source: args.source,
                    tenant: args.tenant,
                    catalog: args.catalog,
                    collections: args.collections,
                    map: args.map,
                    max_items: args.max_items,
                    bookmark: args.bookmark,
                    database_url_env: args.database_url_env,
                    chunk_items: args.chunk_items as usize,
                    strict: args.strict,
                    dry_run: args.dry_run,
                })
                .await
            }
        },
        Command::Variants(args) => match args.command {
            VariantsCommand::Materialize(args) => {
                variants::materialize(variants::MaterializeArgs {
                    config: args.config,
                    collection: args.collection,
                    variant: args.variant,
                    tolerance: args.tolerance,
                    allow_second_geometry_column: args.allow_second_geometry_column,
                    dry_run: args.dry_run,
                })
                .await
            }
        },
        Command::Locking(args) => match args.command {
            LockingCommand::InstallTouchTrigger(args) => {
                touch_trigger::install(touch_trigger::InstallArgs {
                    config: args.config,
                    collection: args.collection,
                    allow_existing_trigger: args.allow_existing_trigger,
                    dry_run: args.dry_run,
                })
                .await
            }
        },
        Command::Postgis(args) => match args.command {
            PostgisCommand::Load(args) => {
                postgis_load::run(postgis_load::LoadArgs {
                    source: args.source,
                    table: args.table,
                    geometry: args.geometry,
                    pk: args.pk,
                    id_type: args.id_type,
                    srid: args.srid,
                    database_url_env: args.database_url_env,
                    chunk_items: args.chunk_items as usize,
                    strict: args.strict,
                })
                .await
            }
        },
    }
}
