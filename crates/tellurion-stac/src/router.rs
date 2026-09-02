//! The axum router this crate exposes. Mounting is the server crate's
//! decision — nested at `/{tenant}/stac/catalogs/{catalog}`, a fifth
//! protocol root beside features/tiles/styles/3dtiles — same shape
//! `tellurion_features::router`'s own doc comment describes.

use std::sync::Arc;

use axum::routing::get;
use axum::Router;

use tellurion_core::AppContext;

use crate::asset_handlers;
use crate::handlers;

pub fn router() -> Router<Arc<AppContext>> {
    Router::new()
        .route("/collections", get(handlers::list_collections))
        .route("/collections/{cid}", get(handlers::get_collection))
        .route("/collections/{cid}/items", get(handlers::list_items))
        .route("/collections/{cid}/items/{fid}", get(handlers::get_item))
        // `#36` slice C: STAC API - Item Search, both methods GET and POST
        // required/recommended by the spec — see `handlers::search_get`/
        // `search_post`'s own docs.
        .route(
            "/search",
            get(handlers::search_get).post(handlers::search_post),
        )
        // Assets-and-object-storage proposal, first slice
        // (`asset_handlers.rs`): one handler set serves both the
        // collection-level and item-level mounts below — `Path<HashMap<...>>`
        // captures whichever named segments a given route declares.
        // Reconcile surface (`#93`, read-only report): collection-level
        // only. Axum's own router (matchit) prefers a literal segment over
        // the sibling `{key}` wildcard below regardless of declaration
        // order, so `reconcile` can never be shadowed by, or shadow, a real
        // asset whose key happens to be the literal string `reconcile`.
        .route(
            "/collections/{cid}/assets/reconcile",
            get(asset_handlers::get_reconcile_report),
        )
        .route(
            "/collections/{cid}/assets/{key}",
            get(asset_handlers::get_asset)
                .put(asset_handlers::put_asset)
                .delete(asset_handlers::delete_asset),
        )
        .route(
            "/collections/{cid}/assets/{key}/data",
            get(asset_handlers::get_asset_data).put(asset_handlers::put_asset_data),
        )
        // `presigned-upload` conformance class (s3-compatible object-store
        // profile, second slice): the negotiation surface alongside the
        // direct-upload `.../data` above, and the commit call that flips
        // pending -> available/failed — see `asset_handlers.rs`'s own docs.
        .route(
            "/collections/{cid}/assets/{key}/data/presign",
            get(asset_handlers::get_asset_presign).put(asset_handlers::put_asset_presign),
        )
        .route(
            "/collections/{cid}/assets/{key}/finalize",
            axum::routing::post(asset_handlers::post_asset_finalize),
        )
        // `resumable-upload` conformance class (`fs`-profile object stores
        // only, third slice): the chunked-append transport alongside
        // direct-upload and presigned-upload — see `asset_handlers.rs`'s own
        // doc. `GET` also answers a literal `HEAD` automatically (axum's own
        // built-in behavior for a `GET`-routed handler).
        .route(
            "/collections/{cid}/assets/{key}/data/uploads",
            axum::routing::post(asset_handlers::post_create_upload)
                .get(asset_handlers::get_upload_offset)
                .patch(asset_handlers::patch_append_upload)
                .delete(asset_handlers::delete_upload),
        )
        .route(
            "/collections/{cid}/assets/{key}/data/uploads/complete",
            axum::routing::post(asset_handlers::post_complete_upload),
        )
        .route(
            "/collections/{cid}/items/{fid}/assets/{key}",
            get(asset_handlers::get_asset)
                .put(asset_handlers::put_asset)
                .delete(asset_handlers::delete_asset),
        )
        .route(
            "/collections/{cid}/items/{fid}/assets/{key}/data",
            get(asset_handlers::get_asset_data).put(asset_handlers::put_asset_data),
        )
        .route(
            "/collections/{cid}/items/{fid}/assets/{key}/data/presign",
            get(asset_handlers::get_asset_presign).put(asset_handlers::put_asset_presign),
        )
        .route(
            "/collections/{cid}/items/{fid}/assets/{key}/finalize",
            axum::routing::post(asset_handlers::post_asset_finalize),
        )
        .route(
            "/collections/{cid}/items/{fid}/assets/{key}/data/uploads",
            axum::routing::post(asset_handlers::post_create_upload)
                .get(asset_handlers::get_upload_offset)
                .patch(asset_handlers::patch_append_upload)
                .delete(asset_handlers::delete_upload),
        )
        .route(
            "/collections/{cid}/items/{fid}/assets/{key}/data/uploads/complete",
            axum::routing::post(asset_handlers::post_complete_upload),
        )
}
