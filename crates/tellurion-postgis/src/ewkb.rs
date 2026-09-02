//! Minimal, hand-rolled EWKB (PostGIS "extended WKB") reader for exactly the
//! 3D solid-geometry types `VolumeSource` needs (`#41`): PolyhedralSurface
//! Z, TIN Z, and MultiPolygon Z (the degenerate roof-print case), plus their
//! shared building block, a Polygon/Triangle Z's rings. The workspace has no
//! WKB decode path anywhere else — the MVT lane never touches raw geometry,
//! since `ST_AsMVTGeom` does that server-side — so this exists purely to
//! read what this driver's own `ST_AsEWKB` query produces for these types.
//! No external WKB crate: the byte layout below is small and fixed, and a
//! general-purpose reader would cover many geometry types this driver never
//! needs to understand.
//!
//! Pure: no I/O, no PostgreSQL types. `driver.rs` is the only caller, and
//! turns [`EwkbError`] into a collection-scoped [`crate::error::PostgisError`].
//!
//! ## Byte layout
//!
//! Verified against a live PostGIS 16 / PostGIS 3.4 instance via
//! `ST_AsEWKB` round-trips — not assumed — see this module's own tests for
//! the exact hex fixtures.
//!
//! - 1 byte: byte order (`0` = big endian, `1` = little endian).
//! - 4 bytes: geometry type — an OGC WKB base type code OR'd with PostGIS's
//!   extended-WKB flag bits: `0x8000_0000` has Z, `0x4000_0000` has M,
//!   `0x2000_0000` has SRID. Base codes this reader recognizes: `3` =
//!   Polygon, `6` = MultiPolygon, `15` = PolyhedralSurface, `16` = TIN,
//!   `17` = Triangle.
//! - 4 bytes, only when the SRID flag is set: SRID. Only ever present on
//!   the outermost geometry — a nested sub-geometry never repeats it.
//! - 4 bytes: number of rings (Polygon/Triangle) or number of member
//!   geometries (PolyhedralSurface/TIN/MultiPolygon).
//! - Each ring: 4 bytes (point count) then that many points, each point a
//!   tightly packed sequence of `f64`s — X, Y, then Z if the Z flag is set,
//!   then M if the M flag is set — in this geometry's own byte order. A
//!   ring is closed: its first point repeats as its last.
//! - Each PolyhedralSurface/MultiPolygon member is a nested Polygon (its
//!   own byte-order + type + rings, no SRID); each TIN member is a nested
//!   Triangle (byte-for-byte identical shape to a Polygon with exactly one
//!   ring of four points).

const FLAG_Z: u32 = 0x8000_0000;
const FLAG_M: u32 = 0x4000_0000;
const FLAG_SRID: u32 = 0x2000_0000;
const TYPE_MASK: u32 = 0x0000_00ff;

const WKB_POLYGON: u32 = 3;
const WKB_MULTIPOLYGON: u32 = 6;
const WKB_POLYHEDRALSURFACE: u32 = 15;
const WKB_TIN: u32 = 16;
const WKB_TRIANGLE: u32 = 17;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum EwkbError {
    #[error("ran out of bytes while decoding")]
    UnexpectedEof,
    #[error("unrecognized WKB geometry type code {0}")]
    UnsupportedGeometryType(u32),
}

pub(crate) type Result<T> = std::result::Result<T, EwkbError>;

/// One planar (or nominally planar — `tellurion_render::triangulate_face`
/// is what actually checks) face: `rings[0]` is the exterior boundary,
/// `rings[1..]` are holes, exactly as read off the wire — still closed
/// (first point repeats as the last) and otherwise unvalidated.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FaceZ {
    pub(crate) rings: Vec<Vec<[f64; 3]>>,
}

/// A decoded solid: every face from one PolyhedralSurface Z / TIN Z /
/// MultiPolygon Z row, flattened to one list. The caller never needs to
/// know which of the three source types produced them — every face goes
/// through the identical triangulate-and-lift pipeline regardless (see
/// `tellurion_render::triangulate_face`), which is exactly how the
/// MultiPolygon Z "degenerate roof-print" case falls out for free instead
/// of needing its own code path.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SolidZ {
    pub(crate) faces: Vec<FaceZ>,
}

impl SolidZ {
    /// Total point count across every ring of every face — the cheap,
    /// pre-triangulation proxy `driver.rs` checks against the per-zoom
    /// vertex budget (`#41`) before doing any triangulation work at all.
    pub(crate) fn total_points(&self) -> u64 {
        self.faces
            .iter()
            .flat_map(|face| face.rings.iter())
            .map(|ring| ring.len() as u64)
            .sum()
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(EwkbError::UnexpectedEof)?;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(EwkbError::UnexpectedEof)?;
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn read_u32(&mut self, big_endian: bool) -> Result<u32> {
        let raw: [u8; 4] = self.take(4)?.try_into().expect("take(4) yields 4 bytes");
        Ok(if big_endian {
            u32::from_be_bytes(raw)
        } else {
            u32::from_le_bytes(raw)
        })
    }

    fn read_f64(&mut self, big_endian: bool) -> Result<f64> {
        let raw: [u8; 8] = self.take(8)?.try_into().expect("take(8) yields 8 bytes");
        Ok(if big_endian {
            f64::from_be_bytes(raw)
        } else {
            f64::from_le_bytes(raw)
        })
    }
}

/// One geometry's own header: its endianness and type flags. A nested
/// sub-geometry reads a fresh header of its own — WKB technically allows a
/// different byte order per sub-geometry, even though PostGIS never
/// actually emits mixed-endianness output.
struct Header {
    big_endian: bool,
    base_type: u32,
    has_z: bool,
    has_m: bool,
    has_srid: bool,
}

fn read_header(r: &mut Reader) -> Result<Header> {
    let big_endian = r.read_u8()? == 0;
    let raw_type = r.read_u32(big_endian)?;
    Ok(Header {
        big_endian,
        base_type: raw_type & TYPE_MASK,
        has_z: raw_type & FLAG_Z != 0,
        has_m: raw_type & FLAG_M != 0,
        has_srid: raw_type & FLAG_SRID != 0,
    })
}

/// Reads one ring: a point count, then that many points. Only X/Y/Z are
/// kept — an M ordinate, when the type flags say one is present, is read
/// (to keep the cursor aligned with the rest of the buffer) and discarded.
fn read_ring(r: &mut Reader, header: &Header) -> Result<Vec<[f64; 3]>> {
    let count = r.read_u32(header.big_endian)? as usize;
    // Capped hint: a corrupt/huge count must not itself try to allocate
    // gigabytes before the very next `take` call fails on real EOF.
    let mut points = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        let x = r.read_f64(header.big_endian)?;
        let y = r.read_f64(header.big_endian)?;
        let z = if header.has_z {
            r.read_f64(header.big_endian)?
        } else {
            0.0
        };
        if header.has_m {
            r.read_f64(header.big_endian)?; // discarded, see module docs
        }
        points.push([x, y, z]);
    }
    Ok(points)
}

/// Reads a nested Polygon/Triangle sub-geometry: its own header, then a
/// ring count, then that many rings — the two source shapes are byte-for-
/// byte identical once the header's base type is known to be one or the
/// other. Never carries its own SRID (EWKB only ever writes SRID once, on
/// the outermost geometry).
fn read_polygon_like(r: &mut Reader) -> Result<FaceZ> {
    let header = read_header(r)?;
    if header.base_type != WKB_POLYGON && header.base_type != WKB_TRIANGLE {
        return Err(EwkbError::UnsupportedGeometryType(header.base_type));
    }
    if !header.has_z {
        return Err(EwkbError::UnsupportedGeometryType(header.base_type));
    }
    let num_rings = r.read_u32(header.big_endian)? as usize;
    let mut rings = Vec::with_capacity(num_rings.min(1 << 12));
    for _ in 0..num_rings {
        rings.push(read_ring(r, &header)?);
    }
    Ok(FaceZ { rings })
}

/// Decodes a top-level PolyhedralSurface Z / TIN Z / MultiPolygon Z EWKB
/// buffer into its flattened face list. Any other geometry type, a 2D (no
/// Z) buffer, or a buffer that runs out of bytes partway through, is an
/// [`EwkbError`] — this reader never panics or reads past `bytes`'s end.
pub(crate) fn decode_solid(bytes: &[u8]) -> Result<SolidZ> {
    let mut r = Reader::new(bytes);
    let header = read_header(&mut r)?;
    if !header.has_z {
        return Err(EwkbError::UnsupportedGeometryType(header.base_type));
    }
    if header.base_type != WKB_POLYHEDRALSURFACE
        && header.base_type != WKB_TIN
        && header.base_type != WKB_MULTIPOLYGON
    {
        return Err(EwkbError::UnsupportedGeometryType(header.base_type));
    }
    if header.has_srid {
        r.read_u32(header.big_endian)?; // SRID, unused -- see module docs
    }
    let count = r.read_u32(header.big_endian)? as usize;
    let mut faces = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        faces.push(read_polygon_like(&mut r)?);
    }
    Ok(SolidZ { faces })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    /// `ST_AsEWKB(ST_SetSRID('POLYHEDRALSURFACE Z(...)'::geometry, 4326))`
    /// for an axis-aligned unit cube, six quad faces — generated against a
    /// live PostGIS 16 instance, not hand-assembled.
    const CUBE_HEX: &str = "010f0000a0e6100000060000000103000080010000000500000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f03f0000000000000000000000000000f03f000000000000f03f0000000000000000000000000000f03f0000000000000000000000000000000000000000000000000000000000000000010300008001000000050000000000000000000000000000000000000000000000000000000000000000000000000000000000f03f0000000000000000000000000000f03f000000000000f03f0000000000000000000000000000f03f0000000000000000000000000000000000000000000000000000000000000000000000000000000001030000800100000005000000000000000000000000000000000000000000000000000000000000000000f03f00000000000000000000000000000000000000000000f03f0000000000000000000000000000f03f00000000000000000000000000000000000000000000f03f00000000000000000000000000000000000000000000000001030000800100000005000000000000000000f03f000000000000f03f0000000000000000000000000000f03f000000000000f03f000000000000f03f000000000000f03f0000000000000000000000000000f03f000000000000f03f00000000000000000000000000000000000000000000f03f000000000000f03f0000000000000000010300008001000000050000000000000000000000000000000000f03f00000000000000000000000000000000000000000000f03f000000000000f03f000000000000f03f000000000000f03f000000000000f03f000000000000f03f000000000000f03f00000000000000000000000000000000000000000000f03f00000000000000000103000080010000000500000000000000000000000000000000000000000000000000f03f000000000000f03f0000000000000000000000000000f03f000000000000f03f000000000000f03f000000000000f03f0000000000000000000000000000f03f000000000000f03f00000000000000000000000000000000000000000000f03f";

    /// `ST_AsEWKB(ST_SetSRID('TIN Z((...),(...))'::geometry, 4326))` for two
    /// right triangles sharing a diagonal — one quad split into a TIN.
    const TIN_HEX: &str = "01100000a0e61000000200000001110000800100000004000000000000000000000000000000000000000000000000000000000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f000000000000000000000000000000000000000000000000000000000000000001110000800100000004000000000000000000f03f00000000000000000000000000000000000000000000f03f000000000000f03f00000000000000000000000000000000000000000000f03f0000000000000000000000000000f03f00000000000000000000000000000000";

    /// `ST_AsEWKB(ST_SetSRID('MULTIPOLYGON Z(((...),(...)))'::geometry, 4326))`
    /// — one square face at Z=5 with a smaller square hole, the degenerate
    /// roof-print case (`#41`).
    const MULTIPOLYGON_WITH_HOLE_HEX: &str = "01060000a0e61000000100000001030000800200000005000000000000000000000000000000000000000000000000001440000000000000f03f00000000000000000000000000001440000000000000f03f000000000000f03f00000000000014400000000000000000000000000000f03f000000000000144000000000000000000000000000000000000000000000144005000000000000000000d03f000000000000d03f0000000000001440000000000000e83f000000000000d03f0000000000001440000000000000e83f000000000000e83f0000000000001440000000000000d03f000000000000e83f0000000000001440000000000000d03f000000000000d03f0000000000001440";

    /// `ST_AsEWKB(ST_SetSRID('POLYHEDRALSURFACE Z EMPTY'::geometry, 4326))`.
    const EMPTY_POLYHEDRAL_HEX: &str = "010f0000a0e610000000000000";

    /// `ST_AsEWKB('POLYHEDRALSURFACE Z(((0 0 0, 1 0 0, 0 1 0, 0 0 0)))'::geometry)`
    /// — no `ST_SetSRID`, so the SRID flag/field is absent entirely.
    const NO_SRID_HEX: &str = "010f0000800100000001030000800100000004000000000000000000000000000000000000000000000000000000000000000000f03f000000000000000000000000000000000000000000000000000000000000f03f0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn decodes_a_polyhedral_surface_cube_into_six_five_point_faces() {
        let solid = decode_solid(&hex_to_bytes(CUBE_HEX)).expect("valid EWKB");
        assert_eq!(solid.faces.len(), 6, "a cube has six faces");
        for face in &solid.faces {
            assert_eq!(face.rings.len(), 1, "each cube face is a single ring");
            assert_eq!(
                face.rings[0].len(),
                5,
                "a closed quad ring repeats its first point as its last"
            );
        }
        // First face: the Z=0 bottom, closed back to (0,0,0).
        assert_eq!(solid.faces[0].rings[0][0], [0.0, 0.0, 0.0]);
        assert_eq!(solid.faces[0].rings[0][4], [0.0, 0.0, 0.0]);
        assert_eq!(solid.total_points(), 30, "6 faces * 5 points each");
    }

    #[test]
    fn decodes_a_tin_into_two_four_point_triangle_faces() {
        let solid = decode_solid(&hex_to_bytes(TIN_HEX)).expect("valid EWKB");
        assert_eq!(solid.faces.len(), 2);
        for face in &solid.faces {
            assert_eq!(face.rings.len(), 1);
            assert_eq!(
                face.rings[0].len(),
                4,
                "a closed triangle ring has 4 points (3 + repeated first)"
            );
        }
        assert_eq!(solid.faces[0].rings[0][0], [0.0, 0.0, 0.0]);
        assert_eq!(solid.faces[0].rings[0][1], [1.0, 0.0, 0.0]);
        assert_eq!(solid.faces[0].rings[0][2], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn decodes_a_multipolygon_face_with_a_hole() {
        let solid = decode_solid(&hex_to_bytes(MULTIPOLYGON_WITH_HOLE_HEX)).expect("valid EWKB");
        assert_eq!(solid.faces.len(), 1, "one polygon member");
        let face = &solid.faces[0];
        assert_eq!(face.rings.len(), 2, "exterior plus one hole");
        assert_eq!(face.rings[0].len(), 5, "exterior square, closed");
        assert_eq!(face.rings[1].len(), 5, "hole square, closed");
        for p in &face.rings[0] {
            assert_eq!(p[2], 5.0, "every exterior point sits at Z=5");
        }
    }

    #[test]
    fn decodes_an_empty_polyhedral_surface_as_zero_faces() {
        let solid = decode_solid(&hex_to_bytes(EMPTY_POLYHEDRAL_HEX)).expect("valid EWKB");
        assert!(solid.faces.is_empty());
        assert_eq!(solid.total_points(), 0);
    }

    #[test]
    fn decodes_a_geometry_with_no_srid_flag_set() {
        let solid = decode_solid(&hex_to_bytes(NO_SRID_HEX)).expect("valid EWKB");
        assert_eq!(solid.faces.len(), 1);
        assert_eq!(solid.faces[0].rings[0].len(), 4);
    }

    #[test]
    fn rejects_a_2d_geometry_type_code() {
        // A bare `POINT` header (no Z flag, no SRID): byte order LE, type 1.
        let bytes = hex_to_bytes("0101000000000000000000f03f0000000000000040");
        assert_eq!(
            decode_solid(&bytes),
            Err(EwkbError::UnsupportedGeometryType(1))
        );
    }

    #[test]
    fn rejects_truncated_bytes_without_panicking() {
        let full = hex_to_bytes(CUBE_HEX);
        for cut in [0, 1, 5, 9, 10, 20, full.len() - 1] {
            let truncated = &full[..cut];
            assert_eq!(decode_solid(truncated), Err(EwkbError::UnexpectedEof));
        }
    }

    #[test]
    fn rejects_an_absurd_ring_count_without_allocating_or_panicking() {
        // Valid header (PolyhedralSurface Z, SRID present) then a member
        // count claiming ~4 billion faces, with no bytes to back it up.
        let mut bytes = hex_to_bytes("010f0000a0e6100000");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode_solid(&bytes), Err(EwkbError::UnexpectedEof));
    }
}
