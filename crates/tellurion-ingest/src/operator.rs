use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use clap::{Args, Subcommand};
use tellurion_core::{
    AppConfig, BootEnvelope, BootstrapOutcome, ControlStore, ControlStoreLocator, PrincipalIdentity,
};

mod postgis_reference;
mod yaml_edit;

use postgis_reference::validate_postgis_reference;
use yaml_edit::append_top_level_sequence;

#[cfg(test)]
use postgis_reference::interpret_postgis_reference;

#[derive(Args)]
pub struct OperatorCli {
    #[command(subcommand)]
    command: OperatorCommand,
}

#[derive(Subcommand)]
enum OperatorCommand {
    /// Add a tenant declaration to an existing config file.
    Tenant(CreateTenantCli),
    /// Add a catalog declaration owned by an existing tenant.
    Catalog(CreateCatalogCli),
    /// Register an existing public PostGIS table without copying its data.
    Reference(ReferenceCollectionCli),
    /// Validate or perform the one-time YAML import into an empty dynamic control store.
    MigrateControlStore(MigrateControlStoreCli),
}

#[derive(Args)]
struct MigrateControlStoreCli {
    /// Boot-envelope YAML containing the destination locator and first-run seed.
    #[arg(long)]
    config: PathBuf,
    /// Validate and describe the import without contacting or changing the destination.
    #[arg(long, conflicts_with = "apply", required_unless_present = "apply")]
    dry_run: bool,
    /// Import into the destination, refusing any already-initialized store.
    #[arg(long, conflicts_with = "dry_run", required_unless_present = "dry_run")]
    apply: bool,
}

#[derive(Args)]
struct CreateTenantCli {
    /// Tellurion config YAML to update. Mode bits are preserved; ownership, ACLs, and extended attributes may not be.
    #[arg(long)]
    config: PathBuf,
    /// Globally unique internal tenant id.
    #[arg(long)]
    id: String,
    /// Optional public URL id. Defaults to the internal id.
    #[arg(long)]
    external_id: Option<String>,
    /// Print the resulting YAML without changing the file.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct CreateCatalogCli {
    /// Tellurion config YAML to update. Mode bits are preserved; ownership, ACLs, and extended attributes may not be.
    #[arg(long)]
    config: PathBuf,
    /// Globally unique internal catalog id.
    #[arg(long)]
    id: String,
    /// Internal id of the tenant that owns this catalog.
    #[arg(long)]
    tenant: String,
    /// Optional public URL id. Defaults to the internal id.
    #[arg(long)]
    external_id: Option<String>,
    /// Print the resulting YAML without changing the file.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct ReferenceCollectionCli {
    /// Tellurion config YAML to update. Mode bits are preserved; ownership, ACLs, and extended attributes may not be.
    #[arg(long)]
    config: PathBuf,
    /// Globally unique internal collection id.
    #[arg(long)]
    id: String,
    /// Internal id of the catalog that owns this collection.
    #[arg(long)]
    catalog: String,
    /// PostGIS storage id declared in the config.
    #[arg(long)]
    storage: String,
    /// Existing table in PostGIS's public schema. It is never copied or altered.
    #[arg(long)]
    table: String,
    /// Existing PostGIS geometry column on the table.
    #[arg(long)]
    geometry: String,
    /// Optional public URL id. Defaults to the internal id.
    #[arg(long)]
    external_id: Option<String>,
    /// Print the resulting YAML without changing the file.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Clone, Debug)]
enum Change {
    CreateTenant {
        id: String,
        external_id: Option<String>,
    },
    CreateCatalog {
        id: String,
        tenant: String,
        external_id: Option<String>,
    },
    ReferenceCollection {
        id: String,
        catalog: String,
        storage: String,
        table: String,
        geometry: String,
        primary_key: Option<String>,
        external_id: Option<String>,
    },
}

pub async fn run(args: OperatorCli) -> anyhow::Result<()> {
    let (config_path, dry_run, mut change) = match args.command {
        OperatorCommand::MigrateControlStore(args) => return migrate_control_store(args).await,
        OperatorCommand::Tenant(args) => (
            args.config,
            args.dry_run,
            Change::CreateTenant {
                id: args.id,
                external_id: args.external_id,
            },
        ),
        OperatorCommand::Catalog(args) => (
            args.config,
            args.dry_run,
            Change::CreateCatalog {
                id: args.id,
                tenant: args.tenant,
                external_id: args.external_id,
            },
        ),
        OperatorCommand::Reference(args) => (
            args.config,
            args.dry_run,
            Change::ReferenceCollection {
                id: args.id,
                catalog: args.catalog,
                storage: args.storage,
                table: args.table,
                geometry: args.geometry,
                primary_key: None,
                external_id: args.external_id,
            },
        ),
    };

    let config_path = canonical_config_path(&config_path)?;
    let _lock = (!dry_run)
        .then(|| ConfigLock::acquire(&config_path))
        .transpose()?;
    let source = fs::read_to_string(&config_path)
        .with_context(|| format!("reading config '{}'", config_path.display()))?;
    let config = parse_config(&source)?;
    validate_change_identifiers(&change)?;
    validate_config_local_change(&source, &change)?;

    if let Change::ReferenceCollection {
        storage,
        table,
        geometry,
        primary_key,
        ..
    } = &mut change
    {
        let storage_decl = config
            .storages
            .iter()
            .find(|decl| decl.id == *storage)
            .ok_or_else(|| anyhow::anyhow!("unknown storage '{storage}'"))?;
        if storage_decl.driver != "postgis" {
            anyhow::bail!(
                "storage '{}' uses driver '{}'; reference supports only postgis storage",
                storage,
                storage_decl.driver
            );
        }
        let client = crate::db::connect(&storage_decl.url_env)
            .await
            .with_context(|| format!("connecting to storage '{storage}'"))?;
        *primary_key = Some(validate_postgis_reference(&client, table, geometry).await?);
    }

    let rendered = apply_change(&source, change)?;
    if dry_run {
        print!("{rendered}");
        return Ok(());
    }

    let backup = write_atomically(&config_path, &source, &rendered)?;
    println!(
        "updated {}\nbackup: {}\nrestart or reload Tellurion to apply the new configuration\nrollback: restore the backup over {} and restart or reload Tellurion\nmetadata warning: mode bits were preserved, but ownership, ACLs, and extended attributes may not be preserved by atomic replacement",
        config_path.display(),
        backup.display(),
        config_path.display()
    );
    Ok(())
}

async fn migrate_control_store(args: MigrateControlStoreCli) -> anyhow::Result<()> {
    let config_path = canonical_config_path(&args.config)?;
    let source = fs::read_to_string(&config_path)
        .with_context(|| format!("reading boot envelope '{}'", config_path.display()))?;
    let envelope: BootEnvelope = serde_yaml::from_str(&source)
        .with_context(|| format!("parsing boot envelope '{}'", config_path.display()))?;
    envelope.validate_initial_seed()?;
    let seed = envelope.seed_snapshot().ok_or_else(|| {
        anyhow::anyhow!(
            "the boot envelope has no seed; add platform configuration or set allow_empty_platform: true"
        )
    })?;
    seed.validate()?;

    let backend = match &envelope.control_store {
        ControlStoreLocator::LegacyFile => {
            anyhow::bail!("migrate-control-store requires a sqlite or postgres destination")
        }
        ControlStoreLocator::Sqlite { .. } => "sqlite",
        ControlStoreLocator::Postgres { .. } => "postgres",
    };
    if args.dry_run {
        println!(
            "valid one-time control-store import\nbackend: {backend}\ntenants: {}\ncatalogs: {}\ncollections: {}\ndestination was not contacted",
            seed.config.tenants.len(),
            seed.config.catalogs.len(),
            seed.config.collections.len(),
        );
        return Ok(());
    }
    debug_assert!(args.apply);

    let store: Box<dyn ControlStore> = match &envelope.control_store {
        ControlStoreLocator::LegacyFile => unreachable!(),
        ControlStoreLocator::Sqlite { path, .. } => {
            Box::new(tellurion_control_sqlite::SqliteControlStore::open(path).await?)
        }
        ControlStoreLocator::Postgres { url_env, .. } => {
            let database_url = std::env::var(url_env).with_context(|| {
                format!(
                    "control_store.url_env names '{url_env}', but that environment variable is not set"
                )
            })?;
            Box::new(
                tellurion_control_postgres::PostgresControlStore::connect(&database_url).await?,
            )
        }
    };
    if store.current_revision().await?.is_some() {
        anyhow::bail!("control-store migration destination is already initialized");
    }
    let actor = PrincipalIdentity {
        issuer: "urn:tellurion:operator".to_string(),
        subject: "migrate-control-store".to_string(),
    };
    match store
        .bootstrap_if_empty(&seed, &actor, envelope.bootstrap_mode())
        .await?
    {
        BootstrapOutcome::Bootstrapped(revision) => {
            println!("control store initialized at revision {revision}");
            Ok(())
        }
        BootstrapOutcome::AlreadyInitialized(_) => {
            anyhow::bail!("control-store migration destination was initialized concurrently")
        }
    }
}

fn apply_change(source: &str, change: Change) -> anyhow::Result<String> {
    parse_config(source)?;
    validate_change_identifiers(&change)?;
    let section = change.section();
    let item = change.sequence_item()?;
    let updated = append_top_level_sequence(source, section, &item)?;
    parse_config(&updated)
        .map_err(|error| anyhow::anyhow!("updated Tellurion config is invalid: {error:#}"))?;
    Ok(updated)
}

fn validate_config_local_change(source: &str, change: &Change) -> anyhow::Result<()> {
    let mut tentative = change.clone();
    if let Change::ReferenceCollection { primary_key, .. } = &mut tentative {
        primary_key.get_or_insert_with(|| "id".to_string());
    }
    apply_change(source, tentative).map(|_| ())
}

fn parse_config(source: &str) -> anyhow::Result<AppConfig> {
    let config: AppConfig =
        serde_yaml::from_str(source).context("parsing Tellurion config YAML")?;
    config.validate().map_err(anyhow::Error::from)?;
    Ok(config)
}

fn validate_change_identifiers(change: &Change) -> anyhow::Result<()> {
    match change {
        Change::CreateTenant { id, external_id } => {
            validate_logical_id("tenant id", id)?;
            if let Some(external_id) = external_id {
                validate_logical_id("tenant external_id", external_id)?;
            }
        }
        Change::CreateCatalog {
            id,
            tenant,
            external_id,
        } => {
            validate_logical_id("catalog id", id)?;
            validate_logical_id("tenant id", tenant)?;
            if let Some(external_id) = external_id {
                validate_logical_id("catalog external_id", external_id)?;
            }
        }
        Change::ReferenceCollection {
            id,
            catalog,
            storage,
            table,
            geometry,
            primary_key,
            external_id,
        } => {
            validate_logical_id("collection id", id)?;
            validate_logical_id("catalog id", catalog)?;
            validate_logical_id("storage id", storage)?;
            validate_physical_identifier("table", table)?;
            validate_physical_identifier("geometry column", geometry)?;
            if let Some(primary_key) = primary_key {
                validate_physical_identifier("primary key column", primary_key)?;
            }
            if let Some(external_id) = external_id {
                validate_logical_id("collection external_id", external_id)?;
            }
        }
    }
    Ok(())
}

impl Change {
    fn section(&self) -> &'static str {
        match self {
            Self::CreateTenant { .. } => "tenants",
            Self::CreateCatalog { .. } => "catalogs",
            Self::ReferenceCollection { .. } => "collections",
        }
    }

    fn sequence_item(&self) -> anyhow::Result<String> {
        let item = match self {
            Self::CreateTenant { id, external_id } => {
                let mut item = format!("- id: {id}");
                if let Some(external_id) = external_id {
                    item.push_str(&format!("\n  external_id: {external_id}"));
                }
                item
            }
            Self::CreateCatalog {
                id,
                tenant,
                external_id,
            } => {
                let mut item = format!("- id: {id}");
                if let Some(external_id) = external_id {
                    item.push_str(&format!("\n  external_id: {external_id}"));
                }
                item.push_str(&format!("\n  tenant: {tenant}"));
                item
            }
            Self::ReferenceCollection {
                id,
                catalog,
                storage,
                table,
                geometry,
                primary_key,
                external_id,
            } => {
                let primary_key = primary_key.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("PostGIS reference must be validated before it can be written")
                })?;
                let mut item = format!("- id: {id}");
                if let Some(external_id) = external_id {
                    item.push_str(&format!("\n  external_id: {external_id}"));
                }
                item.push_str(&format!(
                    "\n  catalog: {catalog}\n  storage: {storage}\n  table: {table}\n  geometry: {geometry}\n  pk: {primary_key}"
                ));
                item
            }
        };
        Ok(item)
    }
}

fn validate_logical_id(kind: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty() || value.len() > 63 {
        anyhow::bail!("{kind} must be between 1 and 63 characters");
    }
    let mut chars = value.chars();
    let first = chars.next().expect("checked non-empty identifier");
    if !first.is_ascii_alphabetic() {
        anyhow::bail!("{kind} must start with a letter");
    }
    if !chars
        .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
    {
        anyhow::bail!("{kind} may contain only letters, digits, '_' and '-'");
    }
    Ok(())
}

fn validate_physical_identifier(kind: &str, value: &str) -> anyhow::Result<()> {
    if value.is_empty() || value.len() > 63 {
        anyhow::bail!("{kind} must be between 1 and 63 characters");
    }
    let mut chars = value.chars();
    let first = chars.next().expect("checked non-empty identifier");
    if !(first.is_ascii_alphabetic() || first == '_') {
        anyhow::bail!("{kind} must start with a letter or '_'");
    }
    if !chars.all(|character| character.is_ascii_alphanumeric() || character == '_') {
        anyhow::bail!("{kind} may contain only letters, digits, and '_'");
    }
    Ok(())
}

fn canonical_config_path(path: &Path) -> anyhow::Result<PathBuf> {
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("resolving config path '{}'", path.display()))?;
    if !fs::metadata(&canonical)
        .with_context(|| format!("reading config metadata '{}'", canonical.display()))?
        .is_file()
    {
        anyhow::bail!("config '{}' is not a regular file", canonical.display());
    }
    Ok(canonical)
}

fn write_atomically(path: &Path, expected_source: &str, contents: &str) -> anyhow::Result<PathBuf> {
    ensure_source_unchanged(path, expected_source)?;
    let metadata = fs::metadata(path)
        .with_context(|| format!("reading existing config '{}'", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("config '{}' is not a regular file", path.display());
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("config path '{}' has no valid filename", path.display()))?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("reading system clock")?
        .as_nanos();
    let backup = parent.join(format!("{filename}.bak.{unique}"));
    fs::copy(path, &backup).with_context(|| format!("creating backup '{}'", backup.display()))?;
    File::open(&backup)
        .with_context(|| format!("opening backup '{}'", backup.display()))?
        .sync_all()
        .with_context(|| format!("syncing backup '{}'", backup.display()))?;
    File::open(parent)
        .with_context(|| format!("opening config directory '{}'", parent.display()))?
        .sync_all()
        .with_context(|| format!("syncing backup directory '{}'", parent.display()))?;

    let temporary = parent.join(format!(".{filename}.{unique}.tmp"));
    let write_result = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("creating temporary config '{}'", temporary.display()))?;
        file.write_all(contents.as_bytes())
            .with_context(|| format!("writing temporary config '{}'", temporary.display()))?;
        file.set_permissions(metadata.permissions())
            .with_context(|| format!("preserving permissions on '{}'", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing temporary config '{}'", temporary.display()))?;
        ensure_source_unchanged(path, expected_source)?;
        fs::rename(&temporary, path)
            .with_context(|| format!("replacing config '{}'", path.display()))?;
        File::open(parent)
            .with_context(|| format!("opening config directory '{}'", parent.display()))?
            .sync_all()
            .with_context(|| format!("syncing config directory '{}'", parent.display()))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result?;
    Ok(backup)
}

fn ensure_source_unchanged(path: &Path, expected_source: &str) -> anyhow::Result<()> {
    let current = fs::read(path)
        .with_context(|| format!("re-reading config '{}' before replacement", path.display()))?;
    if current != expected_source.as_bytes() {
        anyhow::bail!(
            "config '{}' changed since it was read; refusing to overwrite the external edit",
            path.display()
        );
    }
    Ok(())
}

#[derive(Debug)]
struct ConfigLock {
    path: PathBuf,
}

impl ConfigLock {
    fn acquire(config_path: &Path) -> anyhow::Result<Self> {
        let parent = config_path.parent().unwrap_or_else(|| Path::new("."));
        let filename = config_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "config path '{}' has no valid filename",
                    config_path.display()
                )
            })?;
        let path = parent.join(format!(".{filename}.lock"));
        let started_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("reading system clock for operator lock")?
            .as_millis();
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let write_result = writeln!(
                    file,
                    "pid={}\nstarted_unix_ms={started_unix_ms}\nconfig={}",
                    std::process::id(),
                    config_path.display()
                )
                .and_then(|()| file.sync_all());
                if let Err(error) = write_result {
                    let _ = fs::remove_file(&path);
                    return Err(error).context("writing operator update lock metadata");
                }
                Ok(Self { path })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => anyhow::bail!(
                "another operator update appears to be running for '{}'; inspect '{}' before removing it",
                config_path.display(),
                path.display()
            ),
            Err(error) => Err(error)
                .with_context(|| format!("creating operator update lock '{}'", path.display())),
        }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.path) {
            tracing::warn!(%error, path = %self.path.display(), "could not remove operator update lock");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
storages:
  - id: main
    driver: postgis
    url_env: DATABASE_URL
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections: []
"#;

    #[tokio::test]
    async fn control_store_migration_dry_run_is_side_effect_free_and_apply_is_guarded() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("bootstrap.yaml");
        let database = directory.path().join("control.sqlite");
        fs::write(
            &config,
            format!(
                "control_store:\n  backend: sqlite\n  path: {}\nallow_empty_platform: true\nserver: {{ port: 8123 }}\n",
                database.display()
            ),
        )
        .unwrap();

        migrate_control_store(MigrateControlStoreCli {
            config: config.clone(),
            dry_run: true,
            apply: false,
        })
        .await
        .unwrap();
        assert!(!database.exists());

        migrate_control_store(MigrateControlStoreCli {
            config: config.clone(),
            dry_run: false,
            apply: true,
        })
        .await
        .unwrap();
        let store = tellurion_control_sqlite::SqliteControlStore::open(&database)
            .await
            .unwrap();
        assert_eq!(store.current_revision().await.unwrap(), Some(1));

        let error = migrate_control_store(MigrateControlStoreCli {
            config,
            dry_run: false,
            apply: true,
        })
        .await
        .unwrap_err();
        assert!(error.to_string().contains("already initialized"));
    }

    #[test]
    fn creates_a_tenant_catalog_and_reference_collection_in_one_valid_config() {
        let tenant = apply_change(
            CONFIG,
            Change::CreateTenant {
                id: "acme".to_string(),
                external_id: Some("acme-data".to_string()),
            },
        )
        .unwrap();
        let catalog = apply_change(
            &tenant,
            Change::CreateCatalog {
                id: "acme-default".to_string(),
                tenant: "acme".to_string(),
                external_id: Some("default".to_string()),
            },
        )
        .unwrap();
        let updated = apply_change(
            &catalog,
            Change::ReferenceCollection {
                id: "roads".to_string(),
                catalog: "acme-default".to_string(),
                storage: "main".to_string(),
                table: "roads_2026".to_string(),
                geometry: "geom".to_string(),
                primary_key: Some("road_id".to_string()),
                external_id: None,
            },
        )
        .unwrap();

        let config: tellurion_core::AppConfig = serde_yaml::from_str(&updated).unwrap();
        config.validate().unwrap();
        assert_eq!(config.tenants.last().unwrap().external_id(), "acme-data");
        assert_eq!(config.catalogs.last().unwrap().tenant, "acme");
        let collection = config.collections.last().unwrap();
        assert_eq!(collection.table.as_deref(), Some("roads_2026"));
        assert_eq!(collection.geometry.as_deref(), Some("geom"));
        assert_eq!(collection.pk.as_deref(), Some("road_id"));
    }

    #[test]
    fn rejects_a_duplicate_before_returning_changed_yaml() {
        let error = apply_change(
            CONFIG,
            Change::CreateTenant {
                id: "public".to_string(),
                external_id: None,
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("duplicate tenant id 'public'"));
    }

    #[test]
    fn rejects_an_invalid_identifier_before_returning_changed_yaml() {
        let error = apply_change(
            CONFIG,
            Change::CreateCatalog {
                id: "not a catalog".to_string(),
                tenant: "public".to_string(),
                external_id: None,
            },
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("may contain only letters, digits, '_' and '-'"));
    }

    #[test]
    fn rejects_a_reference_to_an_unknown_catalog_before_returning_changed_yaml() {
        let error = apply_change(
            CONFIG,
            Change::ReferenceCollection {
                id: "roads".to_string(),
                catalog: "missing".to_string(),
                storage: "main".to_string(),
                table: "roads".to_string(),
                geometry: "geom".to_string(),
                primary_key: Some("id".to_string()),
                external_id: None,
            },
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("collection 'roads' references unknown catalog 'missing'"));
    }

    #[test]
    fn atomically_replaces_the_config_after_creating_a_recoverable_backup() {
        let path = temporary_config_path();
        fs::write(&path, CONFIG).unwrap();

        let backup = write_atomically(&path, CONFIG, "tenants: []\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "tenants: []\n");
        assert_eq!(fs::read_to_string(&backup).unwrap(), CONFIG);
        fs::remove_file(path).unwrap();
        fs::remove_file(backup).unwrap();
    }

    #[test]
    fn refuses_a_second_operator_while_an_update_lock_is_held() {
        let path = temporary_config_path();
        fs::write(&path, CONFIG).unwrap();
        let lock = ConfigLock::acquire(&path).unwrap();

        let error = ConfigLock::acquire(&path).unwrap_err();

        assert!(error
            .to_string()
            .contains("another operator update appears to be running"));
        drop(lock);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn lock_file_records_process_and_start_metadata_for_manual_inspection() {
        let path = temporary_config_path();
        fs::write(&path, CONFIG).unwrap();
        let lock = ConfigLock::acquire(&path).unwrap();

        let metadata = fs::read_to_string(&lock.path).unwrap();

        assert!(metadata.contains(&format!("pid={}", std::process::id())));
        assert!(metadata.contains("started_unix_ms="));
        drop(lock);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn aborts_without_overwriting_an_external_edit_made_after_rendering() {
        let path = temporary_config_path();
        fs::write(&path, CONFIG).unwrap();
        let external_edit = format!("{CONFIG}# externally edited\n");
        fs::write(&path, &external_edit).unwrap();

        let error = write_atomically(&path, CONFIG, "tenants: []\n").unwrap_err();

        assert!(error.to_string().contains("changed since it was read"));
        assert_eq!(fs::read_to_string(&path).unwrap(), external_edit);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn validates_reference_relationships_before_physical_database_checks() {
        let error = validate_config_local_change(
            CONFIG,
            &Change::ReferenceCollection {
                id: "roads".to_string(),
                catalog: "missing".to_string(),
                storage: "main".to_string(),
                table: "roads".to_string(),
                geometry: "geom".to_string(),
                primary_key: None,
                external_id: None,
            },
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("collection 'roads' references unknown catalog 'missing'"));
    }

    #[test]
    fn preserves_comments_anchors_and_unknown_fields_outside_the_inserted_item() {
        let source = r#"# operator-owned heading
x-metadata: &shared
  owner: keep-exactly
storages:
  - id: main
    driver: postgis
    url_env: DATABASE_URL
tenants: [] # tenant declarations
catalogs: []
collections: []
x-copy: *shared
"#;

        let updated = apply_change(
            source,
            Change::CreateTenant {
                id: "acme".to_string(),
                external_id: Some("acme-data".to_string()),
            },
        )
        .unwrap();

        let expected = source.replacen(
            "tenants: [] # tenant declarations",
            "tenants: # tenant declarations\n  - id: acme\n    external_id: acme-data",
            1,
        );
        assert_eq!(updated, expected);
    }

    #[test]
    fn appends_to_a_block_sequence_without_reserializing_neighboring_sections() {
        let updated = apply_change(
            CONFIG,
            Change::CreateTenant {
                id: "acme".to_string(),
                external_id: None,
            },
        )
        .unwrap();

        assert_eq!(
            updated,
            CONFIG.replacen(
                "tenants:\n  - id: public\n",
                "tenants:\n  - id: public\n  - id: acme\n",
                1,
            )
        );
    }

    #[test]
    fn rejects_non_empty_flow_sequences_with_a_clear_error() {
        let source = CONFIG.replace("tenants:\n  - id: public", "tenants: [{ id: public }]");
        let error = apply_change(
            &source,
            Change::CreateTenant {
                id: "acme".to_string(),
                external_id: None,
            },
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("supports only a block sequence or an inline empty list"));
    }

    #[test]
    fn accepts_a_single_int4_or_int8_primary_key() {
        assert_eq!(
            interpret_postgis_reference(true, vec!["id".into()], vec!["int4".into()]).unwrap(),
            "id"
        );
        assert_eq!(
            interpret_postgis_reference(true, vec!["gid".into()], vec!["int8".into()]).unwrap(),
            "gid"
        );
    }

    #[test]
    fn rejects_missing_composite_and_non_integer_primary_keys() {
        assert!(interpret_postgis_reference(true, vec![], vec![])
            .unwrap_err()
            .to_string()
            .contains("has no primary key"));
        assert!(interpret_postgis_reference(
            true,
            vec!["part_a".into(), "part_b".into()],
            vec!["int4".into(), "int4".into()]
        )
        .unwrap_err()
        .to_string()
        .contains("composite primary key"));
        assert!(
            interpret_postgis_reference(true, vec!["id".into()], vec!["uuid".into()])
                .unwrap_err()
                .to_string()
                .contains("must use int4 or int8")
        );
        assert!(
            interpret_postgis_reference(true, vec!["id".into()], vec!["text".into()])
                .unwrap_err()
                .to_string()
                .contains("must use int4 or int8")
        );
    }

    #[test]
    fn rejects_an_unusable_geometry_before_interpreting_the_primary_key() {
        assert!(
            interpret_postgis_reference(false, vec!["id".into()], vec!["int8".into()])
                .unwrap_err()
                .to_string()
                .contains("geometry column")
        );
    }

    #[cfg(unix)]
    #[test]
    fn canonicalizes_a_symlink_before_locking_and_replacing_the_config() {
        use std::os::unix::fs::symlink;

        let target = temporary_config_path();
        let link = target.with_extension("link.yaml");
        fs::write(&target, CONFIG).unwrap();
        symlink(&target, &link).unwrap();

        let canonical = canonical_config_path(&link).unwrap();
        let backup = write_atomically(&canonical, CONFIG, "tenants: []\n").unwrap();

        assert_eq!(canonical, fs::canonicalize(&target).unwrap());
        assert_eq!(fs::read_to_string(&target).unwrap(), "tenants: []\n");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_file(link).unwrap();
        fs::remove_file(target).unwrap();
        fs::remove_file(backup).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn replacement_preserves_the_original_unix_permission_bits() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let path = temporary_config_path();
        fs::write(&path, CONFIG).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        let backup = write_atomically(&path, CONFIG, "tenants: []\n").unwrap();

        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o640);
        fs::remove_file(path).unwrap();
        fs::remove_file(backup).unwrap();
    }

    fn temporary_config_path() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_PATH: AtomicU64 = AtomicU64::new(0);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "tellurion-ingest-operator-{}-{unique}-{}.yaml",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
