//! HTTP-level tests for the per-item STAC metadata sidecar (`#202`): a
//! fake, in-memory `StacMetadataSource` driven through the real
//! `tellurion_core::Router` and the real axum router this crate exports —
//! no database involved, same style as this crate's own
//! `tests/handlers.rs`.
//!
//! What these pin down, in the issue's own terms:
//!
//! - a collection with no sidecar configured serves byte-identical Items,
//!   and the capability is never even consulted;
//! - a configured sidecar merges with the documented precedence rule
//!   (sidecar wins per key, reserved structural members ignored);
//! - the lookup is batched: ONE call per page carrying every id on that
//!   page, never one call per item.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use tower::ServiceExt;

use tellurion_core::{
    AppConfig, AppContext, CatalogSource, CollectionDecl, DriverFactory, FeaturePage,
    FeatureSource, FileStyleStore, Filter, ItemsQuery, MokaTileCache, PhysicalCollection, Registry,
    Resolver, Result as CoreResult, Router as CoreRouter, SpatialExtent, StacMetadataSource,
    StaticResolver, StorageDecl, StorageDriver, StyleStore, TileCache,
};

struct SidecarCatalog;

#[async_trait::async_trait]
impl CatalogSource for SidecarCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: "demo".to_string(),
            geometry_column: Some("geom".to_string()),
            primary_key: Some("id".to_string()),
            srid: Some(4326),
            geometry_type: None,
        }])
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        Ok(Some(SpatialExtent {
            bbox: [-5.0, 45.0, 5.0, 55.0],
        }))
    }
}

/// Ascending-by-id keyset paging, the same in-memory shape
/// `tests/handlers.rs`'s own `ItemsFeatureSource` uses.
struct SidecarFeatureSource {
    items: Vec<(String, Value)>,
}

#[async_trait::async_trait]
impl FeatureSource for SidecarFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        let start = match &query.token {
            Some(token) => self
                .items
                .iter()
                .position(|(id, _)| id == token)
                .map(|i| i + 1)
                .unwrap_or(0),
            None => 0,
        };
        let remaining = &self.items[start..];
        let limit = query.limit as usize;
        let has_more = remaining.len() > limit;
        let page: Vec<Value> = remaining
            .iter()
            .take(limit)
            .map(|(_, v)| v.clone())
            .collect();
        let next_token = has_more.then(|| remaining[limit - 1].0.clone());
        Ok(FeaturePage {
            features_geojson: page,
            number_matched: Some(self.items.len() as u64),
            next_token,
        })
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        id: &str,
        _filter: Option<&Filter>,
    ) -> CoreResult<Option<Value>> {
        Ok(self
            .items
            .iter()
            .find(|(item_id, _)| item_id == id)
            .map(|(_, v)| v.clone()))
    }
}

/// The fake sidecar: fixed docs by feature id, plus the two things the
/// batching claim needs to be checkable — how many times it was called, and
/// with which ids each time.
#[derive(Default)]
struct FakeSidecar {
    docs: HashMap<String, Value>,
    calls: AtomicUsize,
    seen_ids: Mutex<Vec<Vec<String>>>,
}

#[async_trait::async_trait]
impl StacMetadataSource for FakeSidecar {
    async fn stac_metadata(
        &self,
        _collection: &CollectionDecl,
        feature_ids: &[String],
    ) -> CoreResult<HashMap<String, Value>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen_ids.lock().unwrap().push(feature_ids.to_vec());
        Ok(feature_ids
            .iter()
            .filter_map(|id| self.docs.get(id).map(|doc| (id.clone(), doc.clone())))
            .collect())
    }
}

struct SidecarDriver {
    source: Arc<SidecarFeatureSource>,
    sidecar: Arc<FakeSidecar>,
}

impl StorageDriver for SidecarDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(SidecarCatalog)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(self.source.clone() as Arc<dyn FeatureSource>)
    }

    fn stac_metadata_source(&self) -> Option<Arc<dyn StacMetadataSource>> {
        Some(self.sidecar.clone() as Arc<dyn StacMetadataSource>)
    }
}

struct SidecarFactory {
    source: Arc<SidecarFeatureSource>,
    sidecar: Arc<FakeSidecar>,
}

impl DriverFactory for SidecarFactory {
    fn name(&self) -> &str {
        "sidecar-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(SidecarDriver {
            source: self.source.clone(),
            sidecar: self.sidecar.clone(),
        }))
    }
}

fn config_yaml(collection_extra: &str) -> String {
    format!(
        r#"
storages: [ {{ id: main, driver: sidecar-fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
{collection_extra}
"#
    )
}

fn feature(id: &str, properties: Value) -> Value {
    json!({
        "type": "Feature",
        "id": id,
        "geometry": { "type": "Point", "coordinates": [1.0, 2.0] },
        "properties": properties,
    })
}

fn build_app(
    collection_extra: &str,
    items: Vec<(&str, Value)>,
    docs: Vec<(&str, Value)>,
) -> (axum::Router, Arc<FakeSidecar>) {
    let config: AppConfig = serde_yaml::from_str(&config_yaml(collection_extra)).unwrap();
    config.validate().unwrap();

    let source = Arc::new(SidecarFeatureSource {
        items: items
            .into_iter()
            .map(|(id, v)| (id.to_string(), v))
            .collect(),
    });
    let sidecar = Arc::new(FakeSidecar {
        docs: docs
            .into_iter()
            .map(|(id, v)| (id.to_string(), v))
            .collect(),
        ..FakeSidecar::default()
    });

    let mut registry = Registry::new();
    registry.register(Arc::new(SidecarFactory {
        source,
        sidecar: sidecar.clone(),
    }));

    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    let ctx = Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        None,
        cache,
        style_store,
    ));
    (tellurion_stac::router().with_state(ctx), sidecar)
}

async fn get(app: &axum::Router, uri: &str) -> Response {
    app.clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

const OPT_IN: &str = "    stac_metadata: true";

/// The acceptance criterion this whole slice hangs on: a collection that
/// never declared the sidecar serves the exact same Item bytes it served
/// before `#202` — and the capability is not consulted even once, so a
/// driver that advertises one cannot change the answer.
#[tokio::test]
async fn a_collection_with_no_sidecar_configured_serves_identical_items() {
    let items = vec![("a", feature("a", json!({"name": "acme"})))];
    let docs = vec![("a", json!({"properties": {"name": "OVERRIDDEN"}}))];

    let (opted_out, sidecar) = build_app("", items.clone(), docs.clone());
    let without = body_json(get(&opted_out, "/collections/demo/items").await).await;
    assert_eq!(
        sidecar.calls.load(Ordering::SeqCst),
        0,
        "a collection with no stac_metadata opt-in must never consult the sidecar capability"
    );

    // A sidecar-less deployment of the same collection: identical bytes.
    let (no_docs, _) = build_app("", items, vec![]);
    let baseline = body_json(get(&no_docs, "/collections/demo/items").await).await;
    assert_eq!(without, baseline);
    assert_eq!(without["features"][0]["properties"]["name"], "acme");
}

/// The documented precedence rule: the sidecar wins on a colliding
/// `properties` key, non-colliding feature properties survive, and a
/// non-`properties` member (`stac_extensions`) is set verbatim.
#[tokio::test]
async fn a_configured_sidecar_merges_with_sidecar_precedence_over_feature_properties() {
    let (app, _) = build_app(
        OPT_IN,
        vec![(
            "a",
            feature("a", json!({"name": "acme", "kept": "from-feature"})),
        )],
        vec![(
            "a",
            json!({
                "stac_extensions": ["https://stac-extensions.github.io/eo/v1.1.0/schema.json"],
                "properties": { "name": "from-sidecar", "eo:cloud_cover": 12 }
            }),
        )],
    );

    let body = body_json(get(&app, "/collections/demo/items").await).await;
    let item = &body["features"][0];
    assert_eq!(item["properties"]["name"], "from-sidecar");
    assert_eq!(item["properties"]["kept"], "from-feature");
    assert_eq!(item["properties"]["eo:cloud_cover"], 12);
    assert_eq!(
        item["stac_extensions"],
        json!(["https://stac-extensions.github.io/eo/v1.1.0/schema.json"])
    );
    // An item with no sidecar row of its own is untouched.
    assert_eq!(item["id"], "a");
}

/// Structural members the STAC lane derives are never overridable — see
/// `mapping::RESERVED_ITEM_MEMBERS`. A doc carrying them merges its other
/// members and ignores exactly those.
#[tokio::test]
async fn a_sidecar_can_never_rewrite_the_items_identity_geometry_links_or_assets() {
    let (app, _) = build_app(
        OPT_IN,
        vec![("a", feature("a", json!({})))],
        vec![(
            "a",
            json!({
                "id": "hijacked",
                "collection": "hijacked",
                "geometry": { "type": "Point", "coordinates": [99.0, 99.0] },
                "bbox": [9.0, 9.0, 9.0, 9.0],
                "links": [],
                "assets": { "ghost": { "href": "https://example.test/ghost" } },
                "stac_version": "0.0.1",
                "type": "NotAFeature",
                "properties": { "merged": true }
            }),
        )],
    );

    let body = body_json(get(&app, "/collections/demo/items").await).await;
    let item = &body["features"][0];
    assert_eq!(item["id"], "a");
    assert_eq!(item["collection"], "demo");
    assert_eq!(item["type"], "Feature");
    assert_eq!(item["stac_version"], "1.1.0");
    assert_eq!(item["geometry"]["coordinates"], json!([1.0, 2.0]));
    assert_eq!(item["bbox"], json!([1.0, 2.0, 1.0, 2.0]));
    assert_eq!(item["assets"], json!({}));
    assert!(
        !item["links"].as_array().unwrap().is_empty(),
        "the request's own links must survive a sidecar carrying an empty links array"
    );
    // Everything outside the reserved set still merged.
    assert_eq!(item["properties"]["merged"], true);
}

/// The sidecar's most useful single job: a collection with no datetime
/// column at all serves `properties.datetime: null` today, and a sidecar
/// row replaces exactly that.
#[tokio::test]
async fn a_sidecar_can_supply_the_datetime_a_collection_has_no_column_for() {
    let (app, _) = build_app(
        OPT_IN,
        vec![
            ("a", feature("a", json!({}))),
            ("b", feature("b", json!({}))),
        ],
        vec![(
            "a",
            json!({"properties": {"datetime": "2021-01-01T00:00:00Z"}}),
        )],
    );

    let body = body_json(get(&app, "/collections/demo/items").await).await;
    assert_eq!(
        body["features"][0]["properties"]["datetime"],
        "2021-01-01T00:00:00Z"
    );
    // The item with no sidecar row keeps the documented honest `null`.
    assert!(body["features"][1]["properties"]["datetime"].is_null());
}

/// The cost model: ONE lookup for a whole page, carrying every id on it.
#[tokio::test]
async fn the_page_lookup_is_batched_into_one_call_carrying_every_id() {
    let (app, sidecar) = build_app(
        OPT_IN,
        vec![
            ("a", feature("a", json!({}))),
            ("b", feature("b", json!({}))),
            ("c", feature("c", json!({}))),
        ],
        vec![("b", json!({"properties": {"tagged": true}}))],
    );

    let body = body_json(get(&app, "/collections/demo/items?limit=3").await).await;
    assert_eq!(body["numberReturned"], 3);
    assert_eq!(
        sidecar.calls.load(Ordering::SeqCst),
        1,
        "a page of three items must cost one sidecar round trip, not three"
    );
    assert_eq!(
        sidecar.seen_ids.lock().unwrap()[0],
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
    assert_eq!(body["features"][1]["properties"]["tagged"], true);
    assert!(body["features"][0]["properties"].get("tagged").is_none());
}

#[tokio::test]
async fn the_single_item_lane_merges_the_sidecar_too() {
    let (app, sidecar) = build_app(
        OPT_IN,
        vec![("a", feature("a", json!({"name": "acme"})))],
        vec![("a", json!({"properties": {"name": "from-sidecar"}}))],
    );

    let body = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert_eq!(body["id"], "a");
    assert_eq!(body["properties"]["name"], "from-sidecar");
    assert_eq!(
        sidecar.seen_ids.lock().unwrap()[0],
        vec!["a".to_string()],
        "the single-item lane batches a one-element page, not a differently-shaped lookup"
    );
}

/// `/search` is the same Item mapping, so it merges the same sidecar — and
/// batches it per collection slice of the page, not per item.
#[tokio::test]
async fn search_merges_the_sidecar_and_batches_it_per_page() {
    let (app, sidecar) = build_app(
        OPT_IN,
        vec![
            ("a", feature("a", json!({}))),
            ("b", feature("b", json!({}))),
        ],
        vec![
            ("a", json!({"properties": {"scene": "one"}})),
            ("b", json!({"properties": {"scene": "two"}})),
        ],
    );

    let body = body_json(get(&app, "/search?collections=demo&limit=2").await).await;
    assert_eq!(body["features"][0]["properties"]["scene"], "one");
    assert_eq!(body["features"][1]["properties"]["scene"], "two");
    assert_eq!(
        sidecar.calls.load(Ordering::SeqCst),
        1,
        "a two-item search page must cost one sidecar round trip"
    );
}

/// `/search` in `ids` mode walks the `(collections, ids)` cross product one
/// pair at a time; the sidecar is still fetched once per collection
/// resolved, for every id the request named.
#[tokio::test]
async fn search_by_ids_batches_the_sidecar_once_per_collection() {
    let (app, sidecar) = build_app(
        OPT_IN,
        vec![
            ("a", feature("a", json!({}))),
            ("b", feature("b", json!({}))),
        ],
        vec![("b", json!({"properties": {"scene": "two"}}))],
    );

    let response = get(&app, "/search?collections=demo&ids=a,b").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["features"].as_array().unwrap().len(), 2);
    assert_eq!(body["features"][1]["properties"]["scene"], "two");
    assert_eq!(
        sidecar.calls.load(Ordering::SeqCst),
        1,
        "ids mode must batch the whole id list once per collection, not once per pair"
    );
}
