//! OGC API — Tiles Part 1: tile handlers (WebMercatorQuad, and `#190`'s
//! WorldCRS84Quad where the resolved driver can serve it) + cache policy.
//! Driver-agnostic: reaches storage only through `tellurion-core` traits,
//! and rasterizes only through `tellurion-render`, at the response boundary.

mod conformance;
mod handlers;
mod maps;
mod mercator;
mod tilematrixset;

pub use conformance::{
    CONFORMANCE_MAPS_CORE, CONFORMANCE_MAPS_CRS, CONFORMANCE_MAPS_PNG, CONFORMANCE_MVT,
    CONFORMANCE_PNG, CONFORMANCE_TILESET, CONFORMANCE_TILESETS_LIST, CONFORMANCE_TILES_CORE,
};
// `MAP_REL` (`#220`): re-exported for the same single-spelling reason
// `tellurion-features` re-exports its own rels — the styled-map link a
// STAC/Features document carries must name the identical relation type
// this crate's TileSet resource already advertises.
pub use handlers::{router, DEFAULT_TENANT, MAP_REL, TILE_CACHE_CONTROL};
pub use tilematrixset::{
    document_for, web_mercator_quad_document, web_mercator_quad_matrices,
    world_crs84_quad_document, world_crs84_quad_matrices, TileMatrix, TileMatrixSetDoc,
    WEB_MERCATOR_QUAD_CRS, WEB_MERCATOR_QUAD_ID, WEB_MERCATOR_QUAD_URI, WORLD_CRS84_QUAD_CRS,
    WORLD_CRS84_QUAD_ID, WORLD_CRS84_QUAD_URI,
};
