use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use tellurion_core::{
    locking, AppConfig, AppContext, CatalogSource, CollectionDecl, DriverFactory, FeaturePage,
    FeatureSource, FileStyleStore, Filter, ItemsQuery, MokaTileCache, Mutation, MutationKind,
    PhysicalCollection, Registry, Resolver, Result as CoreResult, Router as CoreRouter, Sequence,
    StaticResolver, StorageDecl, StorageDriver, StyleStore, TileCache, WriteSink,
};

type Store = Arc<Mutex<HashMap<String, Value>>>;

struct PatchBackend {
    store: Store,
}

#[async_trait::async_trait]
impl CatalogSource for PatchBackend {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(Vec::new())
    }
}

#[async_trait::async_trait]
impl FeatureSource for PatchBackend {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        _query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        unreachable!("PATCH tests only read one item")
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        id: &str,
        _filter: Option<&Filter>,
    ) -> CoreResult<Option<Value>> {
        Ok(self.store.lock().unwrap().get(id).cloned())
    }
}

#[async_trait::async_trait]
impl WriteSink for PatchBackend {
    async fn apply(
        &self,
        _collection: &CollectionDecl,
        mutation: Mutation,
    ) -> CoreResult<Sequence> {
        let MutationKind::Upsert(feature) = mutation.kind else {
            unreachable!("PATCH never deletes")
        };
        self.store
            .lock()
            .unwrap()
            .insert(mutation.feature_id, feature);
        Ok(Sequence(1))
    }

    fn locking_conformance_classes(&self) -> Vec<&'static str> {
        vec![locking::OPTIMISTIC_LOCKING_ETAGS_CLASS]
    }

    /// `#150`: this fake stands in for a driver that CAN re-verify a
    /// precondition inside its own write — which is exactly what its
    /// `locking_conformance_classes` declaration above claims, and what a
    /// conditional `PATCH` now genuinely requires of its write lane. The
    /// witness is a fixed token because this in-process store has no
    /// concurrency to detect; the atomic behaviour itself is proved against
    /// a real database in `tellurion-server`'s `optimistic_locking_binary.rs`,
    /// never here.
    async fn row_version(
        &self,
        _collection: &CollectionDecl,
        feature_id: &str,
    ) -> CoreResult<Option<tellurion_core::locking::RowVersion>> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .contains_key(feature_id)
            .then(|| tellurion_core::locking::RowVersion::new("v1")))
    }

    async fn apply_conditional(
        &self,
        collection: &CollectionDecl,
        mutation: Mutation,
        _requested_crs: tellurion_core::RequestedCrs,
        _expected: &tellurion_core::locking::RowVersion,
    ) -> CoreResult<Option<Sequence>> {
        self.apply(collection, mutation).await.map(Some)
    }

    fn update_conformance_classes(&self) -> Vec<&'static str> {
        vec![tellurion_core::outbox::UPDATE_CONFORMANCE_CLASS]
    }
}

impl StorageDriver for PatchBackend {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(PatchBackend {
            store: Arc::clone(&self.store),
        })
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::new(PatchBackend {
            store: Arc::clone(&self.store),
        }))
    }

    fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
        Some(Arc::new(PatchBackend {
            store: Arc::clone(&self.store),
        }))
    }
}

struct PatchFactory {
    store: Store,
}

impl DriverFactory for PatchFactory {
    fn name(&self) -> &str {
        "patch-fake"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(PatchBackend {
            store: Arc::clone(&self.store),
        }))
    }
}

fn seeded_feature() -> Value {
    json!({
        "type": "Feature",
        "id": "x",
        "geometry": null,
        "properties": {
            "name": "old",
            "count": 1,
            "nested": { "keep": true, "remove": true },
            "tags": ["old"]
        }
    })
}

fn build_app() -> (axum::Router, Store) {
    let config: AppConfig = serde_yaml::from_str(
        r#"
storages: [ { id: main, driver: patch-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { write: main }
    schema:
      properties:
        - { name: name, type: string, required: true }
        - { name: count, type: integer }
"#,
    )
    .unwrap();
    config.validate().unwrap();

    let store = Arc::new(Mutex::new(HashMap::from([(
        "x".to_string(),
        seeded_feature(),
    )])));
    let mut registry = Registry::new();
    registry.register(Arc::new(PatchFactory {
        store: Arc::clone(&store),
    }));
    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let styles: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let context = Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        None,
        cache,
        styles,
    ));
    (tellurion_features::router().with_state(context), store)
}

async fn patch(
    app: &axum::Router,
    id: &str,
    content_type: &str,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/collections/demo/items/{id}"))
                .header(header::CONTENT_TYPE, content_type)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn merge_patch_updates_recursively_preserves_path_id_and_returns_validators() {
    let (app, store) = build_app();
    let response = patch(
        &app,
        "x",
        "application/merge-patch+json",
        json!({
            "id": "attacker-chosen",
            "properties": {
                "name": "new",
                "count": null,
                "nested": { "remove": null, "added": 2 },
                "tags": ["replacement"]
            }
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/geo+json"
    );
    assert!(response.headers().get(header::ETAG).is_some());
    assert!(response.headers().get("content-crs").is_some());
    let body = json_body(response).await;
    assert_eq!(body["id"], "x");
    assert_eq!(body["properties"]["name"], "new");
    assert!(body["properties"].get("count").is_some());
    assert_eq!(body["properties"]["count"], Value::Null);
    assert_eq!(
        body["properties"]["nested"],
        json!({"keep": true, "added": 2})
    );
    assert_eq!(body["properties"]["tags"], json!(["replacement"]));
    assert_eq!(store.lock().unwrap().get("x"), Some(&body));
}

#[tokio::test]
async fn merge_patch_returns_404_for_a_missing_feature() {
    let (app, _) = build_app();
    let response = patch(
        &app,
        "missing",
        "application/merge-patch+json",
        json!({"properties": {"name": "new"}}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn merge_patch_requires_its_registered_media_type() {
    let (app, _) = build_app();
    let response = patch(
        &app,
        "x",
        "application/json",
        json!({"properties": {"name": "new"}}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn merge_patch_media_type_is_case_insensitive_and_accepts_parameters() {
    let (app, _) = build_app();
    let response = patch(
        &app,
        "x",
        "Application/Merge-Patch+Json; charset=utf-8",
        json!({"properties": {"name": "new"}}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn merge_patch_validates_the_final_feature_against_the_collection_schema() {
    let (app, store) = build_app();
    let response = patch(
        &app,
        "x",
        "application/merge-patch+json",
        json!({"properties": {"name": null}}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(store.lock().unwrap().get("x"), Some(&seeded_feature()));
}

#[tokio::test]
async fn merge_patch_rejects_removing_the_required_properties_member() {
    let (app, store) = build_app();
    let response = patch(
        &app,
        "x",
        "application/merge-patch+json",
        json!({"properties": null}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(store.lock().unwrap().get("x"), Some(&seeded_feature()));
}

#[tokio::test]
async fn merge_patch_rejects_a_scalar_final_representation() {
    let (app, store) = build_app();
    let response = patch(&app, "x", "application/merge-patch+json", json!(false)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(store.lock().unwrap().get("x"), Some(&seeded_feature()));
}

#[tokio::test]
async fn merge_patch_rejects_malformed_geojson_geometry_before_writing() {
    let (app, store) = build_app();
    let response = patch(
        &app,
        "x",
        "application/merge-patch+json",
        json!({"geometry": {}}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(store.lock().unwrap().get("x"), Some(&seeded_feature()));
}

#[tokio::test]
async fn merge_patch_honors_if_match_before_writing() {
    let (app, store) = build_app();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/collections/demo/items/x")
                .header(header::CONTENT_TYPE, "application/merge-patch+json")
                .header(header::IF_MATCH, "\"stale\"")
                .body(Body::from(
                    json!({"properties": {"name": "new"}}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(store.lock().unwrap().get("x"), Some(&seeded_feature()));
}
