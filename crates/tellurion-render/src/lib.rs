//! MVT -> PNG/glTF rasterization and extrusion. Pure: bytes in, bytes out;
//! no database, no I/O.

mod earclip;
pub mod error;
mod extrude;
mod maplibre;
mod mesh;
mod raster;
pub mod style;
mod styled;
mod volume;
mod window;

pub use earclip::triangulate_face;
pub use error::{RenderError, Result};
pub use extrude::{extrude_mvt_to_glb, ExtrudeParams, MAX_HEIGHT_METERS};
pub use maplibre::{resolve_layer_paints, source_layers, style_paints_any_layer};
pub use raster::{encode_rgba_to_png, render_mvt_to_png};
pub use style::{parse_css_hex_color, RenderStyle};
pub use styled::{render_mvt_to_png_styled, LayerPaint};
pub use volume::volume_mesh_to_glb;
pub use window::{
    render_map_window, render_map_window_styled, render_raster_map_window, MapTile, RasterMapTile,
};
