//! Handler tests: a fake, in-memory `FeatureSource` driven through the real
//! `tellurion_core::Router` and the real axum router this crate exports.
//! Covers paging token round-trip, bbox/datetime validation, content types,
//! and link rels — no database involved.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use tower::ServiceExt;

use tellurion_core::{
    locking, AppConfig, AppContext, AttributeColumn, CatalogSource, CollectionDecl, CompareOp,
    DriverFactory, FeaturePage, FeatureSizeStats, FeatureSource, FileStyleStore, Filter,
    GeometryProfile, ItemsQuery, Literal, MokaTileCache, Mutation, PhysicalCollection,
    RasterSource, RasterWindow, Registry, RequestedCrs, Resolver, Result as CoreResult,
    Router as CoreRouter, Sequence, SpatialExtent, StaticResolver, StorageDecl, StorageDriver,
    StyleStore, TileCache, TileCoord, TileSource, VertexStats, WriteSink,
};

/// A `CatalogSource` that reports no collections — this file's tests
/// exercise handlers directly, not `Router::validate_catalog`, so this is
/// present only to satisfy the trait.
struct EmptyCatalog;

#[async_trait::async_trait]
impl CatalogSource for EmptyCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![])
    }
}

const DEMO_CONFIG: &str = r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#;

fn feature(id: &str) -> Value {
    json!({ "type": "Feature", "id": id, "geometry": null, "properties": {} })
}

struct FakeFeatureSource {
    // Sorted ascending by id — mirrors keyset ordering by primary key.
    items: Vec<(String, Value)>,
}

#[async_trait::async_trait]
impl FeatureSource for FakeFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        let start_index = match &query.token {
            Some(token) => self
                .items
                .iter()
                .position(|(id, _)| id == token)
                .map(|i| i + 1)
                .unwrap_or(0),
            None => 0,
        };
        let remaining = &self.items[start_index..];
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

struct FakeDriver {
    source: Arc<FakeFeatureSource>,
}

impl StorageDriver for FakeDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(EmptyCatalog)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(self.source.clone() as Arc<dyn FeatureSource>)
    }
}

struct FakeFactory {
    source: Arc<FakeFeatureSource>,
}

impl DriverFactory for FakeFactory {
    fn name(&self) -> &str {
        "fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(FakeDriver {
            source: self.source.clone(),
        }))
    }
}

fn build_ctx(items: Vec<(&str, Value)>) -> Arc<AppContext> {
    build_ctx_with_config(DEMO_CONFIG, items)
}

fn build_ctx_with_config(config_yaml: &str, items: Vec<(&str, Value)>) -> Arc<AppContext> {
    let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
    config.validate().unwrap();

    let source = Arc::new(FakeFeatureSource {
        items: items
            .into_iter()
            .map(|(id, v)| (id.to_string(), v))
            .collect(),
    });

    let mut registry = Registry::new();
    registry.register(Arc::new(FakeFactory { source }));

    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        None,
        cache,
        style_store,
    ))
}

fn build_app(items: Vec<(&str, Value)>) -> axum::Router {
    tellurion_features::router().with_state(build_ctx(items))
}

fn build_budgeted_app(items: Vec<(&str, Value)>) -> axum::Router {
    let config = format!("{DEMO_CONFIG}\nsettings: {{ items_vertex_budget: 1 }}\n");
    tellurion_features::router().with_state(build_ctx_with_config(&config, items))
}

/// Same fixtures under a platform-level `page_max_bytes` (`#184`) — the
/// byte-budget trims responses rather than refusing them, so this rides a
/// separate builder from `build_budgeted_app`'s refusal lane.
fn build_byte_budgeted_app(items: Vec<(&str, Value)>, page_max_bytes: u64) -> axum::Router {
    let config = format!("{DEMO_CONFIG}\nsettings: {{ page_max_bytes: {page_max_bytes} }}\n");
    tellurion_features::router().with_state(build_ctx_with_config(&config, items))
}

/// Same fixtures, mounted under a `/{tenant}` prefix by the "server" — proves
/// tenant resolution from the path works, not just the fixed default.
fn build_app_with_tenant_prefix(items: Vec<(&str, Value)>) -> axum::Router {
    axum::Router::new()
        .nest("/{tenant}", tellurion_features::router())
        .with_state(build_ctx(items))
}

fn build_app_with_public_base(items: Vec<(&str, Value)>) -> axum::Router {
    let config = format!(
        "{DEMO_CONFIG}\nserver: {{ public_base_url: 'https://maps.example.test/tellurion/' }}\n"
    );
    axum::Router::new()
        .nest(
            "/{tenant}/features/catalogs/{catalog}",
            tellurion_features::router(),
        )
        .with_state(build_ctx_with_config(&config, items))
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn find_link<'a>(body: &'a Value, rel: &str) -> Option<&'a Value> {
    body["links"].as_array()?.iter().find(|l| l["rel"] == rel)
}

async fn get(app: &axum::Router, uri: impl AsRef<str>) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(uri.as_ref())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn list_items_paginates_with_keyset_token_round_trip() {
    let app = build_app(vec![
        ("a", feature("a")),
        ("b", feature("b")),
        ("c", feature("c")),
    ]);

    let first = get(&app, "/collections/demo/items?limit=2").await;
    assert_eq!(first.status(), StatusCode::OK);
    let body = body_json(first).await;
    assert_eq!(body["numberReturned"], 2);
    assert_eq!(body["numberMatched"], 3);
    assert!(find_link(&body, "self").is_some());
    let next_href = find_link(&body, "next").expect("expected a next link")["href"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(next_href.contains("token=b"));

    let second = get(&app, &next_href).await;
    assert_eq!(second.status(), StatusCode::OK);
    let body2 = body_json(second).await;
    assert_eq!(body2["numberReturned"], 1);
    assert_eq!(body2["features"][0]["id"], "c");
    assert!(find_link(&body2, "next").is_none());
}

#[tokio::test]
async fn configured_public_base_makes_items_self_and_next_links_absolute() {
    let app = build_app_with_public_base(vec![("a", feature("a")), ("b", feature("b"))]);

    let response = get(
        &app,
        "/public/features/catalogs/default/collections/demo/items?limit=1",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    assert_eq!(
        find_link(&body, "self").unwrap()["href"],
        "https://maps.example.test/tellurion/public/features/catalogs/default/collections/demo/items?limit=1"
    );
    assert_eq!(
        find_link(&body, "next").unwrap()["href"],
        "https://maps.example.test/tellurion/public/features/catalogs/default/collections/demo/items?limit=1&token=a"
    );

    let collections = body_json(
        get(
            &app,
            "/public/features/catalogs/default/collections?limit=1",
        )
        .await,
    )
    .await;
    assert_eq!(
        find_link(&collections, "self").unwrap()["href"],
        "https://maps.example.test/tellurion/public/features/catalogs/default/collections?limit=1"
    );
    for link in collections["collections"][0]["links"].as_array().unwrap() {
        assert!(
            link["href"]
                .as_str()
                .unwrap()
                .starts_with("https://maps.example.test/tellurion/"),
            "server-generated collection link was not absolute: {link}"
        );
    }

    let collection =
        body_json(get(&app, "/public/features/catalogs/default/collections/demo").await).await;
    for link in collection["links"].as_array().unwrap() {
        assert!(
            link["href"]
                .as_str()
                .unwrap()
                .starts_with("https://maps.example.test/tellurion/"),
            "server-generated collection link was not absolute: {link}"
        );
    }

    let item = body_json(
        get(
            &app,
            "/public/features/catalogs/default/collections/demo/items/a",
        )
        .await,
    )
    .await;
    assert_eq!(
        find_link(&item, "self").unwrap()["href"],
        "https://maps.example.test/tellurion/public/features/catalogs/default/collections/demo/items/a"
    );
    assert_eq!(
        find_link(&item, "collection").unwrap()["href"],
        "https://maps.example.test/tellurion/public/features/catalogs/default/collections/demo"
    );
}

/// `#184`: an over-budget page is trimmed (never refused), the `next` link's
/// token is re-minted from the last KEPT feature so the dropped tail is
/// re-served on the next request, and `numberMatched` keeps meaning the
/// total match. The token round-trip is walked for real against the same
/// keyset-paging fake the plain paging test uses, proving no feature is
/// ever skipped or duplicated across the trim.
#[tokio::test]
async fn list_items_trims_to_page_max_bytes_and_next_resumes_after_the_last_kept_feature() {
    // Budget exactly one serialized feature: page one of a 3-row collection
    // keeps only "a" and must point `next` at "a", not at the driver's own
    // untrimmed token ("c" never even enters a page here — limit is 3).
    let budget = serde_json::to_vec(&feature("a")).unwrap().len() as u64;
    let app = build_byte_budgeted_app(
        vec![
            ("a", feature("a")),
            ("b", feature("b")),
            ("c", feature("c")),
        ],
        budget,
    );

    let first = get(&app, "/collections/demo/items?limit=3").await;
    assert_eq!(first.status(), StatusCode::OK);
    let body = body_json(first).await;
    assert_eq!(body["numberReturned"], 1);
    assert_eq!(
        body["numberMatched"], 3,
        "the total match count is untouched"
    );
    assert_eq!(body["features"][0]["id"], "a");
    let next_href = find_link(&body, "next").expect("expected a next link")["href"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(next_href.contains("token=a"), "{next_href}");

    let second = get(&app, &next_href).await;
    let body2 = body_json(second).await;
    assert_eq!(body2["numberReturned"], 1);
    assert_eq!(body2["features"][0]["id"], "b");
    let next_href2 = find_link(&body2, "next").expect("expected a next link")["href"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(next_href2.contains("token=b"), "{next_href2}");

    let third = get(&app, &next_href2).await;
    let body3 = body_json(third).await;
    assert_eq!(body3["numberReturned"], 1);
    assert_eq!(body3["features"][0]["id"], "c");
    assert!(find_link(&body3, "next").is_none());
}

/// `#184` off-switch: a config that never declares `page_max_bytes` serves
/// pages exactly as before — no trimming, no re-minted token.
#[tokio::test]
async fn list_items_without_page_max_bytes_is_untrimmed() {
    let app = build_app(vec![("a", feature("a")), ("b", feature("b"))]);
    let response = get(&app, "/collections/demo/items?limit=2").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["numberReturned"], 2);
    assert!(find_link(&body, "next").is_none());
}

#[tokio::test]
async fn exact_item_geometry_over_budget_is_a_named_422_with_no_partial_collection() {
    let large = json!({
        "type": "Feature",
        "id": "large",
        "geometry": {
            "type": "LineString",
            "coordinates": [[0, 0], [1, 1]]
        },
        "properties": {}
    });
    let app = build_budgeted_app(vec![("large", large)]);

    for uri in ["/collections/demo/items", "/collections/demo/items/large"] {
        let response = get(&app, uri).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            tellurion_features::PROBLEM_JSON
        );
        let body = body_json(response).await;
        assert_eq!(body["code"], "ItemsVertexBudgetExceeded");
        assert!(
            body.get("features").is_none(),
            "a refusal must never carry a partial FeatureCollection"
        );
    }
}

#[tokio::test]
async fn bbox_malformed_returns_400_problem_json() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items?bbox=1,2,3").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        tellurion_features::PROBLEM_JSON
    );
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
}

#[tokio::test]
async fn bbox_well_formed_is_accepted() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items?bbox=1,2,3,4").await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn datetime_open_interval_is_accepted() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(
        &app,
        "/collections/demo/items?datetime=2020-01-01T00:00:00Z/..",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn datetime_malformed_returns_400() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items?datetime=a/b/c").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn datetime_syntactically_invalid_instant_returns_400() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items?datetime=notadate").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn feature_collection_content_type_is_geojson() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/geo+json"
    );
}

#[tokio::test]
async fn single_item_has_self_and_collection_links() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items/a").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/geo+json"
    );
    let body = body_json(response).await;
    assert_eq!(
        find_link(&body, "self").unwrap()["href"],
        "/collections/demo/items/a"
    );
    assert_eq!(
        find_link(&body, "collection").unwrap()["href"],
        "/collections/demo"
    );
}

// -- Optimistic Locking: ETags (`#107`, `req/optimistic-locking-etags`) -----

/// A single-feature `GET` always carries a strong `ETag` — no per-collection
/// declaration needed (`tellurion_core::locking`'s own module doc: the hash
/// itself needs nothing beyond a resolving `FeatureSource`) — computed the
/// same way `tellurion_core::locking::compute_feature_etag` documents.
#[tokio::test]
async fn single_item_get_carries_an_etag() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items/a").await;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response
        .headers()
        .get(header::ETAG)
        .expect("ETag header present")
        .to_str()
        .unwrap()
        .to_string();
    assert!(etag.starts_with('"') && etag.ends_with('"'), "got: {etag}");
    assert_eq!(etag, locking::compute_feature_etag(&feature("a")));
}

/// Fetching the SAME item twice yields the SAME ETag — a strong validator
/// must be stable when nothing was written in between.
#[tokio::test]
async fn single_item_get_etag_is_stable_across_repeated_reads() {
    let app = build_app(vec![("a", feature("a"))]);
    let first = get(&app, "/collections/demo/items/a").await;
    let first_etag = first.headers().get(header::ETAG).unwrap().clone();
    let second = get(&app, "/collections/demo/items/a").await;
    let second_etag = second.headers().get(header::ETAG).unwrap().clone();
    assert_eq!(first_etag, second_etag);
}

/// Two different items never share an ETag — the hash is content-derived,
/// not, say, a collection-wide constant.
#[tokio::test]
async fn single_item_get_etag_differs_across_distinct_items() {
    let app = build_app(vec![("a", feature("a")), ("b", feature("b"))]);
    let a = get(&app, "/collections/demo/items/a").await;
    let a_etag = a.headers().get(header::ETAG).unwrap().clone();
    let b = get(&app, "/collections/demo/items/b").await;
    let b_etag = b.headers().get(header::ETAG).unwrap().clone();
    assert_ne!(a_etag, b_etag);
}

/// `DEMO_CONFIG` declares no `modified_column` at all — `Last-Modified`
/// must never appear on this collection's responses (never fabricated).
#[tokio::test]
async fn single_item_get_carries_no_last_modified_without_a_declared_modified_column() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items/a").await;
    assert!(response.headers().get(header::LAST_MODIFIED).is_none());
}

#[tokio::test]
async fn unknown_item_returns_404() {
    let app = build_app(vec![]);
    let response = get(&app, "/collections/demo/items/missing").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_collection_returns_404() {
    let app = build_app(vec![]);
    let response = get(&app, "/collections/nope/items").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_collections_includes_the_demo_collection_with_items_link() {
    let app = build_app(vec![]);
    let response = get(&app, "/collections").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let collections = body["collections"].as_array().unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0]["id"], "demo");
    assert!(find_link(&collections[0], "items").is_some());
}

// -- /collections cursor paging (`#42`) --------------------------------

const MULTI_COLLECTION_CONFIG: &str = r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: alpha
    catalog: default
    storage: main
    table: alpha
    geometry: geom
    pk: id
  - id: bravo
    catalog: default
    storage: main
    table: bravo
    geometry: geom
    pk: id
  - id: charlie
    catalog: default
    storage: main
    table: charlie
    geometry: geom
    pk: id
"#;

fn build_multi_collection_app() -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(MULTI_COLLECTION_CONFIG).unwrap();
    config.validate().unwrap();

    let source = Arc::new(FakeFeatureSource { items: vec![] });
    let mut registry = Registry::new();
    registry.register(Arc::new(FakeFactory { source }));

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
    tellurion_features::router().with_state(ctx)
}

/// A small registry (fewer collections than the default page size) still
/// gets everything back on the one, only page — no `next` link, the same
/// response shape `/collections` has always had.
#[tokio::test]
async fn list_collections_default_limit_returns_everything_on_one_page() {
    let app = build_multi_collection_app();
    let response = get(&app, "/collections").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let collections = body["collections"].as_array().unwrap();
    assert_eq!(collections.len(), 3);
    assert!(
        find_link(&body, "next").is_none(),
        "a registry smaller than the default page size must not paginate"
    );
}

/// The paging round trip itself: `limit=2` over three collections returns
/// the first two (in stable, external-id order) plus a `next` link; walking
/// that link returns the remaining one collection and no further `next`.
#[tokio::test]
async fn list_collections_paginates_with_a_limit_and_a_next_link() {
    let app = build_multi_collection_app();

    let first = get(&app, "/collections?limit=2").await;
    assert_eq!(first.status(), StatusCode::OK);
    let first_body = body_json(first).await;
    let first_ids: Vec<String> = first_body["collections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(first_ids, vec!["alpha", "bravo"]);
    let next_href = find_link(&first_body, "next")
        .expect("a next link when more collections remain")["href"]
        .as_str()
        .unwrap()
        .to_string();

    let second = get(&app, &next_href).await;
    assert_eq!(second.status(), StatusCode::OK);
    let second_body = body_json(second).await;
    let second_ids: Vec<String> = second_body["collections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(second_ids, vec!["charlie"]);
    assert!(
        find_link(&second_body, "next").is_none(),
        "the last page must have no next link"
    );
}

#[tokio::test]
async fn list_collections_rejects_a_zero_limit() {
    let app = build_multi_collection_app();
    let response = get(&app, "/collections?limit=0").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// A `CatalogSource` reporting one real physical collection with a real
/// extent but no geometry column or primary key — the shape a tiles-only
/// archive driver (PMTiles, `#20`) reports.
struct TilesOnlyCatalog;

#[async_trait::async_trait]
impl CatalogSource for TilesOnlyCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: "tiles-only".to_string(),
            geometry_column: None,
            primary_key: None,
            srid: Some(3857),
            geometry_type: None,
        }])
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        Ok(Some(SpatialExtent {
            bbox: [-5.0, 45.0, 5.0, 55.0],
        }))
    }
}

struct TilesOnlySource;

#[async_trait::async_trait]
impl TileSource for TilesOnlySource {
    async fn mvt_tile(
        &self,
        _collection: &CollectionDecl,
        _coord: TileCoord,
        _filter: Option<&Filter>,
    ) -> CoreResult<Option<bytes::Bytes>> {
        Ok(None)
    }
}

/// A driver that implements `CatalogSource` + `TileSource` only — never
/// `FeatureSource` — matching PMTiles's read-only, tiles-only shape.
struct TilesOnlyDriver;

impl StorageDriver for TilesOnlyDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(TilesOnlyCatalog)
    }

    fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
        Some(Arc::new(TilesOnlySource) as Arc<dyn TileSource>)
    }
}

struct TilesOnlyFactory;

impl DriverFactory for TilesOnlyFactory {
    fn name(&self) -> &str {
        "tiles-only-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(TilesOnlyDriver))
    }
}

fn build_tiles_only_app() -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(
        r#"
storages: [ { id: main, driver: tiles-only-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: tiles-only
    catalog: default
    storage: main
"#,
    )
    .unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(TilesOnlyFactory));
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
    tellurion_features::router().with_state(ctx)
}

/// The `#20` proof at the HTTP-handler level: a collection with no
/// `FeatureSource` at all (only `CatalogSource` + `TileSource`, the PMTiles
/// shape) still appears in `/collections` with a real derived extent, and
/// has no `items` link since there is nothing at that route to serve.
#[tokio::test]
async fn list_collections_includes_a_tiles_only_collection_with_its_extent_and_no_items_link() {
    let app = build_tiles_only_app();
    let response = get(&app, "/collections").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let collections = body["collections"].as_array().unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0]["id"], "tiles-only");
    assert!(
        find_link(&collections[0], "items").is_none(),
        "a collection with no FeatureSource must not advertise an items link"
    );
    assert_eq!(
        collections[0]["extent"]["spatial"]["bbox"][0],
        json!([-5.0, 45.0, 5.0, 55.0]),
        "the extent must come through even though this driver never implements FeatureSource"
    );
}

/// `GET /collections/{cid}` (the singular resource) must resolve the same
/// tiles-only collection too, not just the list endpoint.
#[tokio::test]
async fn get_collection_resolves_a_tiles_only_collection() {
    let app = build_tiles_only_app();
    let response = get(&app, "/collections/tiles-only").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["id"], "tiles-only");
    assert!(find_link(&body, "items").is_none());
}

/// A tiles-only collection has no `FeatureSource`, so `/items` on it must
/// still refuse the request the same way an unrouted capability always has —
/// listing the collection elsewhere must not paper over that.
#[tokio::test]
async fn tiles_only_collection_still_refuses_items_requests() {
    let app = build_tiles_only_app();
    let response = get(&app, "/collections/tiles-only/items").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// -- capability-derived advertisement (`#287`) -------------------------------

/// A `CatalogSource` reporting the raster collection's physical facts — a
/// name and a real extent, no geometry column and no primary key, the shape
/// a COG's own tags produce (`tellurion-cog`'s `CatalogSource`).
struct RasterCatalog;

#[async_trait::async_trait]
impl CatalogSource for RasterCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: "gradient".to_string(),
            geometry_column: None,
            primary_key: None,
            srid: None,
            geometry_type: None,
        }])
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        Ok(Some(SpatialExtent {
            bbox: [-1.28, -1.28, 1.28, 1.28],
        }))
    }
}

struct RasterOnlySource;

#[async_trait::async_trait]
impl RasterSource for RasterOnlySource {
    async fn raster_tile(
        &self,
        _collection: &CollectionDecl,
        _coord: TileCoord,
    ) -> CoreResult<Option<RasterWindow>> {
        Ok(None)
    }
}

/// The COG/Zarr shape (`#37`): a `RasterSource` and nothing else — no
/// `FeatureSource`, no vector `TileSource`.
struct RasterOnlyDriver;

impl StorageDriver for RasterOnlyDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(RasterCatalog)
    }

    fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
        Some(Arc::new(RasterOnlySource) as Arc<dyn RasterSource>)
    }
}

struct RasterOnlyFactory;

impl DriverFactory for RasterOnlyFactory {
    fn name(&self) -> &str {
        "raster-only-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(RasterOnlyDriver))
    }
}

fn build_raster_only_app() -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(
        r#"
storages: [ { id: main, driver: raster-only-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: gradient
    catalog: default
    storage: main
"#,
    )
    .unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(RasterOnlyFactory));
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
    tellurion_features::router().with_state(ctx)
}

/// `#287`, the subtractive half: a raster-only collection's `/collections`
/// entry advertises NONE of the vector-capability members it cannot honour —
/// no `itemType`, no `crs`/`storageCrs`, no `cql2ConformanceClasses`/
/// `lockingConformanceClasses`, no `queryables` link, no `items` link. Each
/// absence is asserted by member name, so a mutation that re-derives any one
/// of them unconditionally fails this test naming the exact
/// over-advertisement rather than merely changing a count. (The
/// `tilesets-vector` sibling link is the one member this file cannot see —
/// `sibling_href` has no `catalogs` segment to anchor on under this crate's
/// bare mount — so its gate is proven at the real server mount in
/// `tellurion-server::app`'s own `#287` test instead.)
#[tokio::test]
async fn a_raster_only_collection_advertises_no_vector_capability_members() {
    let app = build_raster_only_app();
    let response = get(&app, "/collections").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let collections = body["collections"].as_array().unwrap();
    assert_eq!(
        collections.len(),
        1,
        "the raster collection must still list"
    );
    let entry = &collections[0];
    assert_eq!(entry["id"], "gradient");

    let members = entry.as_object().unwrap();
    for absent in [
        "itemType",
        "storageCrs",
        "crs",
        "cql2ConformanceClasses",
        "lockingConformanceClasses",
    ] {
        assert!(
            !members.contains_key(absent),
            "a raster-only collection has no FeatureSource, so its document \
             must not carry `{absent}` at all — not empty, not null: absent"
        );
    }
    assert!(
        find_link(entry, "http://www.opengis.net/def/rel/ogc/1.0/queryables").is_none(),
        "a raster-only collection must not advertise a queryables link"
    );
    assert!(
        find_link(entry, "items").is_none(),
        "a raster-only collection must not advertise an items link"
    );

    // What its driver CAN do is still advertised: the collection itself,
    // with its real physical extent — a physical fact, not a feature
    // capability, exactly as before `#287`.
    assert_eq!(
        entry["extent"]["spatial"]["bbox"][0],
        json!([-1.28, -1.28, 1.28, 1.28])
    );
}

/// `#287`'s invariance half: a vector (features-capable) collection's
/// document is BYTE-FOR-BYTE what the pre-`#287` tree (`366bac3`) served for
/// this same fixture — the change is subtractive for collections with no
/// `FeatureSource` and invisible for everything else. The literal below was
/// captured by running this exact fixture against the unmodified tree. A
/// mutation that over-subtracts (gating any of these members on a stricter
/// signal than "the features lane resolves") fails this equality; so does
/// any accidental reorder, rename, or null-for-absent substitution.
#[tokio::test]
async fn a_vector_collections_document_is_byte_for_byte_unchanged() {
    let app = build_app(vec![]);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = std::str::from_utf8(&bytes).unwrap();
    assert_eq!(
        body,
        r#"{"id":"demo","title":"demo","itemType":"feature","extent":null,"storageCrs":null,"crs":["http://www.opengis.net/def/crs/OGC/1.3/CRS84"],"cql2ConformanceClasses":[],"lockingConformanceClasses":[],"links":[{"href":"/collections/demo","rel":"self","type":"application/json"},{"href":"/collections/demo/queryables","rel":"http://www.opengis.net/def/rel/ogc/1.0/queryables","type":"application/schema+json"},{"href":"/collections/demo/items","rel":"items","type":"application/geo+json"}]}"#
    );
}

/// The listing lane serves the same unchanged bytes per entry — proved
/// separately because `list_collections` resolves capabilities through
/// `resolved_canonical` (the `#50` merge) while `get_collection` resolves
/// its own lanes live; `#287` gates both paths and neither may move a byte
/// for a features-capable collection.
#[tokio::test]
async fn a_vector_collections_listing_is_byte_for_byte_unchanged() {
    let app = build_app(vec![]);
    let response = get(&app, "/collections").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = std::str::from_utf8(&bytes).unwrap();
    assert_eq!(
        body,
        r#"{"links":[{"href":"/collections","rel":"self","type":"application/json"}],"collections":[{"id":"demo","title":"demo","itemType":"feature","extent":null,"storageCrs":null,"crs":["http://www.opengis.net/def/crs/OGC/1.3/CRS84"],"cql2ConformanceClasses":[],"lockingConformanceClasses":[],"links":[{"href":"/collections/demo","rel":"self","type":"application/json"},{"href":"/collections/demo/queryables","rel":"http://www.opengis.net/def/rel/ogc/1.0/queryables","type":"application/schema+json"},{"href":"/collections/demo/items","rel":"items","type":"application/geo+json"}]}]}"#
    );
}

// -- lazy-mode lane-capability checks (`#59`) ----------------------------

/// A collection whose `features` lane is explicitly routed to two storages,
/// the second of which never implements `FeatureSource` at all — the
/// misconfiguration `Router::validate_catalog`'s eager boot sweep already
/// catches (`validate_catalog_fails_fast_when_an_explicit_routing_lane_
/// names_a_storage_lacking_the_capability` in `tellurion-core`). None of
/// this crate's handler tests ever call `validate_catalog` (see this file's
/// own doc comment), so every request here already runs under exactly the
/// conditions `registry.validation: lazy` leaves a first request in.
fn build_broken_multi_entry_lane_app() -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(
        r#"
storages:
  - { id: good, driver: fake, url_env: DATABASE_URL }
  - { id: bad, driver: tiles-only-fake, url_env: DATABASE_URL2 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: good
    table: demo
    geometry: geom
    pk: id
    routing: { features: [good, bad] }
"#,
    )
    .unwrap();
    config.validate().unwrap();

    let source = Arc::new(FakeFeatureSource { items: vec![] });
    let mut registry = Registry::new();
    registry.register(Arc::new(FakeFactory { source }));
    registry.register(Arc::new(TilesOnlyFactory));

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
    tellurion_features::router().with_state(ctx)
}

/// `#59`: before the lazy first-touch capability check existed,
/// `good`/`bad`'s fallback chain would have silently dropped `bad` (it
/// implements no `FeatureSource` at all) and served this request normally
/// through `good` alone — the misconfigured `routing:` declaration would
/// never have surfaced outside an eager `validate_catalog` boot sweep. Now
/// it fails at this first touch with the same `Error::Config` an eager boot
/// would have raised, which `ApiError::from` maps to 500
/// `InternalServerError` — contrast `tiles_only_collection_still_refuses_
/// items_requests` just above: an *unrouted* lane's single storage lacking
/// a capability is still a 404 `NotFound` (`CapabilityUnsupported`,
/// unchanged), because that's an ordinary "this collection doesn't do X"
/// refusal, not a misconfigured explicit routing declaration.
#[tokio::test]
async fn an_explicit_multi_entry_lane_with_an_incapable_entry_fails_first_touch_as_a_500_not_silently(
) {
    let app = build_broken_multi_entry_lane_app();
    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InternalServerError");
}

#[tokio::test]
async fn tenant_defaults_to_public_when_not_nested_under_a_path_segment() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn tenant_is_read_from_the_nesting_path_when_present() {
    let app = build_app_with_tenant_prefix(vec![("a", feature("a"))]);

    let matching_tenant = get(&app, "/public/collections/demo/items").await;
    assert_eq!(matching_tenant.status(), StatusCode::OK);

    let wrong_tenant = get(&app, "/other-tenant/collections/demo/items").await;
    assert_eq!(wrong_tenant.status(), StatusCode::NOT_FOUND);
}

// -- filter / filter-lang (`#33`) --------------------------------------------

#[tokio::test]
async fn filter_against_a_driver_without_the_capability_returns_400() {
    // `FakeFeatureSource` (via `build_app`) never overrides `filter_capable`,
    // so it defaults to `false` — the same shape FlatGeobuf has in
    // production. A `filter` request against it must be refused before
    // `items` is ever called, not silently ignored.
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items?filter=name%20%3D%20'a'").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    assert!(
        body["detail"].as_str().unwrap().contains("filter"),
        "detail was: {}",
        body["detail"]
    );
}

/// `#33` follow-up: the new advanced-comparison/spatial operators ride the
/// exact same `source.filter_capable()` gate every other filter shape
/// already does — no per-operator capability check to forget. A `LIKE`
/// filter is enough to prove that; `filter_against_a_driver_without_the_
/// capability_returns_400` already covers the baseline `=` case.
#[tokio::test]
async fn a_like_filter_against_a_driver_without_the_capability_returns_400() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items?filter=name%20LIKE%20'a%25'").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
}

fn filterable_feature(id: &str, name: &str) -> Value {
    json!({ "type": "Feature", "id": id, "geometry": null, "properties": { "name": name } })
}

/// A `CatalogSource` matching `FILTERABLE_CONFIG`'s physical shape (table
/// "demo", geometry "geom", pk "id"), plus a small attribute schema
/// (`name`) so filter-property validation (`#33`) has a real, known column
/// to check requests against.
struct FilterableCatalog;

#[async_trait::async_trait]
impl CatalogSource for FilterableCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: "demo".to_string(),
            geometry_column: Some("geom".to_string()),
            primary_key: Some("id".to_string()),
            srid: Some(4326),
            geometry_type: None,
        }])
    }

    async fn attribute_schema(
        &self,
        _physical: &PhysicalCollection,
    ) -> CoreResult<Option<Vec<AttributeColumn>>> {
        Ok(Some(vec![AttributeColumn {
            name: "name".to_string(),
            sql_type: "text".to_string(),
        }]))
    }
}

/// A tiny in-memory filter evaluator covering exactly the one predicate
/// shape these tests exercise (`name = '...'`) — enough to prove a filter
/// actually reaches `items` and narrows the result set end to end, without
/// reimplementing `tellurion-postgis`'s SQL compiler here.
fn matches_filter(feature: &Value, filter: Option<&Filter>) -> bool {
    match filter {
        None => true,
        Some(Filter::Compare {
            property,
            op: CompareOp::Eq,
            value: Literal::Text(expected),
        }) => feature["properties"][property].as_str() == Some(expected.as_str()),
        Some(_) => panic!("matches_filter: unexpected filter shape in this test fixture"),
    }
}

/// A `FeatureSource` that advertises `filter_capable() == true` and actually
/// evaluates the (narrow) filter shape `matches_filter` understands — the
/// PostGIS-shaped end of the capability spectrum, as opposed to
/// `FakeFeatureSource`'s FlatGeobuf-shaped default.
struct FilterCapableFeatureSource {
    items: Vec<(String, Value)>,
}

#[async_trait::async_trait]
impl FeatureSource for FilterCapableFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        let matched: Vec<Value> = self
            .items
            .iter()
            .filter(|(_, v)| matches_filter(v, query.filter.as_ref()))
            .map(|(_, v)| v.clone())
            .collect();
        Ok(FeaturePage {
            number_matched: Some(matched.len() as u64),
            features_geojson: matched,
            next_token: None,
        })
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
    ) -> CoreResult<Option<Value>> {
        Ok(self
            .items
            .iter()
            .find(|(item_id, v)| item_id == id && matches_filter(v, filter))
            .map(|(_, v)| v.clone()))
    }

    fn filter_capable(&self) -> bool {
        true
    }
}

struct FilterableDriver {
    source: Arc<FilterCapableFeatureSource>,
}

impl StorageDriver for FilterableDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(FilterableCatalog)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(self.source.clone() as Arc<dyn FeatureSource>)
    }
}

struct FilterableFactory {
    source: Arc<FilterCapableFeatureSource>,
}

impl DriverFactory for FilterableFactory {
    fn name(&self) -> &str {
        "filterable-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(FilterableDriver {
            source: self.source.clone(),
        }))
    }
}

const FILTERABLE_CONFIG: &str = r#"
storages: [ { id: main, driver: filterable-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#;

fn build_filterable_app(items: Vec<(&str, Value)>) -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(FILTERABLE_CONFIG).unwrap();
    config.validate().unwrap();

    let source = Arc::new(FilterCapableFeatureSource {
        items: items
            .into_iter()
            .map(|(id, v)| (id.to_string(), v))
            .collect(),
    });

    let mut registry = Registry::new();
    registry.register(Arc::new(FilterableFactory { source }));

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
    tellurion_features::router().with_state(ctx)
}

#[tokio::test]
async fn filter_with_an_unknown_property_returns_400_naming_it() {
    let app = build_filterable_app(vec![("a", filterable_feature("a", "a"))]);
    let response = get(&app, "/collections/demo/items?filter=bogus%20%3D%20'a'").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    assert!(
        body["detail"].as_str().unwrap().contains("bogus"),
        "detail was: {}",
        body["detail"]
    );
}

#[tokio::test]
async fn a_syntactically_invalid_filter_returns_400() {
    let app = build_filterable_app(vec![("a", filterable_feature("a", "a"))]);
    let response = get(&app, "/collections/demo/items?filter=name%20%3D").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn filter_narrows_the_result_set_end_to_end() {
    let app = build_filterable_app(vec![
        ("a", filterable_feature("a", "alpha")),
        ("b", filterable_feature("b", "beta")),
    ]);
    let response = get(&app, "/collections/demo/items?filter=name%20%3D%20'alpha'").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["numberReturned"], 1);
    assert_eq!(body["features"][0]["id"], "a");
}

#[tokio::test]
async fn filter_lang_cql2_json_is_accepted_end_to_end() {
    let app = build_filterable_app(vec![
        ("a", filterable_feature("a", "alpha")),
        ("b", filterable_feature("b", "beta")),
    ]);
    let response = get(
        &app,
        "/collections/demo/items?filter-lang=cql2-json&filter=%7B%22op%22%3A%22%3D%22%2C%22args%22%3A%5B%7B%22property%22%3A%22name%22%7D%2C%22alpha%22%5D%7D",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["numberReturned"], 1);
    assert_eq!(body["features"][0]["id"], "a");
}

#[tokio::test]
async fn no_filter_parameter_behaves_exactly_as_before_filter_existed() {
    let app = build_filterable_app(vec![
        ("a", filterable_feature("a", "alpha")),
        ("b", filterable_feature("b", "beta")),
    ]);
    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["numberReturned"], 2);
}

// -- crs / bbox-crs (OGC API Features Part 2 CRS by Reference) --------------

/// A `CatalogSource` reporting a configurable storage srid — paired with
/// `CRS_CONFIG` below, which omits `table`/`geometry`/`pk` so
/// `Router::effective_decl` actually derives the descriptor (and carries its
/// `srid` onto the served decl) instead of taking the fully-overridden fast
/// path `FILTERABLE_CONFIG` uses, which never derives anything.
///
/// `4326` is what every test predating `#247` gets, and the shape of every
/// live demo. `#247` needs the other side too: a projected storage is the only
/// place where reading a filter's spatial literals as CRS84 costs a driver a
/// real coordinate transform, and therefore the only place a driver that
/// cannot transform has anything to refuse.
struct CrsCatalog {
    srid: i32,
}

#[async_trait::async_trait]
impl CatalogSource for CrsCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: "demo".to_string(),
            geometry_column: Some("geom".to_string()),
            primary_key: Some("id".to_string()),
            srid: Some(self.srid),
            geometry_type: None,
        }])
    }
}

/// A `FeatureSource` whose reprojection capability is configurable —
/// `crs_capable: false` is the FlatGeobuf/GeoParquet shape (trait default),
/// used to prove a genuinely valid, non-default `crs`/`bbox-crs` (this
/// collection's own storage CRS, not just an unrecognized one) still refuses
/// against a driver that can't reproject; `crs_capable: true` is the PostGIS
/// shape, used to prove the same request succeeds once the driver actually
/// can.
struct CrsFeatureSource {
    crs_capable: bool,
    /// `#217`: whether this source can honour a `filter-crs`. Declared
    /// separately from `crs_capable` above for the same reason the trait
    /// declares them separately — the two are genuinely independent, and the
    /// `(true, false)` pair is the state PostGIS was actually in.
    filter_crs_capable: bool,
    /// The `ItemsQuery::filter_crs` the handler last handed this source.
    /// Recorded rather than asserted through a response body: the point of
    /// `filter-crs` is what the *driver* is told, and a fake that evaluated
    /// nothing could still return a plausible-looking page.
    seen_filter_crs: Arc<Mutex<Vec<RequestedCrs>>>,
}

#[async_trait::async_trait]
impl FeatureSource for CrsFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        self.seen_filter_crs.lock().unwrap().push(query.filter_crs);
        Ok(FeaturePage {
            features_geojson: vec![feature("a")],
            number_matched: Some(1),
            next_token: None,
        })
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        id: &str,
        _filter: Option<&Filter>,
    ) -> CoreResult<Option<Value>> {
        Ok((id == "a").then(|| feature("a")))
    }

    fn crs_capable(&self) -> bool {
        self.crs_capable
    }

    /// Always `true` so a `filter` reaches `items` at all — the `filter-crs`
    /// gate under test sits behind the `filter_capable` one, and a fixture
    /// that refused every filter could never reach it.
    fn filter_capable(&self) -> bool {
        true
    }

    fn filter_crs_capable(&self) -> bool {
        self.filter_crs_capable
    }
}

struct CrsDriver {
    srid: i32,
    crs_capable: bool,
    filter_crs_capable: bool,
    seen_filter_crs: Arc<Mutex<Vec<RequestedCrs>>>,
}

impl StorageDriver for CrsDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(CrsCatalog { srid: self.srid })
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::new(CrsFeatureSource {
            crs_capable: self.crs_capable,
            filter_crs_capable: self.filter_crs_capable,
            seen_filter_crs: Arc::clone(&self.seen_filter_crs),
        }))
    }
}

struct CrsFactory {
    srid: i32,
    crs_capable: bool,
    filter_crs_capable: bool,
    seen_filter_crs: Arc<Mutex<Vec<RequestedCrs>>>,
}

impl DriverFactory for CrsFactory {
    fn name(&self) -> &str {
        "crs-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(CrsDriver {
            srid: self.srid,
            crs_capable: self.crs_capable,
            filter_crs_capable: self.filter_crs_capable,
            seen_filter_crs: Arc::clone(&self.seen_filter_crs),
        }))
    }
}

/// `table`/`geometry`/`pk` are all omitted deliberately — see `CrsCatalog`'s
/// own doc comment for why that matters here.
const CRS_CONFIG: &str = r#"
storages: [ { id: main, driver: crs-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
"#;

fn build_crs_app(crs_capable: bool) -> axum::Router {
    // Pre-`#217` PostGIS in one line: reprojection capability as the caller
    // asked for it, `filter-crs` never honoured. Every test that predates
    // `#217` goes through here unchanged.
    build_crs_app_with(crs_capable, false).0
}

/// [`build_crs_app`] with `filter_crs_capable` under the test's control
/// (`#217`), plus the log of every `ItemsQuery::filter_crs` the driver was
/// actually handed.
fn build_crs_app_with(
    crs_capable: bool,
    filter_crs_capable: bool,
) -> (axum::Router, Arc<Mutex<Vec<RequestedCrs>>>) {
    build_crs_app_at_srid(4326, crs_capable, filter_crs_capable)
}

/// [`build_crs_app_with`] over a collection whose storage srid the test picks
/// (`#247`). Only `4326` and a projected value are meaningful: the whole of
/// what the srid decides here is
/// `tellurion_core::crs::crs84_literals_need_transform`.
fn build_crs_app_at_srid(
    srid: i32,
    crs_capable: bool,
    filter_crs_capable: bool,
) -> (axum::Router, Arc<Mutex<Vec<RequestedCrs>>>) {
    let config: AppConfig = serde_yaml::from_str(CRS_CONFIG).unwrap();
    config.validate().unwrap();

    let seen_filter_crs = Arc::new(Mutex::new(Vec::new()));
    let mut registry = Registry::new();
    registry.register(Arc::new(CrsFactory {
        srid,
        crs_capable,
        filter_crs_capable,
        seen_filter_crs: Arc::clone(&seen_filter_crs),
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
    (
        tellurion_features::router().with_state(ctx),
        seen_filter_crs,
    )
}

#[tokio::test]
async fn items_with_no_crs_parameter_still_carries_a_content_crs_header() {
    let app = build_crs_app(false);
    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-crs").unwrap(),
        "<http://www.opengis.net/def/crs/OGC/1.3/CRS84>"
    );
}

#[tokio::test]
async fn items_with_explicit_crs84_is_accepted_even_by_a_non_reprojecting_driver() {
    // CRS84 is always a no-op request (every driver already serves CRS84 by
    // default), so it never needs `crs_capable`.
    let app = build_crs_app(false);
    let response = get(
        &app,
        "/collections/demo/items?crs=http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FOGC%2F1.3%2FCRS84",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-crs").unwrap(),
        "<http://www.opengis.net/def/crs/OGC/1.3/CRS84>"
    );
}

#[tokio::test]
async fn items_with_the_storage_crs_against_a_non_capable_driver_returns_400() {
    let app = build_crs_app(false);
    let response = get(
        &app,
        "/collections/demo/items?crs=http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F4326",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    assert!(
        body["detail"].as_str().unwrap().contains("crs"),
        "detail was: {}",
        body["detail"]
    );
}

#[tokio::test]
async fn items_with_an_unsupported_crs_returns_400() {
    let app = build_crs_app(false);
    let response = get(
        &app,
        "/collections/demo/items?crs=http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F3857",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn items_with_bbox_crs_storage_against_a_non_capable_driver_returns_400() {
    let app = build_crs_app(false);
    let response = get(
        &app,
        "/collections/demo/items?bbox=1,2,3,4&bbox-crs=http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F4326",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn single_item_carries_a_content_crs_header_and_rejects_a_non_default_crs() {
    let app = build_crs_app(false);
    let default_crs = get(&app, "/collections/demo/items/a").await;
    assert_eq!(default_crs.status(), StatusCode::OK);
    assert_eq!(
        default_crs.headers().get("content-crs").unwrap(),
        "<http://www.opengis.net/def/crs/OGC/1.3/CRS84>"
    );

    let non_default = get(
        &app,
        "/collections/demo/items/a?crs=http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F4326",
    )
    .await;
    assert_eq!(non_default.status(), StatusCode::BAD_REQUEST);
}

/// A **4326** collection on a driver that can't reproject advertises CRS84
/// alone in `crs` — the list clients are told they may *request* — or a
/// client following `/req/crs/fc-md-crs-list` straight to
/// `?crs=<storage crs>` gets a 400 from the exact enforcement gate the
/// request-level
/// `items_with_the_storage_crs_against_a_non_capable_driver_returns_400`
/// test above already covers.
///
/// `storageCrs` then has to disappear with it (`#217`): Requirement 4 says
/// its value SHALL be one of the identifiers found in `crs`, so a
/// collection naming its EPSG:4326 storage there while advertising CRS84
/// alone contradicts its own metadata — the two URIs are different strings,
/// and axis-swapped from each other. Omitting the member is the same "never
/// fabricated" behaviour `extent` already has when the fact behind it is
/// unavailable.
///
/// `#227` narrowed the *reason* without moving this case: what such a
/// collection advertises is the one CRS it can actually be served in, and
/// for a 4326 storage that is still CRS84. `a_projected_collection_
/// advertises_the_crs_it_actually_serves` below is the same rule applied to
/// the storage where the answer differs.
#[tokio::test]
async fn collection_metadata_crs_list_is_crs84_only_for_a_4326_non_reprojecting_collection() {
    let app = build_crs_app(false);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["storageCrs"],
        Value::Null,
        "a non-reprojecting driver must not name a storageCrs its own crs list omits"
    );
    let crs_list = body["crs"].as_array().unwrap();
    assert_eq!(
        crs_list,
        &vec![Value::String(
            "http://www.opengis.net/def/crs/OGC/1.3/CRS84".to_string()
        )],
        "a non-reprojecting driver must advertise CRS84 alone, not the storage crs it cannot serve"
    );
}

/// OGC API Features Part 2 Requirement 4 as an invariant over both driver
/// capabilities, driven through real HTTP rather than asserted against
/// hardcoded expectations — the `storageCrs` counterpart of
/// `every_advertised_crs_is_accepted_by_the_items_endpoint` below: whatever a
/// collection advertises as `storageCrs`, its own `crs` list must contain
/// that exact identifier.
#[tokio::test]
async fn every_advertised_storage_crs_is_in_the_collections_own_crs_list() {
    for crs_capable in [false, true] {
        let app = build_crs_app(crs_capable);
        let response = get(&app, "/collections/demo").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        let Some(storage_crs) = body["storageCrs"].as_str() else {
            continue;
        };
        let crs_list = body["crs"].as_array().unwrap();
        assert!(
            crs_list.iter().any(|c| c == storage_crs),
            "storageCrs '{storage_crs}' (crs_capable={crs_capable}) is outside this \
             collection's own crs list {crs_list:?}"
        );
    }
}

#[tokio::test]
async fn collection_metadata_crs_list_includes_the_storage_crs_when_the_driver_can_reproject() {
    let app = build_crs_app(true);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["storageCrs"],
        "http://www.opengis.net/def/crs/EPSG/0/4326"
    );
    let crs_list = body["crs"].as_array().unwrap();
    assert!(crs_list
        .iter()
        .any(|c| c == "http://www.opengis.net/def/crs/OGC/1.3/CRS84"));
    assert!(crs_list
        .iter()
        .any(|c| c == "http://www.opengis.net/def/crs/EPSG/0/4326"));
}

/// Percent-encodes exactly the characters these fixed CRS URIs ever contain
/// (`:` and `/`) — enough for a test query string, not a general encoder.
fn percent_encode_crs_uri(uri: &str) -> String {
    uri.chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The invariant this lane's `crs` metadata exists to uphold, proved by
/// driving both ends through real HTTP rather than asserting two hardcoded
/// lists against each other: whatever a collection's `GET /collections/{cid}`
/// response advertises in `crs`, requesting that exact value back against
/// `/items` must succeed — never a 400 for being "unsupported". Runs against
/// both a non-reprojecting and a reprojecting driver, so it can't pass by
/// accident of only exercising one capability answer; a regression that
/// starts advertising a CRS a driver can't serve (the defect this lane's
/// `crs_capable`-gated `crs::advertised_crs` fixes) fails this test
/// regardless of which specific CRS or driver capability changes underneath
/// it.
#[tokio::test]
async fn every_advertised_crs_is_accepted_by_the_items_endpoint() {
    for crs_capable in [false, true] {
        let app = build_crs_app(crs_capable);
        let collection = get(&app, "/collections/demo").await;
        assert_eq!(collection.status(), StatusCode::OK);
        let body = body_json(collection).await;
        let crs_list: Vec<String> = body["crs"]
            .as_array()
            .expect("a collection always advertises a crs array")
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            !crs_list.is_empty(),
            "a collection must always advertise at least CRS84 (crs_capable={crs_capable})"
        );

        for uri in crs_list {
            let query = percent_encode_crs_uri(&uri);
            let response = get(&app, format!("/collections/demo/items?crs={query}")).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "advertised crs '{uri}' (crs_capable={crs_capable}) was refused by /items"
            );
        }
    }
}

// -- Content-Crs on a projected collection (`#227`) -------------------------

/// This collection's own storage CRS when `build_crs_app_at_srid` is given
/// `3857` — the projected fixture `#227` needs, since a 4326 collection
/// cannot tell the fix from the bug.
const MERCATOR_CRS: &str = "http://www.opengis.net/def/crs/EPSG/0/3857";

/// The header that used to lie, over real HTTP. `RequestedCrs::Omitted`
/// transforms nothing on any driver — that is its own definition — so a
/// collection stored in EPSG:3857 answers a plain `/items` with metres. Until
/// `#227` the header said CRS84 anyway, and a client trusting it (the only
/// thing Part 2 gives it to trust) plotted metres as degrees with nothing in
/// the response to contradict it.
#[tokio::test]
async fn items_on_a_projected_collection_stamp_the_storage_crs() {
    let (app, _) = build_crs_app_at_srid(3857, false, false);
    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-crs").unwrap(),
        &format!("<{MERCATOR_CRS}>"),
        "an untransformed response from a 3857 collection is in 3857"
    );
}

/// The single-feature lane reaches the same verdict as the items lane — the
/// two run the same `crs::can_serve`/`crs::content_crs_uri` pair, so they
/// cannot come apart.
#[tokio::test]
async fn a_single_item_from_a_projected_collection_stamps_the_storage_crs() {
    let (app, _) = build_crs_app_at_srid(3857, false, false);
    let response = get(&app, "/collections/demo/items/a").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-crs").unwrap(),
        &format!("<{MERCATOR_CRS}>")
    );
}

/// Campaign rule 3, at the one place it bites: a client that genuinely needs
/// CRS84 from a projected collection served by a driver that cannot reproject
/// gets a **named 400**, not metres. This is the negotiation the old
/// unconditional CRS84 header made impossible — there was nothing to
/// negotiate with, because the server claimed to have already done it.
/// `bbox-crs` rides the same gate: four numbers read as degrees against a
/// metre-stored collection select the wrong rows just as surely.
#[tokio::test]
async fn crs84_from_a_projected_non_reprojecting_collection_is_refused_by_name() {
    let (app, _) = build_crs_app_at_srid(3857, false, false);
    let crs84 = percent_encode_crs_uri(CRS84);
    for query in [
        format!("/collections/demo/items?crs={crs84}"),
        format!("/collections/demo/items/a?crs={crs84}"),
        format!("/collections/demo/items?bbox=1,2,3,4&bbox-crs={crs84}"),
    ] {
        let response = get(&app, query.clone()).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{query} must be refused, not answered in metres"
        );
        let body = body_json(response).await;
        assert_eq!(body["code"], "InvalidParameter");
        let detail = body["detail"].as_str().unwrap().to_string();
        assert!(
            detail.contains(MERCATOR_CRS),
            "the refusal must name what this collection IS served in, got: {detail}"
        );
    }
}

/// The mirror image of the 4326 case, and the reason the refusal above is
/// phrased as "what work would this need", not "is this the storage CRS": a
/// driver that never reprojects already emits its rows in EPSG:3857, so
/// `crs=<storage>` costs it nothing and is served — which is exactly why the
/// collection may advertise it.
#[tokio::test]
async fn the_storage_crs_is_served_by_a_non_reprojecting_projected_collection() {
    let (app, _) = build_crs_app_at_srid(3857, false, false);
    let storage = percent_encode_crs_uri(MERCATOR_CRS);
    let response = get(&app, format!("/collections/demo/items?crs={storage}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-crs").unwrap(),
        &format!("<{MERCATOR_CRS}>")
    );
}

/// The metadata half, so the `crs` list and the header cannot contradict each
/// other: a projected collection under a non-reprojecting driver advertises
/// its storage CRS — and *only* that, because CRS84 is the one identifier it
/// genuinely cannot produce. `storageCrs`, which `#217` had to omit precisely
/// because the list it must be a member of was CRS84-only, reappears with it.
#[tokio::test]
async fn a_projected_collection_advertises_the_crs_it_actually_serves() {
    let (app, _) = build_crs_app_at_srid(3857, false, false);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["crs"].as_array().unwrap(),
        &vec![Value::String(MERCATOR_CRS.to_string())]
    );
    assert_eq!(body["storageCrs"], MERCATOR_CRS);
}

/// A `crs_capable` driver over the same projected storage: `crs=CRS84` is a
/// real `ST_Transform`, so CRS84 is both advertised and honestly stamped —
/// while the *default* response, which transforms nothing even here (see
/// `RequestedCrs::Omitted`, and `tellurion-postgis::sql::
/// reprojected_geom_expr`'s own `Omitted => geom` arm), still names the
/// storage CRS.
#[tokio::test]
async fn a_reprojecting_driver_stamps_crs84_only_when_it_actually_reprojected() {
    let (app, _) = build_crs_app_at_srid(3857, true, false);

    let default = get(&app, "/collections/demo/items").await;
    assert_eq!(default.status(), StatusCode::OK);
    assert_eq!(
        default.headers().get("content-crs").unwrap(),
        &format!("<{MERCATOR_CRS}>"),
        "an omitted crs is 'no transform' for every driver, PostGIS included"
    );

    let crs84 = get(
        &app,
        format!(
            "/collections/demo/items?crs={}",
            percent_encode_crs_uri(CRS84)
        ),
    )
    .await;
    assert_eq!(crs84.status(), StatusCode::OK);
    assert_eq!(
        crs84.headers().get("content-crs").unwrap(),
        &format!("<{CRS84}>")
    );
}

/// Rule 1, executed: a CRS84-equivalent storage is untouched by all of the
/// above, on every arm and under both driver capabilities. This is every
/// live deployment, including every Render demo, and it must not move a byte.
#[tokio::test]
async fn a_crs84_storage_still_stamps_crs84_everywhere() {
    for crs_capable in [false, true] {
        let (app, _) = build_crs_app_at_srid(4326, crs_capable, false);
        for path in [
            "/collections/demo/items".to_string(),
            format!(
                "/collections/demo/items?crs={}",
                percent_encode_crs_uri(CRS84)
            ),
            "/collections/demo/items/a".to_string(),
        ] {
            let response = get(&app, path.clone()).await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{path} (crs_capable={crs_capable})"
            );
            assert_eq!(
                response.headers().get("content-crs").unwrap(),
                &format!("<{CRS84}>"),
                "{path} (crs_capable={crs_capable})"
            );
        }
    }
}

/// The invariant that keeps the two halves of `#227` from drifting, driven
/// end to end over HTTP across every (srid, capability) pair this lane can
/// produce: whatever `Content-Crs` a 200 carries is one of the identifiers
/// the very same collection's `crs` list advertises. A header naming a CRS
/// outside that list is the same class of defect as the CRS84 lie it
/// replaced — the response and the metadata disagreeing about what the bytes
/// are.
#[tokio::test]
async fn every_stamped_content_crs_is_advertised_by_the_collection() {
    for srid in [4326, 3857] {
        for crs_capable in [false, true] {
            let (app, _) = build_crs_app_at_srid(srid, crs_capable, false);
            let body = body_json(get(&app, "/collections/demo").await).await;
            let advertised: Vec<String> = body["crs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();

            // The default request, plus every advertised CRS asked for by
            // name — the full set of requests this collection can answer.
            let mut paths = vec!["/collections/demo/items".to_string()];
            paths.extend(advertised.iter().map(|uri| {
                format!(
                    "/collections/demo/items?crs={}",
                    percent_encode_crs_uri(uri)
                )
            }));

            for path in paths {
                let response = get(&app, path.clone()).await;
                assert_eq!(
                    response.status(),
                    StatusCode::OK,
                    "{path} (srid={srid}, crs_capable={crs_capable})"
                );
                let stamped = response
                    .headers()
                    .get("content-crs")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string();
                assert!(
                    advertised.contains(&stamped),
                    "{path} (srid={srid}, crs_capable={crs_capable}) stamped \
                     Content-Crs '{stamped}', outside the advertised list {advertised:?}"
                );
            }
        }
    }
}

// -- filter-crs (OGC API Features Part 3 Filtering, Req 7/Req 8, `#217`) ----

/// The CRS84 URI and this collection's own storage CRS URI (EPSG:4326 by
/// authority — `CrsCatalog` reports srid 4326), percent-encoded for a query
/// string. Different identifiers, and axis-swapped from each other: this is
/// the pair `filter-crs` exists to tell apart.
const CRS84: &str = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";
const STORAGE_CRS: &str = "http://www.opengis.net/def/crs/EPSG/0/4326";
/// An `S_INTERSECTS` over `CrsCatalog`'s geometry column, ready to append a
/// `filter-crs` to.
const SPATIAL_FILTER_QUERY: &str =
    "/collections/demo/items?filter=S_INTERSECTS%28geom%2CBBOX%289%2C44%2C10.5%2C45.5%29%29";

/// Campaign rule 1, at the protocol boundary: a request that supplies a
/// `filter` and no `filter-crs` reaches the driver as
/// `RequestedCrs::Omitted` — the value every driver compiles byte-for-byte
/// the way it did before `#217`, and Requirement 7's
/// (`/req/filter/filter-crs-wgs84`) CRS84 default. Asserted against what the
/// driver was handed, not against the response body, because the response
/// body is what a driver that ignored the parameter would also produce.
#[tokio::test]
async fn a_filter_with_no_filter_crs_reaches_the_driver_as_omitted() {
    for filter_crs_capable in [false, true] {
        let (app, seen) = build_crs_app_with(true, filter_crs_capable);
        let response = get(&app, SPATIAL_FILTER_QUERY).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [RequestedCrs::Omitted],
            "no filter-crs parameter must reach the driver as Omitted \
             (filter_crs_capable={filter_crs_capable})"
        );
    }
}

/// Requirement 8 honoured: a `filter-crs` naming this collection's own
/// storage CRS reaches a capable driver as `RequestedCrs::Storage`, so the
/// driver can process the filter's geometries in that CRS rather than
/// silently in another one.
#[tokio::test]
async fn a_storage_filter_crs_reaches_a_capable_driver_as_storage() {
    let (app, seen) = build_crs_app_with(true, true);
    let response = get(
        &app,
        format!(
            "{SPATIAL_FILTER_QUERY}&filter-crs={}",
            percent_encode_crs_uri(STORAGE_CRS)
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(seen.lock().unwrap().as_slice(), [RequestedCrs::Storage]);
}

/// The refusal half of Requirement 8 — "The server SHALL return an error, if
/// it does not support the CRS identified in `filter-crs` for the resource"
/// — and the outcome `#217` insists on for any driver that cannot transform:
/// refused BY NAME, never accepted and quietly evaluated in the storage CRS.
/// The driver must not be called at all.
#[tokio::test]
async fn a_storage_filter_crs_against_a_driver_that_cannot_transform_it_returns_400_by_name() {
    let (app, seen) = build_crs_app_with(true, false);
    let response = get(
        &app,
        format!(
            "{SPATIAL_FILTER_QUERY}&filter-crs={}",
            percent_encode_crs_uri(STORAGE_CRS)
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    assert!(
        body["detail"].as_str().unwrap().contains("filter-crs"),
        "the refusal must name the parameter it refused; detail was: {}",
        body["detail"]
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "the request must be refused before the driver is ever asked to evaluate the filter"
    );
}

/// `filter-crs` rides its own capability, not `crs_capable` (`#217`): a
/// driver that reprojects output geometry perfectly well is refused here
/// anyway, because processing a filter's input geometries is different work.
/// That asymmetry is the whole subject of the issue — a fixture where the
/// two always agree could not show it.
#[tokio::test]
async fn filter_crs_is_gated_independently_of_the_crs_parameter() {
    let (app, _) = build_crs_app_with(true, false);
    // `?crs=<storage>` succeeds: this driver really can reproject output.
    let output_crs = get(
        &app,
        format!(
            "/collections/demo/items?crs={}",
            percent_encode_crs_uri(STORAGE_CRS)
        ),
    )
    .await;
    assert_eq!(output_crs.status(), StatusCode::OK);
    // The same collection, the same driver, the same CRS — refused for
    // `filter-crs`, because that is a capability it does not have.
    let filter_crs = get(
        &app,
        format!(
            "{SPATIAL_FILTER_QUERY}&filter-crs={}",
            percent_encode_crs_uri(STORAGE_CRS)
        ),
    )
    .await;
    assert_eq!(filter_crs.status(), StatusCode::BAD_REQUEST);
}

/// `filter-crs=CRS84` is Requirement 7's own default spelled out, so it is a
/// no-op every driver can honour — accepted even by one that cannot
/// transform, and reaching the driver as `Crs84` rather than being folded
/// into `Omitted`, since a driver whose storage CRS is not CRS84 has real
/// work to do for it.
#[tokio::test]
async fn an_explicit_crs84_filter_crs_is_accepted_by_a_driver_that_cannot_transform() {
    let (app, seen) = build_crs_app_with(false, false);
    let response = get(
        &app,
        format!(
            "{SPATIAL_FILTER_QUERY}&filter-crs={}",
            percent_encode_crs_uri(CRS84)
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(seen.lock().unwrap().as_slice(), [RequestedCrs::Crs84]);
}

// -- Requirement 7 against a projected storage (`#247`) --------------------

/// `#247`, the protocol half: a driver that cannot transform a filter's
/// spatial literals, a collection whose storage is projected, and a request
/// naming **no** `filter-crs` at all.
///
/// Requirement 7 (`/req/filter/filter-crs-wgs84`) says such a request's
/// geometries SHALL be processed in CRS84, and against a 3857 column that is a
/// real coordinate transform — the same work an explicit `filter-crs=CRS84`
/// asks for. This driver cannot do it. It has exactly three options and two
/// are forbidden: hand the literal down anyway and let the storage answer
/// (PostGIS: the mixed-SRID `500` this issue is named for; an in-memory
/// comparator: rows selected in a CRS the client never named, under a `200`),
/// or refuse by name. It refuses by name, before `items` is ever called.
#[tokio::test]
async fn a_default_spatial_filter_against_a_projected_storage_is_refused_by_name() {
    let (app, seen) = build_crs_app_at_srid(3857, false, false);
    let response = get(&app, SPATIAL_FILTER_QUERY).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    let detail = body["detail"].as_str().unwrap().to_string();
    assert!(
        detail.contains("CRS84") && detail.contains("spatial filter"),
        "the refusal must name what it cannot do, not just say no; detail was: {detail}"
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "the driver must never be asked to evaluate a filter it cannot express"
    );
}

/// The same collection and the same driver, one predicate different: a filter
/// carrying no coordinates has nothing to process in any CRS, so the refusal
/// above must not touch it. `geom IS NOT NULL` makes the point at its
/// sharpest — it is a filter *about the geometry column itself* that still
/// holds no geometry literal, so "does the filter mention geometry" would get
/// this wrong where "does it carry coordinates" gets it right.
///
/// This is why the gate asks `Filter::has_spatial_literal` rather than
/// `filter.is_some()`: refusing this request would name a transform it never
/// asked for, and would cost every projected deployment its ordinary
/// attribute filtering over a rule about coordinates.
#[tokio::test]
async fn a_filter_with_no_spatial_literal_against_a_projected_storage_is_untouched() {
    let (app, seen) = build_crs_app_at_srid(3857, false, false);
    let response = get(
        &app,
        "/collections/demo/items?filter=geom%20IS%20NOT%20NULL",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(seen.lock().unwrap().as_slice(), [RequestedCrs::Omitted]);
}

/// A driver that CAN transform gets the same projected collection and the
/// same default request, and is simply handed it — `RequestedCrs::Omitted`,
/// for its own compiler to read as CRS84 and transform into storage. PostGIS
/// is that driver, and this is the request that answered `500` before `#247`.
#[tokio::test]
async fn a_default_spatial_filter_against_a_projected_storage_reaches_a_capable_driver() {
    let (app, seen) = build_crs_app_at_srid(3857, true, true);
    let response = get(&app, SPATIAL_FILTER_QUERY).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(seen.lock().unwrap().as_slice(), [RequestedCrs::Omitted]);
}

/// **The rule `#247` must not break.** A CRS84 storage — every live Render
/// demo — is unmoved: the identical default spatial filter, against every
/// capability pairing including the incapable one, still reaches the driver as
/// `Omitted` and still answers `200`. The refusal above is conditional on the
/// storage SRID, not on the driver's capability alone, and this is the
/// assertion that says so.
///
/// The Italy contract gate cannot catch a regression in the *other* direction,
/// since its collection is 4326 and exercises only this path; that is what
/// `a_default_spatial_filter_against_a_projected_storage_is_refused_by_name`
/// above is for.
#[tokio::test]
async fn a_default_spatial_filter_against_a_crs84_storage_is_unmoved() {
    for (crs_capable, filter_crs_capable) in [(false, false), (true, false), (true, true)] {
        let (app, seen) = build_crs_app_at_srid(4326, crs_capable, filter_crs_capable);
        let response = get(&app, SPATIAL_FILTER_QUERY).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "a CRS84 storage must serve a default spatial filter whatever the driver can do \
             (crs_capable={crs_capable}, filter_crs_capable={filter_crs_capable})"
        );
        assert_eq!(seen.lock().unwrap().as_slice(), [RequestedCrs::Omitted]);
    }
}

// -- bbox with an omitted bbox-crs against a projected storage (`#255`) -----

/// A degree bbox and no `bbox-crs` — the plainest conformant request Part 1
/// defines, and the one every client sends.
const DEFAULT_BBOX_QUERY: &str = "/collections/demo/items?bbox=9,44,10.5,45.5";

/// `#255`, the protocol half: a driver that cannot transform, a collection
/// whose storage is projected, and a `bbox` naming **no** `bbox-crs`.
///
/// Part 1 Requirement 23 (`/req/core/fc-bbox-definition`) clause C and Part 2
/// Requirement 8 (`/req/crs/fc-bbox-crs-valid-default-value`) both fix those
/// four numbers as CRS84, and against a 3857 column reading them that way is a
/// real coordinate transform. This driver cannot do it, and the two other
/// options are both forbidden: compare the numbers raw and answer `200` with
/// rows that violate Requirement 24 (`/req/core/fc-bbox-response`) clause A,
/// or silently reinterpret the box in the storage CRS, which is the invented
/// default rule 1 forbids. It refuses by name, before `items` is ever called.
#[tokio::test]
async fn a_default_bbox_against_a_projected_storage_is_refused_by_name() {
    let (app, seen) = build_crs_app_at_srid(3857, false, false);
    let response = get(&app, DEFAULT_BBOX_QUERY).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    let detail = body["detail"].as_str().unwrap().to_string();
    assert!(
        detail.contains("CRS84") && detail.contains("bbox"),
        "the refusal must name what it cannot do, not just say no; detail was: {detail}"
    );
    assert!(
        detail.contains("EPSG/0/3857"),
        "and must name what the collection IS served in, so the client can send that \
         bbox-crs instead; detail was: {detail}"
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "the driver must never be asked to evaluate a bbox it cannot express"
    );
}

/// The escape hatch the refusal above names, and the reason it is a refusal
/// rather than a dead end: the same projected collection, the same incapable
/// driver, the same four numbers — but `bbox-crs` naming the collection's own
/// storage CRS, which `crs::advertised_crs` lists for exactly this collection.
/// Nothing has to be transformed, so `can_serve` says yes and the driver is
/// handed the request.
#[tokio::test]
async fn an_explicit_storage_bbox_crs_against_a_projected_storage_is_served() {
    let (app, seen) = build_crs_app_at_srid(3857, false, false);
    let response = get(
        &app,
        format!(
            "{DEFAULT_BBOX_QUERY}&bbox-crs={}",
            percent_encode_crs_uri("http://www.opengis.net/def/crs/EPSG/0/3857")
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

/// A driver that CAN transform gets the same projected collection and the same
/// default request, and is simply handed it — `RequestedCrs::Omitted`, for its
/// own compiler to read as CRS84 and transform into storage. PostGIS is that
/// driver, and this is the request that answered `200` with the wrong rows
/// before `#255`.
#[tokio::test]
async fn a_default_bbox_against_a_projected_storage_reaches_a_capable_driver() {
    let (app, seen) = build_crs_app_at_srid(3857, true, true);
    let response = get(&app, DEFAULT_BBOX_QUERY).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

/// **The rule `#255` must not break.** A CRS84 storage — every live Render
/// demo — is unmoved: the identical default bbox, against every capability
/// pairing including the incapable one, still reaches the driver and still
/// answers `200`. The refusal above is conditional on the storage SRID, not on
/// the driver's capability alone, and this is the assertion that says so.
#[tokio::test]
async fn a_default_bbox_against_a_crs84_storage_is_unmoved() {
    for (crs_capable, filter_crs_capable) in [(false, false), (true, false), (true, true)] {
        let (app, seen) = build_crs_app_at_srid(4326, crs_capable, filter_crs_capable);
        let response = get(&app, DEFAULT_BBOX_QUERY).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "a CRS84 storage must serve a default bbox whatever the driver can do \
             (crs_capable={crs_capable}, filter_crs_capable={filter_crs_capable})"
        );
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}

/// The gate is about a `bbox`, not about the collection: the same projected
/// collection under the same incapable driver still serves a request that
/// carries no `bbox` at all. `#255` narrows nothing else — there is no
/// `has_spatial_literal` counterpart to reach for, because a `bbox` always
/// carries coordinates and a request without one carries none.
#[tokio::test]
async fn a_bbox_less_request_against_a_projected_storage_is_untouched() {
    let (app, seen) = build_crs_app_at_srid(3857, false, false);
    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

/// A `filter-crs` naming a CRS this collection never advertised is refused
/// by `crs::resolve` — the same single seam `crs`/`bbox-crs` are validated
/// through, so `filter-crs` can never be handed a CRS the collection's own
/// metadata didn't list.
#[tokio::test]
async fn an_unsupported_filter_crs_is_refused_even_by_a_capable_driver() {
    let (app, seen) = build_crs_app_with(true, true);
    let response = get(
        &app,
        format!(
            "{SPATIAL_FILTER_QUERY}&filter-crs={}",
            percent_encode_crs_uri("http://www.opengis.net/def/crs/EPSG/0/3857")
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(seen.lock().unwrap().is_empty());
}

/// `filter-crs` is a reserved `/items` parameter name (`#52`), so it must
/// never be mistaken for a queryable-equality predicate — which would 400
/// with "not a declared queryable" instead of being honoured. Pinned here
/// because `#217` turned the reservation into a real implementation, and a
/// regression that dropped it from the reserved list would fail loudly on a
/// collection that declares queryables and quietly on one that doesn't.
#[tokio::test]
async fn filter_crs_is_never_read_as_a_queryable_equality_parameter() {
    let (app, seen) = build_crs_app_with(true, true);
    let response = get(
        &app,
        format!(
            "/collections/demo/items?filter-crs={}",
            percent_encode_crs_uri(CRS84)
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(seen.lock().unwrap().as_slice(), [RequestedCrs::Crs84]);
}

/// A `next`/`self` link that dropped `filter-crs` would evaluate page two's
/// filter geometry in a different CRS than page one's — the same silent
/// wrong-CRS evaluation, one page later.
#[tokio::test]
async fn items_links_echo_the_filter_crs_parameter() {
    let (app, _) = build_crs_app_with(true, true);
    let response = get(
        &app,
        format!(
            "{SPATIAL_FILTER_QUERY}&filter-crs={}",
            percent_encode_crs_uri(STORAGE_CRS)
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let self_href = body["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["rel"] == "self")
        .expect("a self link")["href"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        self_href.contains("filter-crs="),
        "self link dropped filter-crs: {self_href}"
    );
}

// -- cql2ConformanceClasses (`#105`) -----------------------------------------

/// A `FeatureSource` whose declared CQL2 classes are configurable — used to
/// prove `GET /collections/{cid}` surfaces exactly what the resolved driver
/// declares, the same per-request-driven shape `CrsFeatureSource` above
/// proves for `crs_capable`.
struct ClassesFeatureSource {
    classes: &'static [&'static str],
}

#[async_trait::async_trait]
impl FeatureSource for ClassesFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        _query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        Ok(FeaturePage {
            features_geojson: vec![feature("a")],
            number_matched: Some(1),
            next_token: None,
        })
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        id: &str,
        _filter: Option<&Filter>,
    ) -> CoreResult<Option<Value>> {
        Ok((id == "a").then(|| feature("a")))
    }

    fn cql2_conformance_classes(&self) -> Vec<&'static str> {
        self.classes.to_vec()
    }
}

struct ClassesDriver {
    classes: &'static [&'static str],
}

impl StorageDriver for ClassesDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(CrsCatalog { srid: 4326 })
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::new(ClassesFeatureSource {
            classes: self.classes,
        }))
    }
}

struct ClassesFactory {
    classes: &'static [&'static str],
}

impl DriverFactory for ClassesFactory {
    fn name(&self) -> &str {
        "classes-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(ClassesDriver {
            classes: self.classes,
        }))
    }
}

const CLASSES_CONFIG: &str = r#"
storages: [ { id: main, driver: classes-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
"#;

fn build_classes_app(classes: &'static [&'static str]) -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(CLASSES_CONFIG).unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(ClassesFactory { classes }));

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
    tellurion_features::router().with_state(ctx)
}

/// A PostGIS-backed collection surfaces the full set, including the three
/// classes the pre-`#105` workspace-wide list withheld from every
/// collection regardless of driver.
#[tokio::test]
async fn collection_metadata_surfaces_the_full_set_for_a_postgis_strength_driver() {
    let app = build_classes_app(&[
        "http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2",
        "http://www.opengis.net/spec/cql2/1.0/conf/cql2-text",
        "http://www.opengis.net/spec/cql2/1.0/conf/cql2-json",
        "http://www.opengis.net/spec/cql2/1.0/conf/basic-spatial-functions",
        "http://www.opengis.net/spec/cql2/1.0/conf/advanced-comparison-operators",
        "http://www.opengis.net/spec/cql2/1.0/conf/spatial-functions",
        "http://www.opengis.net/spec/cql2/1.0/conf/temporal-functions",
    ]);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let classes: Vec<&str> = body["cql2ConformanceClasses"]
        .as_array()
        .expect("a collection always carries a cql2ConformanceClasses array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for reearned in [
        "http://www.opengis.net/spec/cql2/1.0/conf/advanced-comparison-operators",
        "http://www.opengis.net/spec/cql2/1.0/conf/spatial-functions",
        "http://www.opengis.net/spec/cql2/1.0/conf/temporal-functions",
    ] {
        assert!(
            classes.contains(&reearned),
            "a PostGIS-strength collection must re-earn {reearned} on its own surface"
        );
    }
}

/// A GeoPackage/Iceberg-strength collection (the weaker, narrower shape)
/// never surfaces the three richer classes on its own metadata — the
/// per-collection surface is honest about what THIS collection's driver
/// actually compiles, not a blanket claim.
#[tokio::test]
async fn collection_metadata_omits_the_richer_classes_for_a_weaker_driver() {
    let app = build_classes_app(&[
        "http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2",
        "http://www.opengis.net/spec/cql2/1.0/conf/cql2-text",
        "http://www.opengis.net/spec/cql2/1.0/conf/cql2-json",
    ]);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let classes: Vec<&str> = body["cql2ConformanceClasses"]
        .as_array()
        .expect("a collection always carries a cql2ConformanceClasses array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for withheld in [
        "http://www.opengis.net/spec/cql2/1.0/conf/advanced-comparison-operators",
        "http://www.opengis.net/spec/cql2/1.0/conf/spatial-functions",
        "http://www.opengis.net/spec/cql2/1.0/conf/temporal-functions",
    ] {
        assert!(
            !classes.contains(&withheld),
            "a weaker driver's collection must not claim {withheld}"
        );
    }
    assert!(classes.contains(&"http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2"));
}

/// A collection whose driver declines CQL2 filtering entirely (the memory
/// driver's own shape, empty `cql2_conformance_classes`) still carries the
/// field — an empty array, never an absent one, the same "never fabricated,
/// never silently absent" rule `crs` follows.
#[tokio::test]
async fn collection_metadata_carries_an_empty_array_for_a_non_filter_capable_driver() {
    let app = build_classes_app(&[]);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["cql2ConformanceClasses"].as_array().unwrap().len(), 0);
}

/// `case-insensitive-comparison` is never surfaced even on the strongest
/// per-collection driver — pinned end to end, not just at the driver-trait
/// level, since no driver's own `cql2_conformance_classes` implementation
/// ever includes it (`#105`/`#106`).
#[tokio::test]
async fn collection_metadata_never_surfaces_case_insensitive_comparison() {
    let app = build_classes_app(&[
        "http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2",
        "http://www.opengis.net/spec/cql2/1.0/conf/cql2-text",
        "http://www.opengis.net/spec/cql2/1.0/conf/cql2-json",
        "http://www.opengis.net/spec/cql2/1.0/conf/basic-spatial-functions",
        "http://www.opengis.net/spec/cql2/1.0/conf/advanced-comparison-operators",
        "http://www.opengis.net/spec/cql2/1.0/conf/spatial-functions",
        "http://www.opengis.net/spec/cql2/1.0/conf/temporal-functions",
    ]);
    let response = get(&app, "/collections/demo").await;
    let body = body_json(response).await;
    let classes: Vec<&str> = body["cql2ConformanceClasses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        !classes.contains(&"http://www.opengis.net/spec/cql2/1.0/conf/case-insensitive-comparison")
    );
}

/// `GET /collections` (the list endpoint) surfaces the same field per entry
/// as `GET /collections/{cid}` — both routes share `collection_summary`, but
/// this proves the listing's own independent resolution path
/// (`resolved_canonical`) carries it through too.
#[tokio::test]
async fn collections_listing_also_surfaces_cql2_conformance_classes() {
    let app = build_classes_app(&[
        "http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2",
        "http://www.opengis.net/spec/cql2/1.0/conf/cql2-text",
        "http://www.opengis.net/spec/cql2/1.0/conf/cql2-json",
    ]);
    let response = get(&app, "/collections").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let classes = body["collections"][0]["cql2ConformanceClasses"]
        .as_array()
        .expect("collections listing entries carry a cql2ConformanceClasses array");
    assert_eq!(classes.len(), 3);
}

// -- geometryProfile (`#101`) ------------------------------------------------

/// A `CatalogSource` whose `geometry_profile` answer is configurable — used
/// to prove `GET /collections/{cid}` surfaces the profile the resolved
/// driver reports, the same per-request-driven shape `ClassesFeatureSource`
/// above proves for `cql2ConformanceClasses`.
struct GeometryProfileCatalog {
    profile: Option<GeometryProfile>,
}

#[async_trait::async_trait]
impl CatalogSource for GeometryProfileCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: "demo".to_string(),
            geometry_column: Some("geom".to_string()),
            primary_key: Some("id".to_string()),
            srid: Some(4326),
            geometry_type: Some("MULTIPOLYGON".to_string()),
        }])
    }

    async fn geometry_profile(
        &self,
        _physical: &PhysicalCollection,
    ) -> CoreResult<Option<GeometryProfile>> {
        Ok(self.profile)
    }
}

struct GeometryProfileDriver {
    source: Arc<FakeFeatureSource>,
    profile: Option<GeometryProfile>,
}

impl StorageDriver for GeometryProfileDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(GeometryProfileCatalog {
            profile: self.profile,
        })
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(self.source.clone() as Arc<dyn FeatureSource>)
    }
}

struct GeometryProfileFactory {
    profile: Option<GeometryProfile>,
}

impl DriverFactory for GeometryProfileFactory {
    fn name(&self) -> &str {
        "geometry-profile-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(GeometryProfileDriver {
            source: Arc::new(FakeFeatureSource { items: vec![] }),
            profile: self.profile,
        }))
    }
}

const GEOMETRY_PROFILE_CONFIG: &str = r#"
storages: [ { id: main, driver: geometry-profile-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
"#;

fn geometry_profile_fixture() -> GeometryProfile {
    GeometryProfile {
        sample_size: 128,
        computed_at: std::time::SystemTime::now(),
        vertices: VertexStats {
            mean: 5.0,
            median: 4.0,
            p95: 9.0,
            max: 20,
            total_estimated: Some(1_280),
        },
        vertex_density_per_area: Some(0.2),
        multi_part_fraction: 0.05,
        mean_ring_count: Some(1.1),
        feature_size: FeatureSizeStats {
            p50: Some(2.0),
            p95: Some(8.0),
            max: Some(10.0),
        },
    }
}

fn build_geometry_profile_app(profile: Option<GeometryProfile>) -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(GEOMETRY_PROFILE_CONFIG).unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(GeometryProfileFactory { profile }));

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
    tellurion_features::router().with_state(ctx)
}

/// A collection whose driver reports a geometry profile surfaces it as the
/// `geometryProfile` member — `sampleSize`/`computedAt` present alongside
/// every other stat, exactly as recorded on `tellurion_core::catalog::
/// GeometryProfile`.
#[tokio::test]
async fn collection_metadata_surfaces_the_geometry_profile_when_one_was_computed() {
    let app = build_geometry_profile_app(Some(geometry_profile_fixture()));
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let member = &body["geometryProfile"];
    assert_eq!(member["sampleSize"], 128);
    assert!(
        member["computedAt"].as_str().is_some(),
        "computedAt must serialize as a string timestamp: {member}"
    );
    assert_eq!(member["vertices"]["mean"], 5.0);
    assert_eq!(member["vertices"]["max"], 20);
    assert_eq!(member["multiPartFraction"], 0.05);
    assert_eq!(member["meanRingCount"], 1.1);
    assert_eq!(member["featureSize"]["p50"], 2.0);
}

/// A collection whose driver never computed a profile (the trait default,
/// `CatalogSource::geometry_profile`'s own `Ok(None)`) omits the
/// `geometryProfile` member entirely — never `null` — the same "never
/// fabricated, never silently present" rule every other optional
/// `CollectionSummary` member follows.
#[tokio::test]
async fn collection_metadata_omits_the_geometry_profile_member_when_none_was_computed() {
    let app = build_geometry_profile_app(None);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(
        !body.as_object().unwrap().contains_key("geometryProfile"),
        "geometryProfile must be entirely absent, not null, when no profile was computed: {body}"
    );
}

/// `GET /collections` (the list endpoint) surfaces the same member per
/// entry as `GET /collections/{cid}` — both routes share
/// `collection_summary`, the same parity `cql2ConformanceClasses` already
/// proves.
#[tokio::test]
async fn collections_listing_also_surfaces_the_geometry_profile() {
    let app = build_geometry_profile_app(Some(geometry_profile_fixture()));
    let response = get(&app, "/collections").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["collections"][0]["geometryProfile"]["sampleSize"], 128);
}

// -- queryables (`#33` follow-up) --------------------------------------------

/// A `CatalogSource` reporting a richer attribute schema than
/// `FilterableCatalog` — one column per JSON Schema shape the queryables
/// document produces (`string`, `integer`, `boolean`, a `date-time`-format
/// datetime column) plus a geometry column — so the HTTP-level queryables
/// tests below have more than one type mapping to prove against.
struct QueryablesCatalog;

#[async_trait::async_trait]
impl CatalogSource for QueryablesCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: "demo".to_string(),
            geometry_column: Some("geom".to_string()),
            primary_key: Some("id".to_string()),
            srid: Some(4326),
            geometry_type: None,
        }])
    }

    async fn attribute_schema(
        &self,
        _physical: &PhysicalCollection,
    ) -> CoreResult<Option<Vec<AttributeColumn>>> {
        Ok(Some(vec![
            AttributeColumn {
                name: "name".to_string(),
                sql_type: "text".to_string(),
            },
            AttributeColumn {
                name: "population".to_string(),
                sql_type: "integer".to_string(),
            },
            AttributeColumn {
                name: "active".to_string(),
                sql_type: "boolean".to_string(),
            },
            AttributeColumn {
                name: "observed_at".to_string(),
                sql_type: "timestamp with time zone".to_string(),
            },
        ]))
    }

    async fn temporal_column(&self, _physical: &PhysicalCollection) -> CoreResult<Option<String>> {
        Ok(Some("observed_at".to_string()))
    }
}

struct QueryablesDriver {
    source: Arc<FakeFeatureSource>,
}

impl StorageDriver for QueryablesDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(QueryablesCatalog)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(self.source.clone() as Arc<dyn FeatureSource>)
    }
}

struct QueryablesFactory {
    source: Arc<FakeFeatureSource>,
}

impl DriverFactory for QueryablesFactory {
    fn name(&self) -> &str {
        "queryables-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(QueryablesDriver {
            source: self.source.clone(),
        }))
    }
}

const QUERYABLES_CONFIG: &str = r#"
storages: [ { id: main, driver: queryables-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#;

fn build_queryables_app() -> axum::Router {
    build_queryables_app_with_config(QUERYABLES_CONFIG)
}

fn build_queryables_app_with_config(config_yaml: &str) -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
    config.validate().unwrap();

    let source = Arc::new(FakeFeatureSource { items: vec![] });
    let mut registry = Registry::new();
    registry.register(Arc::new(QueryablesFactory { source }));

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
    tellurion_features::router().with_state(ctx)
}

#[tokio::test]
async fn configured_public_base_makes_the_queryables_id_absolute() {
    let config = format!(
        "{QUERYABLES_CONFIG}\nserver: {{ public_base_url: 'https://maps.example.test/tellurion/' }}\n"
    );
    let app = axum::Router::new().nest(
        "/{tenant}/features/catalogs/{catalog}",
        build_queryables_app_with_config(&config),
    );

    let response = get(
        &app,
        "/public/features/catalogs/default/collections/demo/queryables",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["$id"],
        "https://maps.example.test/tellurion/public/features/catalogs/default/collections/demo/queryables"
    );
}

#[tokio::test]
async fn queryables_returns_schema_json_content_type() {
    let app = build_queryables_app();
    let response = get(&app, "/collections/demo/queryables").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/schema+json"
    );
}

#[tokio::test]
async fn queryables_document_shape_and_property_types() {
    let app = build_queryables_app();
    let response = get(&app, "/collections/demo/queryables").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    assert_eq!(
        body["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(body["$id"], "/collections/demo/queryables");
    assert_eq!(body["type"], "object");
    assert_eq!(body["title"], "demo");

    let properties = body["properties"].as_object().unwrap();
    let mut keys: Vec<&str> = properties.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["active", "geom", "name", "observed_at", "population"]
    );

    assert_eq!(properties["name"]["type"], "string");
    assert!(properties["name"].get("format").is_none());

    assert_eq!(properties["population"]["type"], "integer");
    assert_eq!(properties["active"]["type"], "boolean");

    assert_eq!(properties["observed_at"]["type"], "string");
    assert_eq!(properties["observed_at"]["format"], "date-time");

    // Geometry idiom (Requirements 3B/3E): a `format: geometry-*`, never a
    // `type` or `$ref` member.
    assert_eq!(properties["geom"]["format"], "geometry-any");
    assert!(properties["geom"].get("type").is_none());
    assert!(properties["geom"].get("$ref").is_none());
}

#[tokio::test]
async fn queryables_link_is_on_the_collection_resource() {
    let app = build_queryables_app();
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let link = find_link(&body, "http://www.opengis.net/def/rel/ogc/1.0/queryables")
        .expect("the collection resource must link to its queryables document");
    assert_eq!(link["href"], "/collections/demo/queryables");
    assert_eq!(link["type"], "application/schema+json");
}

#[tokio::test]
async fn queryables_link_is_also_on_each_collections_list_entry() {
    let app = build_queryables_app();
    let response = get(&app, "/collections").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let collections = body["collections"].as_array().unwrap();
    assert_eq!(collections.len(), 1);
    assert!(
        find_link(
            &collections[0],
            "http://www.opengis.net/def/rel/ogc/1.0/queryables"
        )
        .is_some(),
        "each collection listed in /collections must also link to its queryables document"
    );
}

#[tokio::test]
async fn queryables_for_an_unknown_collection_returns_404() {
    let app = build_queryables_app();
    let response = get(&app, "/collections/nope/queryables").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A tiles-only collection (no `FeatureSource`, PMTiles-shaped, `#20`) must
/// still resolve a queryables document — `get_queryables` never requires the
/// features capability, only that the collection is routed at all.
#[tokio::test]
async fn queryables_resolves_for_a_tiles_only_collection() {
    let app = build_tiles_only_app();
    let response = get(&app, "/collections/tiles-only/queryables").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["title"], "tiles-only");
}

// -- queryables as query parameters (OGC API Features Part 3, `#52`) --------

fn queryable_param_feature(id: &str, name: &str, population: i64, active: bool) -> Value {
    json!({
        "type": "Feature",
        "id": id,
        "geometry": null,
        "properties": { "name": name, "population": population, "active": active }
    })
}

/// Evaluates the (broader) subset of `Filter` this section's tests exercise:
/// an equality `Compare` against a string, number, or boolean property, and
/// `And` of any number of such predicates — a small in-memory stand-in for
/// `tellurion-postgis`'s real SQL compiler (covered separately by
/// `tellurion-postgis`'s own live-database tests), enough to prove a filter
/// this crate built from bare `?propertyName=value` query parameters
/// actually reaches `items` and narrows the result set, including composed
/// with an ordinary `filter=` expression.
fn matches_queryable_filter(feature: &Value, filter: Option<&Filter>) -> bool {
    match filter {
        None => true,
        Some(Filter::Compare {
            property,
            op: CompareOp::Eq,
            value,
        }) => {
            let actual = &feature["properties"][property];
            match value {
                Literal::Text(expected) => actual.as_str() == Some(expected.as_str()),
                Literal::Number(expected) => actual.as_f64() == Some(*expected),
                Literal::Bool(expected) => actual.as_bool() == Some(*expected),
            }
        }
        Some(Filter::And(items)) => items
            .iter()
            .all(|item| matches_queryable_filter(feature, Some(item))),
        Some(_) => panic!("matches_queryable_filter: unexpected filter shape in this test fixture"),
    }
}

/// A `FeatureSource` that advertises `filter_capable() == true` and actually
/// evaluates every shape `matches_queryable_filter` understands, honoring
/// `query.limit` too — this section's `reserved_parameter_names_are_never_
/// treated_as_queryables` test relies on `limit` still capping the page,
/// proving that name kept its own meaning rather than becoming a query for
/// a (nonexistent) property called `limit`.
struct QueryableParamsFeatureSource {
    items: Vec<(String, Value)>,
}

#[async_trait::async_trait]
impl FeatureSource for QueryableParamsFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        let matched: Vec<Value> = self
            .items
            .iter()
            .filter(|(_, v)| matches_queryable_filter(v, query.filter.as_ref()))
            .map(|(_, v)| v.clone())
            .collect();
        let number_matched = matched.len() as u64;
        let page: Vec<Value> = matched.into_iter().take(query.limit as usize).collect();
        Ok(FeaturePage {
            number_matched: Some(number_matched),
            features_geojson: page,
            next_token: None,
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

    fn filter_capable(&self) -> bool {
        true
    }
}

struct QueryableParamsDriver {
    source: Arc<QueryableParamsFeatureSource>,
}

impl StorageDriver for QueryableParamsDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(QueryablesCatalog)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(self.source.clone() as Arc<dyn FeatureSource>)
    }
}

struct QueryableParamsFactory {
    source: Arc<QueryableParamsFeatureSource>,
}

impl DriverFactory for QueryableParamsFactory {
    fn name(&self) -> &str {
        "queryable-params-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(QueryableParamsDriver {
            source: self.source.clone(),
        }))
    }
}

const QUERYABLE_PARAMS_CONFIG: &str = r#"
storages: [ { id: main, driver: queryable-params-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#;

/// Same physical shape as `QueryablesCatalog` (`name`/`population`/`active`/
/// `observed_at`, a real declared queryable set with one property per JSON
/// Schema shape) but with an open declared schema — every attribute column
/// is a queryable, matching this section's "equality hit"/"unknown
/// parameter"/"reserved name" tests, which need more than one type to prove
/// coercion against.
fn build_queryable_params_app(items: Vec<(&str, Value)>) -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(QUERYABLE_PARAMS_CONFIG).unwrap();
    config.validate().unwrap();

    let source = Arc::new(QueryableParamsFeatureSource {
        items: items
            .into_iter()
            .map(|(id, v)| (id.to_string(), v))
            .collect(),
    });
    let mut registry = Registry::new();
    registry.register(Arc::new(QueryableParamsFactory { source }));

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
    tellurion_features::router().with_state(ctx)
}

/// Same fixture as [`build_queryable_params_app`], except the collection
/// declares a closed schema (`additional_properties: false`) naming only
/// `population` — proving the query-parameter mechanism reuses the exact
/// same closed-schema narrowing the queryables document and `filter=` both
/// already apply (`#44`), not a second, independently-maintained rule.
const QUERYABLE_PARAMS_CLOSED_SCHEMA_CONFIG: &str = r#"
storages: [ { id: main, driver: queryable-params-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    schema:
      properties:
        - { name: population, type: integer }
      additional_properties: false
"#;

fn build_queryable_params_app_with_closed_schema(items: Vec<(&str, Value)>) -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(QUERYABLE_PARAMS_CLOSED_SCHEMA_CONFIG).unwrap();
    config.validate().unwrap();

    let source = Arc::new(QueryableParamsFeatureSource {
        items: items
            .into_iter()
            .map(|(id, v)| (id.to_string(), v))
            .collect(),
    });
    let mut registry = Registry::new();
    registry.register(Arc::new(QueryableParamsFactory { source }));

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
    tellurion_features::router().with_state(ctx)
}

#[tokio::test]
async fn queryable_param_string_equality_narrows_the_result_set() {
    let app = build_queryable_params_app(vec![
        ("a", queryable_param_feature("a", "alpha", 10, true)),
        ("b", queryable_param_feature("b", "beta", 20, false)),
    ]);
    let response = get(&app, "/collections/demo/items?name=alpha").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["numberReturned"], 1);
    assert_eq!(body["features"][0]["id"], "a");
}

#[tokio::test]
async fn queryable_param_number_equality_narrows_the_result_set() {
    let app = build_queryable_params_app(vec![
        ("a", queryable_param_feature("a", "alpha", 10, true)),
        ("b", queryable_param_feature("b", "beta", 20, false)),
    ]);
    let response = get(&app, "/collections/demo/items?population=20").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["numberReturned"], 1);
    assert_eq!(body["features"][0]["id"], "b");
}

#[tokio::test]
async fn queryable_param_boolean_equality_narrows_the_result_set() {
    let app = build_queryable_params_app(vec![
        ("a", queryable_param_feature("a", "alpha", 10, true)),
        ("b", queryable_param_feature("b", "beta", 20, false)),
    ]);
    let response = get(&app, "/collections/demo/items?active=false").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["numberReturned"], 1);
    assert_eq!(body["features"][0]["id"], "b");
}

/// A bare queryable parameter and an ordinary `filter=` expression must AND
/// together, neither one alone deciding the result: all three features share
/// `name=alpha`; `filter=population > 10` alone would match b and c,
/// `?population=20` alone would also match b and c, but composed together
/// with `active=true` only b qualifies.
#[tokio::test]
async fn queryable_param_composes_with_filter_via_and() {
    let app = build_queryable_params_app(vec![
        ("a", queryable_param_feature("a", "alpha", 5, true)),
        ("b", queryable_param_feature("b", "alpha", 20, true)),
        ("c", queryable_param_feature("c", "alpha", 20, false)),
    ]);
    let response = get(
        &app,
        "/collections/demo/items?population=20&active=true&filter=name%20%3D%20'alpha'",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["numberReturned"], 1);
    assert_eq!(body["features"][0]["id"], "b");
}

/// `next`/`self` links must echo the bare queryable parameter too — losing
/// it on a `next` link would silently widen a later page beyond what the
/// first page's caller actually asked for.
#[tokio::test]
async fn queryable_param_is_echoed_in_the_self_link() {
    let app =
        build_queryable_params_app(vec![("a", queryable_param_feature("a", "alpha", 10, true))]);
    let response = get(&app, "/collections/demo/items?population=10").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let self_href = find_link(&body, "self").unwrap()["href"].as_str().unwrap();
    assert!(
        self_href.contains("population=10"),
        "self href was: {self_href}"
    );
}

/// Reserved parameter names (`limit`, `bbox`, `datetime`, `token`, `filter`,
/// `filter-lang`) keep their own fixed meaning and are never matched against
/// a collection's declared queryables — `limit` here narrows the page size,
/// not an (nonexistent) equality predicate on a property named `limit`.
#[tokio::test]
async fn reserved_parameter_names_are_never_treated_as_queryables() {
    let app = build_queryable_params_app(vec![
        ("a", queryable_param_feature("a", "alpha", 10, true)),
        ("b", queryable_param_feature("b", "beta", 20, false)),
    ]);
    let response = get(&app, "/collections/demo/items?limit=1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["numberReturned"], 1);
}

/// A value that doesn't coerce to its queryable's declared type (`integer`
/// for `population`) is a 400 naming the offending parameter, not a 500 from
/// whatever a driver's own cast would have done with garbage input.
#[tokio::test]
async fn an_uncoercible_queryable_value_returns_400_naming_the_parameter() {
    let app =
        build_queryable_params_app(vec![("a", queryable_param_feature("a", "alpha", 10, true))]);
    let response = get(&app, "/collections/demo/items?population=notanumber").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    assert!(
        body["detail"].as_str().unwrap().contains("population"),
        "detail was: {}",
        body["detail"]
    );
}

/// A query parameter matching no declared queryable is "not specified in
/// the API definition" — OGC API Features Part 1 Core Requirement 8
/// (`/req/core/query-param-unknown`) — a 400 naming it, this crate's own
/// resolution of a question the queryables-as-query-parameters requirements
/// class itself leaves open.
#[tokio::test]
async fn an_unknown_query_parameter_returns_400_naming_it() {
    let app =
        build_queryable_params_app(vec![("a", queryable_param_feature("a", "alpha", 10, true))]);
    let response = get(&app, "/collections/demo/items?bogus=1").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    assert!(
        body["detail"].as_str().unwrap().contains("bogus"),
        "detail was: {}",
        body["detail"]
    );
}

/// A closed declared schema (`additional_properties: false`, `#44`)
/// narrows which names the query-parameter mechanism accepts to exactly the
/// declared set, the same way it already narrows `/queryables` and `filter=`
/// — `name` is a real attribute column but not declared, so it 400s exactly
/// like a genuinely nonexistent property name would; `population` is
/// declared and still works.
#[tokio::test]
async fn closed_schema_restricts_which_names_the_mechanism_accepts() {
    let app = build_queryable_params_app_with_closed_schema(vec![
        ("a", queryable_param_feature("a", "alpha", 10, true)),
        ("b", queryable_param_feature("b", "beta", 20, false)),
    ]);

    let undeclared = get(&app, "/collections/demo/items?name=alpha").await;
    assert_eq!(undeclared.status(), StatusCode::BAD_REQUEST);
    let body = body_json(undeclared).await;
    assert!(
        body["detail"].as_str().unwrap().contains("name"),
        "detail was: {}",
        body["detail"]
    );

    let declared = get(&app, "/collections/demo/items?population=20").await;
    assert_eq!(declared.status(), StatusCode::OK);
    let body = body_json(declared).await;
    assert_eq!(body["numberReturned"], 1);
    assert_eq!(body["features"][0]["id"], "b");
}

/// The same capability gate `filter=` already refuses through: a driver that
/// doesn't advertise `filter_capable()` refuses a bare queryable parameter
/// before ever attempting to compile or evaluate it, the identical 400 the
/// `filter=` path gives (`#33`).
#[tokio::test]
async fn queryable_param_against_a_driver_without_the_capability_returns_400() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items?population=5").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
}

// -- `#34` authorization policy layer ----------------------------------------
//
// These tests exercise `authorize_lane` (`handlers.rs`) directly through the
// real axum router `tellurion_features::router()` builds — no
// `tellurion-server` outer tenant gate involved, so they isolate exactly
// what this crate's own handlers are responsible for. Reuses
// `FilterCapableFeatureSource`/`matches_filter` (extended below for `And`)
// so the ABAC filter-merge tests prove a real narrowed query reaches the
// driver, not just that the handler returns 200.

fn matches_filter_and(feature: &Value, filter: Option<&Filter>) -> bool {
    match filter {
        Some(Filter::And(items)) => items
            .iter()
            .all(|item| matches_filter_and(feature, Some(item))),
        other => matches_filter(feature, other),
    }
}

struct PolicyFeatureSource {
    items: Vec<(String, Value)>,
}

#[async_trait::async_trait]
impl FeatureSource for PolicyFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        let matched: Vec<Value> = self
            .items
            .iter()
            .filter(|(_, v)| matches_filter_and(v, query.filter.as_ref()))
            .map(|(_, v)| v.clone())
            .collect();
        Ok(FeaturePage {
            number_matched: Some(matched.len() as u64),
            features_geojson: matched,
            next_token: None,
        })
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
    ) -> CoreResult<Option<Value>> {
        Ok(self
            .items
            .iter()
            .find(|(item_id, v)| item_id == id && matches_filter_and(v, filter))
            .map(|(_, v)| v.clone()))
    }

    fn filter_capable(&self) -> bool {
        true
    }
}

struct PolicyDriver {
    source: Arc<PolicyFeatureSource>,
}

impl StorageDriver for PolicyDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(FilterableCatalog)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(self.source.clone() as Arc<dyn FeatureSource>)
    }
}

struct PolicyFactory {
    source: Arc<PolicyFeatureSource>,
}

impl DriverFactory for PolicyFactory {
    fn name(&self) -> &str {
        "policy-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(PolicyDriver {
            source: self.source.clone(),
        }))
    }
}

/// In-memory `WriteSink` fixture (`#68`) — proves a granted write actually
/// reaches `WriteSink::apply` (a real 204, not merely "past the policy
/// checkpoint"). `PolicyDriver`/`policy-fake` above is deliberately
/// read-only (no `write_sink` override), so a write-lane test needs its own
/// storage entry routed via `routing: { write: ... }`.
///
/// `next_id` also backs `WriteSink::create` (`#88`): a monotonic counter
/// shared (via `Arc`) with every `PolicyWriteSink` `PolicyWriteDriver::
/// write_sink` hands out, so two `POST`s against the same app mint distinct,
/// increasing ids — the same proof `real_binary_writes_and_reads_back_an_
/// item_over_http`'s extension gives against a real PostGIS sequence, just
/// in-memory.
struct PolicyWriteSink {
    next_id: Arc<std::sync::atomic::AtomicI64>,
}

#[async_trait::async_trait]
impl WriteSink for PolicyWriteSink {
    async fn apply(
        &self,
        _collection: &CollectionDecl,
        _mutation: Mutation,
    ) -> CoreResult<Sequence> {
        Ok(Sequence(1))
    }

    /// `#150`: this fake stands in for a driver that CAN re-verify a
    /// precondition inside its own write, so the `If-Match`/`If-Unmodified-
    /// Since` tests below exercise the guard rather than the named refusal a
    /// driver without that capability now gives. The witness is a fixed
    /// token: nothing here is concurrent, and the atomic behaviour itself is
    /// proved against a real database in `tellurion-server`'s
    /// `optimistic_locking_binary.rs`, never against an in-process fake.
    async fn row_version(
        &self,
        _collection: &CollectionDecl,
        _feature_id: &str,
    ) -> CoreResult<Option<locking::RowVersion>> {
        Ok(Some(locking::RowVersion::new("v1")))
    }

    async fn apply_conditional(
        &self,
        _collection: &CollectionDecl,
        _mutation: Mutation,
        _requested_crs: RequestedCrs,
        _expected: &locking::RowVersion,
    ) -> CoreResult<Option<Sequence>> {
        Ok(Some(Sequence(1)))
    }

    async fn create(
        &self,
        _collection: &CollectionDecl,
        _feature: Value,
    ) -> CoreResult<(String, Sequence)> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok((id.to_string(), Sequence(id as u64)))
    }
}

struct PolicyWriteDriver {
    next_id: Arc<std::sync::atomic::AtomicI64>,
}

impl StorageDriver for PolicyWriteDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(EmptyCatalog)
    }

    fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
        Some(Arc::new(PolicyWriteSink {
            next_id: Arc::clone(&self.next_id),
        }))
    }
}

struct PolicyWriteFactory;

impl DriverFactory for PolicyWriteFactory {
    fn name(&self) -> &str {
        "policy-fake-write"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(PolicyWriteDriver {
            next_id: Arc::new(std::sync::atomic::AtomicI64::new(1)),
        }))
    }
}

/// Builds an `AppContext` from `config_yaml` — which must declare an
/// `auth:` section for any of the tests below to be meaningful, since a
/// `None` authorizer skips `authorize_lane` entirely (`#34`'s own
/// byte-for-byte-unchanged rule, already proven by every test above this
/// section, all of which build contexts with no `auth:` at all).
fn build_policy_ctx(config_yaml: &str, items: Vec<(&str, Value)>) -> Arc<AppContext> {
    let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
    config.validate().unwrap();

    let source = Arc::new(PolicyFeatureSource {
        items: items
            .into_iter()
            .map(|(id, v)| (id.to_string(), v))
            .collect(),
    });
    let mut registry = Registry::new();
    registry.register(Arc::new(PolicyFactory { source }));
    registry.register(Arc::new(PolicyWriteFactory));

    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    let authorizer = tellurion_core::build_authorizer(&config.auth)
        .expect("no bearer principal in this fixture reads a token_env");
    Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        authorizer,
        cache,
        style_store,
    ))
}

fn build_policy_app(config_yaml: &str, items: Vec<(&str, Value)>) -> axum::Router {
    tellurion_features::router().with_state(build_policy_ctx(config_yaml, items))
}

async fn get_with_bearer(
    app: &axum::Router,
    uri: impl AsRef<str>,
    token: Option<&str>,
) -> Response {
    let mut request = Request::builder().uri(uri.as_ref());
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// Config with `auth:` configured but no `policy:` section at all —
/// isolation (membership vs the resource's private-by-default visibility)
/// is the only thing this exercises; RBAC never activates.
const AUTH_ONLY_CONFIG: &str = r#"
storages: [ { id: main, driver: policy-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - { token: member-token, tenants: [public] }
"#;

#[tokio::test]
async fn no_credential_against_a_private_resource_is_401_when_auth_is_configured() {
    let app = build_policy_app(
        AUTH_ONLY_CONFIG,
        vec![("a", filterable_feature("a", "alpha"))],
    );
    let response = get_with_bearer(&app, "/collections/demo/items", None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_unrecognized_token_against_a_private_resource_is_403() {
    let app = build_policy_app(
        AUTH_ONLY_CONFIG,
        vec![("a", filterable_feature("a", "alpha"))],
    );
    let response = get_with_bearer(&app, "/collections/demo/items", Some("no-such-token")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_tenant_member_reads_unrestricted_with_no_policy_configured() {
    let app = build_policy_app(
        AUTH_ONLY_CONFIG,
        vec![
            ("a", filterable_feature("a", "alpha")),
            ("b", filterable_feature("b", "beta")),
        ],
    );
    let response = get_with_bearer(&app, "/collections/demo/items", Some("member-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["numberReturned"], 2,
        "membership alone must read unrestricted when no policy is configured: {body}"
    );
}

/// Config with `policy.roles` declared — RBAC is now active for every
/// tenant. `reader` grants unconditional access to lane `features`;
/// `filtered-reader` grants access narrowed by an ABAC claim substitution.
const RBAC_CONFIG: &str = r#"
storages: [ { id: main, driver: policy-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - { token: no-role-token, tenants: [public] }
    - { token: reader-token, tenants: [public], roles: { public: [reader] } }
    - token: filtered-token
      tenants: [public]
      roles: { public: [filtered-reader] }
      claims: { name: alpha }
policy:
  roles:
    - name: reader
      grants:
        - scope: { collections: [demo] }
          lanes: [features]
    - name: filtered-reader
      grants:
        - scope: { collections: [demo] }
          lanes: [features]
          filter: "name = {{claims.name}}"
"#;

#[tokio::test]
async fn rbac_active_denies_a_member_with_no_matching_role() {
    let app = build_policy_app(RBAC_CONFIG, vec![("a", filterable_feature("a", "alpha"))]);
    let response = get_with_bearer(&app, "/collections/demo/items", Some("no-role-token")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rbac_active_allows_an_unconditional_grant_unrestricted() {
    let app = build_policy_app(
        RBAC_CONFIG,
        vec![
            ("a", filterable_feature("a", "alpha")),
            ("b", filterable_feature("b", "beta")),
        ],
    );
    let response = get_with_bearer(&app, "/collections/demo/items", Some("reader-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["numberReturned"], 2);
}

#[tokio::test]
async fn abac_grant_filter_narrows_the_result_set_end_to_end() {
    let app = build_policy_app(
        RBAC_CONFIG,
        vec![
            ("a", filterable_feature("a", "alpha")),
            ("b", filterable_feature("b", "beta")),
        ],
    );
    let response = get_with_bearer(&app, "/collections/demo/items", Some("filtered-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["numberReturned"], 1,
        "the grant's claim-substituted filter must reach the driver: {body}"
    );
    assert_eq!(body["features"][0]["id"], "a");
}

#[tokio::test]
async fn abac_grant_filter_and_merges_with_a_user_supplied_filter() {
    let app = build_policy_app(
        RBAC_CONFIG,
        vec![
            ("a", filterable_feature("a", "alpha")),
            ("b", filterable_feature("b", "beta")),
        ],
    );
    // The grant already narrows to name = 'alpha'; a user filter for a
    // *different* name must AND-merge down to zero results, not silently
    // override the grant.
    let response = get_with_bearer(
        &app,
        "/collections/demo/items?filter=name%20%3D%20'beta'",
        Some("filtered-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["numberReturned"], 0,
        "grant filter (name=alpha) AND user filter (name=beta) must match nothing: {body}"
    );
}

#[tokio::test]
async fn a_subject_missing_the_grants_claim_is_denied() {
    // `no-role-token` has no role at all, so use a bespoke config where the
    // held role's only grant needs a claim the token never carries.
    let config = r#"
storages: [ { id: main, driver: policy-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - { token: no-claim-token, tenants: [public], roles: { public: [filtered-reader] } }
policy:
  roles:
    - name: filtered-reader
      grants:
        - scope: { collections: [demo] }
          lanes: [features]
          filter: "name = {{claims.name}}"
"#;
    let app = build_policy_app(config, vec![("a", filterable_feature("a", "alpha"))]);
    let response = get_with_bearer(&app, "/collections/demo/items", Some("no-claim-token")).await;
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "a grant whose claim placeholder the subject never carries must not be satisfied"
    );
}

/// `#34`: single-item GET now pushes the grant filter into the same
/// single-row query the items-list lane already narrows (`PolicyFeatureSource`
/// advertises `filter_capable() == true`, so a filtered-only grant is served,
/// not denied) — an item the filter matches comes back normally.
#[tokio::test]
async fn single_item_get_serves_an_item_the_grant_filter_matches() {
    let app = build_policy_app(
        RBAC_CONFIG,
        vec![
            ("a", filterable_feature("a", "alpha")),
            ("b", filterable_feature("b", "beta")),
        ],
    );
    // filtered-token's claim substitutes to `name = 'alpha'`; item "a" has
    // name "alpha", so it matches.
    let response = get_with_bearer(&app, "/collections/demo/items/a", Some("filtered-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// The filtered-single-item counterpart: an item that genuinely exists but
/// that the grant's filter excludes must come back 404 — indistinguishable
/// from an id that was never there at all, no existence leak.
#[tokio::test]
async fn single_item_get_404s_an_item_the_grant_filter_excludes() {
    let app = build_policy_app(
        RBAC_CONFIG,
        vec![
            ("a", filterable_feature("a", "alpha")),
            ("b", filterable_feature("b", "beta")),
        ],
    );
    // filtered-token's claim substitutes to `name = 'alpha'`; item "b" has
    // name "beta", so the grant filter excludes it even though it exists.
    let response = get_with_bearer(&app, "/collections/demo/items/b", Some("filtered-token")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A lane that cannot compile a filter at all still denies a filtered-only
/// grant outright, same as before `#34`'s single-item pushdown — never
/// silently serves unfiltered. `FakeFeatureSource` (this file's default
/// fixture) never overrides `filter_capable`, so it stays at the trait
/// default (`false`).
#[tokio::test]
async fn single_item_get_denies_a_filtered_only_grant_when_the_driver_cannot_filter() {
    let config = r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - token: filtered-token
      tenants: [public]
      roles: { public: [filtered-reader] }
      claims: { name: alpha }
policy:
  roles:
    - name: filtered-reader
      grants:
        - scope: { collections: [demo] }
          lanes: [features]
          filter: "name = {{claims.name}}"
"#;
    let config: AppConfig = serde_yaml::from_str(config).unwrap();
    config.validate().unwrap();
    let source = Arc::new(FakeFeatureSource {
        items: vec![("a".to_string(), feature("a"))],
    });
    let mut registry = Registry::new();
    registry.register(Arc::new(FakeFactory { source }));
    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    let authorizer = tellurion_core::build_authorizer(&config.auth)
        .expect("no bearer principal in this fixture reads a token_env");
    let ctx = Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        authorizer,
        cache,
        style_store,
    ));
    let app = tellurion_features::router().with_state(ctx);

    let response = get_with_bearer(&app, "/collections/demo/items/a", Some("filtered-token")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn single_item_get_allows_a_subject_with_an_unconditional_grant() {
    let app = build_policy_app(RBAC_CONFIG, vec![("a", filterable_feature("a", "alpha"))]);
    let response = get_with_bearer(&app, "/collections/demo/items/a", Some("reader-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// Config with `policy:` roles declared but no `auth:` section at all. With
/// no `auth:`, `build_authorizer` returns `None` (`build_authorizer_is_none_
/// for_the_default_permissive_config`) — there is no way to resolve a
/// `Subject` at all, so `authorize_write_lane` (like every read lane's own
/// `authorize_lane`) skips straight to unrestricted, exactly as it does for
/// every other lane without `auth:`. Declaring `policy:` roles alone, with
/// nothing to authenticate a subject against, does not turn access control
/// on — see `writes_with_policy_roles_but_no_auth_configured_reach_the_write_lane`.
const POLICY_ONLY_CONFIG: &str = r#"
storages: [ { id: main, driver: policy-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
policy:
  roles:
    - name: reader
      grants:
        - scope: { collections: [demo] }
          lanes: [features]
"#;

async fn send_write(
    app: &axum::Router,
    method: &str,
    uri: impl AsRef<str>,
    token: Option<&str>,
) -> Response {
    let mut request = Request::builder().method(method).uri(uri.as_ref());
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(r#"{"type":"Feature","properties":{}}"#))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn put_item_with_auth_configured_and_no_credential_is_401() {
    let app = build_policy_app(AUTH_ONLY_CONFIG, vec![]);
    let response = send_write(&app, "PUT", "/collections/demo/items/x", None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn put_item_with_auth_configured_and_no_policy_reaches_the_write_lane_for_a_member() {
    // `AUTH_ONLY_CONFIG` declares `auth:` but no `policy:` roles at all, so
    // RBAC never activates (`#34` directive 10) — isolation alone passes for
    // a tenant member, and the request falls through to `resolve_write`,
    // whose capability refusal for this file's read-only `policy-fake`
    // driver (no `routing.write`, no `write_sink`) is the familiar 404, not
    // a 401/403. This is the flip of the old fail-closed gate's behavior,
    // now that `PolicyLane::Write` exists to actually grant through.
    let app = build_policy_app(AUTH_ONLY_CONFIG, vec![]);
    let response = send_write(
        &app,
        "PUT",
        "/collections/demo/items/x",
        Some("member-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_item_with_auth_configured_and_no_credential_is_401() {
    let app = build_policy_app(AUTH_ONLY_CONFIG, vec![]);
    let response = send_write(&app, "DELETE", "/collections/demo/items/x", None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn delete_item_under_rbac_refuses_a_role_holding_only_a_read_grant() {
    // `reader`'s only grant covers lane `features`, never `write` — a
    // read-only grant on the very same collection scope must not be
    // implied to cover writes (`#68`'s "never implied" requirement).
    let app = build_policy_app(RBAC_CONFIG, vec![("a", filterable_feature("a", "alpha"))]);
    let response = send_write(
        &app,
        "DELETE",
        "/collections/demo/items/a",
        Some("reader-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn writes_with_policy_roles_but_no_auth_configured_reach_the_write_lane() {
    // No `auth:` at all means no authorizer exists to resolve a `Subject`
    // from, so `policy:` roles alone (with nothing to authenticate against)
    // never activate access control — same as `writes_without_any_access_
    // control_still_reach_the_write_lane` below, just with `policy:` present.
    let app = build_policy_app(POLICY_ONLY_CONFIG, vec![]);
    let response = send_write(&app, "PUT", "/collections/demo/items/x", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn writes_without_any_access_control_still_reach_the_write_lane() {
    // No `auth:`, no `policy:` — the gate is a no-op and the request falls
    // through to write-lane resolution, whose capability refusal for this
    // file's read-only fake driver is the familiar 404 (NOT a 401/403).
    let app = build_app(vec![]);
    let response = send_write(&app, "PUT", "/collections/demo/items/x", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Config declaring a real `write`-routed, `WriteSink`-capable storage
/// (`policy-fake-write`) alongside two collections, `demo` and `other`, so a
/// write grant's collection scope can be exercised: `writer` covers `demo`
/// only, `other-writer` covers `other` only.
const WRITE_CONFIG: &str = r#"
storages:
  - { id: main, driver: policy-fake, url_env: DATABASE_URL }
  - { id: writable, driver: policy-fake-write, url_env: DATABASE_URL }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { write: writable }
  - id: other
    catalog: default
    storage: main
    table: other
    geometry: geom
    pk: id
    routing: { write: writable }
auth:
  bearer_tokens:
    - token: writer-token
      tenants: [public]
      roles: { public: [writer] }
    - token: other-writer-token
      tenants: [public]
      roles: { public: [other-writer] }
policy:
  roles:
    - name: writer
      grants:
        - scope: { collections: [demo] }
          lanes: [write]
    - name: other-writer
      grants:
        - scope: { collections: [other] }
          lanes: [write]
"#;

#[tokio::test]
async fn a_write_grant_allows_a_member_and_the_write_reaches_the_capability_layer() {
    let app = build_policy_app(WRITE_CONFIG, vec![]);
    let response = send_write(
        &app,
        "PUT",
        "/collections/demo/items/x",
        Some("writer-token"),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "a granted write must reach WriteSink::apply and actually succeed"
    );
}

#[tokio::test]
async fn a_write_grant_scoped_to_a_different_collection_is_403() {
    // other-writer-token's only grant covers 'other', not 'demo'.
    let app = build_policy_app(WRITE_CONFIG, vec![]);
    let response = send_write(
        &app,
        "PUT",
        "/collections/demo/items/x",
        Some("other-writer-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// `#34`: `GET /collections` omits a collection the subject isn't authorized
/// to see, mirroring `tellurion_stac::handlers::list_collections`'s own
/// rule — a private collection is not merely refused on direct access, it's
/// not advertised in the listing at all.
#[tokio::test]
async fn list_collections_omits_a_collection_the_subject_cannot_see() {
    let config = r#"
storages: [ { id: main, driver: policy-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
  - id: hidden
    catalog: default
    storage: main
    table: hidden
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - token: reader-token
      tenants: [public]
      roles: { public: [reader] }
policy:
  roles:
    - name: reader
      grants:
        - scope: { collections: [demo] }
          lanes: [features]
"#;
    let app = build_policy_app(config, vec![("a", filterable_feature("a", "alpha"))]);
    let response = get_with_bearer(&app, "/collections", Some("reader-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let ids: Vec<&str> = body["collections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["demo"],
        "the subject's grant only covers 'demo'; 'hidden' must be omitted entirely: {body}"
    );
}

// -- write-lane body cap (`#91`) ---------------------------------------------
//
// No `auth:`/`policy:` at all — the write lane's own gate is a no-op, so
// these isolate `settings.max_request_body_bytes` enforcement from access
// control. `send_write`'s fixed body (`{"type":"Feature","properties":{}}"`)
// is exactly 34 bytes, which is what makes it useful as the at-limit case
// below.

/// A real write-capable collection (`demo` -> `writable`) with a
/// platform-level `max_request_body_bytes` cap sized to `send_write`'s own
/// fixed body exactly (34 bytes) — over that cap must refuse, at it must
/// succeed.
const BODY_CAP_CONFIG: &str = r#"
storages:
  - { id: main, driver: policy-fake, url_env: DATABASE_URL }
  - { id: writable, driver: policy-fake-write, url_env: DATABASE_URL }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { write: writable }
settings:
  max_request_body_bytes: 34
"#;

async fn send_write_with_body(
    app: &axum::Router,
    method: &str,
    uri: impl AsRef<str>,
    body: &'static str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri.as_ref())
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn put_item_over_the_configured_body_cap_is_refused_by_name() {
    let app = build_policy_app(BODY_CAP_CONFIG, vec![]);
    // 35 bytes — one over the configured 34-byte cap. Not valid GeoJSON, but
    // the cap must refuse it before the body is ever parsed.
    let response = send_write_with_body(
        &app,
        "PUT",
        "/collections/demo/items/x",
        r#"{"type":"Feature","properties":{}}x"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        tellurion_features::PROBLEM_JSON
    );
    let body = body_json(response).await;
    assert_eq!(body["code"], "PayloadTooLarge");
    assert!(
        body["detail"].as_str().unwrap().contains("34"),
        "the refusal must name the configured limit: {body}"
    );
}

#[tokio::test]
async fn put_item_at_the_configured_body_cap_is_accepted() {
    let app = build_policy_app(BODY_CAP_CONFIG, vec![]);
    let response = send_write(&app, "PUT", "/collections/demo/items/x", None).await;
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "a body exactly at the configured cap must be accepted"
    );
}

#[tokio::test]
async fn put_item_body_cap_resolves_from_config() {
    // Same collection shape as `BODY_CAP_CONFIG`, but capped below even the
    // fixed 34-byte body `send_write` sends — proves the enforced limit is
    // the configured value, not a hardcoded constant (contrast with
    // `a_write_grant_allows_a_member_and_the_write_reaches_the_capability_
    // layer`, which sends the identical body against `WRITE_CONFIG`'s
    // unconfigured (module-default) cap and succeeds).
    let low_cap_config = r#"
storages:
  - { id: main, driver: policy-fake, url_env: DATABASE_URL }
  - { id: writable, driver: policy-fake-write, url_env: DATABASE_URL }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { write: writable }
settings:
  max_request_body_bytes: 10
"#;
    let app = build_policy_app(low_cap_config, vec![]);
    let response = send_write(&app, "PUT", "/collections/demo/items/x", None).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

// -- POST create (`#88`) -----------------------------------------------------
//
// Reuses `AUTH_ONLY_CONFIG`/`RBAC_CONFIG`/`WRITE_CONFIG`/`BODY_CAP_CONFIG`
// from the PUT/DELETE sections above: `create_item` shares the same auth
// checkpoint and body cap as `put_item`, so these mirror those tests'
// shape one-for-one, diverging only where create's own behavior differs
// (a 201 + `Location` instead of a 204, and distinct minted ids).

#[tokio::test]
async fn post_item_with_auth_configured_and_no_credential_is_401() {
    let app = build_policy_app(AUTH_ONLY_CONFIG, vec![]);
    let response = send_write(&app, "POST", "/collections/demo/items", None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_item_with_auth_configured_and_no_policy_reaches_the_write_lane_for_a_member() {
    // Same reasoning as `put_item_with_auth_configured_and_no_policy_
    // reaches_the_write_lane_for_a_member`: `AUTH_ONLY_CONFIG`'s `demo`
    // routes to `policy-fake`, which advertises no `write_sink` at all, so
    // a member falls through isolation straight into `resolve_write`'s own
    // 404 capability refusal.
    let app = build_policy_app(AUTH_ONLY_CONFIG, vec![]);
    let response = send_write(
        &app,
        "POST",
        "/collections/demo/items",
        Some("member-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn posts_without_any_access_control_still_reach_the_write_lane() {
    // `build_app`'s `demo` collection declares no `routing.write` at all —
    // the driver-lacking-write refusal `resolve_write` gives every write
    // verb, unchanged for POST.
    let app = build_app(vec![]);
    let response = send_write(&app, "POST", "/collections/demo/items", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_write_grant_allows_a_member_and_post_creates_returns_201_with_location() {
    let app = build_policy_app(WRITE_CONFIG, vec![]);
    let response = send_write(
        &app,
        "POST",
        "/collections/demo/items",
        Some("writer-token"),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "a granted create must reach WriteSink::create and actually succeed"
    );
    let location = response
        .headers()
        .get(header::LOCATION)
        .expect("a create response carries a Location header")
        .to_str()
        .expect("Location is valid ASCII");
    assert_eq!(location, "/collections/demo/items/1");
}

#[tokio::test]
async fn configured_public_base_qualifies_the_created_item_location() {
    let config = format!(
        "server: {{ public_base_url: 'https://maps.example.test/tellurion/' }}\n{WRITE_CONFIG}"
    );
    let app = axum::Router::new().nest(
        "/{tenant}/features/catalogs/{catalog}",
        build_policy_app(&config, vec![]),
    );

    let response = send_write(
        &app,
        "POST",
        "/public/features/catalogs/default/collections/demo/items",
        Some("writer-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .expect("a create response carries a Location header"),
        "https://maps.example.test/tellurion/public/features/catalogs/default/collections/demo/items/1"
    );
}

#[tokio::test]
async fn two_creates_mint_distinct_monotonic_ids() {
    let app = build_policy_app(WRITE_CONFIG, vec![]);

    let first = send_write(
        &app,
        "POST",
        "/collections/demo/items",
        Some("writer-token"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first_location = first
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let second = send_write(
        &app,
        "POST",
        "/collections/demo/items",
        Some("writer-token"),
    )
    .await;
    assert_eq!(second.status(), StatusCode::CREATED);
    let second_location = second
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    assert_ne!(
        first_location, second_location,
        "two creates must mint distinct ids"
    );
    assert_eq!(first_location, "/collections/demo/items/1");
    assert_eq!(
        second_location, "/collections/demo/items/2",
        "ids must increase monotonically"
    );
}

#[tokio::test]
async fn a_write_grant_scoped_to_a_different_collection_is_403_for_post() {
    // other-writer-token's only grant covers 'other', not 'demo'.
    let app = build_policy_app(WRITE_CONFIG, vec![]);
    let response = send_write(
        &app,
        "POST",
        "/collections/demo/items",
        Some("other-writer-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn post_item_over_the_configured_body_cap_is_refused_by_name() {
    let app = build_policy_app(BODY_CAP_CONFIG, vec![]);
    // 35 bytes — one over the configured 34-byte cap, same as the PUT-lane
    // test this mirrors.
    let response = send_write_with_body(
        &app,
        "POST",
        "/collections/demo/items",
        r#"{"type":"Feature","properties":{}}x"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        tellurion_features::PROBLEM_JSON
    );
    let body = body_json(response).await;
    assert_eq!(body["code"], "PayloadTooLarge");
    assert!(
        body["detail"].as_str().unwrap().contains("34"),
        "the refusal must name the configured limit: {body}"
    );
}

#[tokio::test]
async fn post_item_at_the_configured_body_cap_is_accepted() {
    let app = build_policy_app(BODY_CAP_CONFIG, vec![]);
    let response = send_write(&app, "POST", "/collections/demo/items", None).await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "a body exactly at the configured cap must be accepted"
    );
}

/// A `Uuid` id-type collection's `POST` reaches `WriteSink::create` exactly
/// like an `Integer` one does (`#87`): `create_item` no longer inspects
/// `CollectionDecl::id_type` at all, so whether a create is servable is
/// entirely the driver's own call — this fake `WriteSink` always supports
/// `create`, so a `uuid`-declared collection succeeds through it the same
/// way `post_item_at_the_configured_body_cap_is_accepted` does for the
/// default `Integer` id-type. The real PostGIS driver's own id-type-vs-
/// physical-pk-type mismatch refusal is a `tellurion-postgis` live test, not
/// reachable through this fake fixture.
const UUID_ID_TYPE_CONFIG: &str = r#"
storages:
  - { id: main, driver: policy-fake, url_env: DATABASE_URL }
  - { id: writable, driver: policy-fake-write, url_env: DATABASE_URL }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    id_type: uuid
    routing: { write: writable }
"#;

#[tokio::test]
async fn post_item_against_a_uuid_id_type_collection_reaches_write_sink_create() {
    let app = build_policy_app(UUID_ID_TYPE_CONFIG, vec![]);
    let response = send_write(&app, "POST", "/collections/demo/items", None).await;
    assert_eq!(
        response.status(),
        StatusCode::CREATED,
        "a uuid id_type collection must reach WriteSink::create, not be refused at the handler"
    );
    assert!(
        response.headers().get(header::LOCATION).is_some(),
        "a create response carries a Location header regardless of id_type"
    );
}

// -- Part 4: `supportsNonAutogeneratedResourceIds` ---------------------------
//
// OGC API Features — Part 4, Requirement 38 (`/req/features/collection-
// endpoint`): a collection whose `PUT` can create a new item with a
// caller-supplied id must say so on its own Collection representation.
// `BODY_CAP_CONFIG` (defined above, in the body-cap section) already
// declares exactly that shape: `demo` routed to `writable`, a real
// `WriteSink`. `DEMO_CONFIG` (via `build_app`) is the contrasting case: no
// `routing.write` at all.

#[tokio::test]
async fn list_collections_declares_the_property_when_put_can_create() {
    let app = build_policy_app(BODY_CAP_CONFIG, vec![]);
    let response = get(&app, "/collections").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["collections"][0]["supportsNonAutogeneratedResourceIds"], true,
        "a collection with a real write lane must declare this: {body}"
    );
}

#[tokio::test]
async fn get_collection_declares_the_property_when_put_can_create() {
    let app = build_policy_app(BODY_CAP_CONFIG, vec![]);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["supportsNonAutogeneratedResourceIds"], true,
        "a collection with a real write lane must declare this: {body}"
    );
}

// A collection with no `routing.write` at all never serializes this
// property either way (`Option::None`, not `false`) — see
// `CollectionSummary::supports_non_autogenerated_resource_ids`'s own doc
// for the "never fabricated" rule this follows. Not exercised as its own
// regression test here: a JSON body missing a key is indistinguishable
// from one that was never taught to emit that key at all, so a test
// asserting absence can't tell the two apart — the two `_declares_`
// tests above are what actually exercise this field's logic.

// -- Part 4: `If-Match` on `PUT` to a missing resource (Requirement 12) -----
//
// `/req/create-replace-delete/put-rid-exception` clause B: a `PUT` carrying
// `If-Match` must not be treated as an insert when the target doesn't exist
// — `412`, not a silent create. `BODY_CAP_CONFIG`'s `demo` (`main` for
// reads, `writable` for writes) is reused unchanged: `PolicyFeatureSource`
// seeded with no items reports every id as absent, exactly what this guard
// needs to exercise.

async fn send_put_with_if_match(app: &axum::Router, uri: impl AsRef<str>) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(uri.as_ref())
                .header(header::IF_MATCH, "\"anything\"")
                .body(Body::from(r#"{"type":"Feature","properties":{}}"#))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn put_with_if_match_against_a_missing_resource_is_412() {
    let app = build_policy_app(BODY_CAP_CONFIG, vec![]);
    let response = send_put_with_if_match(&app, "/collections/demo/items/does-not-exist").await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        tellurion_features::PROBLEM_JSON
    );
    let body = body_json(response).await;
    assert_eq!(body["code"], "PreconditionFailed");
}

/// `#107`: `send_put_with_if_match` sends the bogus literal `"anything"` —
/// a resource that already exists now really compares `If-Match` against
/// its current ETag (`req/optimistic-locking-etags`), so an arbitrary,
/// stale value is correctly refused with `412` — this test used to assert
/// the opposite (any `If-Match` against an existing resource always
/// succeeded) back when this crate only implemented the narrower
/// existence-only guard; see `put_with_the_current_etag_as_if_match_
/// succeeds` immediately below for the positive case this negative one is
/// now paired with.
#[tokio::test]
async fn put_with_a_stale_if_match_against_an_existing_resource_is_412() {
    let app = build_policy_app(
        BODY_CAP_CONFIG,
        vec![("x", filterable_feature("x", "alpha"))],
    );
    let response = send_put_with_if_match(&app, "/collections/demo/items/x").await;
    assert_eq!(
        response.status(),
        StatusCode::PRECONDITION_FAILED,
        "a stale/bogus If-Match against an existing resource must be refused"
    );
    let body = body_json(response).await;
    assert_eq!(body["code"], "PreconditionFailed");
}

/// The positive counterpart: `If-Match` carrying the target's OWN, current
/// ETag (computed exactly the way `tellurion_core::locking::
/// compute_feature_etag` — the same function `get_item`'s `ETag` response
/// header and this guard both call — hashes it) must proceed, not refuse.
/// `PolicyFeatureSource::item` returns `filterable_feature("x", "alpha")`
/// completely unchanged (see that fixture's own `impl FeatureSource`), so
/// hashing it directly here is exactly what the guard itself computes.
#[tokio::test]
async fn put_with_the_current_etag_as_if_match_succeeds() {
    let app = build_policy_app(
        BODY_CAP_CONFIG,
        vec![("x", filterable_feature("x", "alpha"))],
    );
    let etag = locking::compute_feature_etag(&filterable_feature("x", "alpha"));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/collections/demo/items/x")
                .header(header::IF_MATCH, etag)
                .body(Body::from(r#"{"type":"Feature","properties":{}}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "If-Match carrying the resource's own current ETag must not be refused"
    );
}

#[tokio::test]
async fn put_without_if_match_against_a_missing_resource_still_creates() {
    // No `If-Match` at all: the ordinary upsert-by-caller-supplied-id
    // behavior is unchanged by this guard.
    let app = build_policy_app(BODY_CAP_CONFIG, vec![]);
    let response = send_write(&app, "PUT", "/collections/demo/items/does-not-exist", None).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

// -- `Content-Crs` on write (OGC API Features Part 4, `/req/features/
// content-crs-header`, `/req/features/crs-other-crs`) -----------------------
//
// `table`/`geometry`/`pk` are omitted from `demo`'s config below, same as
// `CRS_CONFIG` above — this makes `Router::effective_decl` actually derive
// the descriptor (and carry `CrsWriteCatalog`'s `srid` onto the served
// decl) instead of taking the fully-overridden fast path, whose `srid`
// always stays `None`. `storage: main` anchors descriptor derivation
// (`RoutedCollection::anchor` always reads the *features* lane, defaulting
// to the single `storage:` when unrouted); `routing: { write: writable }`
// sends the actual mutation to `CrsWriteSink`, a second, independent
// storage — mirroring `WRITE_CONFIG`'s own read/write storage split.

/// A `CatalogSource` reporting a configurable storage SRID for `demo` — the
/// write-lane counterpart of `CrsCatalog` above.
struct CrsWriteCatalog {
    srid: i32,
}

#[async_trait::async_trait]
impl CatalogSource for CrsWriteCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: "demo".to_string(),
            geometry_column: Some("geom".to_string()),
            primary_key: Some("id".to_string()),
            srid: Some(self.srid),
            geometry_type: None,
        }])
    }
}

struct CrsWriteCatalogDriver {
    srid: i32,
}

impl StorageDriver for CrsWriteCatalogDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(CrsWriteCatalog { srid: self.srid })
    }
}

struct CrsWriteCatalogFactory {
    srid: i32,
}

impl DriverFactory for CrsWriteCatalogFactory {
    fn name(&self) -> &str {
        "crs-write-catalog-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(CrsWriteCatalogDriver { srid: self.srid }))
    }
}

/// Records the `RequestedCrs` `apply_with_crs`/`create_with_crs` actually
/// received — proves `write_handlers.rs` genuinely threads a resolved
/// `Content-Crs` through to the driver, not merely that it decided whether
/// to allow the request through. Shared via `Arc` so a test can inspect it
/// after the HTTP response comes back.
#[derive(Default)]
struct CrsWriteLog {
    last_apply: std::sync::Mutex<Option<RequestedCrs>>,
    last_create: std::sync::Mutex<Option<RequestedCrs>>,
}

/// A `WriteSink` whose reprojection capability is configurable — the
/// write-lane counterpart of `CrsFeatureSource` above (same `crs_capable`-
/// gated shape): `crs_capable: false` proves a genuinely valid, non-default
/// `Content-Crs` (this collection's own storage CRS, not just an
/// unrecognized one) still refuses against a driver that can't reproject;
/// `crs_capable: true` proves the same request succeeds, and that the
/// resolved CRS actually reached the driver.
struct CrsWriteSink {
    crs_capable: bool,
    log: Arc<CrsWriteLog>,
    next_id: Arc<std::sync::atomic::AtomicI64>,
}

#[async_trait::async_trait]
impl WriteSink for CrsWriteSink {
    async fn apply(
        &self,
        _collection: &CollectionDecl,
        _mutation: Mutation,
    ) -> CoreResult<Sequence> {
        Ok(Sequence(1))
    }

    async fn create(
        &self,
        _collection: &CollectionDecl,
        _feature: Value,
    ) -> CoreResult<(String, Sequence)> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok((id.to_string(), Sequence(id as u64)))
    }

    fn crs_capable(&self) -> bool {
        self.crs_capable
    }

    async fn apply_with_crs(
        &self,
        collection: &CollectionDecl,
        mutation: Mutation,
        requested_crs: RequestedCrs,
    ) -> CoreResult<Sequence> {
        *self.log.last_apply.lock().unwrap() = Some(requested_crs);
        self.apply(collection, mutation).await
    }

    async fn create_with_crs(
        &self,
        collection: &CollectionDecl,
        feature: Value,
        requested_crs: RequestedCrs,
    ) -> CoreResult<(String, Sequence)> {
        *self.log.last_create.lock().unwrap() = Some(requested_crs);
        self.create(collection, feature).await
    }
}

struct CrsWriteSinkDriver {
    crs_capable: bool,
    log: Arc<CrsWriteLog>,
}

impl StorageDriver for CrsWriteSinkDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(EmptyCatalog)
    }

    fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
        Some(Arc::new(CrsWriteSink {
            crs_capable: self.crs_capable,
            log: Arc::clone(&self.log),
            next_id: Arc::new(std::sync::atomic::AtomicI64::new(1)),
        }))
    }
}

struct CrsWriteSinkFactory {
    crs_capable: bool,
    log: Arc<CrsWriteLog>,
}

impl DriverFactory for CrsWriteSinkFactory {
    fn name(&self) -> &str {
        "crs-write-sink-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(CrsWriteSinkDriver {
            crs_capable: self.crs_capable,
            log: Arc::clone(&self.log),
        }))
    }
}

fn build_crs_write_app(storage_srid: i32, crs_capable: bool) -> (axum::Router, Arc<CrsWriteLog>) {
    const CRS_WRITE_CONFIG: &str = r#"
storages:
  - { id: main, driver: crs-write-catalog-fake, url_env: DATABASE_URL }
  - { id: writable, driver: crs-write-sink-fake, url_env: DATABASE_URL }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    routing: { write: writable }
"#;
    let config: AppConfig = serde_yaml::from_str(CRS_WRITE_CONFIG).unwrap();
    config.validate().unwrap();

    let log = Arc::new(CrsWriteLog::default());
    let mut registry = Registry::new();
    registry.register(Arc::new(CrsWriteCatalogFactory { srid: storage_srid }));
    registry.register(Arc::new(CrsWriteSinkFactory {
        crs_capable,
        log: Arc::clone(&log),
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
    (tellurion_features::router().with_state(ctx), log)
}

async fn send_write_with_content_crs(
    app: &axum::Router,
    method: &str,
    uri: impl AsRef<str>,
    content_crs: Option<&str>,
) -> Response {
    let mut request = Request::builder().method(method).uri(uri.as_ref());
    if let Some(value) = content_crs {
        request = request.header("content-crs", value);
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(r#"{"type":"Feature","properties":{}}"#))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn put_item_with_no_content_crs_header_reaches_the_sink_with_omitted_crs() {
    // Absent header means CRS84 (Requirement 41) — today's behavior, byte-
    // for-byte, even against a driver that can't reproject at all.
    let (app, log) = build_crs_write_app(3857, false);
    let response =
        send_write_with_content_crs(&app, "PUT", "/collections/demo/items/x", None).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(*log.last_apply.lock().unwrap(), Some(RequestedCrs::Omitted));
}

#[tokio::test]
async fn put_item_with_explicit_crs84_is_accepted_even_by_a_non_reprojecting_sink() {
    let (app, log) = build_crs_write_app(3857, false);
    let response = send_write_with_content_crs(
        &app,
        "PUT",
        "/collections/demo/items/x",
        Some("<http://www.opengis.net/def/crs/OGC/1.3/CRS84>"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(*log.last_apply.lock().unwrap(), Some(RequestedCrs::Crs84));
}

#[tokio::test]
async fn put_item_with_the_storage_crs_against_a_capable_sink_reprojects_and_succeeds() {
    let (app, log) = build_crs_write_app(3857, true);
    let response = send_write_with_content_crs(
        &app,
        "PUT",
        "/collections/demo/items/x",
        Some("<http://www.opengis.net/def/crs/EPSG/0/3857>"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(*log.last_apply.lock().unwrap(), Some(RequestedCrs::Storage));
}

#[tokio::test]
async fn put_item_with_the_storage_crs_against_a_non_capable_sink_returns_400() {
    let (app, log) = build_crs_write_app(3857, false);
    let response = send_write_with_content_crs(
        &app,
        "PUT",
        "/collections/demo/items/x",
        Some("<http://www.opengis.net/def/crs/EPSG/0/3857>"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    assert!(
        body["detail"].as_str().unwrap().contains("3857"),
        "the refusal must name the refused crs: {body}"
    );
    // Never reached the sink at all — refused before the write.
    assert!(log.last_apply.lock().unwrap().is_none());
}

#[tokio::test]
async fn put_item_with_an_unsupported_crs_returns_400() {
    let (app, log) = build_crs_write_app(3857, true);
    let response = send_write_with_content_crs(
        &app,
        "PUT",
        "/collections/demo/items/x",
        Some("<http://www.opengis.net/def/crs/EPSG/0/4326>"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(log.last_apply.lock().unwrap().is_none());
}

#[tokio::test]
async fn put_item_with_a_malformed_content_crs_header_returns_400() {
    let (app, log) = build_crs_write_app(3857, true);
    let response = send_write_with_content_crs(
        &app,
        "PUT",
        "/collections/demo/items/x",
        // No angle brackets — malformed per Requirement 40's own
        // `"<" URI-reference ">"` shape.
        Some("http://www.opengis.net/def/crs/EPSG/0/3857"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(log.last_apply.lock().unwrap().is_none());
}

#[tokio::test]
async fn post_item_with_no_content_crs_header_reaches_the_sink_with_omitted_crs() {
    let (app, log) = build_crs_write_app(3857, false);
    let response = send_write_with_content_crs(&app, "POST", "/collections/demo/items", None).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        *log.last_create.lock().unwrap(),
        Some(RequestedCrs::Omitted)
    );
}

#[tokio::test]
async fn post_item_with_the_storage_crs_against_a_capable_sink_reprojects_and_succeeds() {
    let (app, log) = build_crs_write_app(3857, true);
    let response = send_write_with_content_crs(
        &app,
        "POST",
        "/collections/demo/items",
        Some("<http://www.opengis.net/def/crs/EPSG/0/3857>"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        *log.last_create.lock().unwrap(),
        Some(RequestedCrs::Storage)
    );
}

#[tokio::test]
async fn post_item_with_the_storage_crs_against_a_non_capable_sink_returns_400() {
    let (app, log) = build_crs_write_app(3857, false);
    let response = send_write_with_content_crs(
        &app,
        "POST",
        "/collections/demo/items",
        Some("<http://www.opengis.net/def/crs/EPSG/0/3857>"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(log.last_create.lock().unwrap().is_none());
}

// -- Optimistic Locking: Timestamps (`#107`, `req/optimistic-locking-
// timestamps`) -------------------------------------------------------------
//
// A dedicated fixture, separate from `PolicyFeatureSource`/`PolicyWriteSink`
// above: this collection declares `modified_column: updated_at`, and its
// write sink declares the ETags class too, so `/collections/demo` can prove
// both classes wire up together on one collection. The seeded item carries
// a real `updated_at` property for `Last-Modified`/`If-Unmodified-Since` to
// read.

const TIMESTAMPS_CONFIG: &str = r#"
storages:
  - { id: main, driver: timestamps-fake, url_env: DATABASE_URL }
  - { id: writable, driver: timestamps-fake-write, url_env: DATABASE_URL }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    modified_column: updated_at
    routing: { write: writable }
"#;

fn timestamped_feature(id: &str, updated_at: &str) -> Value {
    json!({
        "type": "Feature",
        "id": id,
        "geometry": null,
        "properties": { "name": "alpha", "updated_at": updated_at }
    })
}

struct TimestampsFeatureSource {
    items: Vec<(String, Value)>,
}

#[async_trait::async_trait]
impl FeatureSource for TimestampsFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        _query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        unreachable!("not exercised by the Timestamps tests")
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

struct TimestampsDriver {
    source: Arc<TimestampsFeatureSource>,
}

impl StorageDriver for TimestampsDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(EmptyCatalog)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(self.source.clone() as Arc<dyn FeatureSource>)
    }
}

struct TimestampsFactory {
    source: Arc<TimestampsFeatureSource>,
}

impl DriverFactory for TimestampsFactory {
    fn name(&self) -> &str {
        "timestamps-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(TimestampsDriver {
            source: self.source.clone(),
        }))
    }
}

/// Declares the ETags class too (`#107`), alongside the per-collection
/// Timestamps declaration `modified_column` above adds — proves
/// `/collections/demo` can carry both URIs together on one collection.
struct TimestampsWriteSink;

#[async_trait::async_trait]
impl WriteSink for TimestampsWriteSink {
    async fn apply(
        &self,
        _collection: &CollectionDecl,
        _mutation: Mutation,
    ) -> CoreResult<Sequence> {
        Ok(Sequence(1))
    }

    fn locking_conformance_classes(&self) -> Vec<&'static str> {
        vec![locking::OPTIMISTIC_LOCKING_ETAGS_CLASS]
    }

    /// `#150`: the atomic half its declaration above now has to be backed
    /// by — see `PolicyWriteSink`'s own note for why a fake may answer with
    /// a fixed token.
    async fn row_version(
        &self,
        _collection: &CollectionDecl,
        _feature_id: &str,
    ) -> CoreResult<Option<locking::RowVersion>> {
        Ok(Some(locking::RowVersion::new("v1")))
    }

    async fn apply_conditional(
        &self,
        _collection: &CollectionDecl,
        _mutation: Mutation,
        _requested_crs: RequestedCrs,
        _expected: &locking::RowVersion,
    ) -> CoreResult<Option<Sequence>> {
        Ok(Some(Sequence(1)))
    }
}

struct TimestampsWriteDriver;

impl StorageDriver for TimestampsWriteDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(EmptyCatalog)
    }

    fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
        Some(Arc::new(TimestampsWriteSink))
    }
}

struct TimestampsWriteFactory;

impl DriverFactory for TimestampsWriteFactory {
    fn name(&self) -> &str {
        "timestamps-fake-write"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(TimestampsWriteDriver))
    }
}

fn build_timestamps_app(items: Vec<(&str, Value)>) -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(TIMESTAMPS_CONFIG).unwrap();
    config.validate().unwrap();

    let source = Arc::new(TimestampsFeatureSource {
        items: items
            .into_iter()
            .map(|(id, v)| (id.to_string(), v))
            .collect(),
    });
    let mut registry = Registry::new();
    registry.register(Arc::new(TimestampsFactory { source }));
    registry.register(Arc::new(TimestampsWriteFactory));

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
    tellurion_features::router().with_state(ctx)
}

async fn send_write_with_headers(
    app: &axum::Router,
    method: &str,
    uri: impl AsRef<str>,
    headers: &[(axum::http::HeaderName, &str)],
) -> Response {
    let mut request = Request::builder().method(method).uri(uri.as_ref());
    for (name, value) in headers {
        request = request.header(name, *value);
    }
    app.clone()
        .oneshot(
            request
                .body(Body::from(r#"{"type":"Feature","properties":{}}"#))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn single_item_get_carries_last_modified_when_a_modified_column_is_declared() {
    let app = build_timestamps_app(vec![(
        "x",
        timestamped_feature("x", "2024-06-01T00:00:00Z"),
    )]);
    let response = get(&app, "/collections/demo/items/x").await;
    assert_eq!(response.status(), StatusCode::OK);
    let last_modified = response
        .headers()
        .get(header::LAST_MODIFIED)
        .expect("Last-Modified header present")
        .to_str()
        .unwrap();
    let expected =
        locking::format_http_date(locking::parse_stored_timestamp("2024-06-01T00:00:00Z").unwrap());
    assert_eq!(last_modified, expected);
}

#[tokio::test]
async fn put_with_if_unmodified_since_older_than_the_stored_value_is_412() {
    let app = build_timestamps_app(vec![(
        "x",
        timestamped_feature("x", "2024-06-01T00:00:00Z"),
    )]);
    let stale_since =
        locking::format_http_date(locking::parse_stored_timestamp("2024-03-01T00:00:00Z").unwrap());
    let response = send_write_with_headers(
        &app,
        "PUT",
        "/collections/demo/items/x",
        &[(header::IF_UNMODIFIED_SINCE, stale_since.as_str())],
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let body = body_json(response).await;
    assert_eq!(body["code"], "PreconditionFailed");
}

#[tokio::test]
async fn put_with_if_unmodified_since_at_the_stored_value_succeeds() {
    let app = build_timestamps_app(vec![(
        "x",
        timestamped_feature("x", "2024-06-01T00:00:00Z"),
    )]);
    let current_since =
        locking::format_http_date(locking::parse_stored_timestamp("2024-06-01T00:00:00Z").unwrap());
    let response = send_write_with_headers(
        &app,
        "PUT",
        "/collections/demo/items/x",
        &[(header::IF_UNMODIFIED_SINCE, current_since.as_str())],
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

/// `POLICY_ONLY_CONFIG`'s `demo` collection (via `BODY_CAP_CONFIG`) declares
/// no `modified_column` — `If-Unmodified-Since` must be silently ignored
/// (the write proceeds) regardless of the value sent, since there is no
/// declared source to evaluate it against.
#[tokio::test]
async fn if_unmodified_since_is_ignored_without_a_declared_modified_column() {
    let app = build_policy_app(
        BODY_CAP_CONFIG,
        vec![("x", filterable_feature("x", "alpha"))],
    );
    let response = send_write_with_headers(
        &app,
        "PUT",
        "/collections/demo/items/x",
        &[(header::IF_UNMODIFIED_SINCE, "Fri, 01 Mar 2024 00:00:00 GMT")],
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "a collection with no declared modified_column must never fail a write over \
         If-Unmodified-Since"
    );
}

/// `If-Match` takes precedence over `If-Unmodified-Since` when both are
/// sent (RFC 7232 section 5): a stale `If-Unmodified-Since` alongside a
/// CURRENT `If-Match` must still succeed.
#[tokio::test]
async fn if_match_takes_precedence_over_a_stale_if_unmodified_since() {
    let app = build_timestamps_app(vec![(
        "x",
        timestamped_feature("x", "2024-06-01T00:00:00Z"),
    )]);
    let etag = locking::compute_feature_etag(&timestamped_feature("x", "2024-06-01T00:00:00Z"));
    let stale_since =
        locking::format_http_date(locking::parse_stored_timestamp("2024-03-01T00:00:00Z").unwrap());
    let response = send_write_with_headers(
        &app,
        "PUT",
        "/collections/demo/items/x",
        &[
            (header::IF_MATCH, etag.as_str()),
            (header::IF_UNMODIFIED_SINCE, stale_since.as_str()),
        ],
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "a current If-Match must win over a stale If-Unmodified-Since sent alongside it"
    );
}

/// `DELETE` honors the same `If-Match` guard `PUT` does (`#107`) — a stale
/// value against an existing resource is `412`, not a silent delete.
#[tokio::test]
async fn delete_with_a_stale_if_match_is_412() {
    let app = build_timestamps_app(vec![(
        "x",
        timestamped_feature("x", "2024-06-01T00:00:00Z"),
    )]);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/collections/demo/items/x")
                .header(header::IF_MATCH, "\"stale\"")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    let body = body_json(response).await;
    assert_eq!(body["code"], "PreconditionFailed");
}

/// `DELETE` with the resource's own current ETag as `If-Match` proceeds.
#[tokio::test]
async fn delete_with_the_current_etag_as_if_match_succeeds() {
    let app = build_timestamps_app(vec![(
        "x",
        timestamped_feature("x", "2024-06-01T00:00:00Z"),
    )]);
    let etag = locking::compute_feature_etag(&timestamped_feature("x", "2024-06-01T00:00:00Z"));
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/collections/demo/items/x")
                .header(header::IF_MATCH, etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

/// `DELETE` with `If-Match` sent against a resource that does not exist at
/// all is `412` — the same narrow existence guard `PUT` already had,
/// extended to `DELETE` (`#107`).
#[tokio::test]
async fn delete_with_if_match_against_a_missing_resource_is_412() {
    let app = build_timestamps_app(vec![]);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/collections/demo/items/does-not-exist")
                .header(header::IF_MATCH, "\"anything\"")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
}

/// `/collections/demo` declares BOTH Optimistic Locking classes for this
/// collection: ETags (the write sink's own declared set) and Timestamps
/// (`modified_column` is declared).
#[tokio::test]
async fn get_collection_declares_both_locking_classes_when_both_are_earned() {
    let app = build_timestamps_app(vec![(
        "x",
        timestamped_feature("x", "2024-06-01T00:00:00Z"),
    )]);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let classes: Vec<&str> = body["lockingConformanceClasses"]
        .as_array()
        .expect("lockingConformanceClasses present")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(classes.contains(&tellurion_core::locking::OPTIMISTIC_LOCKING_ETAGS_CLASS));
    assert!(classes.contains(&tellurion_core::locking::OPTIMISTIC_LOCKING_TIMESTAMPS_CLASS));
}

/// A collection whose write sink declares nothing and which declares no
/// `modified_column` (`BODY_CAP_CONFIG`'s `demo`) reports an empty
/// `lockingConformanceClasses` — present, never omitted, never fabricated.
#[tokio::test]
async fn get_collection_declares_no_locking_classes_when_neither_is_earned() {
    let app = build_policy_app(BODY_CAP_CONFIG, vec![]);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["lockingConformanceClasses"], json!([]));
}

// -- read-lane hints and `X-Tellurion-Source` (`#183`) -----------------------

/// A `FeatureSource` that answers every read with a body naming `label`, or
/// errors every call when `error` is set — the handler-level counterpart of
/// `tellurion-core`'s own `TaggedFeatureSource` router fixture, so these
/// tests can see which chain entry a response was actually served from.
struct HintFeatureSource {
    label: &'static str,
    error: bool,
}

#[async_trait::async_trait]
impl FeatureSource for HintFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        _query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        if self.error {
            return Err(tellurion_core::Error::Timeout);
        }
        Ok(FeaturePage {
            features_geojson: vec![json!({
                "type": "Feature",
                "id": self.label,
                "geometry": null,
                "properties": { "storage": self.label },
            })],
            number_matched: Some(1),
            next_token: None,
        })
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        id: &str,
        _filter: Option<&Filter>,
    ) -> CoreResult<Option<Value>> {
        if self.error {
            return Err(tellurion_core::Error::Timeout);
        }
        Ok(Some(json!({
            "type": "Feature",
            "id": id,
            "geometry": null,
            "properties": { "storage": self.label },
        })))
    }
}

struct HintDriver {
    label: &'static str,
    error: bool,
}

impl StorageDriver for HintDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(EmptyCatalog)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::new(HintFeatureSource {
            label: self.label,
            error: self.error,
        }))
    }
}

struct HintFactory {
    name: &'static str,
    label: &'static str,
    error: bool,
}

impl DriverFactory for HintFactory {
    fn name(&self) -> &str {
        self.name
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(HintDriver {
            label: self.label,
            error: self.error,
        }))
    }
}

/// Two feature-capable storages on `demo`'s features lane, `main` as the
/// configured primary. `broken_alt` makes the `alt` driver's every call
/// error, for the fall-through tests.
fn build_hint_chain_app(broken_alt: bool) -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(
        r#"
storages:
  - { id: main, driver: hint-main, url_env: DATABASE_URL }
  - { id: alt, driver: hint-alt, url_env: DATABASE_URL2 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { features: [main, alt] }
"#,
    )
    .unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(HintFactory {
        name: "hint-main",
        label: "main",
        error: false,
    }));
    registry.register(Arc::new(HintFactory {
        name: "hint-alt",
        label: "alt",
        error: broken_alt,
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
    tellurion_features::router().with_state(ctx)
}

fn read_source_header(response: &Response) -> Option<String> {
    response
        .headers()
        .get("x-tellurion-source")
        .map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn unhinted_items_reads_name_the_configured_primary_in_the_source_header() {
    let app = build_hint_chain_app(false);
    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(read_source_header(&response).as_deref(), Some("main"));
    let body = body_json(response).await;
    assert_eq!(body["features"][0]["properties"]["storage"], "main");
}

#[tokio::test]
async fn prefer_hint_reroutes_an_items_read_to_the_named_chain_entry() {
    let app = build_hint_chain_app(false);
    let response = get(&app, "/collections/demo/items?hints=prefer:alt").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        read_source_header(&response).as_deref(),
        Some("alt"),
        "the header must name the entry the hint moved to the front"
    );
    let body = body_json(response).await;
    assert_eq!(body["features"][0]["properties"]["storage"], "alt");
}

/// `prefer:` reorders — never extends — so a preferred entry that errors
/// falls through to the configured chain instead of failing the request,
/// and the header names the entry that ACTUALLY served, not the preference.
#[tokio::test]
async fn a_preferred_entry_that_errors_falls_back_and_the_header_stays_honest() {
    let app = build_hint_chain_app(true);
    let response = get(&app, "/collections/demo/items?hints=prefer:alt").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(read_source_header(&response).as_deref(), Some("main"));
    let body = body_json(response).await;
    assert_eq!(body["features"][0]["properties"]["storage"], "main");
}

/// Unknown hint tokens are dropped harmlessly (never a 400 — `#183`'s typo
/// rule), and a `prefer:` naming no entry of this lane's chain is exactly
/// as inert. This also proves `hints` is a reserved parameter name, not an
/// implicit queryable-equality predicate (which would have 400ed here as an
/// undeclared queryable, `#52`).
#[tokio::test]
async fn unknown_hint_tokens_and_unknown_prefer_names_are_harmless_no_ops() {
    let app = build_hint_chain_app(false);
    for uri in [
        "/collections/demo/items?hints=bogus-token",
        "/collections/demo/items?hints=prefer:no-such-storage",
        "/collections/demo/items?hints=",
    ] {
        let response = get(&app, uri).await;
        assert_eq!(response.status(), StatusCode::OK, "uri: {uri}");
        assert_eq!(
            read_source_header(&response).as_deref(),
            Some("main"),
            "uri: {uri}"
        );
    }
}

#[tokio::test]
async fn single_item_reads_honor_prefer_and_carry_the_source_header() {
    let app = build_hint_chain_app(false);

    let unhinted = get(&app, "/collections/demo/items/a").await;
    assert_eq!(unhinted.status(), StatusCode::OK);
    assert_eq!(read_source_header(&unhinted).as_deref(), Some("main"));

    let hinted = get(&app, "/collections/demo/items/a?hints=prefer:alt").await;
    assert_eq!(hinted.status(), StatusCode::OK);
    assert_eq!(read_source_header(&hinted).as_deref(), Some("alt"));
    let body = body_json(hinted).await;
    assert_eq!(body["properties"]["storage"], "alt");
}

/// A single-entry lane (the default `storage:`-only shape every existing
/// deployment has) also names its one entry — the header is observability
/// for every read, not a hints-only extra.
#[tokio::test]
async fn a_single_entry_lane_names_its_only_storage_in_the_source_header() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(read_source_header(&response).as_deref(), Some("main"));
}

/// `#183`'s write-lane rule: hints never redirect writes. The write
/// handlers take no hints at all, so a `?hints=prefer:...` on a write URI
/// changes nothing — the mutation still reaches the routed write sink and
/// succeeds exactly as it does unhinted.
#[tokio::test]
async fn a_prefer_hint_on_a_write_request_is_inert() {
    let app = build_policy_app(WRITE_CONFIG, vec![]);
    let response = send_write(
        &app,
        "PUT",
        "/collections/demo/items/x?hints=prefer:main",
        Some("writer-token"),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::NO_CONTENT,
        "a hinted write must behave byte-for-byte like the unhinted one"
    );
    assert_eq!(
        read_source_header(&response),
        None,
        "the read-lane source header must never appear on a write response"
    );
}

// -- `#186`: cross-protocol contributed links -------------------------------

/// A test-local `LinkContributor` — this crate never depends on the wiring
/// layer's real contributors (crate decoupling is the whole point of the
/// seam), so proving the serializer side means registering a fake one the
/// way `tellurion-server`'s boot does, and asserting the anchor filter:
/// Collection-anchored links land on collection responses,
/// Item-anchored ones never do.
struct ContributingFake;

#[async_trait::async_trait]
impl tellurion_core::LinkContributor for ContributingFake {
    async fn contribute(
        &self,
        _router: &CoreRouter,
        resource: &tellurion_core::ResourceRef<'_>,
    ) -> Vec<tellurion_core::ContributedLink> {
        vec![
            tellurion_core::ContributedLink {
                anchor: tellurion_core::LinkAnchor::Collection,
                rel: "tiles".to_string(),
                href: format!(
                    "{}/{}/tiles/catalogs/{}/collections/{}/tiles/WebMercatorQuad/{{tileMatrix}}/{{tileRow}}/{{tileCol}}.mvt",
                    resource.base_url.trim_end_matches('/'),
                    resource.tenant,
                    resource.catalog,
                    resource.collection
                ),
                media_type: "application/vnd.mapbox-vector-tile".to_string(),
                title: Some("Vector tiles (MVT)".to_string()),
                templated: true,
            },
            tellurion_core::ContributedLink {
                anchor: tellurion_core::LinkAnchor::Item,
                rel: "item-anchored".to_string(),
                href: "/never-on-a-collection".to_string(),
                media_type: "application/json".to_string(),
                title: None,
                templated: false,
            },
        ]
    }
}

/// `build_app`, plus a registered fake contributor — the only difference
/// from every other fixture in this file, so any behavioral delta in these
/// tests is attributable to registration alone.
fn build_contributing_app(items: Vec<(&str, Value)>) -> axum::Router {
    build_contributing_app_with_config(DEMO_CONFIG, items)
}

fn build_contributing_app_with_config(
    config_yaml: &str,
    items: Vec<(&str, Value)>,
) -> axum::Router {
    let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
    config.validate().unwrap();
    let source = Arc::new(FakeFeatureSource {
        items: items
            .into_iter()
            .map(|(id, v)| (id.to_string(), v))
            .collect(),
    });
    let mut registry = Registry::new();
    registry.register(Arc::new(FakeFactory { source }));
    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    let mut contributors = tellurion_core::LinkContributors::new();
    contributors.register("fake", Arc::new(ContributingFake));
    let ctx = AppContext::new(config, core_router, resolver, None, cache, style_store)
        .with_link_contributors(contributors);
    tellurion_features::router().with_state(Arc::new(ctx))
}

#[tokio::test]
async fn configured_public_base_is_passed_to_contributors_without_breaking_uri_templates() {
    let config = format!(
        "{DEMO_CONFIG}\nserver: {{ public_base_url: 'https://maps.example.test/tellurion/' }}\n"
    );
    let app = axum::Router::new().nest(
        "/{tenant}/features/catalogs/{catalog}",
        build_contributing_app_with_config(&config, vec![("a", feature("a"))]),
    );

    let body =
        body_json(get(&app, "/public/features/catalogs/default/collections/demo").await).await;
    let tiles = find_link(&body, "tiles").expect("contributed tiles link present");
    assert_eq!(
        tiles["href"],
        "https://maps.example.test/tellurion/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.mvt"
    );
    assert_eq!(tiles["templated"], true);
}

/// `GET /collections/{cid}` appends Collection-anchored contributed links
/// after this crate's own, carrying the contributed `title`/`templated`
/// members onto the wire; Item-anchored contributions are filtered out.
#[tokio::test]
async fn get_collection_appends_collection_anchored_contributed_links() {
    let app = build_contributing_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    let tiles = find_link(&body, "tiles").expect("contributed tiles link present");
    assert_eq!(
        tiles["href"],
        "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}.mvt"
    );
    assert_eq!(tiles["type"], "application/vnd.mapbox-vector-tile");
    assert_eq!(tiles["title"], "Vector tiles (MVT)");
    assert_eq!(tiles["templated"], true);

    assert!(
        find_link(&body, "item-anchored").is_none(),
        "an Item-anchored contribution must never land on a collection document"
    );
    // Appended after this crate's own links — existing consumers' link
    // order is untouched.
    let last = body["links"].as_array().unwrap().last().unwrap();
    assert_eq!(last["rel"], "tiles");
}

/// `GET /collections` appends the same per-collection contributed links to
/// each summary the listing serves.
#[tokio::test]
async fn list_collections_appends_contributed_links_per_summary() {
    let app = build_contributing_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    let summary = &body["collections"][0];
    let tiles = summary["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["rel"] == "tiles")
        .expect("contributed tiles link present on the listed summary");
    assert_eq!(tiles["templated"], true);
}

/// Nothing registered (every other fixture in this file) means no
/// contributed links AND no `title`/`templated` members anywhere — the
/// serialized shape is byte-for-byte what it was before the seam existed.
#[tokio::test]
async fn without_registration_collection_links_are_unchanged() {
    let app = build_app(vec![("a", feature("a"))]);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    assert!(find_link(&body, "tiles").is_none());
    for link in body["links"].as_array().unwrap() {
        assert!(
            link.get("templated").is_none() && link.get("title").is_none(),
            "no link may grow new members while the seam is unregistered: {link}"
        );
    }
}

// ---------------------------------------------------------------------------
// `#188`: rate ceilings as policy grant conditions, end to end.
// ---------------------------------------------------------------------------

/// Two tokens holding the same role, whose grant declares a ceiling of two
/// requests per hour, scoped per principal. An hour-long window so the test
/// never races a boundary: every assertion is about counts, not clocks.
const RATE_LIMIT_CONFIG: &str = r#"
storages: [ { id: main, driver: policy-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - { token: alice-token, tenants: [public], roles: { public: [reader] }, principal: alice }
    - { token: bob-token, tenants: [public], roles: { public: [reader] }, principal: bob }
policy:
  roles:
    - name: reader
      grants:
        - scope: { collections: [demo] }
          lanes: [features]
          rate:
            scope: principal
            window_seconds: 3600
            ceiling: 2
            on_counter_unavailable: strict
"#;

/// The same role and the same two tokens, with no `rate:` block at all —
/// the pre-`#188` document, unchanged.
const NO_RATE_LIMIT_CONFIG: &str = r#"
storages: [ { id: main, driver: policy-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - { token: alice-token, tenants: [public], roles: { public: [reader] }, principal: alice }
policy:
  roles:
    - name: reader
      grants:
        - scope: { collections: [demo] }
          lanes: [features]
"#;

#[tokio::test]
async fn a_declared_ceiling_serves_its_budget_then_answers_429_with_retry_after() {
    let app = build_policy_app(
        RATE_LIMIT_CONFIG,
        vec![("a", filterable_feature("a", "alpha"))],
    );
    for attempt in 1..=2 {
        let response = get_with_bearer(&app, "/collections/demo/items", Some("alice-token")).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "request {attempt} is inside a ceiling of 2"
        );
    }

    let refused = get_with_bearer(&app, "/collections/demo/items", Some("alice-token")).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after = refused
        .headers()
        .get(header::RETRY_AFTER)
        .expect("a 429 must carry Retry-After")
        .to_str()
        .unwrap()
        .parse::<u64>()
        .expect("Retry-After must be whole seconds");
    assert!(
        (1..=3600).contains(&retry_after),
        "Retry-After must point inside the declared window, got {retry_after}"
    );
    assert_eq!(
        refused
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "application/problem+json"
    );
    let body = body_json(refused).await;
    assert_eq!(body["status"], 429);
    assert_eq!(body["code"], "RateLimited");
    assert_eq!(body["retryAfter"], retry_after);
    let detail = body["detail"].as_str().unwrap();
    assert!(
        detail.contains('2'),
        "the detail names the ceiling: {detail}"
    );
    assert!(
        !detail.contains("alice"),
        "a refusal must never echo the principal it counted: {detail}"
    );
}

/// The ceiling is per principal, so one token exhausting its budget leaves
/// the other's untouched — the whole point of `scope: principal`.
#[tokio::test]
async fn one_principals_exhausted_ceiling_never_refuses_another_principal() {
    let app = build_policy_app(
        RATE_LIMIT_CONFIG,
        vec![("a", filterable_feature("a", "alpha"))],
    );
    for _ in 0..3 {
        get_with_bearer(&app, "/collections/demo/items", Some("alice-token")).await;
    }
    assert_eq!(
        get_with_bearer(&app, "/collections/demo/items", Some("alice-token"))
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        get_with_bearer(&app, "/collections/demo/items", Some("bob-token"))
            .await
            .status(),
        StatusCode::OK,
        "bob must not pay for alice's traffic"
    );
}

/// A collections listing runs the policy checkpoint once per candidate
/// collection to decide what to advertise. That is not a served request, so
/// it must leave the caller's whole budget intact — otherwise a catalog with
/// many collections would silently shrink every ceiling.
#[tokio::test]
async fn a_collections_listing_never_spends_the_callers_budget() {
    let app = build_policy_app(
        RATE_LIMIT_CONFIG,
        vec![("a", filterable_feature("a", "alpha"))],
    );
    for _ in 0..10 {
        assert_eq!(
            get_with_bearer(&app, "/collections", Some("alice-token"))
                .await
                .status(),
            StatusCode::OK
        );
    }
    for attempt in 1..=2 {
        assert_eq!(
            get_with_bearer(&app, "/collections/demo/items", Some("alice-token"))
                .await
                .status(),
            StatusCode::OK,
            "request {attempt} of the untouched budget must still be served"
        );
    }
}

/// Nothing configured, nothing changed: the identical role and tokens with
/// no `rate:` block serve without bound, and no response grows a
/// `Retry-After`.
#[tokio::test]
async fn a_grant_declaring_no_ceiling_is_never_rate_limited() {
    let app = build_policy_app(
        NO_RATE_LIMIT_CONFIG,
        vec![("a", filterable_feature("a", "alpha"))],
    );
    for _ in 0..20 {
        let response = get_with_bearer(&app, "/collections/demo/items", Some("alice-token")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::RETRY_AFTER).is_none());
    }
}
