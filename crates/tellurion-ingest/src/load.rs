//! `load`: ingests a local file or http(s)-fetched dataset into a fresh
//! physical table by shelling out to `ogr2ogr`, which owns the DDL it
//! creates; ingest never creates or alters tables itself here either — see
//! `ogr2ogr_loader`'s own doc comment for exactly what it invokes.

use tellurion_core::{CollectionDecl, RoutingDecl, StyleConf, TilesConf};

pub struct LoadArgs {
    pub source: String,
    pub collection: String,
    pub database_url_env: String,
    pub layer: Option<String>,
    pub catalog: String,
    pub storage: String,
}

pub async fn run(args: LoadArgs) -> anyhow::Result<()> {
    let db_url = crate::db::read_url(&args.database_url_env)?;
    let table = crate::sanitize::sanitize_identifier(&args.collection);
    let resolved = crate::source::resolve(&args.source).await?;

    let result =
        crate::ogr2ogr_loader::load(&resolved.path, &table, &db_url, args.layer.as_deref()).await;
    resolved.cleanup().await;
    result?;

    let decl = generic_collection_decl(&args.collection, &table, &args.catalog, &args.storage);
    println!("{}", crate::yaml_snippet::render_collection_snippet(decl)?);
    Ok(())
}

/// The loaded table's primary key column follows `ogr2ogr`'s own default FID
/// naming (`ogc_fid`) so the printed snippet is correct regardless of which
/// loader path produced the table.
fn generic_collection_decl(id: &str, table: &str, catalog: &str, storage: &str) -> CollectionDecl {
    CollectionDecl {
        id: id.to_string(),
        kind: tellurion_core::CollectionKind::Vector,
        external_id: None,
        catalog: catalog.to_string(),
        storage: storage.to_string(),
        routing: RoutingDecl::default(),
        table: Some(table.to_string()),
        geometry: Some("geom".to_string()),
        pk: Some("ogc_fid".to_string()),
        id_type: tellurion_core::IdType::default(),
        datetime: None,
        modified_column: None,
        row_estimate: None,
        srid: None,
        projection: None,
        geometry_profile: None,
        tiles: TilesConf::default(),
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
    fn generic_decl_uses_sanitized_table_and_default_pk() {
        let decl = generic_collection_decl("My Roads!", "my_roads_", "default", "main");
        assert_eq!(decl.id, "My Roads!");
        assert_eq!(decl.table.as_deref(), Some("my_roads_"));
        assert_eq!(decl.pk.as_deref(), Some("ogc_fid"));
        assert_eq!(decl.geometry.as_deref(), Some("geom"));
        assert_eq!(decl.datetime, None);
    }
}
