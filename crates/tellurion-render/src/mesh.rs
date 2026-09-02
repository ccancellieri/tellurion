//! In-memory triangle mesh (positions only, `u32` indices) and its glTF 2.0
//! binary (.glb) container encoding. No glTF crate dependency: a single
//! mesh / single node / single material asset with a POSITION-only
//! primitive is small enough to hand-write — JSON chunk + BIN chunk per the
//! glb container format (khronos glTF 2.0, binary variant).

use serde_json::{json, Number, Value};

const COMPONENT_TYPE_FLOAT: u32 = 5126;
const COMPONENT_TYPE_UNSIGNED_INT: u32 = 5125;
const TARGET_ARRAY_BUFFER: u32 = 34962;
const TARGET_ELEMENT_ARRAY_BUFFER: u32 = 34963;
const TRIANGLES_MODE: u32 = 4;

/// A zero-area placeholder triangle so an empty mesh still produces
/// structurally valid (non-empty) accessors — the glTF schema requires
/// `mesh.primitives` to be non-empty and its accessors to have `count >= 1`.
const PLACEHOLDER_POSITIONS: [[f32; 3]; 3] = [[0.0, 0.0, 0.0]; 3];
const PLACEHOLDER_INDICES: [u32; 3] = [0, 1, 2];

#[derive(Default)]
pub(crate) struct Mesh {
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl Mesh {
    /// Appends a vertex, returning its index for use in `push_triangle`.
    /// Non-finite coordinates (should not occur given callers clamp their
    /// inputs, but never trust that alone) are sanitized to `0.0` rather
    /// than propagated into an unparseable glb.
    pub(crate) fn push_vertex(&mut self, x: f64, y: f64, z: f64) -> u32 {
        let idx = self.positions.len() as u32;
        let finite = |v: f64| if v.is_finite() { v as f32 } else { 0.0 };
        self.positions.push([finite(x), finite(y), finite(z)]);
        idx
    }

    pub(crate) fn push_triangle(&mut self, a: u32, b: u32, c: u32) {
        self.indices.push(a);
        self.indices.push(b);
        self.indices.push(c);
    }

    /// Encodes this mesh as a glTF 2.0 binary (.glb) buffer: one scene, one
    /// node, one mesh with a single POSITION-only triangle primitive, one
    /// flat-grey material. Always produces a structurally valid, parseable
    /// glb, even for an empty mesh.
    pub(crate) fn to_glb(&self) -> Vec<u8> {
        let (positions, indices): (&[[f32; 3]], &[u32]) =
            if self.positions.is_empty() || self.indices.is_empty() {
                (&PLACEHOLDER_POSITIONS, &PLACEHOLDER_INDICES)
            } else {
                (&self.positions, &self.indices)
            };

        let mut bin = Vec::with_capacity(positions.len() * 12 + indices.len() * 4);
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in positions {
            for (k, c) in p.iter().enumerate() {
                bin.extend_from_slice(&c.to_le_bytes());
                min[k] = min[k].min(*c);
                max[k] = max[k].max(*c);
            }
        }
        let indices_byte_offset = bin.len();
        for i in indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }

        let doc = json!({
            "asset": { "version": "2.0", "generator": "tellurion-render" },
            "scene": 0,
            "scenes": [{ "nodes": [0] }],
            "nodes": [{ "mesh": 0 }],
            "meshes": [{
                "primitives": [{
                    "attributes": { "POSITION": 0 },
                    "indices": 1,
                    "material": 0,
                    "mode": TRIANGLES_MODE,
                }],
            }],
            "materials": [{
                "pbrMetallicRoughness": {
                    "baseColorFactor": [0.6, 0.6, 0.6, 1.0],
                    "metallicFactor": 0.0,
                    "roughnessFactor": 1.0,
                },
                "doubleSided": true,
            }],
            "accessors": [
                {
                    "bufferView": 0,
                    "byteOffset": 0,
                    "componentType": COMPONENT_TYPE_FLOAT,
                    "count": positions.len(),
                    "type": "VEC3",
                    "min": vec3_json(min),
                    "max": vec3_json(max),
                },
                {
                    "bufferView": 1,
                    "byteOffset": 0,
                    "componentType": COMPONENT_TYPE_UNSIGNED_INT,
                    "count": indices.len(),
                    "type": "SCALAR",
                },
            ],
            "bufferViews": [
                {
                    "buffer": 0,
                    "byteOffset": 0,
                    "byteLength": positions.len() * 12,
                    "target": TARGET_ARRAY_BUFFER,
                },
                {
                    "buffer": 0,
                    "byteOffset": indices_byte_offset,
                    "byteLength": indices.len() * 4,
                    "target": TARGET_ELEMENT_ARRAY_BUFFER,
                },
            ],
            "buffers": [{ "byteLength": bin.len() }],
        });

        // `doc` is built entirely from finite floats (see `vec3_json`) and
        // plain integers, so serialization cannot fail in practice; the
        // fallback keeps this function infallible without an `unwrap`.
        let mut json_bytes = serde_json::to_vec(&doc).unwrap_or_else(|_| b"{}".to_vec());
        pad(&mut json_bytes, b' ');

        let mut bin_padded = bin;
        pad(&mut bin_padded, 0);

        let total_len = 12 + 8 + json_bytes.len() + 8 + bin_padded.len();
        let mut out = Vec::with_capacity(total_len);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total_len as u32).to_le_bytes());

        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_bytes);

        out.extend_from_slice(&(bin_padded.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin_padded);

        out
    }
}

/// Builds a `[min, min, min]`/`[max, max, max]`-style JSON array from
/// already-finite `f32`s without going through `serde_json`'s implicit
/// serialize-and-unwrap for interpolated values, so a NaN/Infinity slipping
/// through (should be impossible; `push_vertex` sanitizes) degrades to `0`
/// instead of panicking mid-encode.
fn vec3_json(v: [f32; 3]) -> Value {
    Value::Array(
        v.into_iter()
            .map(|c| {
                Number::from_f64(c as f64)
                    .map(Value::Number)
                    .unwrap_or_else(|| Value::Number(0.into()))
            })
            .collect(),
    )
}

fn pad(buf: &mut Vec<u8>, pad_byte: u8) {
    while !buf.len().is_multiple_of(4) {
        buf.push(pad_byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLB_MAGIC: [u8; 4] = *b"glTF";

    #[test]
    fn empty_mesh_still_encodes_a_valid_glb() {
        let glb = Mesh::default().to_glb();
        assert_eq!(&glb[0..4], &GLB_MAGIC);
        assert_eq!(u32::from_le_bytes(glb[4..8].try_into().unwrap()), 2);
    }

    #[test]
    fn non_finite_vertex_is_sanitized_not_propagated() {
        let mut mesh = Mesh::default();
        let a = mesh.push_vertex(f64::NAN, f64::INFINITY, f64::NEG_INFINITY);
        let b = mesh.push_vertex(1.0, 0.0, 0.0);
        let c = mesh.push_vertex(0.0, 1.0, 0.0);
        mesh.push_triangle(a, b, c);
        let glb = mesh.to_glb();
        assert_eq!(&glb[0..4], &GLB_MAGIC);
    }
}
