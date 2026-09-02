//! The axum router this crate exposes. Mounting is the server crate's
//! decision: nest at the root for a single fixed tenant, or under a
//! `/{tenant}` segment for multi-tenant serving — both work unchanged
//! (see `handlers::tenant_of`).

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use tellurion_core::AppContext;

use crate::batch_handlers;
use crate::feed_handlers;
use crate::handlers;
use crate::write_handlers;

pub fn router() -> Router<Arc<AppContext>> {
    Router::new()
        .route("/collections", get(handlers::list_collections))
        .route("/collections/{cid}", get(handlers::get_collection))
        .route(
            "/collections/{cid}/queryables",
            get(handlers::get_queryables),
        )
        .route(
            "/collections/{cid}/items",
            get(handlers::list_items).post(write_handlers::create_item),
        )
        .route(
            "/collections/{cid}/items/batch",
            post(batch_handlers::batch_items),
        )
        .route(
            "/collections/{cid}/items/{fid}",
            get(handlers::get_item)
                .put(write_handlers::put_item)
                .patch(write_handlers::patch_item)
                .delete(write_handlers::delete_item),
        )
        .route(
            "/collections/{cid}/changes",
            get(feed_handlers::list_changes),
        )
}
