//! Serves a UI built from the workspace's `ui/` sources and embedded in the
//! binary, mounted at `/ui`. The ordinary `ui` feature selects the operator
//! bundle in this crate's `ui/dist`; combining it with `public-demo` selects
//! `ui/public-demo-dist` instead. Keeping the bundles distinct means packaged
//! builds cannot silently serve the wrong shell. `build.rs` fails early, with
//! the matching npm command, if the selected bundle does not exist.
//!
//! `rust-embed`'s `debug-embed` feature (see this crate's `Cargo.toml`) is
//! on so the embed is unconditional in every build profile, not just
//! release — the point of this feature is a self-contained binary
//! regardless of how it was built.
//!
//! The standalone bundle keeps Vite's relative asset base. Embedded scoped
//! control shells add a `/ui/` base element so deep links resolve the same
//! assets without changing the standalone bundle.

use std::sync::Arc;

use axum::extract::{OriginalUri, Path, RawQuery};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use rust_embed::RustEmbed;

use tellurion_core::AppContext;

use crate::control_workspace::ControlWorkspace;

#[derive(RustEmbed)]
#[cfg(not(feature = "public-demo"))]
#[folder = "ui/dist"]
struct UiAssets;

#[derive(RustEmbed)]
#[cfg(feature = "public-demo")]
#[folder = "ui/public-demo-dist"]
struct UiAssets;

const INDEX_HTML: &str = "index.html";
const THIRD_PARTY_NOTICES: &str = include_str!("../ui/THIRD_PARTY_NOTICES.txt");

fn asset_response(path: &str) -> Response {
    match UiAssets::get(path) {
        Some(file) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, file.metadata.mimetype().to_string())],
            file.data.into_owned(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn serve_index() -> Response {
    asset_response(INDEX_HTML)
}

async fn serve_control_index() -> Response {
    let Some(file) = UiAssets::get(INDEX_HTML) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(index) = std::str::from_utf8(file.data.as_ref()) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html")],
        index.replacen("<head>", "<head><base href=\"/ui/\">", 1),
    )
        .into_response()
}

async fn serve_third_party_notices() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        THIRD_PARTY_NOTICES,
    )
        .into_response()
}

async fn serve_path(Path(path): Path<String>) -> Response {
    asset_response(&path)
}

fn redirect_with_query(path: &str, query: Option<&str>) -> Redirect {
    let location = match query {
        Some(query) if !query.is_empty() => format!("{path}?{query}"),
        _ => path.to_string(),
    };
    Redirect::permanent(&location)
}

async fn control_deep_link(OriginalUri(uri): OriginalUri) -> Response {
    let path = uri.path();
    let canonical = path.strip_suffix('/').unwrap_or(path);
    if ControlWorkspace::parse(canonical).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    if canonical != path {
        return redirect_with_query(canonical, uri.query()).into_response();
    }
    serve_control_index().await
}

async fn redirect_control_demo(RawQuery(query): RawQuery) -> Redirect {
    redirect_with_query("/ui/control-demo", query.as_deref())
}

/// Builds the `/ui` route table. Mounted directly into the server's
/// top-level router (not nested under a prefix) so `serve_path`'s
/// wildcard capture receives the path exactly as `UiAssets::get` expects
/// it — relative to the selected crate-local bundle, no leading `/ui`.
pub fn router() -> Router<Arc<AppContext>> {
    Router::new()
        .route("/ui", get(|| async { Redirect::permanent("/ui/") }))
        .route("/ui/", get(serve_index))
        .route("/ui/control", get(serve_index))
        .route("/ui/control/", get(control_deep_link))
        .route("/ui/control/{*path}", get(control_deep_link))
        .route("/ui/control-demo", get(serve_index))
        .route("/ui/control-demo/", get(redirect_control_demo))
        .route("/ui/control-demo/{*path}", get(redirect_control_demo))
        .route(
            "/ui/THIRD_PARTY_NOTICES.txt",
            get(serve_third_party_notices),
        )
        .route("/ui/{*path}", get(serve_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    use tellurion_core::{
        AppConfig, FileStyleStore, MokaTileCache, Registry, Resolver, Router as CoreRouter,
        StaticResolver,
    };

    /// This module's routes never touch `AppContext` (the embedded UI is
    /// entirely static), so the fixture only needs to satisfy the router's
    /// state type — same empty-config shape `tellurion-styles`' handler
    /// tests use for the same reason. `resolver` and `authorizer` follow the
    /// same minimal-fixture pattern as `app.rs`'s own `test_ctx`: a
    /// `StaticResolver` built from the same empty config, no authorizer
    /// (`None`, matching a config with no `auth:` section).
    fn test_app() -> Router {
        let config = AppConfig::default();
        config.validate().unwrap();
        let registry = Registry::new();
        let core_router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn tellurion_core::TileCache> =
            Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn tellurion_core::StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            core_router,
            resolver,
            None,
            cache,
            style_store,
        ));
        router().with_state(ctx)
    }

    #[tokio::test]
    async fn bare_ui_redirects_to_the_trailing_slash() {
        // The bundle's relative asset base only resolves correctly against
        // a document URL ending in `/` (see this module's doc comment), so
        // `/ui` must redirect rather than serve the shell directly.
        let app = test_app();
        let response = app
            .oneshot(Request::builder().uri("/ui").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(response.status().is_redirection());
        assert_eq!(response.headers().get(header::LOCATION).unwrap(), "/ui/");
    }

    #[tokio::test]
    async fn trailing_slash_serves_the_index_shell() {
        let app = test_app();
        let response = app
            .oneshot(Request::builder().uri("/ui/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Tellurion"));
    }

    #[tokio::test]
    async fn notices_route_serves_the_canonical_notice_file() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ui/THIRD_PARTY_NOTICES.txt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; charset=utf-8"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            body.as_ref(),
            include_bytes!("../ui/THIRD_PARTY_NOTICES.txt")
        );
    }

    #[tokio::test]
    async fn control_routes_preserve_scope_and_canonicalize_only_trailing_slashes() {
        let app = test_app();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ui/control/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_redirection());
        assert_eq!(response.headers()[header::LOCATION], "/ui/control");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ui/control/tenants/acme?panel=settings&scope=effective")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("<base href=\"/ui/\">"));

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/ui/control/tenants/acme/catalogs/roads/?panel=collections")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/ui/control/tenants/acme/catalogs/roads?panel=collections"
        );

        for path in [
            "/ui/control/tenants/acme/settings",
            "/ui/control/tenants/acme/catalogs/roads/collections",
            "/ui/control/tenants/acme/catalogs/roads//",
            "/ui/control/tenants/a%2Fb",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ui/control-demo/tenants/fixture?panel=plugins")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_redirection());
        assert_eq!(
            response.headers()[header::LOCATION],
            "/ui/control-demo?panel=plugins"
        );
    }

    #[tokio::test]
    async fn canonical_control_route_serves_the_index_shell() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ui/control")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Tellurion"));
        assert_eq!(
            body.as_ref(),
            UiAssets::get(INDEX_HTML).unwrap().data.as_ref()
        );
    }

    #[tokio::test]
    async fn unknown_asset_path_is_not_served_as_html() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ui/assets/does-not-exist.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
