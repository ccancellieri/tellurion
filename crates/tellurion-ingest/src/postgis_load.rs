//! `postgis load` (`#114`): drives the same chunked apply
//! `geopackage_load.rs` does, in-process, against an EXISTING PostGIS table
//! — through the real `postgis` `WriteSink::apply_batch`
//! (`tellurion_postgis`), never a raw-SQL shortcut. Distinct from the
//! top-level `load` subcommand (`load.rs`): that one shells out to
//! `ogr2ogr` to create a brand-new table from an arbitrary vector dataset,
//! bypassing the outbox entirely (it's a DDL/bulk-import tool, the crate's
//! own "seed and load are the only places physical schema comes from"
//! rule); this one applies into a table that already exists and is already
//! outbox-provisioned, through the identical transactional write path the
//! HTTP batch route uses, so derived state (a read index, a search lane)
//! sees every row the same way a live `PUT` would have produced it.
//!
//! Every feature in the source dataset must carry its own top-level `id`
//! (`batch_apply::stage_one`'s own doc) — this command never mints one,
//! matching `id_type: text`'s own "caller-supplied" rule even for
//! `integer`/`uuid` collections, since a batch has no per-item URL to read
//! a server-minted id back from.

use tellurion_core::{
    CollectionDecl, DriverFactory, IdType, RoutingDecl, SearchConf, SettingsDecl, StorageDecl,
    StorageDriver, StyleConf, TilesConf, VisibilityDecl,
};
use tellurion_postgis::PostgisDriverFactory;

use crate::batch_apply;

pub struct LoadArgs {
    pub source: String,
    pub table: String,
    pub geometry: String,
    pub pk: String,
    pub id_type: String,
    pub srid: Option<i32>,
    pub database_url_env: String,
    pub chunk_items: usize,
    pub strict: bool,
}

pub async fn run(args: LoadArgs) -> anyhow::Result<()> {
    let id_type = parse_id_type(&args.id_type)?;

    let factory = PostgisDriverFactory::new(60);
    let decl = StorageDecl {
        id: "postgis-load".to_string(),
        driver: "postgis".to_string(),
        url_env: args.database_url_env,
        pool_size: None,
    };
    let driver: std::sync::Arc<dyn StorageDriver> = factory.build(&decl)?;
    let write_sink = driver
        .write_sink()
        .ok_or_else(|| anyhow::anyhow!("postgis storage does not advertise a write sink"))?;
    let outbox = driver
        .outbox_source()
        .ok_or_else(|| anyhow::anyhow!("postgis storage does not advertise an outbox source"))?;

    let collection =
        load_collection_decl(&args.table, &args.geometry, &args.pk, id_type, args.srid);

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

fn parse_id_type(raw: &str) -> anyhow::Result<IdType> {
    match raw.to_ascii_lowercase().as_str() {
        "integer" => Ok(IdType::Integer),
        "uuid" => Ok(IdType::Uuid),
        "text" => Ok(IdType::Text),
        other => anyhow::bail!("--id-type must be one of integer, uuid, text (got '{other}')"),
    }
}

fn load_collection_decl(
    table: &str,
    geometry: &str,
    pk: &str,
    id_type: IdType,
    srid: Option<i32>,
) -> CollectionDecl {
    CollectionDecl {
        projection: None,
        id: table.to_string(),
        kind: tellurion_core::CollectionKind::Vector,
        external_id: None,
        catalog: "default".to_string(),
        storage: "main".to_string(),
        routing: RoutingDecl::default(),
        table: Some(table.to_string()),
        geometry: Some(geometry.to_string()),
        pk: Some(pk.to_string()),
        id_type,
        datetime: None,
        modified_column: None,
        row_estimate: None,
        srid,
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
