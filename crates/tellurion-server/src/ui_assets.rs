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
//! The workspace's `ui/vite.config.ts` builds with a relative asset base (`./assets/...`)
//! rather than an absolute `/ui/assets/...` one, so the exact same
//! bundles also work hosted standalone at the root of any static
//! file server, not just embedded here. Relative paths only resolve
//! correctly against a document URL ending in `/`, which is why bare
//! `/ui` redirects to `/ui/` below instead of serving the shell directly.

use std::sync::Arc;

use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use rust_embed::RustEmbed;

use tellurion_core::AppContext;

#[derive(RustEmbed)]
#[cfg(not(feature = "public-demo"))]
#[folder = "ui/dist"]
struct UiAssets;

#[derive(RustEmbed)]
#[cfg(feature = "public-demo")]
#[folder = "ui/public-demo-dist"]
struct UiAssets;

const INDEX_HTML: &str = "index.html";

fn asset_response(path: &str) -> Response {
    match UiAssets::get(path) {
        Some(file) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, file.metadata.mimetype().to_string())],
            file.data.into_owned(),
        )
            .into_response(),
        // Any path this embed doesn't recognize falls back to the shell
        // (the same "index fallback" a static-file server gives a
        // single-page app) rather than a bare 404 — this UI has no
        // client-side router yet, but a bookmarked/refreshed `/ui/...`
        // deep link should still load something instead of dead-ending.
        None => match UiAssets::get(INDEX_HTML) {
            Some(index) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html".to_string())],
                index.data.into_owned(),
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
    }
}

async fn serve_index() -> Response {
    asset_response(INDEX_HTML)
}

async fn serve_path(Path(path): Path<String>) -> Response {
    asset_response(&path)
}

/// Builds the `/ui` route table. Mounted directly into the server's
/// top-level router (not nested under a prefix) so `serve_path`'s
/// wildcard capture receives the path exactly as `UiAssets::get` expects
/// it — relative to the selected crate-local bundle, no leading `/ui`.
pub fn router() -> Router<Arc<AppContext>> {
    Router::new()
        .route("/ui", get(|| async { Redirect::permanent("/ui/") }))
        .route("/ui/", get(serve_index))
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
    async fn unknown_path_falls_back_to_the_index_shell() {
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ui/does/not/exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Tellurion"));
    }
}
