//! HTTP-level tests for projecting item-scoped asset records into STAC
//! Items (`#221`): a fake, in-memory `AssetRecordStore` driven through the
//! real `tellurion_core::Router` and the real axum router this crate
//! exports — no database involved, same style as this crate's own
//! `tests/sidecar.rs`, whose machinery this slice deliberately reuses
//! rather than parallels.
//!
//! What these pin down, in the issue's own terms:
//!
//! - a collection that never opted in serves byte-identical Items, and the
//!   capability is never even consulted;
//! - available item-scoped records appear in *that* Item's `assets`, and
//!   nowhere else;
//! - a collection-scoped record never leaks onto an Item;
//! - managed records resolve to the stable Tellurion data resource, remote
//!   records keep their external href;
//! - pending/failed managed records are not advertised;
//! - a persisted record wins the documented collision against a
//!   capability-derived entry of the same key;
//! - **the read is batched**: ONE `item_assets` call per page carrying
//!   every id on it, and ZERO per-item `get` calls — the N+1 guard. A
//!   reimplementation that looked records up inside the per-feature loop
//!   would still produce correct JSON and would fail every one of the
//!   counting assertions below.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use tower::ServiceExt;

use tellurion_core::{
    AppConfig, AppContext, AssetKind, AssetRecord, AssetRecordEntry, AssetRecordStore, AssetState,
    CatalogSource, CollectionDecl, DriverFactory, Error as CoreError, FeaturePage, FeatureSource,
    FileStyleStore, Filter, FinalizeOutcome, ItemsQuery, MokaTileCache, NewAssetRecord,
    PhysicalCollection, Registry, Resolver, Result as CoreResult, Router as CoreRouter,
    SpatialExtent, StaticResolver, StorageDecl, StorageDriver, StyleStore, TileCache,
};

struct ItemAssetsCatalog;

#[async_trait::async_trait]
impl CatalogSource for ItemAssetsCatalog {
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
/// `tests/sidecar.rs`'s own feature source uses.
struct ItemAssetsFeatureSource {
    items: Vec<(String, Value)>,
}

#[async_trait::async_trait]
impl FeatureSource for ItemAssetsFeatureSource {
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

/// The fake store: fixed records, plus the three things the batching claim
/// needs to be checkable — how many times the batched read was called, with
/// which ids each time, and how many times the *per-key* `get` was called
/// (which must stay zero: a per-item implementation would reach for it).
#[derive(Default)]
struct FakeAssetStore {
    /// `(item_id, key) -> record`; `item_id: None` is collection-level.
    records: Vec<(Option<String>, String, AssetRecord)>,
    batched_calls: AtomicUsize,
    seen_ids: Mutex<Vec<Vec<String>>>,
    get_calls: AtomicUsize,
}

#[async_trait::async_trait]
impl AssetRecordStore for FakeAssetStore {
    async fn register(
        &self,
        _collection: &CollectionDecl,
        _item_id: Option<&str>,
        _key: &str,
        _new_record: NewAssetRecord,
    ) -> CoreResult<AssetRecord> {
        Err(CoreError::NotFound)
    }

    async fn get(
        &self,
        _collection: &CollectionDecl,
        _item_id: Option<&str>,
        _key: &str,
    ) -> CoreResult<Option<AssetRecord>> {
        self.get_calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }

    async fn finalize(
        &self,
        _collection: &CollectionDecl,
        _item_id: Option<&str>,
        _key: &str,
        _outcome: FinalizeOutcome,
    ) -> CoreResult<AssetRecord> {
        Err(CoreError::NotFound)
    }

    async fn delete(
        &self,
        _collection: &CollectionDecl,
        _item_id: Option<&str>,
        _key: &str,
    ) -> CoreResult<Option<AssetRecord>> {
        Ok(None)
    }

    async fn list(&self, _collection: &CollectionDecl) -> CoreResult<Vec<AssetRecordEntry>> {
        panic!("the Item projection must never fall back to the unbounded reconcile walk");
    }

    /// Implements the trait's own documented contract, including the two
    /// rules a fixture could otherwise quietly get wrong and pass for the
    /// wrong reason: the `""` collection-level scope is excluded no matter
    /// what was asked for (the real driver enforces this in SQL —
    /// `asset_sql::build_item_lookup_plan`'s `item_id <> ''`), and every
    /// lifecycle state is reported, leaving the advertisability rule to the
    /// STAC lane.
    async fn item_assets(
        &self,
        _collection: &CollectionDecl,
        item_ids: &[String],
    ) -> CoreResult<Vec<AssetRecordEntry>> {
        if item_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.batched_calls.fetch_add(1, Ordering::SeqCst);
        self.seen_ids.lock().unwrap().push(item_ids.to_vec());
        Ok(self
            .records
            .iter()
            .filter(|(item_id, _, _)| match item_id {
                None => false,
                Some(id) => !id.is_empty() && item_ids.iter().any(|asked| asked == id),
            })
            .map(|(item_id, key, record)| AssetRecordEntry {
                item_id: item_id.clone(),
                key: key.clone(),
                record: record.clone(),
            })
            .collect())
    }
}

struct ItemAssetsDriver {
    source: Arc<ItemAssetsFeatureSource>,
    store: Arc<FakeAssetStore>,
    /// `false` reproduces a driver with no asset capability at all — the
    /// `CapabilityUnsupported` half of the resolution contract.
    advertise_assets: bool,
}

impl StorageDriver for ItemAssetsDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(ItemAssetsCatalog)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(self.source.clone() as Arc<dyn FeatureSource>)
    }

    fn asset_record_store(&self) -> Option<Arc<dyn AssetRecordStore>> {
        self.advertise_assets
            .then(|| self.store.clone() as Arc<dyn AssetRecordStore>)
    }
}

struct ItemAssetsFactory {
    source: Arc<ItemAssetsFeatureSource>,
    store: Arc<FakeAssetStore>,
    advertise_assets: bool,
}

impl DriverFactory for ItemAssetsFactory {
    fn name(&self) -> &str {
        "item-assets-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(ItemAssetsDriver {
            source: self.source.clone(),
            store: self.store.clone(),
            advertise_assets: self.advertise_assets,
        }))
    }
}

fn config_yaml(collection_extra: &str) -> String {
    format!(
        r#"
storages: [ {{ id: main, driver: item-assets-fake, url_env: DATABASE_URL }} ]
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

fn feature(id: &str) -> Value {
    json!({
        "type": "Feature",
        "id": id,
        "geometry": { "type": "Point", "coordinates": [1.0, 2.0] },
        "properties": { "name": id },
    })
}

fn remote(href: &str) -> AssetRecord {
    AssetRecord {
        id: uuid::Uuid::nil(),
        kind: AssetKind::Remote,
        state: AssetState::Available,
        href: Some(href.to_string()),
        media_type: Some("image/tiff; application=geotiff; profile=cloud-optimized".to_string()),
        title: Some("Scene COG".to_string()),
        description: Some("this scene's own COG".to_string()),
        roles: vec!["data".to_string()],
        declared_size: None,
        digest: None,
        failure_reason: None,
    }
}

fn managed(state: AssetState) -> AssetRecord {
    AssetRecord {
        id: uuid::Uuid::nil(),
        kind: AssetKind::Managed,
        state,
        href: None,
        media_type: Some("image/tiff".to_string()),
        title: None,
        description: None,
        roles: vec!["data".to_string()],
        declared_size: Some(4),
        digest: None,
        failure_reason: None,
    }
}

type RecordSpec<'a> = (Option<&'a str>, &'a str, AssetRecord);

fn build_app_with(
    collection_extra: &str,
    items: Vec<&str>,
    records: Vec<RecordSpec<'_>>,
    advertise_assets: bool,
) -> (axum::Router, Arc<FakeAssetStore>) {
    let config: AppConfig = serde_yaml::from_str(&config_yaml(collection_extra)).unwrap();
    config.validate().unwrap();

    let source = Arc::new(ItemAssetsFeatureSource {
        items: items
            .into_iter()
            .map(|id| (id.to_string(), feature(id)))
            .collect(),
    });
    let store = Arc::new(FakeAssetStore {
        records: records
            .into_iter()
            .map(|(item_id, key, record)| (item_id.map(str::to_string), key.to_string(), record))
            .collect(),
        ..FakeAssetStore::default()
    });

    let mut registry = Registry::new();
    registry.register(Arc::new(ItemAssetsFactory {
        source,
        store: store.clone(),
        advertise_assets,
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
    (tellurion_stac::router().with_state(ctx), store)
}

fn build_app(
    collection_extra: &str,
    items: Vec<&str>,
    records: Vec<RecordSpec<'_>>,
) -> (axum::Router, Arc<FakeAssetStore>) {
    build_app_with(collection_extra, items, records, true)
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

const OPT_IN: &str = "    stac_item_assets: true";

/// The acceptance criterion the whole slice hangs on: a collection that
/// never opted in serves the exact same Item bytes it served before `#221`
/// — and the capability is not consulted even once, so a driver that
/// advertises one cannot change the answer.
#[tokio::test]
async fn a_collection_with_no_opt_in_serves_identical_items() {
    let records = vec![(Some("a"), "cog", remote("https://example.test/a.tif"))];

    let (opted_out, store) = build_app("", vec!["a"], records.clone());
    let without = body_json(get(&opted_out, "/collections/demo/items").await).await;
    assert_eq!(
        store.batched_calls.load(Ordering::SeqCst),
        0,
        "a collection with no stac_item_assets opt-in must never consult the assets capability"
    );
    assert_eq!(store.get_calls.load(Ordering::SeqCst), 0);

    // A deployment of the same collection with no records at all: identical
    // bytes, down to the whole document.
    let (no_records, _) = build_app("", vec!["a"], vec![]);
    let baseline = body_json(get(&no_records, "/collections/demo/items").await).await;
    assert_eq!(without, baseline);
    assert_eq!(without["features"][0]["assets"], json!({}));
}

/// Each Item carries its own records and no other Item's — the distinction
/// a single collection-level asset map cannot express, which is the whole
/// reason for this slice.
#[tokio::test]
async fn item_scoped_records_appear_on_their_own_item_only() {
    let (app, _) = build_app(
        OPT_IN,
        vec!["a", "b", "c"],
        vec![
            (Some("a"), "cog", remote("https://example.test/a.tif")),
            (Some("b"), "zarr", remote("https://example.test/b.zarr")),
        ],
    );

    let body = body_json(get(&app, "/collections/demo/items?limit=3").await).await;
    let items = body["features"].as_array().unwrap();

    assert_eq!(
        items[0]["assets"]["cog"]["href"],
        "https://example.test/a.tif"
    );
    assert_eq!(
        items[0]["assets"]["cog"]["type"],
        "image/tiff; application=geotiff; profile=cloud-optimized"
    );
    assert_eq!(items[0]["assets"]["cog"]["title"], "Scene COG");
    assert_eq!(
        items[0]["assets"]["cog"]["description"],
        "this scene's own COG"
    );
    assert_eq!(items[0]["assets"]["cog"]["roles"], json!(["data"]));
    assert!(items[0]["assets"].get("zarr").is_none());

    assert_eq!(
        items[1]["assets"]["zarr"]["href"],
        "https://example.test/b.zarr"
    );
    assert!(items[1]["assets"].get("cog").is_none());

    // An item on the same page with no records of its own.
    assert_eq!(items[2]["assets"], json!({}));
}

/// A managed record advertises the stable Tellurion data resource — byte
/// for byte the href the assets API's own `GET .../assets/{key}` returns
/// for the same record, because both come from `assets::asset_data_href`.
#[tokio::test]
async fn a_managed_record_resolves_to_the_stable_data_resource() {
    let (app, _) = build_app(
        OPT_IN,
        vec!["a"],
        vec![(Some("a"), "cog", managed(AssetState::Available))],
    );

    let body = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert_eq!(
        body["assets"]["cog"]["href"],
        "/public/stac/catalogs/default/collections/demo/items/a/assets/cog/data"
    );
}

/// Lifecycle: a managed asset whose bytes have not arrived (or never will)
/// is not advertised, so no served document carries an href this server
/// already knows would fail.
#[tokio::test]
async fn pending_and_failed_managed_records_are_not_advertised() {
    let mut failed = managed(AssetState::Failed);
    failed.failure_reason = Some("digest mismatch".to_string());
    let (app, _) = build_app(
        OPT_IN,
        vec!["a"],
        vec![
            (Some("a"), "pending", managed(AssetState::Pending)),
            (Some("a"), "failed", failed),
            (Some("a"), "cog", remote("https://example.test/a.tif")),
        ],
    );

    let body = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert!(body["assets"].get("pending").is_none());
    assert!(body["assets"].get("failed").is_none());
    assert_eq!(
        body["assets"]["cog"]["href"], "https://example.test/a.tif",
        "an unavailable sibling must not suppress an available record"
    );
}

/// A collection-scoped record stays at Collection scope: it is never
/// flattened onto an Item. Enforced at the storage layer (the fake mirrors
/// the driver's own `item_id <> ''` predicate) and again in the projection.
#[tokio::test]
async fn a_collection_scoped_record_never_appears_on_an_item() {
    let (app, _) = build_app(
        OPT_IN,
        vec!["a"],
        vec![
            (None, "license", remote("https://example.test/LICENSE")),
            (Some("a"), "cog", remote("https://example.test/a.tif")),
        ],
    );

    let body = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert!(
        body["assets"].get("license").is_none(),
        "a collection-level record must not ride onto an Item"
    );
    assert!(body["assets"].get("cog").is_some());
}

/// The documented collision rule: a persisted record wins over a
/// capability-derived entry sharing its key. This collection has no tiles
/// lane, so the derived map is empty — the precedence itself is unit-tested
/// against a populated derived map in `src/assets.rs`; here we only pin
/// that two records under distinct keys both survive into one map.
#[tokio::test]
async fn several_records_for_one_item_all_land_in_its_asset_map() {
    let (app, _) = build_app(
        OPT_IN,
        vec!["a"],
        vec![
            (Some("a"), "cog", remote("https://example.test/a.tif")),
            (Some("a"), "thumbnail", remote("https://example.test/a.png")),
        ],
    );

    let body = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert_eq!(body["assets"]["cog"]["href"], "https://example.test/a.tif");
    assert_eq!(
        body["assets"]["thumbnail"]["href"],
        "https://example.test/a.png"
    );
}

// -- the N+1 guard --------------------------------------------------------

/// The cost model, stated as a count rather than as output: ONE batched
/// call for a whole page, carrying every id on it, and ZERO per-key `get`
/// calls. An implementation that looked each item's records up inside the
/// per-feature loop would serve exactly the same JSON and fail here.
#[tokio::test]
async fn a_page_costs_one_batched_call_and_never_a_per_item_read() {
    let (app, store) = build_app(
        OPT_IN,
        vec!["a", "b", "c", "d"],
        vec![(Some("c"), "cog", remote("https://example.test/c.tif"))],
    );

    let body = body_json(get(&app, "/collections/demo/items?limit=4").await).await;
    assert_eq!(body["numberReturned"], 4);
    assert_eq!(
        store.batched_calls.load(Ordering::SeqCst),
        1,
        "a page of four items must cost one asset round trip, not four"
    );
    assert_eq!(
        store.get_calls.load(Ordering::SeqCst),
        0,
        "the Item projection must never issue a per-item asset read"
    );
    assert_eq!(
        store.seen_ids.lock().unwrap()[0],
        vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string()
        ],
        "the one call must carry every id on the page"
    );
    assert_eq!(
        body["features"][2]["assets"]["cog"]["href"],
        "https://example.test/c.tif"
    );
}

/// Paging does not multiply the cost per item: each page pays exactly one
/// call carrying exactly that page's ids.
#[tokio::test]
async fn each_page_of_a_paged_item_collection_costs_exactly_one_call() {
    let (app, store) = build_app(
        OPT_IN,
        vec!["a", "b", "c", "d"],
        vec![(Some("d"), "cog", remote("https://example.test/d.tif"))],
    );

    let first = body_json(get(&app, "/collections/demo/items?limit=2").await).await;
    assert_eq!(first["numberReturned"], 2);
    assert_eq!(store.batched_calls.load(Ordering::SeqCst), 1);

    let next = first["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|link| link["rel"] == "next")
        .expect("a next link")["href"]
        .as_str()
        .unwrap()
        .to_string();
    let second = body_json(get(&app, &next).await).await;
    assert_eq!(second["numberReturned"], 2);

    assert_eq!(
        store.batched_calls.load(Ordering::SeqCst),
        2,
        "two pages of two items must cost two calls, not four"
    );
    assert_eq!(store.get_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        *store.seen_ids.lock().unwrap(),
        vec![
            vec!["a".to_string(), "b".to_string()],
            vec!["c".to_string(), "d".to_string()]
        ]
    );
    assert_eq!(
        second["features"][1]["assets"]["cog"]["href"],
        "https://example.test/d.tif"
    );
}

#[tokio::test]
async fn the_single_item_lane_batches_a_one_element_page() {
    let (app, store) = build_app(
        OPT_IN,
        vec!["a", "b"],
        vec![(Some("a"), "cog", remote("https://example.test/a.tif"))],
    );

    let body = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert_eq!(body["id"], "a");
    assert_eq!(body["assets"]["cog"]["href"], "https://example.test/a.tif");
    assert_eq!(
        store.seen_ids.lock().unwrap()[0],
        vec!["a".to_string()],
        "the single-item lane batches a one-element page, not a differently-shaped lookup"
    );
    assert_eq!(store.get_calls.load(Ordering::SeqCst), 0);
}

/// `/search` is the same Item mapping, so it projects the same records —
/// and batches them per collection slice of the page, not per item.
#[tokio::test]
async fn search_projects_the_records_and_batches_them_per_page() {
    let (app, store) = build_app(
        OPT_IN,
        vec!["a", "b"],
        vec![
            (Some("a"), "cog", remote("https://example.test/a.tif")),
            (Some("b"), "cog", remote("https://example.test/b.tif")),
        ],
    );

    let body = body_json(get(&app, "/search?collections=demo&limit=2").await).await;
    assert_eq!(
        body["features"][0]["assets"]["cog"]["href"],
        "https://example.test/a.tif"
    );
    assert_eq!(
        body["features"][1]["assets"]["cog"]["href"],
        "https://example.test/b.tif"
    );
    assert_eq!(
        store.batched_calls.load(Ordering::SeqCst),
        1,
        "a two-item search page must cost one asset round trip"
    );
    assert_eq!(store.get_calls.load(Ordering::SeqCst), 0);
}

/// `/search` in `ids` mode walks the `(collections, ids)` cross product one
/// pair at a time; the records are still fetched once per collection
/// resolved, for every id the request named — never once per pair.
#[tokio::test]
async fn search_by_ids_batches_once_per_collection_not_once_per_pair() {
    let (app, store) = build_app(
        OPT_IN,
        vec!["a", "b"],
        vec![(Some("b"), "cog", remote("https://example.test/b.tif"))],
    );

    let response = get(&app, "/search?collections=demo&ids=a,b").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["features"].as_array().unwrap().len(), 2);
    assert_eq!(
        body["features"][1]["assets"]["cog"]["href"],
        "https://example.test/b.tif"
    );
    assert_eq!(
        store.batched_calls.load(Ordering::SeqCst),
        1,
        "ids mode must batch the whole id list once per collection, not once per pair"
    );
    assert_eq!(store.get_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.seen_ids.lock().unwrap()[0],
        vec!["a".to_string(), "b".to_string()]
    );
}

/// An opted-in collection whose driver advertises no `asset_record_store`
/// refuses by name rather than silently serving Items with no assets, which
/// would be indistinguishable from the un-opted-in case.
#[tokio::test]
async fn an_opted_in_collection_on_an_asset_incapable_driver_refuses_by_name() {
    let (app, _) = build_app_with(OPT_IN, vec!["a"], vec![], false);

    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    assert!(
        body["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("assets"),
        "the refusal must name the missing capability, body was: {body}"
    );
}
