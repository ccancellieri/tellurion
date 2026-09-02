//! 3D Tiles 1.1 delivery for 3D places (`/collections/{cid}/3dtiles`,
//! `.../tiles/{tileMatrix}/{tileRow}/{tileCol}.glb`). Driver-agnostic — every access to storage
//! goes through `AppContext.router`; extrusion goes through
//! `tellurion-render` at the response boundary only. Follows the
//! `/collections/{cid}/3dtiles` URL shape from OGC API — 3D GeoVolumes
//! (OGC 22-001) without claiming conformance to it — that standard is still
//! a candidate draft with no approved conformance class to cite.

mod conformance;
mod handlers;

pub use conformance::{MEDIA_TYPE_GLB, MEDIA_TYPE_TILESET, REL_3D_TILES};
pub use handlers::{router, DEFAULT_TENANT, TILE_CACHE_CONTROL};
