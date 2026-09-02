//! `geopackage load` (`#114`): closes the GeoPackage gap the README's
//! Quickstart names — until now, loading a real (non-synthetic) dataset
//! into a `.gpkg` file meant writing it through the HTTP API by hand, one
//! feature at a time. This drives the identical chunked apply the HTTP
//! batch route (`tellurion-features::batch_handlers`) does, in-process,
//! against a `.gpkg` file already provisioned by `geopackage create-tables`
//! — through the real `geopackage` `WriteSink::apply_batch`
//! (`tellurion_geopackage`), never a raw-SQL shortcut, the same "write
//! through the driver crate, not around it" rule `geopackage_seed.rs`
//! documents for its own write path.
//!
//! Every feature in the source dataset must carry its own top-level `id`
//! (`batch_apply::stage_one`'s own doc) — this command never mints one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use tellurion_core::{
    CollectionDecl, DriverFactory, IdType, PhysicalCollection, RoutingDecl, SearchConf,
    SettingsDecl, StorageDecl, StorageDriver, StyleConf, TilesConf, VisibilityDecl,
};
use tellurion_geopackage::GeopackageDriverFactory;

use crate::batch_apply;

pub struct LoadArgs {
    pub path: PathBuf,
    pub table: String,
    pub source: String,
    pub chunk_items: usize,
    pub strict: bool,
}

pub async fn run(args: LoadArgs) -> anyhow::Result<()> {
    let driver = open_driver(&args.path)?;
    let physical = find_feature_table(&driver, &args.table, &args.path).await?;

    let geometry_column = physical.geometry_column.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "table '{}' in '{}' has no registered geometry column",
            args.table,
            args.path.display()
        )
    })?;
    let pk = physical.primary_key.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "table '{}' in '{}' has no single-column INTEGER PRIMARY KEY this driver's v0.1 write path can address",
            args.table,
            args.path.display()
        )
    })?;
    let srid = physical.srid.unwrap_or(4326);

    let write_sink = driver.write_sink().ok_or_else(|| {
        anyhow::anyhow!(
            "geopackage storage at '{}' does not advertise a write sink",
            args.path.display()
        )
    })?;
    let outbox = driver.outbox_source().ok_or_else(|| {
        anyhow::anyhow!(
            "geopackage storage at '{}' does not advertise an outbox source",
            args.path.display()
        )
    })?;

    let collection = load_collection_decl(&args.table, &geometry_column, &pk, srid);

    let resolved = crate::source::resolve(&args.source).await?;
    let result = batch_apply::run(
        write_sink.as_ref(),
        outbox.as_ref(),
        &collection,
        &resolved.path,
        args.chunk_items,
        args.strict,
    )
    .await;
    resolved.cleanup().await;
    let summary = result?;

    eprintln!(
        "applied {}, refused {}, unapplied {} in {:.2}s ({:.0} features/sec); terminal={:?}, batch_high_water={:?}, primary_high_water={:?}",
        summary.applied,
        summary.refused,
        summary.unapplied,
        summary.elapsed.as_secs_f64(),
        summary.features_per_second(),
        summary.terminal,
        summary.batch_high_water,
        summary.outbox_high_water,
    );
    anyhow::ensure!(
        !matches!(
            summary.terminal,
            tellurion_core::BatchTerminalCondition::ChunkError
                | tellurion_core::BatchTerminalCondition::TransportError
        ),
        "batch load terminated with {:?}",
        summary.terminal
    );
    Ok(())
}

/// Byte-for-byte `geopackage_seed.rs`'s own `open_driver` — see that
/// function's own doc for why the env-var name is per-call, not a fixed
/// literal.
fn open_driver(path: &Path) -> anyhow::Result<Arc<dyn StorageDriver>> {
    static NEXT_CALL_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call_id = NEXT_CALL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path_env = format!("TELLURION_INGEST_GEOPACKAGE_LOAD_PATH_{call_id}");
    std::env::set_var(&path_env, path);
    let decl = StorageDecl {
        id: "geopackage-load".to_string(),
        driver: "geopackage".to_string(),
        url_env: path_env,
        pool_size: None,
    };
    GeopackageDriverFactory::new()
        .build(&decl)
        .with_context(|| format!("opening geopackage storage at '{}'", path.display()))
}

async fn find_feature_table(
    driver: &Arc<dyn StorageDriver>,
    table: &str,
    path: &Path,
) -> anyhow::Result<PhysicalCollection> {
    let collections: Vec<PhysicalCollection> = driver.catalog_source().collections().await?;
    collections
        .into_iter()
        .find(|c| c.name == table)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "table '{table}' is not a provisioned feature table in '{}'; run \
                 `tellurion-ingest geopackage create-tables --path {} --table {table} ...` first",
                path.display(),
                path.display(),
            )
        })
}

fn load_collection_decl(table: &str, geometry: &str, pk: &str, srid: i32) -> CollectionDecl {
    CollectionDecl {
        id: table.to_string(),
        kind: tellurion_core::CollectionKind::Vector,
        external_id: None,
        catalog: "default".to_string(),
        storage: "main".to_string(),
        routing: RoutingDecl::default(),
        table: Some(table.to_string()),
        geometry: Some(geometry.to_string()),
        pk: Some(pk.to_string()),
        id_type: IdType::default(),
        datetime: None,
        modified_column: None,
        row_estimate: None,
        srid: Some(srid),
        projection: None,
        geometry_profile: None,
        tiles: TilesConf::default(),
        geometry_variants: Vec::new(),
        style: StyleConf::default(),
        places3d: None,
        schema: None,
        search: SearchConf::default(),
        tile_invalidation: false,
        settings: SettingsDecl::default(),
        attribute_columns: None,
        tile_properties: Vec::new(),
        visibility: VisibilityDecl::default(),
        object_store: None,
        stac_metadata: false,
        stac_item_assets: false,
    }
}
