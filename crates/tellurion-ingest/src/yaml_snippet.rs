//! Renders a `collections:` YAML snippet for a single freshly-ingested
//! collection, in the exact shape `tellurion-core::AppConfig` expects, so an
//! operator can paste it straight into the server's config file.

use serde::{Deserialize, Serialize};
use tellurion_core::CollectionDecl;

#[derive(Serialize, Deserialize)]
struct CollectionsSnippet {
    collections: Vec<CollectionDecl>,
}

pub fn render_collection_snippet(decl: CollectionDecl) -> anyhow::Result<String> {
    let snippet = CollectionsSnippet {
        collections: vec![decl],
    };
    Ok(serde_yaml::to_string(&snippet)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tellurion_core::{RoutingDecl, SettingsDecl, StyleConf, TilesConf, ZoomCaps};

    fn sample_decl() -> CollectionDecl {
        let mut caps = BTreeMap::new();
        caps.insert(0u8, 2000u64);
        CollectionDecl {
            id: "demo".to_string(),
            kind: tellurion_core::CollectionKind::Vector,
            external_id: None,
            catalog: "default".to_string(),
            storage: "main".to_string(),
            routing: RoutingDecl::default(),
            table: Some("demo".to_string()),
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
            settings: SettingsDecl::default(),
            attribute_columns: None,
            tile_properties: Vec::new(),
            visibility: tellurion_core::VisibilityDecl::default(),
            object_store: None,
            stac_metadata: false,
            stac_item_assets: false,
        }
    }

    #[test]
    fn renders_parseable_yaml() {
        let yaml = render_collection_snippet(sample_decl()).unwrap();
        assert!(yaml.contains("collections:"));
        assert!(yaml.contains("id: demo"));

        let parsed: CollectionsSnippet = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.collections.len(), 1);
        assert_eq!(parsed.collections[0], sample_decl());
    }

    #[test]
    fn round_trips_into_app_config_shape() {
        let yaml = render_collection_snippet(sample_decl()).unwrap();
        // The snippet must slot directly under a top-level `collections:` key,
        // matching `tellurion_core::AppConfig`'s field of the same name.
        let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        assert!(value.get("collections").is_some());
    }
}
