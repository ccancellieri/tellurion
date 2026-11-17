//! The axum router this crate exposes. Mounting is the server crate's
//! decision — `tellurion-server` nests it at
//! `/{tenant}/records/catalogs/{catalog}`, gated by the `protocols.records`
//! exposure key (`#185`/`#192`), the same shape every other protocol root
//! is mounted with.
//!
//! Read-only by construction: no route here registers a method other than
//! `GET`. OGC API — Records — Part 1: Core defines transactional record
//! management nowhere in Part 1, and this workspace's own write lane (OGC
//! API Features Part 4) is reached through the Features root; `#192`'s first
//! slice is explicitly a read-only surface.

use std::sync::Arc;

use axum::routing::get;
use axum::Router;

use tellurion_core::AppContext;

use crate::{dimensions, handlers};

pub fn router() -> Router<Arc<AppContext>> {
    Router::new()
        .route("/collections", get(handlers::list_catalogs))
        .route("/collections/{cid}", get(handlers::get_catalog))
        .route("/collections/{cid}/items", get(handlers::list_records))
        .route(
            "/collections/{cid}/items/{recordId}",
            get(handlers::get_record),
        )
        .route("/dimensions", get(dimensions::list_dimensions))
        .route("/dimensions/{did}", get(dimensions::get_dimension))
        .route(
            "/dimensions/{did}/items",
            get(dimensions::list_dimension_items),
        )
        .route(
            "/dimensions/{did}/items/{memberId}",
            get(dimensions::get_dimension_item),
        )
}
