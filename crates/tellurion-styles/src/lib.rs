//! OGC API — Styles read-only surface (`/styles`, `/styles/{styleId}`,
//! `/styles/{styleId}/metadata`) over `tellurion_core::StyleStore`. DB-free,
//! like every other protocol crate. The MapLibre-paint-to-`LayerPaint`
//! resolver a styled-tile lane needs lives in `tellurion_render` (re-exported
//! there as `resolve_layer_paints`) rather than here, so that resolving a
//! styled tile never requires one protocol crate to depend on another.

mod conformance;
mod handlers;
mod model;

pub use conformance::{CONFORMANCE_MAPBOX_STYLES, CONFORMANCE_STYLES_CORE};
pub use handlers::{router, STYLE_MEDIA_TYPE};
pub use model::{
    LayerRef, Link, StyleMetadataResponse, StyleSummary, StylesListResponse, StylesheetRef,
};
