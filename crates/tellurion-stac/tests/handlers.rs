//! HTTP-level tests (`#36`, slices A and B): a fake, in-memory
//! `CatalogSource`/`FeatureSource` (plus, for the items/assets tests, a
//! `TileSource`) driven through the real `tellurion_core::Router` and the
//! real axum router this crate exports — no database involved. Mirrors
//! `tellurion-features`' own `tests/handlers.rs` style: landing/collections/
//! items shape, unknown-collection/item 404, paging round-trip, and the
//! internal-id-never-serializes guard.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use tower::ServiceExt;

use tellurion_core::{
    AppConfig, AppContext, AttributeColumn, CatalogSource, CollectionDecl, CompareOp,
    DriverFactory, FeaturePage, FeatureSource, FileStyleStore, Filter, GeometryLiteral, ItemsQuery,
    Literal, MokaTileCache, PhysicalCollection, ProjectionFacts, RasterSource, RasterWindow,
    Registry, RequestedCrs, Resolver, Result as CoreResult, Router as CoreRouter, SpatialExtent,
    StaticResolver, StorageDecl, StorageDriver, StyleStore, TileCache, TileCoord,
};

/// A `CatalogSource` reporting one physical collection with a real spatial
/// extent, matching `DEMO_CONFIG`'s "demo" collection shape.
struct DemoCatalog;

#[async_trait::async_trait]
impl CatalogSource for DemoCatalog {
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

struct EmptyFeatureSource;

#[async_trait::async_trait]
impl FeatureSource for EmptyFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        _query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        Ok(FeaturePage {
            features_geojson: vec![],
            number_matched: Some(0),
            next_token: None,
        })
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        _id: &str,
        _filter: Option<&Filter>,
    ) -> CoreResult<Option<Value>> {
        Ok(None)
    }
}

struct DemoDriver;

impl StorageDriver for DemoDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(DemoCatalog)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::new(EmptyFeatureSource) as Arc<dyn FeatureSource>)
    }
}

struct DemoFactory;

impl DriverFactory for DemoFactory {
    fn name(&self) -> &str {
        "fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(DemoDriver))
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

fn build_ctx(config_yaml: &str) -> Arc<AppContext> {
    let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(DemoFactory));

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

fn build_app(config_yaml: &str) -> axum::Router {
    tellurion_stac::router().with_state(build_ctx(config_yaml))
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
async fn collections_list_shape_and_content_type() {
    let app = build_app(DEMO_CONFIG);
    let response = get(&app, "/collections").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );

    let body = body_json(response).await;
    assert!(find_link(&body, "root").is_some());
    assert!(find_link(&body, "self").is_some());

    let collections = body["collections"].as_array().unwrap();
    assert_eq!(collections.len(), 1);
    let demo = &collections[0];
    assert_eq!(demo["type"], "Collection");
    assert_eq!(demo["stac_version"], "1.1.0");
    assert_eq!(demo["id"], "demo");
    assert_eq!(demo["license"], "other");
    assert_eq!(
        demo["extent"]["spatial"]["bbox"][0],
        serde_json::json!([-5.0, 45.0, 5.0, 55.0])
    );
    assert_eq!(
        demo["extent"]["temporal"]["interval"][0],
        serde_json::json!([null, null])
    );
    assert!(find_link(demo, "root").is_some());
    assert!(find_link(demo, "self").is_some());
}

#[tokio::test]
async fn get_collection_shape_and_links() {
    // Mounted under `/{tenant}` (the real server's shape, `/{tenant}/stac/
    // catalogs/{catalog}/...`) so `root`/`parent` resolve to a real,
    // non-empty href — same reasoning as
    // `tenant_is_read_from_the_nesting_path_when_present`.
    let app = axum::Router::new()
        .nest("/{tenant}", tellurion_stac::router())
        .with_state(build_ctx(DEMO_CONFIG));
    let response = get(&app, "/public/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );

    let body = body_json(response).await;
    assert_eq!(body["type"], "Collection");
    assert_eq!(body["id"], "demo");
    assert_eq!(
        find_link(&body, "self").unwrap()["href"],
        "/public/collections/demo"
    );
    assert_eq!(find_link(&body, "root").unwrap()["href"], "/public");
    assert_eq!(find_link(&body, "parent").unwrap()["href"], "/public");
}

// -- ISO 19139 alternate representation (`#50`) --------------------------

const ISO19139_MEDIA_TYPE: &str = "application/vnd.iso.19139+xml";

#[tokio::test]
async fn get_collection_json_carries_an_alternate_link_to_the_iso19139_representation() {
    let app = axum::Router::new()
        .nest("/{tenant}", tellurion_stac::router())
        .with_state(build_ctx(DEMO_CONFIG));
    let response = get(&app, "/public/collections/demo").await;
    let body = body_json(response).await;
    let alternate = find_link(&body, "alternate").expect("an alternate link");
    assert_eq!(alternate["type"], ISO19139_MEDIA_TYPE);
    assert_eq!(alternate["href"], "/public/collections/demo?f=xml");
}

#[tokio::test]
async fn get_collection_with_f_xml_serves_iso19139_xml() {
    let app = build_app(DEMO_CONFIG);
    let response = get(&app, "/collections/demo?f=xml").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        ISO19139_MEDIA_TYPE
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.starts_with("<?xml"));
    assert!(text.contains("<gmd:MD_Metadata"));
    assert!(text.contains(">demo<"));
}

#[tokio::test]
async fn get_collection_with_iso19139_accept_header_serves_xml_with_no_query_param() {
    let app = build_app(DEMO_CONFIG);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/collections/demo")
                .header(header::ACCEPT, ISO19139_MEDIA_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        ISO19139_MEDIA_TYPE
    );
}

#[tokio::test]
async fn get_collection_default_response_is_still_json_without_f_or_accept() {
    let app = build_app(DEMO_CONFIG);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
}

/// `#50` lineage slice, end to end: a `settings.stac.lineage` declared in
/// config reaches the ISO 19139 document as a real
/// `gmd:dataQualityInfo/…/gmd:LI_Lineage`, while the demo config — which
/// declares none — keeps a document with no `dataQualityInfo` at all (the
/// no-facts byte-invariance the projection's own unit tests pin, proven
/// here through the full config -> settings chain -> canonical descriptor
/// -> XML path).
#[tokio::test]
async fn configured_lineage_reaches_the_iso19139_document_and_undeclared_stays_absent() {
    let config_yaml = r#"
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
    settings:
      stac:
        lineage:
          statement: Digitised from the 1:25000 IGM series.
          sources:
            - description: IGM 1:25000 sheet 45
          process_steps:
            - description: Reprojected to EPSG:4326 with ogr2ogr
"#;
    let app = build_app(config_yaml);
    let response = get(&app, "/collections/demo?f=xml").await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("<gmd:dataQualityInfo>"), "{text}");
    assert!(text.contains("<gmd:LI_Lineage>"));
    assert!(text.contains("Digitised from the 1:25000 IGM series."));
    assert!(text.contains("IGM 1:25000 sheet 45"));
    assert!(text.contains("Reprojected to EPSG:4326 with ogr2ogr"));

    let bare = build_app(DEMO_CONFIG);
    let response = get(&bare, "/collections/demo?f=xml").await;
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("dataQualityInfo"), "{text}");
}

#[tokio::test]
async fn unknown_collection_returns_problem_json_404() {
    let app = build_app(DEMO_CONFIG);
    let response = get(&app, "/collections/nope").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let body = body_json(response).await;
    assert_eq!(body["code"], "NotFound");
}

#[tokio::test]
async fn unknown_collection_in_the_list_route_is_simply_absent() {
    let app = build_app(DEMO_CONFIG);
    let response = get(&app, "/collections").await;
    let body = body_json(response).await;
    assert_eq!(body["collections"].as_array().unwrap().len(), 1);
}

// -- /collections cursor paging (`#42`, `#59`) ---------------------------

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

/// A small registry (fewer collections than the default page size) still
/// gets everything back on the one, only page — no `next` link, mirroring
/// `tellurion_features`' own no-regression guard for the identical case.
#[tokio::test]
async fn list_collections_default_limit_returns_everything_on_one_page() {
    let app = build_app(MULTI_COLLECTION_CONFIG);
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

/// The paging round trip: `limit=2` over three collections returns the
/// first two (in stable, external-id order) plus a `next` link; walking
/// that link returns the remaining one collection and no further `next` —
/// same mechanism `tellurion_features`' own `/collections` paging exercises.
#[tokio::test]
async fn list_collections_paginates_with_a_limit_and_a_next_link() {
    let app = build_app(MULTI_COLLECTION_CONFIG);

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
    let app = build_app(MULTI_COLLECTION_CONFIG);
    let response = get(&app, "/collections?limit=0").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// Same fixtures, mounted under a `/{tenant}` prefix by the "server" —
/// proves tenant resolution from the path works, not just the fixed
/// default, mirroring `tellurion_features`' own tenant-prefix test.
#[tokio::test]
async fn tenant_is_read_from_the_nesting_path_when_present() {
    let app = axum::Router::new()
        .nest("/{tenant}", tellurion_stac::router())
        .with_state(build_ctx(DEMO_CONFIG));

    let matching_tenant = get(&app, "/public/collections/demo").await;
    assert_eq!(matching_tenant.status(), StatusCode::OK);

    let wrong_tenant = get(&app, "/other-tenant/collections/demo").await;
    assert_eq!(wrong_tenant.status(), StatusCode::NOT_FOUND);
}

/// `#36` design guard, mirroring `tellurion-server`'s own internal-id
/// leak sweep: a config where every internal id is deliberately distinct
/// from — and textually unrelated to — its external id must never surface
/// an internal id in a response body reachable through this crate's router.
#[tokio::test]
async fn internal_ids_never_appear_in_any_response_body() {
    const TENANT_INTERNAL: &str = "zzz-tenant-internal-marker";
    const CATALOG_INTERNAL: &str = "zzz-catalog-internal-marker";
    const COLLECTION_INTERNAL: &str = "zzz-collection-internal-marker";

    let config_yaml = format!(
        r#"
storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: {TENANT_INTERNAL}, external_id: acme }} ]
catalogs: [ {{ id: {CATALOG_INTERNAL}, external_id: default, tenant: {TENANT_INTERNAL} }} ]
collections:
  - id: {COLLECTION_INTERNAL}
    external_id: demo
    catalog: {CATALOG_INTERNAL}
    storage: main
    table: demo
    geometry: geom
    pk: id
"#
    );

    let app = axum::Router::new()
        .nest("/{tenant}", tellurion_stac::router())
        .with_state(build_ctx(&config_yaml));

    let paths = [
        "/acme/collections".to_string(),
        "/acme/collections/demo".to_string(),
    ];

    for path in paths {
        let response = get(&app, &path).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(status, StatusCode::OK, "path {path} was not 200: {text}");
        assert!(
            !text.contains(TENANT_INTERNAL),
            "{path} leaked the tenant internal id: {text}"
        );
        assert!(
            !text.contains(CATALOG_INTERNAL),
            "{path} leaked the catalog internal id: {text}"
        );
        assert!(
            !text.contains(COLLECTION_INTERNAL),
            "{path} leaked the collection internal id: {text}"
        );
    }
}

// -- items (`#36` slice B) ---------------------------------------------

/// A raw GeoJSON Feature shaped exactly like a real driver's output
/// (`tellurion-postgis::sql::properties_expr`'s own
/// `json_build_object('type','Feature','id',...,'geometry',...,'properties',...)`
/// shape) — `properties` carries whatever columns the fixture wants,
/// including a datetime column under its own configured name.
fn stac_feature(id: &str, properties: serde_json::Value) -> Value {
    json!({
        "type": "Feature",
        "id": id,
        "geometry": { "type": "Point", "coordinates": [1.0, 2.0] },
        "properties": properties,
    })
}

/// Same in-memory keyset-paging `FeatureSource` shape
/// `tellurion_features::tests::FakeFeatureSource` uses — sorted ascending by
/// id, token = "resume after this id".
struct ItemsFeatureSource {
    items: Vec<(String, Value)>,
}

#[async_trait::async_trait]
impl FeatureSource for ItemsFeatureSource {
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

struct ItemsTileSource;

#[async_trait::async_trait]
impl tellurion_core::TileSource for ItemsTileSource {
    async fn mvt_tile(
        &self,
        _collection: &CollectionDecl,
        _coord: tellurion_core::TileCoord,
        _filter: Option<&Filter>,
    ) -> CoreResult<Option<bytes::Bytes>> {
        Ok(None)
    }
}

/// `with_tiles`/`with_places3d` let a single driver shape stand in for every
/// capability combination the asset-materialization tests need, instead of
/// four near-duplicate driver types.
struct ItemsDriver {
    source: Arc<ItemsFeatureSource>,
    with_tiles: bool,
}

impl StorageDriver for ItemsDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(DemoCatalog)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(self.source.clone() as Arc<dyn FeatureSource>)
    }

    fn tile_source(&self) -> Option<Arc<dyn tellurion_core::TileSource>> {
        self.with_tiles
            .then(|| Arc::new(ItemsTileSource) as Arc<dyn tellurion_core::TileSource>)
    }
}

struct ItemsFactory {
    source: Arc<ItemsFeatureSource>,
    with_tiles: bool,
}

impl DriverFactory for ItemsFactory {
    fn name(&self) -> &str {
        "items-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(ItemsDriver {
            source: self.source.clone(),
            with_tiles: self.with_tiles,
        }))
    }
}

/// `collection_yaml` is spliced directly under `collections:\n  - id: demo\n`
/// so a test can add `tiles: {}`/`places3d: {...}`/`datetime: ...` without a
/// whole new config template.
fn items_config_yaml(collection_extra: &str) -> String {
    format!(
        r#"
storages: [ {{ id: main, driver: items-fake, url_env: DATABASE_URL }} ]
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

fn build_items_ctx(
    collection_yaml: &str,
    items: Vec<(&str, Value)>,
    with_tiles: bool,
) -> Arc<AppContext> {
    let config: AppConfig = serde_yaml::from_str(collection_yaml).unwrap();
    config.validate().unwrap();

    let source = Arc::new(ItemsFeatureSource {
        items: items
            .into_iter()
            .map(|(id, v)| (id.to_string(), v))
            .collect(),
    });

    let mut registry = Registry::new();
    registry.register(Arc::new(ItemsFactory { source, with_tiles }));

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

fn build_items_app(
    collection_yaml: &str,
    items: Vec<(&str, Value)>,
    with_tiles: bool,
) -> axum::Router {
    tellurion_stac::router().with_state(build_items_ctx(collection_yaml, items, with_tiles))
}

// -- the Collection `items` link (`#245`) --------------------------------
//
// OGC API - Features - Part 1: Core (17-069r4, 1.0.1) Requirement 15
// `/req/core/fc-md-items-links` on `/collections`, carried onto
// `/collections/{cid}` by Requirement 19 `/req/core/sfc-md-success`; STAC
// API - Features (`v1.0.0`) states the same rule in prose ("This endpoint
// must be exposed via a link in the individual collection's endpoint with
// `rel=items`"). Both classes are declared by this crate's own
// `CONFORMANCE_CLASSES`, and until this slice neither was honoured.
//
// Every assertion below FOLLOWS the link. A link with the right `rel` and a
// dangling href would be the same defect one level down, so "the link is
// present" is never the assertion on its own.

/// The href a link with `rel` carries, as a `String`.
fn link_href(body: &Value, rel: &str) -> String {
    find_link(body, rel)
        .unwrap_or_else(|| panic!("expected a {rel} link in {body}"))
        .get("href")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a {rel} link with no href in {body}"))
        .to_string()
}

/// The single-collection resource's `items` link is dereferenceable: the
/// href it advertises really answers `200` with this collection's own rows,
/// not merely a URL shaped like one.
#[tokio::test]
async fn the_collection_items_link_is_followable_and_reaches_that_collections_items() {
    let app = build_items_app(
        &items_config_yaml(""),
        vec![
            ("a", stac_feature("a", json!({}))),
            ("b", stac_feature("b", json!({}))),
        ],
        false,
    );

    let collection = body_json(get(&app, "/collections/demo").await).await;
    let items = find_link(&collection, "items").expect("a Collection carries an items link");
    // Requirement 15.B: "All links SHALL include the `rel` and `type`
    // properties." The type is the one encoding `/items` actually serves.
    assert_eq!(items["type"], "application/geo+json");
    let href = link_href(&collection, "items");
    assert_eq!(href, "/collections/demo/items");

    let response = get(&app, &href).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the advertised items link must resolve, not 404"
    );
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/geo+json",
        "and serve the media type the link declared"
    );
    let page = body_json(response).await;
    assert_eq!(page["type"], "FeatureCollection");
    let ids: Vec<&str> = page["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["a", "b"],
        "and the rows it serves must be this collection's own"
    );
}

/// The same link on the `/collections` listing entry — Requirement 15 is
/// stated about the listing, and Requirement 19 only then carries it onto
/// the single-collection resource, so the listing is the primary site and
/// gets its own follow-through. Nested under `/{tenant}` so the href is
/// built against a real prefix rather than a bare root.
#[tokio::test]
async fn the_collections_listing_entry_items_link_is_followable_too() {
    let app = axum::Router::new()
        .nest("/{tenant}", tellurion_stac::router())
        .with_state(build_items_ctx(
            &items_config_yaml(""),
            vec![("a", stac_feature("a", json!({})))],
            false,
        ));

    let listing = body_json(get(&app, "/public/collections").await).await;
    let entry = &listing["collections"][0];
    let href = link_href(entry, "items");
    assert_eq!(href, "/public/collections/demo/items");
    assert_eq!(
        find_link(entry, "items").unwrap()["type"],
        "application/geo+json"
    );

    let page = body_json(get(&app, &href).await).await;
    assert_eq!(page["type"], "FeatureCollection");
    assert_eq!(page["features"][0]["id"], "a");

    // Requirement 19 `/req/core/sfc-md-success`: the single-collection
    // response's links must include every link its `/collections` entry
    // carries. Asserted as a real string comparison, so the two sites can
    // never drift into advertising different hrefs for the same resource.
    let collection = body_json(get(&app, "/public/collections/demo").await).await;
    assert_eq!(link_href(&collection, "items"), href);
}

/// A tiles-only collection — describable at this root, with no items
/// resource behind it. The model is `#220`'s
/// `a_stac_document_never_links_into_a_root_the_operator_switched_off`:
/// assert BOTH that no `items` link is emitted and that the href one would
/// have pointed at genuinely 404s, so the absence is proven to be the honest
/// answer rather than an oversight.
#[tokio::test]
async fn a_tiles_only_collection_advertises_no_items_link_and_its_items_route_really_404s() {
    let app = build_tiles_only_app();

    // It IS listed and describable — this root serves metadata for anything
    // either lane can back, which is what makes the missing link a decision
    // rather than an omission.
    let listing = body_json(get(&app, "/collections").await).await;
    let entry = &listing["collections"][0];
    assert_eq!(entry["id"], "demo");
    assert!(
        find_link(entry, "items").is_none(),
        "a collection with no features lane must not advertise items: {entry}"
    );

    let collection = body_json(get(&app, "/collections/demo").await).await;
    assert_eq!(collection["id"], "demo");
    assert!(
        find_link(&collection, "items").is_none(),
        "...and neither must its own resource: {collection}"
    );

    // And the resource really is absent, which is what would have made the
    // link a broken promise.
    assert_eq!(
        get(&app, "/collections/demo/items").await.status(),
        StatusCode::NOT_FOUND
    );
}

/// The mirror of the items-link tests, for the classes this root
/// deliberately does NOT declare (`#245`'s audit of the rest of the declared
/// list).
///
/// `CONFORMANCE_CLASSES`' own unit tests assert the three OGC API - Features
/// Part 3 classes are absent from the list. This asserts the other half —
/// that their absence is the honest answer: the Queryables resource
/// (19-079r2 Requirement 4/13, `/collections/{collectionId}/queryables`),
/// which the `Queryables` class defines and the `Filter` class names as its
/// only dependency, is genuinely not mounted at this root. A future slice
/// that mounts it and forgets to declare the class, or declares the class
/// and forgets to mount it, fails one side or the other.
#[tokio::test]
async fn the_undeclared_queryables_classes_correspond_to_routes_this_root_really_lacks() {
    let app = build_app(DEMO_CONFIG);
    for path in ["/collections/demo/queryables", "/queryables"] {
        assert_eq!(
            get(&app, path).await.status(),
            StatusCode::NOT_FOUND,
            "{path} must not exist while the Part 3 classes are withheld"
        );
    }
    // ...while the resource the declared Features class DOES promise is
    // right there, which is what makes withholding one and honouring the
    // other a distinction rather than an inconsistency.
    assert_eq!(
        get(&app, "/collections/demo/items").await.status(),
        StatusCode::OK
    );
}

/// A `TileSource`-only driver: no `feature_source` at all, so
/// `Router::resolve_features` refuses and `/items` 404s while the collection
/// stays describable through the tiles lane.
struct TilesOnlyDriver;

impl StorageDriver for TilesOnlyDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(DemoCatalog)
    }

    fn tile_source(&self) -> Option<Arc<dyn tellurion_core::TileSource>> {
        Some(Arc::new(ItemsTileSource) as Arc<dyn tellurion_core::TileSource>)
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
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
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
    tellurion_stac::router().with_state(Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        None,
        cache,
        style_store,
    )))
}

// -- `proj:epsg` on Items from the derived SRID (`#36`, projection) ----------

/// A features collection whose physical shape is DERIVED (nothing pinned in
/// config, so `Router::effective_decl` runs the descriptor path and carries
/// the catalog's SRID onto the decl): every Item emits `proj:epsg` and
/// declares the projection extension — and emits nothing else, because
/// `transform`/`shape` are raster concepts a vector table does not have.
/// The fully-pinned configs every other test in this file uses take
/// `effective_decl`'s fast path, whose decl carries no SRID — those Items
/// stay byte-identical, which is exactly the existing assertions' proof.
#[tokio::test]
async fn items_of_a_derived_srid_collection_emit_proj_epsg_and_the_extension() {
    let app = build_items_app(
        r#"
storages: [ { id: main, driver: items-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
"#,
        vec![("a", stac_feature("a", json!({})))],
        false,
    );

    let body = body_json(get(&app, "/collections/demo/items").await).await;
    let item = &body["features"][0];
    assert_eq!(item["properties"]["proj:epsg"], json!(4326));
    assert!(item["properties"].get("proj:transform").is_none());
    assert!(item["properties"].get("proj:shape").is_none());
    assert_eq!(item["stac_extensions"], json!([PROJECTION_URI]));

    let single = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert_eq!(single["properties"]["proj:epsg"], json!(4326));
    assert_eq!(single["stac_extensions"], json!([PROJECTION_URI]));
}

// -- raster-backed collections on the STAC root (`#36`, projection) ---------
//
// A COG/Zarr-shaped driver implements `RasterSource` + `CatalogSource` and
// neither `FeatureSource` nor `TileSource`. Before `#36`'s projection slice
// the STAC root omitted such a collection entirely (its sibling Features
// root already listed it, via the canonical capability probe's own
// tiles-or-raster rule) — these tests pin the new tolerance AND the only
// place that collection's driver-read georeferencing can surface: its
// Collection document's `summaries`.

/// A `CatalogSource` shaped like the COG driver's: one physical collection
/// with no table-shaped geometry/pk concepts, a real extent, and the
/// projection facts read from the file's own georeferencing.
struct RasterCatalog;

#[async_trait::async_trait]
impl CatalogSource for RasterCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: "gradient".to_string(),
            geometry_column: None,
            primary_key: None,
            srid: Some(4326),
            geometry_type: None,
        }])
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        Ok(Some(SpatialExtent {
            bbox: [-1.28, -1.28, 1.28, 1.28],
        }))
    }

    async fn projection(
        &self,
        _physical: &PhysicalCollection,
    ) -> CoreResult<Option<ProjectionFacts>> {
        Ok(Some(ProjectionFacts {
            epsg: Some(4326),
            transform: Some([0.01, 0.0, -1.28, 0.0, -0.01, 1.28]),
            shape: Some([256, 256]),
        }))
    }
}

/// A `RasterSource` that never draws anything — these tests exercise the
/// STAC metadata surface, not pixels.
struct EmptyRasterSource;

#[async_trait::async_trait]
impl RasterSource for EmptyRasterSource {
    async fn raster_tile(
        &self,
        _collection: &CollectionDecl,
        _coord: TileCoord,
    ) -> CoreResult<Option<RasterWindow>> {
        Ok(None)
    }
}

struct RasterOnlyDriver;

impl StorageDriver for RasterOnlyDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(RasterCatalog)
    }

    fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
        Some(Arc::new(EmptyRasterSource) as Arc<dyn RasterSource>)
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
storages: [ { id: raster, driver: raster-only-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: gradient
    catalog: default
    storage: raster
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
    tellurion_stac::router().with_state(Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        None,
        cache,
        style_store,
    )))
}

const PROJECTION_URI: &str = "https://stac-extensions.github.io/projection/v1.1.0/schema.json";

/// The raster tolerance and the projection surface together, end to end
/// over HTTP: the collection is listed, its Collection document declares
/// the projection extension exactly because it emits `proj:` summaries the
/// driver genuinely read, and it advertises no `items` link — a raster
/// collection has no rows to page, and the same request answers an error.
#[tokio::test]
async fn a_raster_only_collection_is_listed_with_its_driver_read_projection_summaries() {
    let app = build_raster_only_app();

    let listing = body_json(get(&app, "/collections").await).await;
    let listed: Vec<&str> = listing["collections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(listed, vec!["gradient"]);

    let response = get(&app, "/collections/gradient").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["stac_extensions"], json!([PROJECTION_URI]));
    assert_eq!(body["summaries"]["proj:epsg"], json!([4326]));
    assert_eq!(
        body["summaries"]["proj:transform"],
        json!([[0.01, 0.0, -1.28, 0.0, -0.01, 1.28]])
    );
    assert_eq!(body["summaries"]["proj:shape"], json!([[256, 256]]));
    assert_eq!(
        body["extent"]["spatial"]["bbox"],
        json!([[-1.28, -1.28, 1.28, 1.28]])
    );
    assert!(
        find_link(&body, "items").is_none(),
        "a raster collection has no items resource to advertise: {body}"
    );

    let items = get(&app, "/collections/gradient/items").await;
    assert_ne!(
        items.status(),
        StatusCode::OK,
        "a raster collection must still refuse /items"
    );
}

/// The counter-half of the summaries surface: a features-backed collection
/// (whose catalog reports SRID 4326 but no driver projection facts) keeps a
/// byte-identical Collection document — no `summaries`, no
/// `stac_extensions`. Its EPSG belongs on its Items, where the sidecar
/// override channel and the disagreement log live.
#[tokio::test]
async fn a_vector_collections_document_gains_no_summaries_from_its_srid() {
    let app = build_app(DEMO_CONFIG);
    let body = body_json(get(&app, "/collections/demo").await).await;
    assert!(body.get("summaries").is_none(), "no summaries: {body}");
    assert!(
        body.get("stac_extensions").is_none(),
        "no extension declared without an emitted field: {body}"
    );
}

#[tokio::test]
async fn list_items_paginates_with_keyset_token_round_trip() {
    let app = build_items_app(
        &items_config_yaml(""),
        vec![
            ("a", stac_feature("a", json!({}))),
            ("b", stac_feature("b", json!({}))),
            ("c", stac_feature("c", json!({}))),
        ],
        false,
    );

    let first = get(&app, "/collections/demo/items?limit=2").await;
    assert_eq!(first.status(), StatusCode::OK);
    let body = body_json(first).await;
    assert_eq!(body["type"], "FeatureCollection");
    assert_eq!(body["numberReturned"], 2);
    assert_eq!(body["numberMatched"], 3);
    assert!(find_link(&body, "self").is_some());
    assert!(find_link(&body, "root").is_some());
    assert!(find_link(&body, "collection").is_some());
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
async fn exact_stac_item_geometry_over_budget_is_a_named_422_without_partial_results() {
    let large = json!({
        "type": "Feature",
        "id": "large",
        "geometry": {
            "type": "LineString",
            "coordinates": [[0, 0], [1, 1]]
        },
        "properties": {}
    });
    let app = build_items_app(
        &items_config_yaml("    settings: { items_vertex_budget: 1 }"),
        vec![("large", large)],
        false,
    );

    for uri in [
        "/collections/demo/items",
        "/collections/demo/items/large",
        "/search?collections=demo",
    ] {
        let response = get(&app, uri).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let body = body_json(response).await;
        assert_eq!(body["code"], "ItemsVertexBudgetExceeded", "{uri}");
        assert!(
            body.get("features").is_none(),
            "a refusal must never carry a partial FeatureCollection"
        );
    }
}

#[tokio::test]
async fn item_shape_carries_every_required_stac_member() {
    let app = build_items_app(
        &items_config_yaml("    datetime: observed_at"),
        vec![(
            "a",
            stac_feature("a", json!({ "observed_at": "2020-06-01T00:00:00Z" })),
        )],
        false,
    );
    let response = get(&app, "/collections/demo/items/a").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/geo+json"
    );

    let body = body_json(response).await;
    assert_eq!(body["type"], "Feature");
    assert_eq!(body["stac_version"], "1.1.0");
    assert_eq!(body["id"], "a");
    assert_eq!(body["collection"], "demo");
    assert!(body["geometry"].is_object());
    assert!(body["bbox"].is_array());
    assert!(body["properties"].is_object());
    assert_eq!(body["properties"]["datetime"], "2020-06-01T00:00:00Z");
    assert!(body["links"].is_array());
    assert!(body["assets"].is_object());

    assert!(find_link(&body, "self").is_some());
    assert!(find_link(&body, "root").is_some());
    assert!(find_link(&body, "collection").is_some());
    assert!(find_link(&body, "parent").is_some());
    assert_eq!(
        find_link(&body, "collection").unwrap()["href"],
        "/collections/demo"
    );
}

/// `#36` slice B datetime rule, verified against `stac-spec`'s
/// `item-spec.md`/`commons/common-metadata.md` at the `v1.1.0` tag: a row
/// with no real datetime value gets `properties.datetime: null` and no
/// `start_datetime`/`end_datetime` — the documented honest fallback
/// (`mapping::to_stac_item`'s own doc explains why fabricating a bounds pair
/// would be worse), not a spec-legal-looking-but-fabricated interval.
#[tokio::test]
async fn datetime_is_null_without_a_fabricated_start_end_pair_when_the_collection_has_no_datetime_column(
) {
    let app = build_items_app(
        &items_config_yaml(""),
        vec![("a", stac_feature("a", json!({})))],
        false,
    );
    let response = get(&app, "/collections/demo/items/a").await;
    let body = body_json(response).await;
    assert!(body["properties"]["datetime"].is_null());
    assert!(body["properties"].get("start_datetime").is_none());
    assert!(body["properties"].get("end_datetime").is_none());
}

#[tokio::test]
async fn datetime_is_sourced_from_the_collections_datetime_column_when_the_row_has_one() {
    let app = build_items_app(
        &items_config_yaml("    datetime: observed_at"),
        vec![(
            "a",
            stac_feature("a", json!({ "observed_at": "2021-03-15T12:00:00Z" })),
        )],
        false,
    );
    let response = get(&app, "/collections/demo/items/a").await;
    let body = body_json(response).await;
    assert_eq!(body["properties"]["datetime"], "2021-03-15T12:00:00Z");
}

#[tokio::test]
async fn unknown_item_returns_problem_json_404() {
    let app = build_items_app(&items_config_yaml(""), vec![], false);
    let response = get(&app, "/collections/demo/items/missing").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let body = body_json(response).await;
    assert_eq!(body["code"], "NotFound");
}

#[tokio::test]
async fn unknown_collection_items_route_returns_problem_json_404() {
    let app = build_items_app(&items_config_yaml(""), vec![], false);
    let response = get(&app, "/collections/nope/items").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

// -- assets reflect exactly the collection's advertised capabilities ----

#[tokio::test]
async fn a_features_only_collection_gets_no_tile_or_glb_assets_on_the_collection_or_its_items() {
    let app = build_items_app(
        &items_config_yaml(""),
        vec![("a", stac_feature("a", json!({})))],
        false,
    );

    // The `StacCollection.assets` field is `skip_serializing_if` empty (an
    // OPTIONAL member per the Collection spec, unlike an Item's own
    // unconditionally-REQUIRED `assets`) — so a features-only collection
    // omits the key entirely rather than emitting `{}`.
    let collection = body_json(get(&app, "/collections/demo").await).await;
    assert!(
        collection.get("assets").is_none(),
        "a features-only collection must have no assets: {collection}"
    );

    let item = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert_eq!(
        item["assets"],
        json!({}),
        "a features-only collection's item must have no assets either"
    );
}

#[tokio::test]
async fn a_tiles_capable_collection_gets_mvt_and_png_but_no_glb_asset() {
    let app = build_items_app(
        &items_config_yaml(""),
        vec![("a", stac_feature("a", json!({})))],
        true,
    );

    let collection = body_json(get(&app, "/collections/demo").await).await;
    assert!(collection["assets"]["mvt"]["href"].is_string());
    assert_eq!(
        collection["assets"]["mvt"]["type"],
        "application/vnd.mapbox-vector-tile"
    );
    assert!(collection["assets"]["png"]["href"].is_string());
    assert_eq!(collection["assets"]["png"]["type"], "image/png");
    assert!(
        collection["assets"].get("glb").is_none(),
        "a tiles-only collection must not advertise a glb asset"
    );

    let item = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert!(item["assets"]["mvt"]["href"].is_string());
    assert!(item["assets"].get("glb").is_none());
}

#[tokio::test]
async fn a_places3d_collection_also_gets_a_glb_asset() {
    let app = build_items_app(
        &items_config_yaml("    places3d: { height_property: height }"),
        vec![("a", stac_feature("a", json!({})))],
        true,
    );

    let collection = body_json(get(&app, "/collections/demo").await).await;
    assert!(collection["assets"]["glb"]["href"].is_string());
    assert_eq!(collection["assets"]["glb"]["type"], "model/gltf-binary");

    let item = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert!(item["assets"]["glb"]["href"].is_string());
}

// -- `stac.service_assets` (`#220`) ---------------------------------------

/// The opt-in, end-to-end through the settings chain: with
/// `stac.service_assets: links`, the templated service assets a STAC client
/// cannot dereference disappear from both the Collection and its Items,
/// while the declared `stac.assets` entry — a literal, retrievable href —
/// stays exactly where it was.
#[tokio::test]
async fn service_assets_links_mode_drops_the_templates_and_keeps_declared_assets() {
    let collection_extra = "    places3d: { height_property: height }\n    settings:\n      stac:\n        service_assets: links\n        assets:\n          thumbnail:\n            href: https://example.com/thumb.png\n";
    let app = build_items_app(
        &items_config_yaml(collection_extra),
        vec![("a", stac_feature("a", json!({})))],
        true,
    );

    let collection = body_json(get(&app, "/collections/demo").await).await;
    for key in ["mvt", "png", "glb"] {
        assert!(
            collection["assets"].get(key).is_none(),
            "{key} must not be advertised in links mode: {collection}"
        );
    }
    assert_eq!(
        collection["assets"]["thumbnail"]["href"], "https://example.com/thumb.png",
        "an operator-declared literal asset is untouched"
    );

    let item = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert_eq!(
        item["assets"],
        json!({}),
        "an item of a links-mode collection carries no templated assets either"
    );
}

/// The same collection with the key absent keeps every template — the
/// "unconfigured deployments are byte-for-byte unchanged" half of the same
/// contract, asserted against the identical fixture so the only difference
/// between the two tests is the setting itself.
#[tokio::test]
async fn without_the_setting_every_service_asset_is_still_materialized() {
    let app = build_items_app(
        &items_config_yaml("    places3d: { height_property: height }"),
        vec![("a", stac_feature("a", json!({})))],
        true,
    );

    let collection = body_json(get(&app, "/collections/demo").await).await;
    for key in ["mvt", "png", "glb"] {
        assert!(
            collection["assets"][key]["href"].is_string(),
            "{key} must still be advertised by default: {collection}"
        );
    }
    let item = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert!(item["assets"]["mvt"]["href"].is_string());
}

/// The mode resolves through the ordinary settings chain like every other
/// `stac:` field: declared once at the catalog level, it governs the
/// collection below it — nothing in this slice reads a level of its own.
#[tokio::test]
async fn service_assets_mode_is_inherited_from_a_higher_settings_level() {
    let config_yaml = r#"
storages: [ { id: main, driver: items-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs:
  - id: default
    tenant: public
    settings:
      stac:
        service_assets: links
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#;
    let app = build_items_app(config_yaml, vec![("a", stac_feature("a", json!({})))], true);
    let collection = body_json(get(&app, "/collections/demo").await).await;
    assert!(
        collection.get("assets").is_none(),
        "nothing left to advertise, so the key is omitted entirely: {collection}"
    );
}

/// `#36` design guard, extending slice A's own internal-id leak sweep
/// (`internal_ids_never_appear_in_any_response_body`) to the new items
/// routes: a config where every internal id is deliberately distinct from —
/// and textually unrelated to — its external id must never surface an
/// internal id in an items response body either.
#[tokio::test]
async fn internal_ids_never_appear_in_items_response_bodies() {
    const TENANT_INTERNAL: &str = "zzz-tenant-internal-marker";
    const CATALOG_INTERNAL: &str = "zzz-catalog-internal-marker";
    const COLLECTION_INTERNAL: &str = "zzz-collection-internal-marker";

    let config_yaml = format!(
        r#"
storages: [ {{ id: main, driver: items-fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: {TENANT_INTERNAL}, external_id: acme }} ]
catalogs: [ {{ id: {CATALOG_INTERNAL}, external_id: default, tenant: {TENANT_INTERNAL} }} ]
collections:
  - id: {COLLECTION_INTERNAL}
    external_id: demo
    catalog: {CATALOG_INTERNAL}
    storage: main
    table: demo
    geometry: geom
    pk: id
"#
    );

    let ctx = build_items_ctx(
        &config_yaml,
        vec![("a", stac_feature("a", json!({})))],
        true,
    );
    let app = axum::Router::new()
        .nest("/{tenant}", tellurion_stac::router())
        .with_state(ctx);

    let paths = [
        "/acme/collections/demo/items".to_string(),
        "/acme/collections/demo/items/a".to_string(),
    ];

    for path in paths {
        let response = get(&app, &path).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(status, StatusCode::OK, "path {path} was not 200: {text}");
        assert!(
            !text.contains(TENANT_INTERNAL),
            "{path} leaked the tenant internal id: {text}"
        );
        assert!(
            !text.contains(CATALOG_INTERNAL),
            "{path} leaked the catalog internal id: {text}"
        );
        assert!(
            !text.contains(COLLECTION_INTERNAL),
            "{path} leaked the collection internal id: {text}"
        );
        // The tile asset hrefs use the external tenant/catalog/collection
        // ids too — confirms `asset_capabilities`/`collection_assets` never
        // leaked an internal id into a templated href either.
        assert!(text.contains("/acme/tiles/catalogs/default/collections/demo/"));
    }
}

/// A collection with a configured `stac:` settings subtree (license,
/// keywords, providers) serves those values instead of the crate's
/// defaults.
#[tokio::test]
async fn configured_stac_settings_are_honored() {
    let config_yaml = r#"
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
    settings:
      stac:
        license: CC-BY-4.0
        keywords: [imagery, satellite]
        providers:
          - name: Example Provider
            roles: [producer]
            url: https://example.com
"#;
    let app = build_app(config_yaml);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["license"], "CC-BY-4.0");
    assert_eq!(
        body["keywords"],
        serde_json::json!(["imagery", "satellite"])
    );
    assert_eq!(body["providers"][0]["name"], "Example Provider");
}

// -- declared `stac.assets` (`#36` slice 1, "a real, driver-neutral assets
// model") ------------------------------------------------------------------

/// A collection with a declared `stac.assets` block serves those assets on
/// the STAC Collection response with the exact spec-conformant shape: `href`
/// always present, `type`/`title`/`roles` carried through when declared and
/// omitted entirely when not (never a fabricated `""`/`[]`). This collection
/// has no tiles/places3d capability at all, so `thumbnail`/`doc` prove
/// declared assets appear on their own, not only alongside capability-
/// derived ones.
#[tokio::test]
async fn configured_stac_assets_appear_on_the_collection_with_the_declared_shape() {
    let config_yaml = r#"
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
    settings:
      stac:
        assets:
          thumbnail:
            href: https://example.com/thumb.png
            type: image/png
            title: Thumbnail
            roles: [thumbnail]
          doc:
            href: https://example.com/doc.pdf
"#;
    let app = build_app(config_yaml);
    let response = get(&app, "/collections/demo").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    let thumbnail = &body["assets"]["thumbnail"];
    assert_eq!(thumbnail["href"], "https://example.com/thumb.png");
    assert_eq!(thumbnail["type"], "image/png");
    assert_eq!(thumbnail["title"], "Thumbnail");
    assert_eq!(thumbnail["roles"], json!(["thumbnail"]));
    assert_eq!(thumbnail["templated"], false);

    // `doc` declared only `href` — `type`/`title`/`roles` must be absent
    // from the wire, not defaulted to `""`/`[]`.
    let doc = &body["assets"]["doc"];
    assert_eq!(doc["href"], "https://example.com/doc.pdf");
    assert!(doc.get("type").is_none(), "type must be omitted: {doc}");
    assert!(doc.get("title").is_none(), "title must be omitted: {doc}");
    assert!(doc.get("roles").is_none(), "roles must be omitted: {doc}");
}

/// A collection with neither a declared `stac.assets` block nor any
/// capability-derived asset omits the `assets` key entirely — declaring the
/// new mechanism must not change this no-regression baseline.
#[tokio::test]
async fn a_collection_with_no_declared_or_capability_assets_still_omits_the_key() {
    let app = build_app(DEMO_CONFIG);
    let response = get(&app, "/collections/demo").await;
    let body = body_json(response).await;
    assert!(body.get("assets").is_none());
}

/// Declared collection-level assets are a STAC Collection concept (`#36`
/// slice 1) — they must NOT also ride onto every item the way the
/// capability-derived assets already do (see `to_stac_collection`'s own doc
/// for why). An item's `assets` stays exactly what the capability-derived
/// mechanism produces (empty here, since `with_tiles` is `false`), even
/// though the collection itself carries a declared asset.
#[tokio::test]
async fn declared_stac_assets_do_not_appear_on_items() {
    let collection_extra = "    settings:\n      stac:\n        assets:\n          doc:\n            href: https://example.com/doc.pdf\n";
    let app = build_items_app(
        &items_config_yaml(collection_extra),
        vec![("a", stac_feature("a", json!({})))],
        false,
    );

    let collection = body_json(get(&app, "/collections/demo").await).await;
    assert!(collection["assets"]["doc"]["href"].is_string());

    let item = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert_eq!(
        item["assets"],
        json!({}),
        "a declared collection-level asset must not appear on an item"
    );
}

/// A declared asset id colliding with a capability-derived one (`mvt`) wins
/// on the collection response — the operator's explicit intent beats the
/// generated default. The item's own `mvt` asset is untouched (still the
/// capability-derived one), since declared assets never reach items at all
/// (the previous test).
#[tokio::test]
async fn a_declared_asset_overrides_a_capability_derived_asset_of_the_same_id_on_the_collection() {
    let collection_extra = "    settings:\n      stac:\n        assets:\n          mvt:\n            href: https://example.com/custom-mvt\n            title: Operator override\n";
    let app = build_items_app(
        &items_config_yaml(collection_extra),
        vec![("a", stac_feature("a", json!({})))],
        true,
    );

    let collection = body_json(get(&app, "/collections/demo").await).await;
    assert_eq!(
        collection["assets"]["mvt"]["href"],
        "https://example.com/custom-mvt"
    );
    assert_eq!(collection["assets"]["mvt"]["title"], "Operator override");

    let item = body_json(get(&app, "/collections/demo/items/a").await).await;
    assert!(
        item["assets"]["mvt"]["href"]
            .as_str()
            .unwrap()
            .ends_with(".mvt"),
        "an item's own mvt asset must stay capability-derived: {item}"
    );
}

// -- /search (`#36` slice C) ---------------------------------------------
//
// A dedicated in-memory `FeatureSource` (`FilterableFeatureSource`, below)
// that genuinely evaluates `ItemsQuery::bbox`/`datetime`/`filter` against
// its fixture data, unlike `ItemsFeatureSource` above (which never looks at
// any of the three). Real SQL-evaluation correctness is `tellurion-postgis`'s
// own test suite's job (`sql.rs`'s golden tests, `tests/live.rs`'s round
// trip) — what these tests prove is this crate's own responsibility: GET and
// POST both parse to the identical `Filter`/`ItemsQuery`, `intersects`
// composes into the same `Filter::Intersects` node `S_INTERSECTS` already
// uses, both CQL2 encodings parse to filters that narrow identically, the
// capability gate 400s/skips correctly, and cross-collection paging produces
// a stable, complete, token-round-tripping result stream.

/// One fixture collection's in-memory `FeatureSource`: paginates by index
/// (same keyset-by-id convention `ItemsFeatureSource` above uses) and
/// genuinely narrows by `bbox` (point-in-box on the feature's own Point
/// geometry), `datetime` (lexicographic compare — fine for same-format RFC
/// 3339 UTC fixtures), and `filter` (`eval_filter`, below — a small,
/// self-contained evaluator covering exactly the operators these tests
/// exercise, not a general CQL2 implementation).
struct FilterableFeatureSource {
    items: Vec<(String, Value)>,
    filter_capable: bool,
    /// `#248`: whether this fixture driver claims it can transform a filter's
    /// spatial literals into the CRS a `filter-crs` names — PostGIS's one
    /// override, `false` everywhere else in this workspace.
    filter_crs_capable: bool,
    /// `#255`: whether this fixture driver claims it can reproject at all —
    /// the capability a `bbox` rides (`ItemsQuery::bbox_crs`'s own doc), kept
    /// separate from `filter_crs_capable` above because the trait keeps them
    /// separate. `false` for every driver in this workspace but PostGIS, and
    /// so for every fixture that does not opt in.
    crs_capable: bool,
    /// `#248`: every `ItemsQuery::filter_crs` this source was actually handed,
    /// in call order. The whole point of the parameter is that it reaches the
    /// compiler, so "the handler passed it through" is the thing to assert;
    /// this fixture cannot reproject anything, so it could never demonstrate
    /// it by narrowing rows the way real PostGIS does in `tellurion-server`'s
    /// own live test.
    seen_filter_crs: std::sync::Mutex<Vec<RequestedCrs>>,
}

#[async_trait::async_trait]
impl FeatureSource for FilterableFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        self.seen_filter_crs.lock().unwrap().push(query.filter_crs);
        let start_index = match &query.token {
            Some(token) => self
                .items
                .iter()
                .position(|(id, _)| id == token)
                .map(|i| i + 1)
                .unwrap_or(0),
            None => 0,
        };
        let matched: Vec<&(String, Value)> = self.items[start_index..]
            .iter()
            .filter(|(_, feature)| matches_query(feature, query))
            .collect();
        let limit = query.limit as usize;
        let has_more = matched.len() > limit;
        let page: Vec<Value> = matched
            .iter()
            .take(limit)
            .map(|(_, v)| (*v).clone())
            .collect();
        let next_token = has_more.then(|| matched[limit - 1].0.clone());

        Ok(FeaturePage {
            features_geojson: page,
            number_matched: Some(matched.len() as u64),
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

    fn filter_capable(&self) -> bool {
        self.filter_capable
    }

    fn filter_crs_capable(&self) -> bool {
        self.filter_crs_capable
    }

    fn crs_capable(&self) -> bool {
        self.crs_capable
    }
}

fn point_of(feature: &Value) -> Option<[f64; 2]> {
    let coords = feature.get("geometry")?.get("coordinates")?.as_array()?;
    Some([coords.first()?.as_f64()?, coords.get(1)?.as_f64()?])
}

/// `[minx, miny, maxx, maxy]` spanning every numeric coordinate pair inside
/// `geom["coordinates"]` — the same "walk the nested arrays for a leaf
/// `[x, y, ...]` pair" approach `mapping::bbox_from_geometry` uses in this
/// crate's own source (not reused directly: that function is private to
/// `mapping`), narrowed here to exactly what these fixtures need (a Point
/// falling inside a Polygon's own bounding box — good enough to prove
/// `intersects` composes and narrows, not a real point-in-polygon test).
fn bbox_of_geojson(geom: &Value) -> Option<[f64; 4]> {
    fn walk(value: &Value, min: &mut [f64; 2], max: &mut [f64; 2], found: &mut bool) {
        let Some(items) = value.as_array() else {
            return;
        };
        let is_leaf = items.len() >= 2 && items.iter().take(2).all(Value::is_number);
        if is_leaf {
            let x = items[0].as_f64().unwrap_or(f64::NAN);
            let y = items[1].as_f64().unwrap_or(f64::NAN);
            if x.is_finite() && y.is_finite() {
                min[0] = min[0].min(x);
                min[1] = min[1].min(y);
                max[0] = max[0].max(x);
                max[1] = max[1].max(y);
                *found = true;
            }
            return;
        }
        for item in items {
            walk(item, min, max, found);
        }
    }

    let mut min = [f64::INFINITY, f64::INFINITY];
    let mut max = [f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut found = false;
    walk(geom.get("coordinates")?, &mut min, &mut max, &mut found);
    found.then_some([min[0], min[1], max[0], max[1]])
}

fn eval_filter(filter: &Filter, feature: &Value) -> bool {
    match filter {
        Filter::Compare {
            property,
            op,
            value,
        } => {
            let actual = feature["properties"].get(property);
            match (actual, value) {
                (Some(Value::String(s)), Literal::Text(t)) => match op {
                    CompareOp::Eq => s == t,
                    CompareOp::Ne => s != t,
                    _ => false,
                },
                (Some(Value::Number(n)), Literal::Number(t)) => {
                    let n = n.as_f64().unwrap_or(f64::NAN);
                    match op {
                        CompareOp::Eq => n == *t,
                        CompareOp::Ne => n != *t,
                        CompareOp::Lt => n < *t,
                        CompareOp::Gt => n > *t,
                        CompareOp::Le => n <= *t,
                        CompareOp::Ge => n >= *t,
                    }
                }
                _ => false,
            }
        }
        Filter::IsNull { property, negated } => {
            let is_null = feature["properties"]
                .get(property)
                .map(Value::is_null)
                .unwrap_or(true);
            is_null != *negated
        }
        Filter::And(items) => items.iter().all(|f| eval_filter(f, feature)),
        Filter::Or(items) => items.iter().any(|f| eval_filter(f, feature)),
        Filter::Not(inner) => !eval_filter(inner, feature),
        Filter::Intersects { geometry, .. } => {
            let Some(point) = point_of(feature) else {
                return false;
            };
            let bbox = match geometry {
                GeometryLiteral::Bbox(bbox) => Some(*bbox),
                GeometryLiteral::GeoJson(geom) => bbox_of_geojson(geom),
                // Not exercised by any test in this file — see the
                // `Filter::After | ... => true` fallback below for the same
                // reasoning applied to the WKT geometry literal shape.
                GeometryLiteral::Wkt(_) => None,
            };
            match bbox {
                Some([minx, miny, maxx, maxy]) => {
                    point[0] >= minx && point[0] <= maxx && point[1] >= miny && point[1] <= maxy
                }
                None => false,
            }
        }
        // Not exercised by any test in this file — a real temporal-operator
        // evaluation would duplicate `tellurion-postgis`'s own SQL semantics
        // for no test benefit here. Same reasoning covers the advanced
        // comparison/CASEI/new-spatial operators below: no test in this file
        // builds one of these, so there's nothing here to duplicate
        // `tellurion-postgis`'s own SQL semantics for.
        Filter::After { .. }
        | Filter::Before { .. }
        | Filter::During { .. }
        | Filter::Like { .. }
        | Filter::Between { .. }
        | Filter::In { .. }
        | Filter::CaseInsensitiveCompare { .. }
        | Filter::Spatial { .. }
        | Filter::Temporal { .. } => true,
    }
}

fn matches_query(feature: &Value, query: &ItemsQuery) -> bool {
    if let Some(bbox) = query.bbox {
        match point_of(feature) {
            Some([x, y]) => {
                if !(x >= bbox[0] && x <= bbox[2] && y >= bbox[1] && y <= bbox[3]) {
                    return false;
                }
            }
            None => return false,
        }
    }
    if let Some(range) = &query.datetime {
        let Some(dt) = feature["properties"]["observed_at"].as_str() else {
            return false;
        };
        if let Some(start) = &range.start {
            if dt < start.as_str() {
                return false;
            }
        }
        if let Some(end) = &range.end {
            if dt > end.as_str() {
                return false;
            }
        }
    }
    if let Some(filter) = &query.filter {
        if !eval_filter(filter, feature) {
            return false;
        }
    }
    true
}

struct FilterableCatalog {
    table: String,
    /// `#248`: the storage SRID this fixture collection reports. 4326 for
    /// every pre-existing test (the value this catalog hardcoded before), and
    /// a projected one where a test needs `filter-crs=CRS84` to be something a
    /// driver would have to genuinely transform for.
    srid: Option<i32>,
}

#[async_trait::async_trait]
impl CatalogSource for FilterableCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: self.table.clone(),
            geometry_column: Some("geom".to_string()),
            primary_key: Some("id".to_string()),
            srid: self.srid,
            geometry_type: None,
        }])
    }

    /// `filter::validate` (`#33`) rejects any property not reported here —
    /// every fixture's `search_feature` carries exactly these three, so
    /// declaring them is what lets `filter`/`intersects`-bearing tests reach
    /// `FilterableFeatureSource::items` at all rather than 400ing on
    /// property validation before ever getting there.
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
                name: "observed_at".to_string(),
                sql_type: "timestamptz".to_string(),
            },
        ]))
    }
}

struct FilterableDriver {
    table: String,
    srid: Option<i32>,
    source: Arc<FilterableFeatureSource>,
}

impl StorageDriver for FilterableDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(FilterableCatalog {
            table: self.table.clone(),
            srid: self.srid,
        })
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(self.source.clone() as Arc<dyn FeatureSource>)
    }
}

struct FilterableFactory {
    drivers: HashMap<String, (String, Option<i32>, Arc<FilterableFeatureSource>)>,
}

impl DriverFactory for FilterableFactory {
    fn name(&self) -> &str {
        "filterable-fake"
    }

    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        let (table, srid, source) = self
            .drivers
            .get(&decl.id)
            .cloned()
            .expect("every storage this factory builds was configured with a fixture");
        Ok(Arc::new(FilterableDriver {
            table,
            srid,
            source,
        }))
    }
}

/// One fixture collection for [`build_search_app`]: `id` doubles as both the
/// collection's external id and its physical table name (kept equal for
/// fixture simplicity — nothing in these tests exercises id-vs-table
/// divergence).
struct SearchCollectionFixture {
    id: &'static str,
    items: Vec<(&'static str, Value)>,
    filter_capable: bool,
    /// `#248`: PostGIS's own capability, `false` for every other driver in
    /// this workspace and so for every fixture that does not opt in.
    filter_crs_capable: bool,
    /// `#255`: PostGIS's other capability — reprojecting at all, which is what
    /// a `bbox` rides. Same default and same reason as its sibling above.
    crs_capable: bool,
    /// `#248`: the storage SRID this collection reports; `Some(4326)` is what
    /// every fixture reported before, and CRS84-equivalent for filter
    /// literals, so a CRS84 `filter-crs` costs a driver nothing there.
    srid: Option<i32>,
    /// `#248`: whether to let `Router::effective_decl` derive this
    /// collection's physical descriptor instead of taking the pinned
    /// `table`/`geometry`/`pk` fast path. Only a derived descriptor carries
    /// `srid` onto the decl a handler sees (`CollectionDecl::srid`'s own doc:
    /// "never operator-configured"), so any test whose subject is the storage
    /// CRS has to opt in — the same reason `tellurion-server`'s live PostGIS
    /// fixture omits all three fields. Off by default so every pre-`#248`
    /// search test keeps the exact decl it always had.
    derive_physical: bool,
    datetime_column: Option<&'static str>,
}

fn search_collection(
    id: &'static str,
    items: Vec<(&'static str, Value)>,
) -> SearchCollectionFixture {
    SearchCollectionFixture {
        id,
        items,
        filter_capable: true,
        filter_crs_capable: false,
        crs_capable: false,
        srid: Some(4326),
        derive_physical: false,
        datetime_column: Some("observed_at"),
    }
}

/// One catalog (`default`, tenant `public`), one collection per fixture in
/// `specs`, each backed by its own storage + `FilterableFeatureSource` — so
/// different collections in the same search can have different fixture data
/// and different `filter_capable` answers, which the cross-collection
/// capability-skip tests need.
fn build_search_app(specs: Vec<SearchCollectionFixture>) -> axum::Router {
    build_search_app_with_sources(specs).0
}

/// [`build_search_app`] plus each fixture's own `FilterableFeatureSource`,
/// keyed by collection id — needed only by the `#248` tests, which assert on
/// what the handler *handed the driver* (`ItemsQuery::filter_crs`) rather than
/// only on the response body.
#[allow(clippy::type_complexity)]
fn build_search_app_with_sources(
    specs: Vec<SearchCollectionFixture>,
) -> (axum::Router, HashMap<String, Arc<FilterableFeatureSource>>) {
    let mut drivers: HashMap<String, (String, Option<i32>, Arc<FilterableFeatureSource>)> =
        HashMap::new();
    let mut by_collection: HashMap<String, Arc<FilterableFeatureSource>> = HashMap::new();
    let mut collections_yaml = String::new();
    for spec in &specs {
        let storage_id = format!("storage-{}", spec.id);
        let source = Arc::new(FilterableFeatureSource {
            items: spec
                .items
                .iter()
                .map(|(id, v)| (id.to_string(), v.clone()))
                .collect(),
            filter_capable: spec.filter_capable,
            filter_crs_capable: spec.filter_crs_capable,
            crs_capable: spec.crs_capable,
            seen_filter_crs: std::sync::Mutex::new(Vec::new()),
        });
        by_collection.insert(spec.id.to_string(), source.clone());
        drivers.insert(storage_id.clone(), (spec.id.to_string(), spec.srid, source));
        collections_yaml.push_str(&format!(
            "  - id: {id}\n    catalog: default\n    storage: {storage_id}\n    table: {id}\n    geometry: geom\n",
            id = spec.id
        ));
        // Pinning `pk` too is what sends `Router::effective_decl` down its
        // fully-overridden fast path, which never derives (and so never
        // carries) the storage SRID — see `SearchCollectionFixture::
        // derive_physical`.
        if !spec.derive_physical {
            collections_yaml.push_str("    pk: id\n");
        }
        if let Some(dt) = spec.datetime_column {
            collections_yaml.push_str(&format!("    datetime: {dt}\n"));
        }
    }
    let storages_yaml: String = drivers
        .keys()
        .map(|id| format!("  - {{ id: {id}, driver: filterable-fake, url_env: DATABASE_URL }}\n"))
        .collect();
    let config_yaml = format!(
        "storages:\n{storages_yaml}tenants: [ {{ id: public }} ]\ncatalogs: [ {{ id: default, tenant: public }} ]\ncollections:\n{collections_yaml}"
    );

    let config: AppConfig = serde_yaml::from_str(&config_yaml).unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(FilterableFactory { drivers }));

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
    (tellurion_stac::router().with_state(ctx), by_collection)
}

/// A single-item-shaped Point feature at `(lon, lat)`, carrying `name`,
/// `population`, and `observed_at` — the three properties these tests filter
/// on.
fn search_feature(
    id: &str,
    lon: f64,
    lat: f64,
    name: &str,
    population: i64,
    observed_at: &str,
) -> Value {
    json!({
        "type": "Feature",
        "id": id,
        "geometry": { "type": "Point", "coordinates": [lon, lat] },
        "properties": { "name": name, "population": population, "observed_at": observed_at },
    })
}

async fn post(app: &axum::Router, uri: impl AsRef<str>, body: Value) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri.as_ref())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn item_ids(body: &Value) -> Vec<String> {
    body["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn search_get_and_post_return_the_same_items_for_equivalent_parameters() {
    let app = build_search_app(vec![search_collection(
        "demo",
        vec![
            (
                "a",
                search_feature("a", 1.0, 1.0, "alpha", 10, "2020-01-01T00:00:00Z"),
            ),
            (
                "b",
                search_feature("b", 5.0, 5.0, "beta", 20, "2020-06-01T00:00:00Z"),
            ),
        ],
    )]);

    let get_response = get(&app, "/search?collections=demo&bbox=0,0,2,2").await;
    assert_eq!(get_response.status(), StatusCode::OK);
    let get_body = body_json(get_response).await;
    assert_eq!(get_body["type"], "FeatureCollection");

    let post_response = post(
        &app,
        "/search",
        json!({ "collections": ["demo"], "bbox": [0.0, 0.0, 2.0, 2.0] }),
    )
    .await;
    assert_eq!(post_response.status(), StatusCode::OK);
    let post_body = body_json(post_response).await;

    assert_eq!(item_ids(&get_body), vec!["a".to_string()]);
    assert_eq!(item_ids(&get_body), item_ids(&post_body));
    assert_eq!(get_body["numberReturned"], post_body["numberReturned"]);
}

#[tokio::test]
async fn collections_parameter_narrows_to_the_named_collection() {
    let app = build_search_app(vec![
        search_collection(
            "alpha",
            vec![(
                "a1",
                search_feature("a1", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
            )],
        ),
        search_collection(
            "beta",
            vec![(
                "b1",
                search_feature("b1", 2.0, 2.0, "y", 2, "2020-01-01T00:00:00Z"),
            )],
        ),
    ]);

    let response = get(&app, "/search?collections=alpha").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(item_ids(&body), vec!["a1".to_string()]);
    // The single-collection fast path's `numberMatched` is a real, driver-
    // reported total (unlike the cross-collection fan-out's, which is always
    // `None` — see `collections_omitted_fans_out_and_merges_every_collection`
    // below).
    assert_eq!(body["numberMatched"], 1);
}

#[tokio::test]
async fn ids_parameter_returns_items_by_id_across_collections() {
    let app = build_search_app(vec![
        search_collection(
            "alpha",
            vec![
                (
                    "a1",
                    search_feature("a1", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
                ),
                (
                    "a2",
                    search_feature("a2", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
                ),
            ],
        ),
        search_collection(
            "beta",
            vec![(
                "b1",
                search_feature("b1", 2.0, 2.0, "y", 2, "2020-01-01T00:00:00Z"),
            )],
        ),
    ]);

    let response = get(&app, "/search?ids=a1,b1,missing").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let mut ids = item_ids(&body);
    ids.sort();
    assert_eq!(ids, vec!["a1".to_string(), "b1".to_string()]);
    assert!(
        find_link(&body, "next").is_none(),
        "the full ids set fit in one page"
    );

    // POST parity for the same lookup.
    let post_response = post(&app, "/search", json!({ "ids": ["a1", "b1", "missing"] })).await;
    let post_body = body_json(post_response).await;
    let mut post_ids = item_ids(&post_body);
    post_ids.sort();
    assert_eq!(post_ids, ids);
}

#[tokio::test]
async fn bbox_and_datetime_both_narrow_within_a_single_collection() {
    let app = build_search_app(vec![search_collection(
        "demo",
        vec![
            (
                "a",
                search_feature("a", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
            ),
            (
                "b",
                search_feature("b", 1.0, 1.0, "x", 1, "2021-01-01T00:00:00Z"),
            ),
            (
                "c",
                search_feature("c", 9.0, 9.0, "x", 1, "2020-01-01T00:00:00Z"),
            ),
        ],
    )]);

    // bbox alone excludes "c" (far outside).
    let bbox_only = body_json(get(&app, "/search?bbox=0,0,2,2").await).await;
    let mut bbox_ids = item_ids(&bbox_only);
    bbox_ids.sort();
    assert_eq!(bbox_ids, vec!["a".to_string(), "b".to_string()]);

    // bbox + datetime together narrow to exactly "a".
    let combined = body_json(
        get(
            &app,
            "/search?bbox=0,0,2,2&datetime=2019-01-01T00:00:00Z/2020-12-31T00:00:00Z",
        )
        .await,
    )
    .await;
    assert_eq!(item_ids(&combined), vec!["a".to_string()]);
}

#[tokio::test]
async fn filter_narrows_identically_through_cql2_text_and_cql2_json() {
    let app = build_search_app(vec![search_collection(
        "demo",
        vec![
            (
                "a",
                search_feature("a", 1.0, 1.0, "alpha", 100, "2020-01-01T00:00:00Z"),
            ),
            (
                "b",
                search_feature("b", 1.0, 1.0, "beta", 200, "2020-01-01T00:00:00Z"),
            ),
        ],
    )]);

    let text =
        body_json(get(&app, "/search?filter=name='alpha'&filter-lang=cql2-text").await).await;
    assert_eq!(item_ids(&text), vec!["a".to_string()]);

    let json_filter = r#"{"op":"=","args":[{"property":"name"},"alpha"]}"#;
    let json_href = format!(
        "/search?filter={}&filter-lang=cql2-json",
        urlencoding_minimal(json_filter)
    );
    let json_lang = body_json(get(&app, &json_href).await).await;
    assert_eq!(item_ids(&json_lang), item_ids(&text));

    // POST defaults filter-lang to cql2-json when omitted.
    let post_body = post(
        &app,
        "/search",
        json!({ "filter": { "op": "=", "args": [{ "property": "name" }, "alpha"] } }),
    )
    .await;
    assert_eq!(item_ids(&body_json(post_body).await), item_ids(&text));
}

/// Minimal query-value percent-encoding for this test file's own GET
/// requests — this crate's real percent-encoding (`params::percent_encode`)
/// is private; axum's `Query` extractor only needs the reserved characters a
/// CQL2-JSON filter's `{}":,` actually contain escaped.
fn urlencoding_minimal(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[tokio::test]
async fn intersects_composes_into_the_same_filter_path_as_s_intersects() {
    let app = build_search_app(vec![search_collection(
        "demo",
        vec![
            (
                "inside",
                search_feature("inside", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
            ),
            (
                "outside",
                search_feature("outside", 9.0, 9.0, "x", 1, "2020-01-01T00:00:00Z"),
            ),
        ],
    )]);

    let geometry = json!({
        "type": "Polygon",
        "coordinates": [[[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0], [0.0, 0.0]]],
    });
    let href = format!(
        "/search?intersects={}",
        urlencoding_minimal(&geometry.to_string())
    );
    let get_body = body_json(get(&app, &href).await).await;
    assert_eq!(item_ids(&get_body), vec!["inside".to_string()]);

    let post_body = body_json(post(&app, "/search", json!({ "intersects": geometry })).await).await;
    assert_eq!(item_ids(&post_body), vec!["inside".to_string()]);
}

#[tokio::test]
async fn bbox_and_intersects_together_is_a_400() {
    let app = build_search_app(vec![search_collection("demo", vec![])]);
    let geometry = json!({ "type": "Point", "coordinates": [1.0, 1.0] });
    let href = format!(
        "/search?bbox=0,0,1,1&intersects={}",
        urlencoding_minimal(&geometry.to_string())
    );
    let response = get(&app, &href).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn a_single_named_collection_that_cannot_filter_is_a_400() {
    let mut demo = search_collection(
        "demo",
        vec![(
            "a",
            search_feature("a", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
        )],
    );
    demo.filter_capable = false;
    let app = build_search_app(vec![demo]);

    let response = get(&app, "/search?collections=demo&filter=name='x'").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    assert!(body["detail"].as_str().unwrap().contains("demo"));
}

/// The cross-collection fan-out's own documented judgment call: a collection
/// that can't filter is silently skipped (not a 400) when it's one of
/// several candidates, so the whole search still answers with whatever the
/// capable collections found.
#[tokio::test]
async fn cross_collection_search_skips_a_collection_that_cannot_filter_instead_of_400ing() {
    let mut alpha = search_collection(
        "alpha",
        vec![(
            "a1",
            search_feature("a1", 1.0, 1.0, "match", 1, "2020-01-01T00:00:00Z"),
        )],
    );
    alpha.filter_capable = true;
    let mut beta = search_collection(
        "beta",
        vec![(
            "b1",
            search_feature("b1", 1.0, 1.0, "match", 1, "2020-01-01T00:00:00Z"),
        )],
    );
    beta.filter_capable = false;
    let app = build_search_app(vec![alpha, beta]);

    let response = get(&app, "/search?filter=name='match'").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(item_ids(&body), vec!["a1".to_string()]);
}

// -- filter-crs on /search (`#248`, STAC API Filter Extension) --------------
//
// The extension gives this parameter a much narrower value space than OGC
// API — Features Part 3 gives the identically-named one on `/items`
// (`search::resolve_search_filter_crs` carries both quotes verbatim): CRS84 is
// the default AND the only value a server must accept, and it may reject any
// others. Before `#248` the parameter had nowhere to land at all — a
// `filter-crs` was dropped and the filter's geometries processed in CRS84
// regardless, which is a `200` carrying rows selected in a CRS the client
// never named.

/// EPSG:4326 referenced *by authority* — latitude-before-longitude, and
/// exactly what a Part 3 client that had read `/items`' own `filter-crs`
/// documentation would send here. On `/search` it is refused by name.
fn epsg_4326_uri() -> String {
    tellurion_core::crs::epsg_uri(4326)
}

fn crs84_uri() -> String {
    tellurion_core::crs::CRS84_URI.to_string()
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn one_collection_app(
    filter_crs_capable: bool,
    srid: Option<i32>,
) -> (axum::Router, HashMap<String, Arc<FilterableFeatureSource>>) {
    let mut demo = search_collection(
        "demo",
        vec![(
            "a",
            search_feature("a", 10.0, 45.0, "alpha", 1, "2020-01-01T00:00:00Z"),
        )],
    );
    demo.filter_crs_capable = filter_crs_capable;
    demo.srid = srid;
    demo.derive_physical = true;
    build_search_app_with_sources(vec![demo])
}

/// Campaign rule 1, executed: a request that supplies no `filter-crs` hands
/// the driver `RequestedCrs::Omitted` — the value every compiler in this
/// workspace treats as "compile exactly what you always compiled" — whatever
/// the collection's storage SRID happens to be.
#[tokio::test]
async fn an_absent_filter_crs_hands_the_driver_omitted_for_every_storage_srid() {
    for srid in [None, Some(4326), Some(3857)] {
        let (app, sources) = one_collection_app(true, srid);
        let response = get(&app, "/search?collections=demo&filter=name='alpha'").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(item_ids(&body_json(response).await), vec!["a".to_string()]);
        assert_eq!(
            *sources["demo"].seen_filter_crs.lock().unwrap(),
            vec![RequestedCrs::Omitted],
            "srid {srid:?}: an absent filter-crs must reach the driver as Omitted"
        );
    }
}

/// The other half of the same rule, on the POST lane: no `filter-crs` key in
/// the body is the same `Omitted`, so a body that never mentions the
/// parameter is served exactly as it was before `#248`.
#[tokio::test]
async fn a_post_body_without_filter_crs_hands_the_driver_omitted() {
    let (app, sources) = one_collection_app(false, Some(3857));
    let response = post(
        &app,
        "/search",
        json!({ "collections": ["demo"], "filter": "name='alpha'", "filter-lang": "cql2-text" }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        *sources["demo"].seen_filter_crs.lock().unwrap(),
        vec![RequestedCrs::Omitted]
    );
}

/// An explicit CRS84 is honoured, not merely tolerated: it reaches the driver
/// as `RequestedCrs::Crs84`, which is a *different* instruction from
/// `Omitted` — on a projected collection PostGIS turns it into a real
/// `ST_Transform` of the filter's spatial literals
/// (`tellurion-postgis::sql::geometry_literal_expr`).
///
/// Both encodings in one test on purpose: the extension names "three GET query
/// parameters or POST JSON fields" and re-spells none of them for the body, so
/// the parameter is `filter-crs` either way and must resolve identically.
#[tokio::test]
async fn an_explicit_crs84_filter_crs_reaches_the_driver_as_crs84_on_get_and_post() {
    let (app, sources) = one_collection_app(true, Some(3857));
    let response = get(
        &app,
        &format!(
            "/search?collections=demo&filter=name='alpha'&filter-crs={}",
            urlencode(&crs84_uri())
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = post(
        &app,
        "/search",
        json!({
            "collections": ["demo"],
            "filter": "name='alpha'",
            "filter-lang": "cql2-text",
            "filter-crs": crs84_uri(),
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    assert_eq!(
        *sources["demo"].seen_filter_crs.lock().unwrap(),
        vec![RequestedCrs::Crs84, RequestedCrs::Crs84],
        "the GET query parameter and the POST body field are the same parameter, spelled \
         'filter-crs' in both"
    );
}

/// The refusal `#248` exists for, on both HTTP methods: a `filter-crs` naming
/// any CRS but CRS84 is a 400 **naming the parameter**, never a 200 whose rows
/// were selected by reading the filter's geometry in a CRS the client did not
/// ask for. The URI used is EPSG:4326 by authority — datum-identical to CRS84,
/// opposite axis order — precisely the value that changes which rows match
/// while looking harmless.
#[tokio::test]
async fn a_non_crs84_filter_crs_is_refused_by_name_on_get_and_post() {
    let (app, sources) = one_collection_app(true, Some(4326));

    let response = get(
        &app,
        &format!(
            "/search?collections=demo&filter=S_INTERSECTS(geom,BBOX(9,44,10.5,45.5))&filter-crs={}",
            urlencode(&epsg_4326_uri())
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    assert!(
        body["detail"].as_str().unwrap().contains("filter-crs"),
        "the refusal must name the parameter, got: {}",
        body["detail"]
    );

    let response = post(
        &app,
        "/search",
        json!({
            "collections": ["demo"],
            "filter": "S_INTERSECTS(geom,BBOX(9,44,10.5,45.5))",
            "filter-lang": "cql2-text",
            "filter-crs": epsg_4326_uri(),
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(body_json(response).await["detail"]
        .as_str()
        .unwrap()
        .contains("filter-crs"));

    assert!(
        sources["demo"].seen_filter_crs.lock().unwrap().is_empty(),
        "a refused filter-crs must never reach a driver at all"
    );
}

/// The per-collection half of the refusal: CRS84 is the one value this lane
/// accepts, but honouring it against a collection whose storage is *not*
/// CRS84 means a real transform, which only a driver declaring
/// `FeatureSource::filter_crs_capable` can perform. A driver that cannot must
/// refuse by name rather than evaluate the filter in the storage CRS anyway.
#[tokio::test]
async fn crs84_against_a_projected_collection_is_refused_by_a_driver_that_cannot_transform() {
    let (app, sources) = one_collection_app(false, Some(3857));
    let response = get(
        &app,
        &format!(
            "/search?collections=demo&filter=name='alpha'&filter-crs={}",
            urlencode(&crs84_uri())
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert!(
        body["detail"].as_str().unwrap().contains("filter-crs"),
        "got: {}",
        body["detail"]
    );
    assert!(sources["demo"].seen_filter_crs.lock().unwrap().is_empty());
}

/// ...and the same driver keeps serving a CRS84 `filter-crs` for a
/// CRS84-stored collection, where honouring it asks for nothing at all. This
/// is the case every live GeoPackage deployment in this workspace is in, so
/// the demos keep serving exactly what they served.
#[tokio::test]
async fn crs84_against_a_crs84_collection_is_served_by_every_driver() {
    let (app, sources) = one_collection_app(false, Some(4326));
    let response = get(
        &app,
        &format!(
            "/search?collections=demo&filter=name='alpha'&filter-crs={}",
            urlencode(&crs84_uri())
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(item_ids(&body_json(response).await), vec!["a".to_string()]);
    assert_eq!(
        *sources["demo"].seen_filter_crs.lock().unwrap(),
        vec![RequestedCrs::Crs84]
    );
}

// -- an omitted filter-crs on a projected collection (`#247`) --------------

/// `#247` on the `/search` lane: a **spatial** filter, no `filter-crs` on the
/// wire at all, a collection whose storage is projected, and a driver that
/// cannot transform.
///
/// The Filter Extension says "the parameter `filter-crs` always defaults to
/// `http://www.opengis.net/def/crs/OGC/1.3/CRS84` for a STAC API", so the
/// numbers in `BBOX(9,44,10.5,45.5)` are degrees whatever the storage is.
/// Evaluating them against 3857 metres selects rows by coordinates the client
/// never wrote — under a `200`. So this refuses by name instead, exactly as
/// the explicitly-declared CRS84 case above already does.
#[tokio::test]
async fn a_default_spatial_filter_on_a_projected_collection_is_refused_by_name() {
    let (app, sources) = one_collection_app(false, Some(3857));
    let response = get(
        &app,
        "/search?collections=demo&filter=S_INTERSECTS(geom,BBOX(9,44,10.5,45.5))",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    let detail = body["detail"].as_str().unwrap().to_string();
    assert!(
        detail.contains("CRS84") && detail.contains("spatial filter"),
        "the refusal must name what it cannot do; detail was: {detail}"
    );
    assert!(
        sources["demo"].seen_filter_crs.lock().unwrap().is_empty(),
        "the driver must never be asked to evaluate a filter it cannot express"
    );
}

/// ...and the same driver, same projected collection, keeps serving an
/// attribute-only filter. Nothing about `name='alpha'` is expressed in a CRS,
/// so the refusal above must not reach it — the reason the `#247` branch asks
/// `Filter::has_spatial_literal` rather than "is there a filter at all".
#[tokio::test]
async fn a_default_attribute_filter_on_a_projected_collection_is_still_served() {
    let (app, sources) = one_collection_app(false, Some(3857));
    let response = get(&app, "/search?collections=demo&filter=name='alpha'").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(item_ids(&body_json(response).await), vec!["a".to_string()]);
    assert_eq!(
        *sources["demo"].seen_filter_crs.lock().unwrap(),
        vec![RequestedCrs::Omitted]
    );
}

/// **The rule `#247` must not break**, on this lane: a CRS84-stored collection
/// serves the identical default spatial filter unchanged, whatever the driver
/// can do. Reading a CRS84 literal against CRS84 storage asks for nothing, so
/// every driver has always been able to do it — which is why every live
/// GeoPackage demo is untouched by this slice.
#[tokio::test]
async fn a_default_spatial_filter_on_a_crs84_collection_is_unmoved() {
    for (srid, filter_crs_capable) in [(None, false), (Some(4326), false), (Some(4326), true)] {
        let (app, sources) = one_collection_app(filter_crs_capable, srid);
        let response = get(
            &app,
            "/search?collections=demo&filter=S_INTERSECTS(geom,BBOX(9,44,10.5,45.5))",
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "srid {srid:?} (filter_crs_capable={filter_crs_capable}) must still serve a default \
             spatial filter"
        );
        assert_eq!(item_ids(&body_json(response).await), vec!["a".to_string()]);
        assert_eq!(
            *sources["demo"].seen_filter_crs.lock().unwrap(),
            vec![RequestedCrs::Omitted]
        );
    }
}

/// A driver that CAN transform is handed the projected collection's default
/// spatial filter as `Omitted`, for its own compiler to read as CRS84 and
/// transform into storage. PostGIS is that driver — and this is the `/search`
/// request that answered `500` before `#247`.
#[tokio::test]
async fn a_default_spatial_filter_on_a_projected_collection_reaches_a_capable_driver() {
    let (app, sources) = one_collection_app(true, Some(3857));
    let response = get(
        &app,
        "/search?collections=demo&filter=S_INTERSECTS(geom,BBOX(9,44,10.5,45.5))",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        *sources["demo"].seen_filter_crs.lock().unwrap(),
        vec![RequestedCrs::Omitted]
    );
}

// -- bbox on a projected collection (`#255`) --------------------------------
//
// Neither STAC lane has a `bbox-crs` parameter at all: the item search's
// `bbox` is WGS 84 longitude/latitude and nothing else, the same fixed reading
// OGC API - Features Part 1 Requirement 23 (`/req/core/fc-bbox-definition`)
// clause C gives a four-number `bbox` that arrives with no `bbox-crs`. So
// every `bbox` on these lanes is CRS84, and against a projected collection
// honouring it is a real transform — with no parameter a client could drop to
// avoid it, which is why the refusal has to name the collection.

/// [`one_collection_app`] with the reprojection capability a `bbox` rides
/// (`#255`) under the test's control, rather than the `filter-crs` one.
fn one_collection_app_with_crs(
    crs_capable: bool,
    srid: Option<i32>,
) -> (axum::Router, HashMap<String, Arc<FilterableFeatureSource>>) {
    let mut demo = search_collection(
        "demo",
        vec![(
            "a",
            search_feature("a", 10.0, 45.0, "alpha", 1, "2020-01-01T00:00:00Z"),
        )],
    );
    demo.crs_capable = crs_capable;
    demo.srid = srid;
    demo.derive_physical = true;
    build_search_app_with_sources(vec![demo])
}

const STAC_BBOX_QUERY: &str = "/search?collections=demo&bbox=9,44,10.5,45.5";

/// `#255` on the `/search` lane: a `bbox`, a projected collection, a driver
/// that cannot reproject, and one explicitly named target collection.
///
/// Comparing those four degrees against metre coordinates answers `200` with
/// rows selected by numbers the client never wrote — the same failure mode
/// `#247` closed for `filter`, except that here PostGIS's `&&` does not even
/// raise, and a driver evaluating the box in memory never could. Refused by
/// name instead.
#[tokio::test]
async fn a_bbox_on_a_projected_collection_is_refused_by_name() {
    let (app, sources) = one_collection_app_with_crs(false, Some(3857));
    let response = get(&app, STAC_BBOX_QUERY).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["code"], "InvalidParameter");
    let detail = body["detail"].as_str().unwrap().to_string();
    assert!(
        detail.contains("CRS84") && detail.contains("bbox"),
        "the refusal must name what it cannot do; detail was: {detail}"
    );
    assert!(
        sources["demo"].seen_filter_crs.lock().unwrap().is_empty(),
        "the driver must never be asked to evaluate a bbox it cannot express"
    );
}

/// The same refusal on the POST lane, where the `bbox` is a JSON array rather
/// than a comma-joined string — one parse, one gate, both methods.
#[tokio::test]
async fn a_post_bbox_on_a_projected_collection_is_refused_by_name() {
    let (app, _) = one_collection_app_with_crs(false, Some(3857));
    let response = post(
        &app,
        "/search",
        json!({ "collections": ["demo"], "bbox": [9.0, 44.0, 10.5, 45.5] }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["code"], "InvalidParameter");
}

/// A driver that CAN reproject is simply handed the same projected
/// collection's `bbox`. PostGIS is that driver, and this is the `/search`
/// request that answered `200` with the wrong rows before `#255`.
#[tokio::test]
async fn a_bbox_on_a_projected_collection_reaches_a_capable_driver() {
    let (app, sources) = one_collection_app_with_crs(true, Some(3857));
    let response = get(&app, STAC_BBOX_QUERY).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(sources["demo"].seen_filter_crs.lock().unwrap().len(), 1);
}

/// **The rule `#255` must not break**, on this lane: a CRS84-stored
/// collection serves the identical `bbox` unchanged, whatever the driver can
/// do. Reading a CRS84 box against CRS84 storage asks for nothing, so every
/// driver has always been able to do it — which is why every live GeoPackage
/// demo is untouched by this slice.
#[tokio::test]
async fn a_bbox_on_a_crs84_collection_is_unmoved() {
    for (srid, crs_capable) in [(None, false), (Some(4326), false), (Some(4326), true)] {
        let (app, sources) = one_collection_app_with_crs(crs_capable, srid);
        let response = get(&app, STAC_BBOX_QUERY).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "srid {srid:?} (crs_capable={crs_capable}) must still serve a bbox"
        );
        assert_eq!(item_ids(&body_json(response).await), vec!["a".to_string()]);
        assert_eq!(sources["demo"].seen_filter_crs.lock().unwrap().len(), 1);
    }
}

/// The `/collections/{cid}/items` lane, which has no `filter` surface at all
/// and so was never touched by `#247` — the completeness check that slice's
/// own `FilterCrs::grant_only` finding asks for. One named collection, so the
/// fan-out's skip tolerance does not apply and this is a 400.
#[tokio::test]
async fn a_bbox_on_a_projected_collection_is_refused_on_the_items_lane_too() {
    let (app, sources) = one_collection_app_with_crs(false, Some(3857));
    let response = get(&app, "/collections/demo/items?bbox=9,44,10.5,45.5").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let detail = body_json(response).await["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.contains("CRS84") && detail.contains("bbox"),
        "detail was: {detail}"
    );
    assert!(sources["demo"].seen_filter_crs.lock().unwrap().is_empty());

    // ...and the same collection still serves the same lane without a `bbox`.
    let response = get(&app, "/collections/demo/items").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(sources["demo"].seen_filter_crs.lock().unwrap().len(), 1);
}

/// The fan-out's documented judgment call, applied to `bbox`: one collection
/// that cannot honour it is skipped rather than failing the whole search — and
/// the skip is machine-detectable, in its own `bboxIncapableCollections`
/// rather than in `filterIncapableCollections`. This request carries no
/// `filter` at all, which is exactly why the two lists have to be separate:
/// naming `filter` here would send the client to drop a parameter it never
/// sent.
#[tokio::test]
async fn a_fan_out_records_a_collection_that_cannot_honour_a_bbox_instead_of_400ing() {
    let mut alpha = search_collection(
        "alpha",
        vec![(
            "a1",
            search_feature("a1", 10.0, 45.0, "match", 1, "2020-01-01T00:00:00Z"),
        )],
    );
    alpha.srid = Some(4326);
    alpha.derive_physical = true;
    let mut beta = search_collection(
        "beta",
        vec![(
            "b1",
            search_feature("b1", 10.0, 45.0, "match", 1, "2020-01-01T00:00:00Z"),
        )],
    );
    beta.srid = Some(3857);
    beta.crs_capable = false;
    beta.derive_physical = true;
    let app = build_search_app(vec![alpha, beta]);

    let response = get(&app, "/search?bbox=9,44,10.5,45.5").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(item_ids(&body), vec!["a1".to_string()]);
    assert_eq!(
        body["bboxIncapableCollections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["beta"]
    );
    assert!(
        body.get("filterIncapableCollections").is_none(),
        "no filter was refused, and naming one would misdirect the client: {body}"
    );
}

/// The fan-out keeps its documented judgment call for this reason too: one
/// collection that cannot honour the declared `filter-crs` is skipped, not
/// fatal — and the skip is machine-detectable in `filterIncapableCollections`,
/// exactly as a skip for a missing `filter` capability already is, rather than
/// silent.
#[tokio::test]
async fn a_fan_out_records_a_collection_that_cannot_honour_filter_crs_instead_of_400ing() {
    let mut alpha = search_collection(
        "alpha",
        vec![(
            "a1",
            search_feature("a1", 10.0, 45.0, "match", 1, "2020-01-01T00:00:00Z"),
        )],
    );
    alpha.srid = Some(4326);
    alpha.derive_physical = true;
    let mut beta = search_collection(
        "beta",
        vec![(
            "b1",
            search_feature("b1", 10.0, 45.0, "match", 1, "2020-01-01T00:00:00Z"),
        )],
    );
    beta.srid = Some(3857);
    beta.filter_crs_capable = false;
    beta.derive_physical = true;
    let app = build_search_app(vec![alpha, beta]);

    let response = get(
        &app,
        &format!(
            "/search?filter=name='match'&filter-crs={}",
            urlencode(&crs84_uri())
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(item_ids(&body), vec!["a1".to_string()]);
    assert_eq!(
        body["filterIncapableCollections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["beta"]
    );
}

/// A `next`/`self` link that dropped `filter-crs` would evaluate page two's
/// filter geometry in a different CRS than page one's — the same reason
/// `tellurion-features`' `items_href` echoes it (`#217`).
#[tokio::test]
async fn search_links_carry_filter_crs_through_to_the_next_page() {
    let mut demo = search_collection(
        "demo",
        vec![
            (
                "a",
                search_feature("a", 10.0, 45.0, "match", 1, "2020-01-01T00:00:00Z"),
            ),
            (
                "b",
                search_feature("b", 10.0, 45.0, "match", 2, "2020-01-02T00:00:00Z"),
            ),
        ],
    );
    demo.srid = Some(4326);
    let app = build_search_app(vec![demo]);

    let response = get(
        &app,
        &format!(
            "/search?collections=demo&limit=1&filter=name='match'&filter-crs={}",
            urlencode(&crs84_uri())
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    for rel in ["self", "next"] {
        let href = find_link(&body, rel)
            .unwrap_or_else(|| panic!("a {rel} link"))
            .get("href")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            href.contains("filter-crs="),
            "the {rel} link dropped filter-crs: {href}"
        );
    }
}

/// Cross-collection paging (`#36` slice C's own fan-out): walks every page
/// via the `next` link until exhausted, and proves the full, stable,
/// collection-then-item-ordered result set arrives across more than one
/// page — not that any single page boundary lands at a specific offset
/// (an implementation detail this test deliberately doesn't pin).
#[tokio::test]
async fn cross_collection_search_merges_and_pages_every_collection_to_completion() {
    let app = build_search_app(vec![
        search_collection(
            "alpha",
            vec![
                (
                    "a1",
                    search_feature("a1", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
                ),
                (
                    "a2",
                    search_feature("a2", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
                ),
                (
                    "a3",
                    search_feature("a3", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
                ),
            ],
        ),
        search_collection(
            "beta",
            vec![
                (
                    "b1",
                    search_feature("b1", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
                ),
                (
                    "b2",
                    search_feature("b2", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
                ),
            ],
        ),
    ]);

    let mut all_ids: Vec<String> = Vec::new();
    let mut href = "/search?limit=2".to_string();
    let mut pages = 0;
    loop {
        let response = get(&app, &href).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        pages += 1;
        all_ids.extend(item_ids(&body));

        // Every page from a fan-out search has an unknowable total, so
        // `numberMatched` must be absent — see `run_cursor_search`'s own doc
        // for why (there's no cheap total across heterogeneous collections).
        assert!(body.get("numberMatched").is_none() || body["numberMatched"].is_null());

        match find_link(&body, "next") {
            Some(next) => href = next["href"].as_str().unwrap().to_string(),
            None => break,
        }
        assert!(pages < 10, "paging did not terminate: {all_ids:?}");
    }

    assert_eq!(
        all_ids,
        vec![
            "a1".to_string(),
            "a2".to_string(),
            "a3".to_string(),
            "b1".to_string(),
            "b2".to_string(),
        ],
        "expected every item in stable alphabetical-collection, then in-collection order"
    );
    assert!(
        pages > 1,
        "expected more than one page with limit=2 across 5 items"
    );
}

#[tokio::test]
async fn search_with_no_matches_returns_an_empty_feature_collection() {
    let app = build_search_app(vec![search_collection("demo", vec![])]);
    let response = get(&app, "/search").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["type"], "FeatureCollection");
    assert_eq!(body["features"], serde_json::json!([]));
    assert_eq!(body["numberReturned"], 0);
    assert!(find_link(&body, "next").is_none());
}

/// `#36` design guard, same internal-id leak sweep
/// `internal_ids_never_appear_in_items_response_bodies` already runs for
/// `/items`, extended to `/search` — both a bare listing and a filtered
/// request.
#[tokio::test]
async fn internal_ids_never_appear_in_search_response_bodies() {
    const TENANT_INTERNAL: &str = "zzz-tenant-internal-marker";
    const CATALOG_INTERNAL: &str = "zzz-catalog-internal-marker";
    const COLLECTION_INTERNAL: &str = "zzz-collection-internal-marker";

    let source = Arc::new(FilterableFeatureSource {
        items: vec![(
            "a".to_string(),
            search_feature("a", 1.0, 1.0, "x", 1, "2020-01-01T00:00:00Z"),
        )],
        filter_capable: true,
        filter_crs_capable: false,
        crs_capable: false,
        seen_filter_crs: std::sync::Mutex::new(Vec::new()),
    });
    let mut drivers = HashMap::new();
    drivers.insert(
        "storage-demo".to_string(),
        (COLLECTION_INTERNAL.to_string(), Some(4326), source),
    );

    let config_yaml = format!(
        r#"
storages: [ {{ id: storage-demo, driver: filterable-fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: {TENANT_INTERNAL}, external_id: acme }} ]
catalogs: [ {{ id: {CATALOG_INTERNAL}, external_id: default, tenant: {TENANT_INTERNAL} }} ]
collections:
  - id: {COLLECTION_INTERNAL}
    external_id: demo
    catalog: {CATALOG_INTERNAL}
    storage: storage-demo
    table: {COLLECTION_INTERNAL}
    geometry: geom
    pk: id
    datetime: observed_at
"#
    );
    let config: AppConfig = serde_yaml::from_str(&config_yaml).unwrap();
    config.validate().unwrap();
    let mut registry = Registry::new();
    registry.register(Arc::new(FilterableFactory { drivers }));
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
    let app = axum::Router::new()
        .nest("/{tenant}", tellurion_stac::router())
        .with_state(ctx);

    let paths = [
        "/acme/search".to_string(),
        "/acme/search?filter=name='x'".to_string(),
        "/acme/search?ids=a".to_string(),
    ];
    for path in paths {
        let response = get(&app, &path).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(status, StatusCode::OK, "path {path} was not 200: {text}");
        assert!(
            !text.contains(TENANT_INTERNAL),
            "{path} leaked the tenant internal id: {text}"
        );
        assert!(
            !text.contains(CATALOG_INTERNAL),
            "{path} leaked the catalog internal id: {text}"
        );
        assert!(
            !text.contains(COLLECTION_INTERNAL),
            "{path} leaked the collection internal id: {text}"
        );
    }
}

// -- `#34` authorization policy layer ----------------------------------------
//
// A self-contained fixture (deliberately not reusing `FilterableFeatureSource`
// above, to keep these tests independent of that fixture's own composition
// rules) proving the policy checkpoint is wired through every handler in this
// crate: isolation on `/collections`, `/collections/{cid}`, `/collections/{cid}/items`,
// `/collections/{cid}/items/{fid}`, and `/search`; RBAC/ABAC on the items-list
// and search lanes, which push a grant's filter down the same way
// `tellurion-features` does.

fn policy_feature(id: &str, name: &str) -> Value {
    json!({ "type": "Feature", "id": id, "geometry": null, "properties": { "name": name } })
}

fn policy_matches(feature: &Value, filter: Option<&Filter>) -> bool {
    match filter {
        None => true,
        Some(Filter::And(items)) => items.iter().all(|f| policy_matches(feature, Some(f))),
        Some(Filter::Compare {
            property,
            op: CompareOp::Eq,
            value: Literal::Text(expected),
        }) => feature["properties"][property].as_str() == Some(expected.as_str()),
        Some(_) => panic!("policy_matches: unexpected filter shape in this test fixture"),
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
            .filter(|(_, v)| policy_matches(v, query.filter.as_ref()))
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
            .find(|(item_id, v)| item_id == id && policy_matches(v, filter))
            .map(|(_, v)| v.clone()))
    }

    fn filter_capable(&self) -> bool {
        true
    }
}

/// Matches `DemoCatalog`'s physical shape, plus a `name` attribute column —
/// needed so `run_cursor_search`'s own `filter::validate` step (which
/// `/items` doesn't run, but `/search`'s cursor path does) recognizes
/// `name` as a real, filterable property instead of rejecting it as
/// unknown.
struct PolicyCatalog;

#[async_trait::async_trait]
impl CatalogSource for PolicyCatalog {
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

struct PolicyDriver {
    source: Arc<PolicyFeatureSource>,
}

impl StorageDriver for PolicyDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(PolicyCatalog)
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
    tellurion_stac::router().with_state(build_policy_ctx(config_yaml, items))
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

const AUTH_ONLY_STAC_CONFIG: &str = r#"
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

const RBAC_STAC_CONFIG: &str = r#"
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
          lanes: [stac]
    - name: filtered-reader
      grants:
        - scope: { collections: [demo] }
          lanes: [stac]
          filter: "name = {{claims.name}}"
"#;

/// Two collections under one catalog: `open` has a matching grant under
/// `RBAC_TWO_COLLECTIONS_CONFIG`'s `reader` role, `closed` isn't in scope
/// for any role at all — enough to prove `/collections` omits an
/// inaccessible collection from the listing rather than merely refusing
/// direct access to it.
const RBAC_TWO_COLLECTIONS_CONFIG: &str = r#"
storages: [ { id: main, driver: policy-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: open
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
  - id: closed
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - { token: reader-token, tenants: [public], roles: { public: [reader] } }
policy:
  roles:
    - name: reader
      grants:
        - scope: { collections: [open] }
          lanes: [stac]
"#;

#[tokio::test]
async fn no_credential_against_a_private_collection_is_401_when_auth_is_configured() {
    let app = build_policy_app(
        AUTH_ONLY_STAC_CONFIG,
        vec![("a", policy_feature("a", "alpha"))],
    );
    let response = get_with_bearer(&app, "/collections/demo/items", None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_unrecognized_token_against_a_private_collection_is_403() {
    let app = build_policy_app(
        AUTH_ONLY_STAC_CONFIG,
        vec![("a", policy_feature("a", "alpha"))],
    );
    let response = get_with_bearer(&app, "/collections/demo/items", Some("no-such-token")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_tenant_member_reads_items_unrestricted_with_no_policy_configured() {
    let app = build_policy_app(
        AUTH_ONLY_STAC_CONFIG,
        vec![
            ("a", policy_feature("a", "alpha")),
            ("b", policy_feature("b", "beta")),
        ],
    );
    let response = get_with_bearer(&app, "/collections/demo/items", Some("member-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["numberReturned"], 2);
}

#[tokio::test]
async fn get_collection_is_gated_the_same_way() {
    let app = build_policy_app(
        AUTH_ONLY_STAC_CONFIG,
        vec![("a", policy_feature("a", "alpha"))],
    );
    let denied = get_with_bearer(&app, "/collections/demo", None).await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let allowed = get_with_bearer(&app, "/collections/demo", Some("member-token")).await;
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test]
async fn single_item_get_is_gated_the_same_way() {
    let app = build_policy_app(
        AUTH_ONLY_STAC_CONFIG,
        vec![("a", policy_feature("a", "alpha"))],
    );
    let denied = get_with_bearer(&app, "/collections/demo/items/a", None).await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let allowed = get_with_bearer(&app, "/collections/demo/items/a", Some("member-token")).await;
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_collection_the_subject_cannot_access_is_omitted_from_the_listing() {
    let app = build_policy_app(
        RBAC_TWO_COLLECTIONS_CONFIG,
        vec![("a", policy_feature("a", "alpha"))],
    );
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
        vec!["open"],
        "a collection with no matching grant must not be advertised in the listing: {ids:?}"
    );
}

#[tokio::test]
async fn rbac_active_denies_a_member_with_no_matching_role() {
    let app = build_policy_app(RBAC_STAC_CONFIG, vec![("a", policy_feature("a", "alpha"))]);
    let response = get_with_bearer(&app, "/collections/demo/items", Some("no-role-token")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn abac_grant_filter_narrows_items_list_end_to_end() {
    let app = build_policy_app(
        RBAC_STAC_CONFIG,
        vec![
            ("a", policy_feature("a", "alpha")),
            ("b", policy_feature("b", "beta")),
        ],
    );
    let response = get_with_bearer(&app, "/collections/demo/items", Some("filtered-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["numberReturned"], 1,
        "grant filter must reach the driver: {body}"
    );
    assert_eq!(body["features"][0]["id"], "a");
}

/// `#34`: single-item GET now pushes the grant filter into the same
/// single-row query the items-list lane already narrows
/// (`PolicyFeatureSource` advertises `filter_capable() == true`) — an item
/// the filter matches comes back normally.
#[tokio::test]
async fn single_item_get_serves_an_item_the_grant_filter_matches() {
    let app = build_policy_app(
        RBAC_STAC_CONFIG,
        vec![
            ("a", policy_feature("a", "alpha")),
            ("b", policy_feature("b", "beta")),
        ],
    );
    let response = get_with_bearer(&app, "/collections/demo/items/a", Some("filtered-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// The filtered-single-item counterpart: an item that genuinely exists but
/// that the grant's filter excludes must come back 404 — indistinguishable
/// from an id that was never there, no existence leak.
#[tokio::test]
async fn single_item_get_404s_an_item_the_grant_filter_excludes() {
    let app = build_policy_app(
        RBAC_STAC_CONFIG,
        vec![
            ("a", policy_feature("a", "alpha")),
            ("b", policy_feature("b", "beta")),
        ],
    );
    let response = get_with_bearer(&app, "/collections/demo/items/b", Some("filtered-token")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// `/search?ids=...` rides the same `FeatureSource::item` pushdown — an id
/// the grant filter excludes is simply absent from the result, the same
/// `Ok(None)` behavior the plain single-item route gets.
#[tokio::test]
async fn ids_mode_search_omits_an_item_the_grant_filter_excludes() {
    let app = build_policy_app(
        RBAC_STAC_CONFIG,
        vec![
            ("a", policy_feature("a", "alpha")),
            ("b", policy_feature("b", "beta")),
        ],
    );
    let response = get_with_bearer(
        &app,
        "/search?ids=a,b&collections=demo",
        Some("filtered-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let ids: Vec<&str> = body["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec!["a"],
        "the grant filter must exclude 'b' from an ids-mode search too: {body}"
    );
}

#[tokio::test]
async fn abac_grant_filter_narrows_search_end_to_end() {
    let app = build_policy_app(
        RBAC_STAC_CONFIG,
        vec![
            ("a", policy_feature("a", "alpha")),
            ("b", policy_feature("b", "beta")),
        ],
    );
    let response = get_with_bearer(&app, "/search?collections=demo", Some("filtered-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["numberReturned"], 1,
        "the grant's filter must AND-merge into the search query: {body}"
    );
    assert_eq!(body["features"][0]["id"], "a");
}

/// A fan-out search across two collections, one accessible and one not
/// (`RBAC_TWO_COLLECTIONS_CONFIG`): the inaccessible collection is skipped,
/// not treated as a fatal error for the whole search — the same tolerance
/// the search module already documents for a collection that fails to
/// resolve or lacks a needed capability.
#[tokio::test]
async fn cross_collection_search_skips_a_collection_the_subject_cannot_access() {
    let app = build_policy_app(
        RBAC_TWO_COLLECTIONS_CONFIG,
        vec![("a", policy_feature("a", "alpha"))],
    );
    let response = get_with_bearer(&app, "/search", Some("reader-token")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let ids: Vec<&str> = body["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["collection"].as_str().unwrap())
        .collect();
    assert!(
        ids.iter().all(|id| *id == "open"),
        "search must never return an item from a collection the subject cannot access: {ids:?}"
    );
}

// -- q / free-text search (`#181`) ---------------------------------------

/// The derived-index side of a `#181` fixture collection: an in-memory
/// document store standing in for `"<table>_index"`, matching `q` the same
/// way `websearch_to_tsquery('simple', ...)` does for the cases these tests
/// exercise (every whitespace-separated term must appear, case-insensitively,
/// in some text-typed property value) — enough to prove dispatch, never a
/// re-implementation of PostgreSQL's tokenizer, which `tests/search_live.rs`
/// covers against the real thing.
struct FakeSearchSource {
    docs: Vec<Value>,
    applied: u64,
    text_capable: bool,
}

#[async_trait::async_trait]
impl tellurion_core::SearchSource for FakeSearchSource {
    async fn search(
        &self,
        _collection: &CollectionDecl,
        query: &tellurion_core::SearchQuery,
    ) -> CoreResult<tellurion_core::SearchPage> {
        let matches = |doc: &Value| match query.q.as_deref() {
            None => true,
            Some(q) => {
                let haystack = doc["properties"]
                    .as_object()
                    .map(|props| {
                        props
                            .values()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" ")
                            .to_lowercase()
                    })
                    .unwrap_or_default();
                q.split_whitespace()
                    .all(|term| haystack.contains(&term.to_lowercase()))
            }
        };
        let features_geojson = self
            .docs
            .iter()
            .filter(|doc| matches(doc))
            .take(query.limit as usize)
            .cloned()
            .collect();
        Ok(tellurion_core::SearchPage { features_geojson })
    }

    async fn applied_high_water(
        &self,
        _collection: &CollectionDecl,
    ) -> CoreResult<tellurion_core::Sequence> {
        Ok(tellurion_core::Sequence(self.applied))
    }

    fn text_search_capable(&self) -> bool {
        self.text_capable
    }
}

/// The write lane's outbox, reporting a fixed primary high-water of 5 — so
/// a fixture index with `applied: 5` is fresh under the default
/// `freshness_bound: 0`, and anything lower is stale.
struct FakeOutbox;

#[async_trait::async_trait]
impl tellurion_core::OutboxSource for FakeOutbox {
    async fn read_after(
        &self,
        _collection: &CollectionDecl,
        _after: tellurion_core::Sequence,
        _limit: u32,
    ) -> CoreResult<Vec<tellurion_core::Obligation>> {
        Ok(Vec::new())
    }

    async fn primary_high_water(
        &self,
        _collection: &CollectionDecl,
    ) -> CoreResult<tellurion_core::Sequence> {
        Ok(tellurion_core::Sequence(5))
    }
}

/// One storage in a `#181` fixture: either the collection's main storage
/// (feature source + outbox, so the search lane has a degraded tail and the
/// freshness gate has a primary high-water to measure against) or its
/// derived index (search source only — deliberately NOT feature-capable, so
/// nothing can mistake it for a second main chain).
struct QDriver {
    features: Option<Arc<FilterableFeatureSource>>,
    search: Option<Arc<FakeSearchSource>>,
}

impl StorageDriver for QDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(FilterableCatalog {
            table: "unused".to_string(),
            srid: Some(4326),
        })
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        self.features
            .clone()
            .map(|source| source as Arc<dyn FeatureSource>)
    }

    fn search_source(&self) -> Option<Arc<dyn tellurion_core::SearchSource>> {
        self.search
            .clone()
            .map(|source| source as Arc<dyn tellurion_core::SearchSource>)
    }

    fn outbox_source(&self) -> Option<Arc<dyn tellurion_core::OutboxSource>> {
        self.features
            .as_ref()
            .map(|_| Arc::new(FakeOutbox) as Arc<dyn tellurion_core::OutboxSource>)
    }

    fn index_sink(&self) -> Option<Arc<dyn tellurion_core::IndexSink>> {
        None
    }
}

struct QFactory {
    drivers: HashMap<String, Arc<QDriver>>,
}

impl DriverFactory for QFactory {
    fn name(&self) -> &str {
        "q-fake"
    }

    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(self
            .drivers
            .get(&decl.id)
            .cloned()
            .expect("every storage this factory builds was configured with a fixture")
            as Arc<dyn StorageDriver>)
    }
}

/// One `#181` fixture collection: `index` is `None` for a collection with no
/// `routing.search`/`routing.index` at all (the "this collection cannot
/// serve free text, ever" case), else the derived index's stored documents,
/// its applied high-water, and whether its source advertises free text.
struct QCollectionFixture {
    id: &'static str,
    docs: Vec<Value>,
    index: Option<QIndexFixture>,
}

struct QIndexFixture {
    applied: u64,
    text_capable: bool,
}

fn q_collection(id: &'static str, docs: Vec<Value>) -> QCollectionFixture {
    QCollectionFixture {
        id,
        docs,
        index: Some(QIndexFixture {
            applied: 5,
            text_capable: true,
        }),
    }
}

/// Builds a catalog whose collections each route `search: [index-{id},
/// storage-{id}]` with `index: index-{id}` and `write: storage-{id}` — the
/// exact `search: [index, main]` shape the design doc names, across two
/// distinct storage ids so nothing can confuse the degraded tail for a
/// second index attempt.
fn build_q_app(specs: Vec<QCollectionFixture>) -> axum::Router {
    let mut drivers: HashMap<String, Arc<QDriver>> = HashMap::new();
    let mut collections_yaml = String::new();
    for spec in &specs {
        let main_id = format!("storage-{}", spec.id);
        drivers.insert(
            main_id.clone(),
            Arc::new(QDriver {
                features: Some(Arc::new(FilterableFeatureSource {
                    items: spec
                        .docs
                        .iter()
                        .map(|doc| (doc["id"].as_str().unwrap().to_string(), doc.clone()))
                        .collect(),
                    filter_capable: true,
                    filter_crs_capable: false,
                    crs_capable: false,
                    seen_filter_crs: std::sync::Mutex::new(Vec::new()),
                })),
                search: None,
            }),
        );
        collections_yaml.push_str(&format!(
            "  - id: {id}\n    catalog: default\n    storage: {main_id}\n    table: {id}\n    geometry: geom\n    pk: id\n",
            id = spec.id
        ));
        match &spec.index {
            Some(index) => {
                let index_id = format!("index-{}", spec.id);
                drivers.insert(
                    index_id.clone(),
                    Arc::new(QDriver {
                        features: None,
                        search: Some(Arc::new(FakeSearchSource {
                            docs: spec.docs.clone(),
                            applied: index.applied,
                            text_capable: index.text_capable,
                        })),
                    }),
                );
                collections_yaml.push_str(&format!(
                    "    routing:\n      write: {main_id}\n      index: {index_id}\n      search: [{index_id}, {main_id}]\n"
                ));
            }
            None => {
                collections_yaml.push_str(&format!("    routing:\n      write: {main_id}\n"));
            }
        }
    }
    let storages_yaml: String = drivers
        .keys()
        .map(|id| format!("  - {{ id: {id}, driver: q-fake, url_env: DATABASE_URL }}\n"))
        .collect();
    let config_yaml = format!(
        "storages:\n{storages_yaml}tenants: [ {{ id: public }} ]\ncatalogs: [ {{ id: default, tenant: public }} ]\ncollections:\n{collections_yaml}"
    );

    let config: AppConfig = serde_yaml::from_str(&config_yaml).unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(QFactory { drivers }));

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
    tellurion_stac::router().with_state(ctx)
}

/// A fresh, text-capable index serves `q` — and serves it from the index's
/// own stored documents, narrowed by the free-text predicate, identically
/// on GET and POST.
#[tokio::test]
async fn q_is_served_from_a_fresh_derived_index_on_get_and_post() {
    let app = build_q_app(vec![q_collection(
        "demo",
        vec![
            search_feature("a", 1.0, 1.0, "acme harbour", 10, "2020-01-01T00:00:00Z"),
            search_feature("b", 2.0, 2.0, "beta quarry", 20, "2020-01-01T00:00:00Z"),
        ],
    )]);

    let get_body = body_json(get(&app, "/search?collections=demo&q=acme").await).await;
    assert_eq!(item_ids(&get_body), vec!["a".to_string()], "{get_body}");
    assert_eq!(get_body["numberReturned"], 1);
    assert!(
        get_body.get("numberMatched").is_none(),
        "the derived-index query reports no total, and one is never invented: {get_body}"
    );
    assert!(
        get_body.get("searchIncapableCollections").is_none(),
        "nothing was skipped: {get_body}"
    );

    let post_body = body_json(
        post(
            &app,
            "/search",
            json!({ "collections": ["demo"], "q": "acme" }),
        )
        .await,
    )
    .await;
    assert_eq!(item_ids(&post_body), item_ids(&get_body));
}

/// The freshness gate governs `q` exactly as it governs any other search
/// read: a lagging index falls through to the degraded tail — which cannot
/// serve free text — so the request is refused by name (`503`), never
/// approximated by the main chain.
#[tokio::test]
async fn q_against_a_stale_index_is_refused_rather_than_approximated() {
    let mut spec = q_collection(
        "demo",
        vec![search_feature(
            "a",
            1.0,
            1.0,
            "acme harbour",
            10,
            "2020-01-01T00:00:00Z",
        )],
    );
    spec.index = Some(QIndexFixture {
        applied: 3,
        text_capable: true,
    });
    let app = build_q_app(vec![spec]);

    let response = get(&app, "/search?collections=demo&q=acme").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = body_json(response).await;
    assert_eq!(body["code"], "SearchIndexUnavailable");
}

/// An index source that never advertises free text is the permanent flavor
/// of the same refusal — a named capability 404, never a silent fallback to
/// a substring scan on the main chain.
#[tokio::test]
async fn q_against_a_text_incapable_index_is_a_named_capability_refusal() {
    let mut spec = q_collection(
        "demo",
        vec![search_feature(
            "a",
            1.0,
            1.0,
            "acme harbour",
            10,
            "2020-01-01T00:00:00Z",
        )],
    );
    spec.index = Some(QIndexFixture {
        applied: 5,
        text_capable: false,
    });
    let app = build_q_app(vec![spec]);

    let response = get(&app, "/search?collections=demo&q=acme").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A collection with no `routing.search` at all can never serve `q` — the
/// same named refusal `Router::resolve_search` already gives an unrouted
/// search lane.
#[tokio::test]
async fn q_against_a_collection_with_no_search_lane_is_refused() {
    let app = build_q_app(vec![QCollectionFixture {
        id: "demo",
        docs: vec![search_feature(
            "a",
            1.0,
            1.0,
            "acme harbour",
            10,
            "2020-01-01T00:00:00Z",
        )],
        index: None,
    }]);

    let response = get(&app, "/search?collections=demo&q=acme").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A fan-out across heterogeneous collections serves whatever can serve and
/// reports the rest by name in `searchIncapableCollections` — the same
/// skip-instead-of-fail judgment `filterIncapableCollections` already makes
/// for the identical situation, and the same machine-detectable honesty.
#[tokio::test]
async fn a_q_fan_out_skips_and_reports_a_collection_that_cannot_serve_free_text() {
    let mut stale = q_collection(
        "beta",
        vec![search_feature(
            "b1",
            2.0,
            2.0,
            "acme quarry",
            20,
            "2020-01-01T00:00:00Z",
        )],
    );
    stale.index = Some(QIndexFixture {
        applied: 3,
        text_capable: true,
    });
    let app = build_q_app(vec![
        q_collection(
            "alpha",
            vec![search_feature(
                "a1",
                1.0,
                1.0,
                "acme harbour",
                10,
                "2020-01-01T00:00:00Z",
            )],
        ),
        stale,
    ]);

    let response = get(&app, "/search?q=acme").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(item_ids(&body), vec!["a1".to_string()], "{body}");
    assert_eq!(
        body["searchIncapableCollections"],
        json!(["beta"]),
        "a skipped collection must be reported, never silently dropped: {body}"
    );
}

/// Gate 1 at the HTTP boundary: `q` combined with a predicate the index
/// entry cannot express is a `400`, identically on GET and POST — never a
/// partial answer with the other predicate quietly dropped.
#[tokio::test]
async fn q_combined_with_another_predicate_is_a_400_on_both_verbs() {
    let app = build_q_app(vec![q_collection(
        "demo",
        vec![search_feature(
            "a",
            1.0,
            1.0,
            "acme harbour",
            10,
            "2020-01-01T00:00:00Z",
        )],
    )]);

    let get_response = get(&app, "/search?q=acme&bbox=0,0,2,2").await;
    assert_eq!(get_response.status(), StatusCode::BAD_REQUEST);

    let post_response = post(
        &app,
        "/search",
        json!({ "q": "acme", "filter": { "op": "=", "args": [{ "property": "name" }, "acme"] } }),
    )
    .await;
    assert_eq!(post_response.status(), StatusCode::BAD_REQUEST);
}
