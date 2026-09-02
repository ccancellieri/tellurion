//! Private bounded vector-feature to Mapbox Vector Tile encoding.

mod geometry;

use bytes::Bytes;
use geozero::mvt::{Message, MvtWriter, Tile};
use geozero::{ColumnValue, FeatureProcessor, GeozeroGeometry, ProcessorSink, PropertyProcessor};
use tellurion_core::{TileCoord, VertexBudget};

use crate::geometry::{
    clip_to_tile, inspect_coordinates, normalize_geometry_collection, project_to_web_mercator,
    tile_envelope_3857_unchecked,
};

const MAX_WEB_MERCATOR_ZOOM: u8 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCrs {
    Crs84,
    WebMercator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClippingPolicy {
    TopologyClip,
    PreserveGeometry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileRequest {
    pub coord: TileCoord,
    pub layer_name: String,
    pub selected_properties: Vec<String>,
    pub feature_cap: usize,
    pub vertex_cap: u64,
    pub extent: u32,
    pub source_crs: SourceCrs,
    clipping_policy: ClippingPolicy,
}

impl TileRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        coord: TileCoord,
        layer_name: impl Into<String>,
        selected_properties: Vec<String>,
        feature_cap: usize,
        vertex_cap: u64,
        extent: u32,
        source_crs: SourceCrs,
    ) -> Self {
        Self {
            coord,
            layer_name: layer_name.into(),
            selected_properties,
            feature_cap,
            vertex_cap,
            extent,
            source_crs,
            clipping_policy: ClippingPolicy::TopologyClip,
        }
    }

    pub fn preserve_unclipped_geometry(mut self) -> Self {
        self.clipping_policy = ClippingPolicy::PreserveGeometry;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TileScalar {
    Null,
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    String(String),
}

impl TileScalar {
    fn column_value<'a>(
        &'a self,
        feature_index: usize,
        property: &str,
    ) -> Result<Option<ColumnValue<'a>>, TileEncodeError> {
        match self {
            Self::Null => Ok(None),
            Self::Bool(value) => Ok(Some(ColumnValue::Bool(*value))),
            Self::Signed(value) => Ok(Some(ColumnValue::Long(*value))),
            Self::Unsigned(value) => Ok(Some(ColumnValue::ULong(*value))),
            Self::Float(value) if value.is_finite() => Ok(Some(ColumnValue::Double(*value))),
            Self::Float(_) => Err(TileEncodeError::NonFiniteProperty {
                feature_index,
                property: property.to_string(),
            }),
            Self::String(value) => Ok(Some(ColumnValue::String(value))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TileFeature {
    id: String,
    geometry: geo_types::Geometry<f64>,
    properties: Vec<(String, TileScalar)>,
}

impl TileFeature {
    pub fn new(
        id: impl Into<String>,
        geometry: geo_types::Geometry<f64>,
        properties: Vec<(String, TileScalar)>,
    ) -> Self {
        Self {
            id: id.into(),
            geometry,
            properties,
        }
    }
}

#[derive(Debug)]
pub struct TileEncodeOutcome {
    pub tile: Option<Bytes>,
    pub vertex_limit_exceeded: bool,
    pub vertices_used: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum TileEncodeError {
    #[error("tile zoom {found} exceeds the supported WebMercatorQuad maximum {max}")]
    UnsupportedZoom { found: u8, max: u8 },
    #[error("invalid tile coordinate z={z}, x={x}, y={y}")]
    InvalidCoordinate { z: u8, x: u32, y: u32 },
    #[error("feature {feature_index} property '{property}' is not finite")]
    NonFiniteProperty {
        feature_index: usize,
        property: String,
    },
    #[error("feature {feature_index} contains a non-finite source coordinate")]
    NonFiniteSourceCoordinate { feature_index: usize },
    #[error("feature {feature_index} projection produced a non-finite coordinate")]
    NonFiniteProjectedCoordinate { feature_index: usize },
    #[error("feature {feature_index} has a mixed or empty GeometryCollection")]
    MixedGeometryCollection { feature_index: usize },
    #[error("feature {feature_index} geometry is malformed: {message}")]
    MalformedGeometry {
        feature_index: usize,
        message: String,
    },
    #[error("feature source failed: {0}")]
    Source(String),
    #[error("MVT encoding failed: {0}")]
    Encoding(String),
}

pub fn encode_tile(
    request: TileRequest,
    features: impl IntoIterator<Item = Result<TileFeature, TileEncodeError>>,
) -> Result<Option<Bytes>, TileEncodeError> {
    Ok(encode_tile_with_outcome(request, features)?.tile)
}

pub fn tile_envelope_3857(coord: TileCoord) -> Result<[f64; 4], TileEncodeError> {
    validate_coordinate(coord)?;
    Ok(tile_envelope_3857_unchecked(coord))
}

pub fn encode_tile_with_outcome<E>(
    request: TileRequest,
    features: impl IntoIterator<Item = Result<TileFeature, E>>,
) -> Result<TileEncodeOutcome, E>
where
    E: From<TileEncodeError>,
{
    let [minx, miny, maxx, maxy] = tile_envelope_3857(request.coord)?;
    let mut writer = MvtWriter::new(request.extent, minx, miny, maxx, maxy)
        .map_err(|error| TileEncodeError::Encoding(error.to_string()))?;
    let mut vertex_budget = VertexBudget::new(request.vertex_cap);
    let mut encoded = 0usize;
    let mut features = features.into_iter();

    for feature_index in 0..request.feature_cap {
        let Some(feature) = features.next() else {
            break;
        };
        let TileFeature {
            id,
            geometry,
            properties,
        } = feature?;
        let mut geometry = normalize_geometry_collection(geometry)
            .map_err(|()| TileEncodeError::MixedGeometryCollection { feature_index })?;
        let (_, source_is_finite) = inspect_coordinates(&geometry).map_err(|message| {
            TileEncodeError::MalformedGeometry {
                feature_index,
                message,
            }
        })?;
        if !source_is_finite {
            return Err(TileEncodeError::NonFiniteSourceCoordinate { feature_index }.into());
        }
        if request.source_crs == SourceCrs::Crs84 {
            project_to_web_mercator(&mut geometry);
        }
        let (vertices, projected_is_finite) =
            inspect_coordinates(&geometry).map_err(|message| {
                TileEncodeError::MalformedGeometry {
                    feature_index,
                    message,
                }
            })?;
        if !projected_is_finite {
            return Err(TileEncodeError::NonFiniteProjectedCoordinate { feature_index }.into());
        }
        if !vertex_budget.try_include(vertices) {
            break;
        }
        let geometry = match request.clipping_policy {
            ClippingPolicy::TopologyClip => {
                let Some(geometry) = clip_to_tile(geometry, [minx, miny, maxx, maxy]) else {
                    continue;
                };
                geometry
            }
            ClippingPolicy::PreserveGeometry => geometry,
        };

        encode_feature(
            &mut writer,
            encoded as u64,
            feature_index,
            &id,
            &properties,
            &request.selected_properties,
            &geometry,
        )?;
        encoded += 1;
    }

    let mut layer = writer.layer(&request.layer_name);
    // Scaling to integer tile coordinates can collapse a valid source
    // geometry to zero area. `MvtWriter` still emits that geometry, but its
    // own reader used by the PNG renderer rejects it. The writer guarantees
    // command-stream structure, so validate its completed features against
    // that decoder and omit only those made invalid by quantization.
    layer
        .features
        .retain(|feature| feature.process_geom(&mut ProcessorSink::new()).is_ok());
    let tile = if layer.features.is_empty() {
        None
    } else {
        Some(Bytes::from(
            Tile {
                layers: vec![layer],
            }
            .encode_to_vec(),
        ))
    };
    Ok(TileEncodeOutcome {
        tile,
        vertex_limit_exceeded: vertex_budget.exceeded(),
        vertices_used: vertex_budget.spent(),
    })
}

fn validate_coordinate(coord: TileCoord) -> Result<(), TileEncodeError> {
    if coord.z > MAX_WEB_MERCATOR_ZOOM {
        return Err(TileEncodeError::UnsupportedZoom {
            found: coord.z,
            max: MAX_WEB_MERCATOR_ZOOM,
        });
    }
    let side = 1u64
        .checked_shl(u32::from(coord.z))
        .ok_or(TileEncodeError::UnsupportedZoom {
            found: coord.z,
            max: MAX_WEB_MERCATOR_ZOOM,
        })?;
    if u64::from(coord.x) >= side || u64::from(coord.y) >= side {
        return Err(TileEncodeError::InvalidCoordinate {
            z: coord.z,
            x: coord.x,
            y: coord.y,
        });
    }
    Ok(())
}

fn encode_feature(
    writer: &mut MvtWriter,
    writer_index: u64,
    feature_index: usize,
    feature_id: &str,
    properties: &[(String, TileScalar)],
    selected_properties: &[String],
    geometry: &geo_types::Geometry<f64>,
) -> Result<(), TileEncodeError> {
    writer.feature_begin(writer_index).map_err(encoding_error)?;
    writer.properties_begin().map_err(encoding_error)?;
    writer
        .property(0, "id", &ColumnValue::String(feature_id))
        .map_err(encoding_error)?;
    for (tag, name) in selected_properties.iter().enumerate() {
        let Some((_, value)) = properties.iter().find(|(property, _)| property == name) else {
            continue;
        };
        if let Some(value) = value.column_value(feature_index, name)? {
            writer
                .property(tag + 1, name, &value)
                .map_err(encoding_error)?;
        }
    }
    writer.properties_end().map_err(encoding_error)?;
    writer.geometry_begin().map_err(encoding_error)?;
    geometry
        .process_geom(&mut geometry::WindingNormalizer::new(writer))
        .map_err(encoding_error)?;
    writer.geometry_end().map_err(encoding_error)?;
    writer.feature_end(writer_index).map_err(encoding_error)
}

fn encoding_error(error: geozero::error::GeozeroError) -> TileEncodeError {
    TileEncodeError::Encoding(error.to_string())
}
