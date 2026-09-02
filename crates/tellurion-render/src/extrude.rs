//! MVT polygon footprints -> extruded glTF binary (.glb) meshes. Pure: no
//! I/O, no async. Reuses the crate's MVT decode path (geozero) the same way
//! [`crate::raster`] does; the new work is turning 2D footprints into 3D
//! prisms ([`crate::earclip`] for the caps) and hand-writing a glb container
//! ([`crate::mesh`]).
//!
//! ## Coordinate convention
//!
//! Output positions are right-handed, Z-up, local to one tile:
//! - X, Y: the feature's raw MVT tile-local coordinates divided by the
//!   layer's `extent`, i.e. the tile's own footprint spans roughly
//!   `[0, 1] x [0, 1]`. Geometry in the MVT buffer margin can fall slightly
//!   outside that range; unlike the PNG rasterizer this function has no
//!   fixed canvas to clip to, so it is left as-is.
//! - Z: `height * exaggeration`, in whatever numeric units the height
//!   property carries (conventionally meters) — **not** normalized against
//!   `extent`. This function has no notion of the tile's geographic bounds
//!   or zoom level, so it cannot convert meters into a fraction of the
//!   tile's ground footprint; a caller placing this content on a globe
//!   (e.g. a 3D Tiles tileset transform) applies that zoom-dependent scale
//!   downstream.
//!
//! Each footprint polygon becomes a closed, two-cap prism (top cap at
//! `height`, bottom cap at `min_height`, side walls along every ring
//! including hole boundaries) rather than an open-bottomed shell, so the
//! mesh is a valid manifold solid from any viewing angle.

use std::ops::Range;

use geozero::error::Result as GzResult;
use geozero::mvt::{Message, Tile};
use geozero::{ColumnValue, FeatureProcessor, GeomProcessor, GeozeroDatasource, PropertyProcessor};

use crate::earclip::{bridge_holes, ear_clip, orient};
use crate::error::{RenderError, Result};
use crate::mesh::Mesh;

/// MVT layer extent per the Mapbox Vector Tile spec when a layer omits one.
const DEFAULT_EXTENT: u32 = 4096;

/// Feature heights past this are treated as bad data (e.g. a unit mismatch
/// upstream) and clamped before `exaggeration` is applied, so one absurd
/// property value can't blow up a tile's bounding box. Public so a caller
/// computing a bounding volume from config (e.g. 3D Tiles `tileset.json`)
/// can derive the true worst-case output height (`MAX_HEIGHT_METERS *
/// exaggeration`) instead of guessing a second, potentially-drifting bound.
pub const MAX_HEIGHT_METERS: f64 = 10_000.0;

/// Extrusion inputs: which MVT feature properties carry footprint heights,
/// and the fallback/scale to apply when they're missing or unusable.
pub struct ExtrudeParams {
    pub height_property: String,
    pub min_height_property: Option<String>,
    pub default_height: f64,
    pub exaggeration: f64,
}

/// Decodes `mvt`, extrudes every polygon/multipolygon feature into a prism
/// using `params`, and returns a glTF 2.0 binary (.glb) buffer holding the
/// combined mesh. Never panics: an unreadable height falls back to
/// `default_height`, an absurd height is clamped, non-polygon geometry is
/// skipped, and a tile with no extrudable geometry still yields a valid
/// (near-empty) glb.
pub fn extrude_mvt_to_glb(mvt: &[u8], params: &ExtrudeParams) -> Result<Vec<u8>> {
    let mut decoded =
        Tile::decode(mvt).map_err(|source| RenderError::Decode(source.to_string()))?;
    let mut mesh = Mesh::default();
    for layer in &mut decoded.layers {
        let extent = f64::from(layer.extent.unwrap_or(DEFAULT_EXTENT).max(1));
        let mut collector = FeatureCollector::new(params, extent);
        layer
            .process(&mut collector)
            .map_err(|source| RenderError::Geometry(source.to_string()))?;
        for prism in &collector.prisms {
            add_prism(&mut mesh, prism);
        }
    }
    Ok(mesh.to_glb())
}

/// One footprint's rings, in raw (un-normalized) MVT tile coordinates, plus
/// its resolved top/bottom Z and the extent needed to normalize X/Y at mesh
/// time.
struct PrismInput {
    /// `rings[0]` is the exterior ring; `rings[1..]` are holes. Every ring
    /// is closed-duplicate-stripped (first point != last) and has >= 3
    /// points.
    rings: Vec<Vec<(f64, f64)>>,
    extent: f64,
    z_top: f64,
    z_bottom: f64,
}

fn add_prism(mesh: &mut Mesh, prism: &PrismInput) {
    // Orient rings so the standard earcut convention holds: exterior CCW,
    // holes CW. This simultaneously (a) makes the merged boundary's
    // triangulation come out with an upward-facing (CCW-from-above) winding
    // for the top cap, matching glTF's CCW front-face default, and (b)
    // makes the single wall-quad formula below produce outward-facing walls
    // for every ring, hole or exterior, with no special case.
    let mut rings = prism.rings.clone();
    for (i, ring) in rings.iter_mut().enumerate() {
        orient(ring, i == 0);
    }

    // Cap: bridge holes into the exterior ring, then ear-clip. The cap gets
    // its own top/bottom vertex copies (not shared with the wall vertices
    // below) — no NORMAL attribute is emitted, so nothing needs matching
    // per-face normals; separate index spaces are just the simplest correct
    // way to keep cap and wall triangles independent.
    let mut flat: Vec<(f64, f64)> = rings[0].clone();
    let mut hole_ranges: Vec<Range<usize>> = Vec::with_capacity(rings.len().saturating_sub(1));
    for hole in &rings[1..] {
        let start = flat.len();
        flat.extend_from_slice(hole);
        hole_ranges.push(start..flat.len());
    }
    let merged = bridge_holes(rings[0].len(), &hole_ranges, &flat);
    let cap_triangles = ear_clip(&merged, &flat);

    let unit = |(x, y): (f64, f64)| (x / prism.extent, y / prism.extent);

    let top_of: Vec<u32> = flat
        .iter()
        .map(|&p| {
            let (ux, uy) = unit(p);
            mesh.push_vertex(ux, uy, prism.z_top)
        })
        .collect();
    let bottom_of: Vec<u32> = flat
        .iter()
        .map(|&p| {
            let (ux, uy) = unit(p);
            mesh.push_vertex(ux, uy, prism.z_bottom)
        })
        .collect();
    for tri in &cap_triangles {
        mesh.push_triangle(top_of[tri[0]], top_of[tri[1]], top_of[tri[2]]);
        // Bottom cap faces -Z: reverse the winding the top cap used.
        mesh.push_triangle(bottom_of[tri[0]], bottom_of[tri[2]], bottom_of[tri[1]]);
    }

    // Walls: independent per-ring vertex copies (not the bridged/duplicated
    // cap list), so no spurious zero-width quad forms at a bridge seam.
    for ring in &rings {
        let n = ring.len();
        let top: Vec<u32> = ring
            .iter()
            .map(|&p| {
                let (ux, uy) = unit(p);
                mesh.push_vertex(ux, uy, prism.z_top)
            })
            .collect();
        let bottom: Vec<u32> = ring
            .iter()
            .map(|&p| {
                let (ux, uy) = unit(p);
                mesh.push_vertex(ux, uy, prism.z_bottom)
            })
            .collect();
        for i in 0..n {
            let j = (i + 1) % n;
            mesh.push_triangle(top[i], bottom[i], bottom[j]);
            mesh.push_triangle(top[i], bottom[j], top[j]);
        }
    }
}

fn finite_or(v: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

fn strip_closing_duplicate(ring: &[(f64, f64)]) -> Vec<(f64, f64)> {
    match (ring.first(), ring.last()) {
        (Some(&first), Some(&last)) if ring.len() > 1 && first == last => {
            ring[..ring.len() - 1].to_vec()
        }
        _ => ring.to_vec(),
    }
}

fn column_value_to_f64(value: &ColumnValue) -> Option<f64> {
    match value {
        ColumnValue::Byte(v) => Some(*v as f64),
        ColumnValue::UByte(v) => Some(*v as f64),
        ColumnValue::Short(v) => Some(*v as f64),
        ColumnValue::UShort(v) => Some(*v as f64),
        ColumnValue::Int(v) => Some(*v as f64),
        ColumnValue::UInt(v) => Some(*v as f64),
        ColumnValue::Long(v) => Some(*v as f64),
        ColumnValue::ULong(v) => Some(*v as f64),
        ColumnValue::Float(v) => Some(*v as f64),
        ColumnValue::Double(v) => Some(*v),
        ColumnValue::String(s) | ColumnValue::Json(s) | ColumnValue::DateTime(s) => {
            s.trim().parse::<f64>().ok()
        }
        ColumnValue::Bool(_) | ColumnValue::Binary(_) => None,
    }
}

/// Walks one MVT layer's features, capturing polygon/multipolygon rings and
/// the two height-carrying properties, and turns each finished feature into
/// zero or more [`PrismInput`]s. Non-polygon geometry (points, lines) simply
/// never populates `feature_polygons`, so it contributes nothing — not an
/// error.
struct FeatureCollector<'p> {
    params: &'p ExtrudeParams,
    extent: f64,
    prisms: Vec<PrismInput>,
    in_polygon: bool,
    in_ring: bool,
    ring: Vec<(f64, f64)>,
    polygon_rings: Vec<Vec<(f64, f64)>>,
    feature_polygons: Vec<Vec<Vec<(f64, f64)>>>,
    height_raw: Option<f64>,
    min_height_raw: Option<f64>,
}

impl<'p> FeatureCollector<'p> {
    fn new(params: &'p ExtrudeParams, extent: f64) -> Self {
        Self {
            params,
            extent,
            prisms: Vec::new(),
            in_polygon: false,
            in_ring: false,
            ring: Vec::new(),
            polygon_rings: Vec::new(),
            feature_polygons: Vec::new(),
            height_raw: None,
            min_height_raw: None,
        }
    }

    fn finish_feature(&mut self) {
        let height = self
            .height_raw
            .unwrap_or(self.params.default_height)
            .clamp(0.0, MAX_HEIGHT_METERS);
        let min_height = self
            .min_height_raw
            .unwrap_or(0.0)
            .clamp(0.0, MAX_HEIGHT_METERS)
            .min(height);
        let z_top = finite_or(height * self.params.exaggeration, 0.0);
        let z_bottom = finite_or(min_height * self.params.exaggeration, 0.0);

        for polygon in self.feature_polygons.drain(..) {
            let Some(raw_exterior) = polygon.first() else {
                continue;
            };
            let exterior = strip_closing_duplicate(raw_exterior);
            if exterior.len() < 3 {
                continue; // degenerate exterior sinks the whole polygon
            }
            let mut rings = vec![exterior];
            for hole in &polygon[1..] {
                let hole = strip_closing_duplicate(hole);
                if hole.len() >= 3 {
                    rings.push(hole); // else: degenerate hole, silently dropped
                }
            }
            self.prisms.push(PrismInput {
                rings,
                extent: self.extent,
                z_top,
                z_bottom,
            });
        }
    }
}

impl GeomProcessor for FeatureCollector<'_> {
    fn xy(&mut self, x: f64, y: f64, _idx: usize) -> GzResult<()> {
        if self.in_ring {
            self.ring.push((x, y));
        }
        Ok(())
    }

    fn linestring_begin(&mut self, tagged: bool, size: usize, _idx: usize) -> GzResult<()> {
        self.in_ring = self.in_polygon && !tagged;
        if self.in_ring {
            self.ring = Vec::with_capacity(size);
        }
        Ok(())
    }

    fn linestring_end(&mut self, _tagged: bool, _idx: usize) -> GzResult<()> {
        if self.in_ring {
            self.polygon_rings.push(std::mem::take(&mut self.ring));
        }
        self.in_ring = false;
        Ok(())
    }

    fn polygon_begin(&mut self, _tagged: bool, _size: usize, _idx: usize) -> GzResult<()> {
        self.in_polygon = true;
        self.polygon_rings = Vec::new();
        Ok(())
    }

    fn polygon_end(&mut self, _tagged: bool, _idx: usize) -> GzResult<()> {
        self.in_polygon = false;
        self.feature_polygons
            .push(std::mem::take(&mut self.polygon_rings));
        Ok(())
    }
}

impl PropertyProcessor for FeatureCollector<'_> {
    fn property(&mut self, _idx: usize, name: &str, value: &ColumnValue) -> GzResult<bool> {
        if name == self.params.height_property {
            self.height_raw = column_value_to_f64(value);
        } else if self.params.min_height_property.as_deref() == Some(name) {
            self.min_height_raw = column_value_to_f64(value);
        }
        Ok(false)
    }
}

impl FeatureProcessor for FeatureCollector<'_> {
    fn feature_begin(&mut self, _idx: u64) -> GzResult<()> {
        self.height_raw = None;
        self.min_height_raw = None;
        Ok(())
    }

    fn feature_end(&mut self, _idx: u64) -> GzResult<()> {
        self.finish_feature();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geozero::mvt::tile;
    use serde_json::Value;

    /// Encodes an MVT geometry command header: 3 low bits are the command id
    /// (1 = MoveTo, 2 = LineTo, 7 = ClosePath), the rest is the repeat count.
    fn cmd(id: u32, count: u32) -> u32 {
        id | (count << 3)
    }
    fn zz(n: i32) -> u32 {
        ((n << 1) ^ (n >> 31)) as u32
    }
    fn move_to(dx: i32, dy: i32) -> Vec<u32> {
        vec![cmd(1, 1), zz(dx), zz(dy)]
    }
    fn line_to(deltas: &[(i32, i32)]) -> Vec<u32> {
        let mut v = vec![cmd(2, deltas.len() as u32)];
        for (dx, dy) in deltas {
            v.push(zz(*dx));
            v.push(zz(*dy));
        }
        v
    }
    fn close_path() -> Vec<u32> {
        vec![cmd(7, 1)]
    }

    /// A 10x10 square (0,0)-(10,10) with a centered 4x4 hole (3,3)-(7,7), as
    /// a single MVT Polygon feature (two rings). The exterior ring is CCW
    /// and the hole CW (opposite windings, per the MVT spec) — geozero's
    /// decoder uses each ring's signed area to decide whether it starts a
    /// new polygon or is a hole of the previous one, so getting this
    /// backwards would silently decode as two separate one-ring polygons
    /// instead of one polygon with a hole.
    fn square_with_hole_geometry() -> Vec<u32> {
        [
            move_to(0, 0),
            line_to(&[(10, 0), (0, 10), (-10, 0)]),
            close_path(),
            move_to(3, -7), // cursor sits at (0,10) after ring 1; move to (3,3)
            line_to(&[(0, 4), (4, 0), (0, -4)]), // CW hole: (3,3)->(3,7)->(7,7)->(7,3)
            close_path(),
        ]
        .concat()
    }

    fn feature_with_props(geometry: Vec<u32>, props: &[(&str, tile::Value)]) -> tile::Layer {
        let mut keys = Vec::new();
        let mut values = Vec::new();
        let mut tags = Vec::new();
        for (k, v) in props {
            tags.push(keys.len() as u32);
            keys.push(k.to_string());
            tags.push(values.len() as u32);
            values.push(v.clone());
        }
        let mut feature = tile::Feature {
            geometry,
            tags,
            ..Default::default()
        };
        feature.set_type(tile::GeomType::Polygon);
        tile::Layer {
            version: 2,
            name: "buildings".to_string(),
            extent: Some(100),
            features: vec![feature],
            keys,
            values,
        }
    }

    fn double_value(v: f64) -> tile::Value {
        tile::Value {
            double_value: Some(v),
            ..Default::default()
        }
    }
    fn string_value(v: &str) -> tile::Value {
        tile::Value {
            string_value: Some(v.to_string()),
            ..Default::default()
        }
    }

    fn tile_bytes(layers: Vec<tile::Layer>) -> Vec<u8> {
        Tile { layers }.encode_to_vec()
    }

    fn default_params() -> ExtrudeParams {
        ExtrudeParams {
            height_property: "height".to_string(),
            min_height_property: Some("min_height".to_string()),
            default_height: 5.0,
            exaggeration: 1.0,
        }
    }

    /// Parses just enough of a glb to hand back its JSON chunk as a `Value`
    /// and the raw bytes, mirroring what a real consumer's parser would
    /// check (magic, version, chunk framing) before trusting the content.
    fn parse_glb(glb: &[u8]) -> Value {
        assert_eq!(&glb[0..4], b"glTF", "magic");
        assert_eq!(
            u32::from_le_bytes(glb[4..8].try_into().unwrap()),
            2,
            "version"
        );
        let total_len = u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize;
        assert_eq!(
            total_len,
            glb.len(),
            "declared length must match actual buffer length"
        );

        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(&glb[16..20], b"JSON", "first chunk type");
        let json_bytes = &glb[20..20 + json_len];
        let doc: Value = serde_json::from_slice(json_bytes).expect("JSON chunk must parse");

        let bin_header_start = 20 + json_len;
        let bin_len = u32::from_le_bytes(
            glb[bin_header_start..bin_header_start + 4]
                .try_into()
                .unwrap(),
        ) as usize;
        assert_eq!(
            &glb[bin_header_start + 4..bin_header_start + 8],
            b"BIN\0",
            "second chunk type"
        );
        assert_eq!(
            bin_header_start + 8 + bin_len,
            glb.len(),
            "BIN chunk must run to the end of the buffer"
        );

        doc
    }

    #[test]
    fn golden_glb_structure_parses_and_accessor_counts_match() {
        let layer = feature_with_props(
            square_with_hole_geometry(),
            &[("height", double_value(20.0))],
        );
        let glb = extrude_mvt_to_glb(&tile_bytes(vec![layer]), &default_params()).unwrap();

        let doc = parse_glb(&glb);
        assert_eq!(doc["asset"]["version"], "2.0");
        assert_eq!(
            doc["meshes"][0]["primitives"][0]["attributes"]["POSITION"],
            0
        );
        assert_eq!(doc["meshes"][0]["primitives"][0]["indices"], 1);

        let position_count = doc["accessors"][0]["count"].as_u64().unwrap();
        let index_count = doc["accessors"][1]["count"].as_u64().unwrap();

        // ext(4) + hole(4) rings, bridged into one 10-point ring: cap =
        // (10-2)*2 (top + mirrored bottom) = 16 triangles; walls =
        // (4+4)*2 = 16 triangles. 32 triangles total, 96 indices.
        // Vertices: cap uses its own copies of the 8 un-bridged ring points
        // (top+bottom = 16); walls use their own per-ring copies (ext 4 +
        // hole 4, top+bottom = 16). 32 vertices total.
        assert_eq!(index_count, 32 * 3);
        assert_eq!(position_count, 32);
    }

    #[test]
    fn height_from_property_scales_the_bounding_box_above_default() {
        let tall = feature_with_props(
            square_with_hole_geometry(),
            &[("height", double_value(50.0))],
        );
        let default = feature_with_props(square_with_hole_geometry(), &[]);

        let params = default_params();
        let tall_glb = extrude_mvt_to_glb(&tile_bytes(vec![tall]), &params).unwrap();
        let default_glb = extrude_mvt_to_glb(&tile_bytes(vec![default]), &params).unwrap();

        let tall_max_z = parse_glb(&tall_glb)["accessors"][0]["max"][2]
            .as_f64()
            .unwrap();
        let default_max_z = parse_glb(&default_glb)["accessors"][0]["max"][2]
            .as_f64()
            .unwrap();

        assert!((tall_max_z - 50.0).abs() < 1e-3);
        assert!(
            (default_max_z - 5.0).abs() < 1e-3,
            "falls back to default_height"
        );
        assert!(tall_max_z > default_max_z);
    }

    #[test]
    fn numeric_string_height_is_parsed() {
        let layer = feature_with_props(
            square_with_hole_geometry(),
            &[("height", string_value("12.5"))],
        );
        let glb = extrude_mvt_to_glb(&tile_bytes(vec![layer]), &default_params()).unwrap();
        let max_z = parse_glb(&glb)["accessors"][0]["max"][2].as_f64().unwrap();
        assert!((max_z - 12.5).abs() < 1e-3);
    }

    #[test]
    fn absurd_height_is_clamped_before_exaggeration() {
        let layer = feature_with_props(
            square_with_hole_geometry(),
            &[("height", double_value(1e12))],
        );
        let params = ExtrudeParams {
            exaggeration: 2.0,
            ..default_params()
        };
        let glb = extrude_mvt_to_glb(&tile_bytes(vec![layer]), &params).unwrap();
        let max_z = parse_glb(&glb)["accessors"][0]["max"][2].as_f64().unwrap();
        assert!(
            (max_z - 20_000.0).abs() < 1e-3,
            "clamped to MAX_HEIGHT_METERS * exaggeration, got {max_z}"
        );
    }

    #[test]
    fn non_ascii_height_property_falls_back_to_default() {
        let layer = feature_with_props(
            square_with_hole_geometry(),
            &[("height", string_value("höhe\u{0}"))],
        );
        let glb = extrude_mvt_to_glb(&tile_bytes(vec![layer]), &default_params()).unwrap();
        let max_z = parse_glb(&glb)["accessors"][0]["max"][2].as_f64().unwrap();
        assert!(
            (max_z - 5.0).abs() < 1e-3,
            "unparseable height falls back to default_height"
        );
    }

    #[test]
    fn malformed_mvt_is_a_decode_error() {
        assert!(matches!(
            extrude_mvt_to_glb(b"not a tile", &default_params()),
            Err(RenderError::Decode(_))
        ));
    }

    #[test]
    fn tile_with_no_polygons_still_yields_a_valid_glb() {
        let mut feature = tile::Feature {
            geometry: move_to(1, 1),
            ..Default::default()
        };
        feature.set_type(tile::GeomType::Point);
        let layer = tile::Layer {
            version: 2,
            name: "points".to_string(),
            extent: Some(100),
            features: vec![feature],
            ..Default::default()
        };
        let glb = extrude_mvt_to_glb(&tile_bytes(vec![layer]), &default_params()).unwrap();
        let doc = parse_glb(&glb);
        assert!(doc["accessors"][0]["count"].as_u64().unwrap() >= 1);
    }
}
