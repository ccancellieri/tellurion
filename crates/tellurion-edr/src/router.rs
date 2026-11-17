//! Axum router for OGC API - EDR Part 1 v1.1.

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use tellurion_core::AppContext;

use crate::handlers;

pub fn router() -> Router<Arc<AppContext>> {
    Router::new()
        .route("/collections", get(handlers::list_collections))
        .route("/collections/{cid}", get(handlers::get_collection))
        .route(
            "/collections/{cid}/instances",
            get(handlers::list_instances),
        )
        .route(
            "/collections/{cid}/instances/{iid}",
            get(handlers::get_instance),
        )
        .route(
            "/collections/{cid}/locations",
            get(handlers::list_locations),
        )
        .route(
            "/collections/{cid}/locations/{locId}",
            get(handlers::get_location),
        )
        .route("/collections/{cid}/position", get(handlers::query_position))
        .route("/collections/{cid}/area", get(handlers::query_area))
        .route("/collections/{cid}/cube", get(handlers::query_cube))
        .route("/collections/{cid}/radius", get(handlers::query_radius))
        .route(
            "/collections/{cid}/trajectory",
            get(handlers::query_trajectory),
        )
        .route("/collections/{cid}/corridor", get(handlers::query_corridor))
        .route("/collections/{cid}/items", get(handlers::list_items))
        .route("/collections/{cid}/items/{itemId}", get(handlers::get_item))
        .route("/collections/{cid}/scenes", get(handlers::list_items))
        .route(
            "/collections/{cid}/scenes/{itemId}",
            get(handlers::get_item),
        )
}

/// Standalone router including `/` and `/conformance` for direct crate-level testing.
pub fn standalone_router() -> Router<Arc<AppContext>> {
    router()
        .route("/", get(handlers::landing_page))
        .route("/conformance", get(handlers::conformance))
}
