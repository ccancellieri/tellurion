//! Encodes an already-triangulated mesh straight to glTF binary (.glb) —
//! the `VolumeSource` capability's counterpart to [`crate::extrude`]'s
//! footprint+height fallback: a driver that already has real solid
//! geometry hands over finished positions/indices instead of a 2D footprint
//! for this crate to extrude and cap.

use crate::mesh::Mesh;

/// Encodes `positions`/`indices` (a flat triangle list, three indices per
/// triangle) as a glTF 2.0 binary (.glb) buffer, via the same encoder
/// [`crate::extrude_mvt_to_glb`] uses. Pure, infallible: an index that would
/// point outside `positions`, or a trailing one/two-index remainder past the
/// last full triangle, is simply skipped rather than panicking — geometry
/// crossing this boundary comes from a driver this crate does not control —
/// and an empty or fully-invalid input still yields a valid (near-empty)
/// glb, the same guarantee `extrude_mvt_to_glb` gives for a tile with no
/// extrudable geometry.
pub fn volume_mesh_to_glb(positions: &[[f64; 3]], indices: &[u32]) -> Vec<u8> {
    let mut mesh = Mesh::default();
    let vertices: Vec<u32> = positions
        .iter()
        .map(|p| mesh.push_vertex(p[0], p[1], p[2]))
        .collect();
    for tri in indices.chunks_exact(3) {
        let (Some(&a), Some(&b), Some(&c)) = (
            vertices.get(tri[0] as usize),
            vertices.get(tri[1] as usize),
            vertices.get(tri[2] as usize),
        ) else {
            continue;
        };
        mesh.push_triangle(a, b, c);
    }
    mesh.to_glb()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLB_MAGIC: [u8; 4] = *b"glTF";

    /// Parses just enough of a glb to hand back its JSON chunk, mirroring
    /// what `extrude.rs`'s own tests check.
    fn parse_glb(glb: &[u8]) -> serde_json::Value {
        assert_eq!(&glb[0..4], &GLB_MAGIC, "magic");
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(&glb[16..20], b"JSON", "first chunk type");
        serde_json::from_slice(&glb[20..20 + json_len]).expect("JSON chunk must parse")
    }

    #[test]
    fn a_single_triangle_round_trips_into_one_accessor_pair() {
        let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 3.0]];
        let indices = vec![0u32, 1, 2];

        let glb = volume_mesh_to_glb(&positions, &indices);
        let doc = parse_glb(&glb);

        assert_eq!(doc["asset"]["version"], "2.0");
        assert_eq!(doc["accessors"][0]["count"], 3, "3 positions");
        assert_eq!(doc["accessors"][1]["count"], 3, "3 indices, one triangle");
        assert_eq!(doc["accessors"][0]["max"][2], 3.0, "max Z carries through");
    }

    #[test]
    fn out_of_range_indices_are_skipped_not_panicking() {
        let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        // The second triangle references index 9, which doesn't exist.
        let indices = vec![0u32, 1, 2, 0, 1, 9];

        let glb = volume_mesh_to_glb(&positions, &indices);
        let doc = parse_glb(&glb);

        assert_eq!(
            doc["accessors"][1]["count"], 3,
            "only the valid triangle's 3 indices are emitted"
        );
    }

    #[test]
    fn a_trailing_partial_triangle_is_ignored() {
        let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let indices = vec![0u32, 1, 2, 0]; // one leftover index, not a full triangle

        let glb = volume_mesh_to_glb(&positions, &indices);
        let doc = parse_glb(&glb);

        assert_eq!(doc["accessors"][1]["count"], 3);
    }

    #[test]
    fn empty_input_still_yields_a_valid_glb() {
        let glb = volume_mesh_to_glb(&[], &[]);
        assert_eq!(&glb[0..4], &GLB_MAGIC);
    }
}
