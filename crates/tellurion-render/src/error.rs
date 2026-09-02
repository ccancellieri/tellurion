//! Errors returned by MVT decoding, geometry processing, style parsing, and PNG encoding.

use thiserror::Error;

/// Errors produced while rasterizing an MVT tile or parsing a [`crate::RenderStyle`].
#[derive(Debug, Error)]
pub enum RenderError {
    /// `tile_size` was zero or otherwise rejected by the rasterizer.
    #[error("invalid tile size: {0}")]
    InvalidTileSize(u32),
    /// The input bytes are not a valid Mapbox Vector Tile protobuf message.
    #[error("malformed MVT: {0}")]
    Decode(String),
    /// A feature's geometry command stream could not be decoded safely.
    #[error("geometry processing failed: {0}")]
    Geometry(String),
    /// The rasterized pixmap could not be encoded as PNG.
    #[error("PNG encoding failed: {0}")]
    Encode(String),
    /// `encode_rgba_to_png`'s input buffer didn't match `width * height * 4`
    /// bytes, or `width`/`height` was zero.
    #[error("invalid raster dimensions: {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },
    /// A style color string was not a valid `"#rrggbb"` / `"#rrggbbaa"` CSS-hex value.
    #[error("invalid style color \"{value}\": {reason}")]
    InvalidColor { value: String, reason: &'static str },
}

/// Result alias used throughout this crate.
pub type Result<T> = std::result::Result<T, RenderError>;
