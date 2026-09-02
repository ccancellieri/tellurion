//! OGC API — Styles read-only handlers: `GET /styles`, `GET
//! /styles/{styleId}`, `GET /styles/{styleId}/metadata`. Every handler reads
//! through `AppContext.style_store` — no handler here names a concrete
//! store implementation. Every request runs under a
//! `/{tenant}/styles/catalogs/{catalog}` mount (`#39`); `tenant`/`catalog`
//! path parameters carry EXTERNAL ids exactly as the client typed them —
//! `resolve_tenant_catalog` turns them into the internal ids needed only to
//! gate the request (an unknown tenant/catalog 404s, same as every other
//! protocol root). The style document registry itself stays global/unscoped
//! this wave: there is no per-tenant or per-catalog style data, so once the
//! gate passes, `ctx.style_store` lookups are unchanged. A handler that runs
//! with no mount at all (this crate's own unit tests) falls back to
//! [`DEFAULT_TENANT`]/[`DEFAULT_CATALOG`], the same convention
//! `tellurion-tiles`/`tellurion-features` use.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{OriginalUri, Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use tellurion_core::problem::{Problem, PROBLEM_JSON};
use tellurion_core::AppContext;

use crate::model::{
    LayerRef, Link, StyleMetadataResponse, StyleSummary, StylesListResponse, StylesheetRef,
};

pub const DEFAULT_TENANT: &str = "public";
pub const DEFAULT_CATALOG: &str = "default";

/// Media type for `GET /styles/{styleId}`.
///
/// There is no IANA-registered media type for MapLibre (or Mapbox) Style
/// JSON — verified 2026-07 against the IANA media types registry, which has
/// no `vnd.mapbox` or `vnd.maplibre` entry. The OGC API — Styles draft (OGC
/// 20-009), the closest thing to an interoperability spec for this format,
/// defines `application/vnd.mapbox.style+json` as the (also unregistered,
/// by the draft's own admission) media type its `mapbox-styles` requirement
/// class uses, and it is the value Mapbox's own Styles API accepts via
/// `?f=mapbox`. MapLibre Style JSON is a compatible superset of the Mapbox
/// Style Spec, so that value — not a fabricated `vnd.maplibre-style+json`,
/// which appears nowhere in any spec or implementation — is what this crate
/// serves. `application/json` is used for the wrapper resources (`/styles`,
/// `/styles/{styleId}/metadata`) that are Tellurion-specific JSON, not the
/// style document itself.
pub const STYLE_MEDIA_TYPE: &str = "application/vnd.mapbox.style+json";
const JSON_MEDIA_TYPE: &str = "application/json";

pub fn router() -> Router<Arc<AppContext>> {
    Router::new()
        .route("/styles", get(list_styles))
        .route("/styles/{styleId}", get(get_style))
        .route("/styles/{styleId}/metadata", get(get_style_metadata))
}

fn tenant_of(params: &HashMap<String, String>) -> String {
    params
        .get("tenant")
        .cloned()
        .unwrap_or_else(|| DEFAULT_TENANT.to_string())
}

fn catalog_of(params: &HashMap<String, String>) -> String {
    params
        .get("catalog")
        .cloned()
        .unwrap_or_else(|| DEFAULT_CATALOG.to_string())
}

/// Resolves this request's `(tenant, catalog)` path segments — external ids
/// — to internal ones (`#39`), gating access the same way every other
/// protocol root does. The internal ids themselves are never used below
/// this: the style registry lookup that follows is unscoped, so a caller
/// only needs `None` (unknown tenant/catalog -> 404) vs `Some(())` (known ->
/// proceed).
async fn resolve_tenant_catalog(ctx: &AppContext, params: &HashMap<String, String>) -> Option<()> {
    let state = ctx.current();
    let tenant_ext = tenant_of(params);
    let catalog_ext = catalog_of(params);
    let tenant_id = state.resolver.resolve_tenant(&tenant_ext).await.ok()?;
    state
        .resolver
        .resolve_catalog(&tenant_id, &catalog_ext)
        .await
        .ok()?;
    Some(())
}

fn style_href(styles_root: &str, id: &str) -> String {
    format!("{styles_root}/{id}")
}

fn metadata_href(styles_root: &str, id: &str) -> String {
    format!("{styles_root}/{id}/metadata")
}

fn set_content_type(response: &mut Response, media_type: &'static str) {
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
}

/// Shared RFC 9457 problem-details body — same type `tellurion-features` and
/// `tellurion-tiles` serve for their own API errors.
fn problem_response(status: StatusCode, code: &str, detail: impl Into<String>) -> Response {
    let problem = Problem::new(status.as_u16(), code, detail);
    let mut response = (status, Json(problem)).into_response();
    set_content_type(&mut response, PROBLEM_JSON);
    response
}

/// GET /styles
async fn list_styles(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if resolve_tenant_catalog(&ctx, &params).await.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let ids = match ctx.style_store.list() {
        Ok(ids) => ids,
        Err(error) => {
            tracing::error!(%error, "style store failed to list styles");
            return problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                "an internal storage error occurred",
            );
        }
    };

    // This handler is mounted at `.../styles` (see `router` below) —
    // `uri.path()` IS the styles root every sibling href in the list is
    // built relative to.
    let styles_root = uri.path().trim_end_matches('/').to_string();
    let styles = ids
        .into_iter()
        .map(|id| {
            let links = vec![
                Link::new(
                    style_href(&styles_root, &id),
                    "stylesheet",
                    STYLE_MEDIA_TYPE,
                ),
                // No OGC API — Styles example defines a standard rel for
                // the metadata resource; "describedby" (IANA-registered:
                // "refers to a resource providing information about the
                // link's context") is the honest, defensible choice here.
                Link::new(
                    metadata_href(&styles_root, &id),
                    "describedby",
                    JSON_MEDIA_TYPE,
                ),
            ];
            StyleSummary { id, links }
        })
        .collect();

    let mut response = Json(StylesListResponse { styles }).into_response();
    set_content_type(&mut response, JSON_MEDIA_TYPE);
    response
}

/// GET /styles/{styleId} — the raw MapLibre Style JSON document.
async fn get_style(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
) -> Response {
    if resolve_tenant_catalog(&ctx, &params).await.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(style_id) = params.get("styleId") else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match ctx.style_store.load(style_id) {
        Ok(Some(doc)) => {
            let mut response = Json(doc).into_response();
            set_content_type(&mut response, STYLE_MEDIA_TYPE);
            response
        }
        Ok(None) => problem_response(
            StatusCode::NOT_FOUND,
            "NotFound",
            "the requested resource was not found",
        ),
        Err(error) => {
            tracing::error!(%error, style_id, "style store failed to load a style");
            problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                "an internal storage error occurred",
            )
        }
    }
}

/// GET /styles/{styleId}/metadata
async fn get_style_metadata(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if resolve_tenant_catalog(&ctx, &params).await.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(style_id) = params.get("styleId") else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match ctx.style_store.load(style_id) {
        Ok(Some(doc)) => {
            let title = doc.get("name").and_then(|v| v.as_str()).map(str::to_string);
            let layers = doc
                .get("layers")
                .and_then(|v| v.as_array())
                .map(|layers| {
                    layers
                        .iter()
                        .filter_map(|layer| {
                            layer
                                .get("id")
                                .and_then(|v| v.as_str())
                                .map(|id| LayerRef { id: id.to_string() })
                        })
                        .collect()
                })
                .unwrap_or_default();

            // This handler is mounted at `.../styles/{styleId}/metadata` —
            // stripping that known suffix off `uri.path()` recovers the
            // styles root the sibling stylesheet link is built from, same
            // idiom as tiles' `tileset` handler.
            let self_path = uri.path().to_string();
            let styles_root = self_path
                .strip_suffix(&format!("/{style_id}/metadata"))
                .unwrap_or(&self_path)
                .to_string();

            let body = StyleMetadataResponse {
                id: style_id.clone(),
                title,
                stylesheets: vec![StylesheetRef {
                    link: Link::new(
                        style_href(&styles_root, style_id),
                        "stylesheet",
                        STYLE_MEDIA_TYPE,
                    ),
                    native: true,
                }],
                layers,
            };

            let mut response = Json(body).into_response();
            set_content_type(&mut response, JSON_MEDIA_TYPE);
            response
        }
        Ok(None) => problem_response(
            StatusCode::NOT_FOUND,
            "NotFound",
            "the requested resource was not found",
        ),
        Err(error) => {
            tracing::error!(%error, style_id, "style store failed to load style metadata");
            problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalServerError",
                "an internal storage error occurred",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use tellurion_core::{
        AppConfig, Error as CoreError, MokaTileCache, Registry, Resolver, Result as CoreResult,
        Router as CoreRouter, StaticResolver, StyleStore, TileCache,
    };

    struct FakeStyleStore {
        docs: HashMap<String, serde_json::Value>,
        error_id: Mutex<Option<String>>,
    }

    impl FakeStyleStore {
        fn new(docs: HashMap<String, serde_json::Value>) -> Self {
            Self {
                docs,
                error_id: Mutex::new(None),
            }
        }

        fn erroring(mut self, id: &str) -> Self {
            self.error_id = Mutex::new(Some(id.to_string()));
            self
        }
    }

    impl StyleStore for FakeStyleStore {
        fn load(&self, id: &str) -> CoreResult<Option<serde_json::Value>> {
            if self.error_id.lock().unwrap().as_deref() == Some(id) {
                return Err(CoreError::Config("simulated store failure".to_string()));
            }
            Ok(self.docs.get(id).cloned())
        }

        fn list(&self) -> CoreResult<Vec<String>> {
            let mut ids: Vec<String> = self.docs.keys().cloned().collect();
            ids.sort();
            Ok(ids)
        }
    }

    fn basic_style_doc() -> serde_json::Value {
        serde_json::json!({
            "version": 8,
            "name": "Basic",
            "layers": [
                { "id": "background", "type": "background" },
                { "id": "buildings-fill", "type": "fill", "source-layer": "buildings" },
            ],
        })
    }

    fn test_context(store: FakeStyleStore) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let registry = Registry::new();
        let router = CoreRouter::build(&config, &registry).unwrap();
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            Arc::new(store),
        ))
    }

    /// No `tenant`/`catalog` path params — every handler falls back to
    /// [`DEFAULT_TENANT`]/[`DEFAULT_CATALOG`], which `test_context`'s config
    /// always declares.
    fn no_params() -> Path<HashMap<String, String>> {
        Path(HashMap::new())
    }

    fn style_params(style_id: &str) -> Path<HashMap<String, String>> {
        Path(HashMap::from([(
            "styleId".to_string(),
            style_id.to_string(),
        )]))
    }

    fn uri(path: &str) -> OriginalUri {
        OriginalUri(axum::http::Uri::try_from(path).unwrap())
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Asserts a response carries the shared RFC 9457 problem-details body:
    /// `application/problem+json` content type plus `type`/`title`/`status`/
    /// `detail`/`code` fields, with `code` and `status` matching the given
    /// values — same helper shape `tellurion-places` uses for its own error
    /// tests.
    async fn assert_problem_json(response: Response, status: StatusCode, code: &str) {
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let json = body_json(response).await;
        assert_eq!(json["type"], "about:blank");
        assert_eq!(json["status"], status.as_u16());
        assert_eq!(json["code"], code);
        assert!(json["title"].is_string());
        assert!(json["detail"].is_string());
    }

    #[tokio::test]
    async fn lists_registered_styles_with_stylesheet_and_metadata_links() {
        let mut docs = HashMap::new();
        docs.insert("basic".to_string(), basic_style_doc());
        let ctx = test_context(FakeStyleStore::new(docs));

        let response = list_styles(State(ctx), no_params(), uri("/styles")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            JSON_MEDIA_TYPE
        );

        let json = body_json(response).await;
        assert_eq!(json["styles"][0]["id"], "basic");
        assert_eq!(json["styles"][0]["links"][0]["rel"], "stylesheet");
        assert_eq!(json["styles"][0]["links"][0]["href"], "/styles/basic");
        assert_eq!(json["styles"][0]["links"][0]["type"], STYLE_MEDIA_TYPE);
        assert_eq!(json["styles"][0]["links"][1]["rel"], "describedby");
        assert_eq!(
            json["styles"][0]["links"][1]["href"],
            "/styles/basic/metadata"
        );
    }

    #[tokio::test]
    async fn list_is_empty_when_no_styles_registered() {
        let ctx = test_context(FakeStyleStore::new(HashMap::new()));
        let response = list_styles(State(ctx), no_params(), uri("/styles")).await;
        let json = body_json(response).await;
        assert_eq!(json["styles"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn get_style_returns_the_raw_document_with_the_style_media_type() {
        let mut docs = HashMap::new();
        docs.insert("basic".to_string(), basic_style_doc());
        let ctx = test_context(FakeStyleStore::new(docs));

        let response = get_style(State(ctx), style_params("basic")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            STYLE_MEDIA_TYPE
        );
        let json = body_json(response).await;
        assert_eq!(json["name"], "Basic");
    }

    #[tokio::test]
    async fn get_style_unknown_id_is_404() {
        let ctx = test_context(FakeStyleStore::new(HashMap::new()));
        let response = get_style(State(ctx), style_params("missing")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_problem_json(response, StatusCode::NOT_FOUND, "NotFound").await;
    }

    #[tokio::test]
    async fn get_style_store_error_is_internal_server_error() {
        let mut docs = HashMap::new();
        docs.insert("broken".to_string(), basic_style_doc());
        let ctx = test_context(FakeStyleStore::new(docs).erroring("broken"));
        let response = get_style(State(ctx), style_params("broken")).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_problem_json(
            response,
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalServerError",
        )
        .await;
    }

    #[tokio::test]
    async fn get_style_metadata_extracts_title_and_layer_ids() {
        let mut docs = HashMap::new();
        docs.insert("basic".to_string(), basic_style_doc());
        let ctx = test_context(FakeStyleStore::new(docs));

        let response = get_style_metadata(
            State(ctx),
            style_params("basic"),
            uri("/styles/basic/metadata"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            JSON_MEDIA_TYPE
        );
        let json = body_json(response).await;
        assert_eq!(json["id"], "basic");
        assert_eq!(json["title"], "Basic");
        assert_eq!(json["stylesheets"][0]["native"], true);
        assert_eq!(json["stylesheets"][0]["link"]["href"], "/styles/basic");
        assert_eq!(
            json["layers"],
            serde_json::json!([{ "id": "background" }, { "id": "buildings-fill" }])
        );
    }

    #[tokio::test]
    async fn get_style_metadata_omits_title_when_the_document_has_no_name() {
        let mut docs = HashMap::new();
        docs.insert(
            "unnamed".to_string(),
            serde_json::json!({ "version": 8, "layers": [] }),
        );
        let ctx = test_context(FakeStyleStore::new(docs));

        let response = get_style_metadata(
            State(ctx),
            style_params("unnamed"),
            uri("/styles/unnamed/metadata"),
        )
        .await;
        let json = body_json(response).await;
        assert!(json.get("title").is_none());
    }

    #[tokio::test]
    async fn get_style_metadata_unknown_id_is_404() {
        let ctx = test_context(FakeStyleStore::new(HashMap::new()));
        let response = get_style_metadata(
            State(ctx),
            style_params("missing"),
            uri("/styles/missing/metadata"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_problem_json(response, StatusCode::NOT_FOUND, "NotFound").await;
    }

    /// An unknown tenant in the path must 404 before the style store is ever
    /// consulted — the same gate every other protocol root applies (`#39`).
    #[tokio::test]
    async fn unknown_tenant_is_not_found() {
        let mut docs = HashMap::new();
        docs.insert("basic".to_string(), basic_style_doc());
        let ctx = test_context(FakeStyleStore::new(docs));

        let params = Path(HashMap::from([(
            "tenant".to_string(),
            "other-tenant".to_string(),
        )]));
        let response = list_styles(State(ctx), params, uri("/other-tenant/styles")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
