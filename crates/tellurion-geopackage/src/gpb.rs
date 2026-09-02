//! GeoPackage Binary (GPB) geometry BLOB codec — the small header the
//! GeoPackage spec (OGC 12-128r19 §2.1.3, "GeoPackage SQLite Extensions")
//! wraps every geometry column value in, ahead of a plain ISO WKB body:
//!
//! ```text
//! byte 0-1: magic "GP"
//! byte 2:   version (0)
//! byte 3:   flags — bit 0 byte order (0=big,1=little); bits 1-3 envelope
//!           contents indicator code (0=none, 1=[minx,maxx,miny,maxy],
//!           2/3 add z or m, 4 adds both); bit 4 empty-geometry flag;
//!           bits 5-7 reserved, must be 0
//! byte 4-7: srs_id (int32, byte order per flags bit 0)
//! ...:      envelope (0/32/48/48/64 bytes of float64, per indicator code)
//! ...:      WKB body (its own embedded byte-order byte, independent of
//!           the header's)
//! ```
//!
//! This driver always *writes* envelope indicator code 1 (a plain 2D
//! `[minx,maxx,miny,maxy]` envelope, 32 bytes) for a non-empty geometry, or
//! no envelope with the empty-geometry flag set for one that decodes to zero
//! coordinates — never Z/M envelope variants, since v0.1 features are 2D.
//! Geozero's own WKB reader/writer (`with-wkb`) handles the body; this
//! module only ever touches the small fixed header around it, and geozero
//! offers no ready-made GeoPackage header codec at the version this
//! workspace pins (see the crate's own `Cargo.toml` doc comment) — hence the
//! small hand-rolled encoder/decoder here, exactly as the driver contract
//! doc anticipates for this case.

use geozero::GeozeroGeometry;

use crate::error::{GeopackageError, Result};

const MAGIC: [u8; 2] = *b"GP";
const HEADER_FIXED_LEN: usize = 8;
const ENVELOPE_XY_LEN: usize = 32;

/// A decoded GPB blob's header fields plus a borrowed view of its WKB body —
/// no allocation beyond what the caller already owns.
pub(crate) struct Decoded<'a> {
    /// Read and exercised by this module's own round-trip tests; no
    /// production call site needs a per-row SRID cross-check yet (the
    /// collection-level SRID already gates the tiles lane in `driver.rs`).
    #[allow(dead_code)]
    pub(crate) srs_id: i32,
    /// `[minx, miny, maxx, maxy]` — `None` for an empty geometry (this
    /// driver never writes any other envelope-less, non-empty combination).
    pub(crate) envelope: Option<[f64; 4]>,
    pub(crate) is_empty: bool,
    pub(crate) wkb: &'a [u8],
}

fn read_i32(bytes: &[u8], little_endian: bool) -> i32 {
    let arr: [u8; 4] = bytes.try_into().expect("caller sliced exactly 4 bytes");
    if little_endian {
        i32::from_le_bytes(arr)
    } else {
        i32::from_be_bytes(arr)
    }
}

fn read_f64(bytes: &[u8], little_endian: bool) -> f64 {
    let arr: [u8; 8] = bytes.try_into().expect("caller sliced exactly 8 bytes");
    if little_endian {
        f64::from_le_bytes(arr)
    } else {
        f64::from_be_bytes(arr)
    }
}

/// Byte length of the envelope section for indicator `code` (0..=4), or
/// `None` for the reserved codes 5-7 the spec forbids.
fn envelope_len(code: u8) -> Option<usize> {
    match code {
        0 => Some(0),
        1 => Some(32),
        2 | 3 => Some(48),
        4 => Some(64),
        _ => None,
    }
}

/// Parses a GPB blob's header and returns its `srs_id`, 2D envelope (when
/// present — every envelope-indicator code starts with the same
/// `minx,maxx,miny,maxy` quartet regardless of whether a z/m extension
/// follows, so this always reads just those four regardless of `code`), the
/// empty-geometry flag, and the remaining WKB body.
pub(crate) fn decode(blob: &[u8]) -> Result<Decoded<'_>> {
    if blob.len() < HEADER_FIXED_LEN || blob[0..2] != MAGIC {
        return Err(GeopackageError::MalformedGeometry(
            "missing 'GP' magic bytes".to_string(),
        ));
    }
    let flags = blob[3];
    let little_endian = flags & 0x1 == 1;
    let code = (flags >> 1) & 0x7;
    let is_empty = (flags >> 4) & 0x1 == 1;
    let srs_id = read_i32(&blob[4..8], little_endian);

    let env_len = envelope_len(code).ok_or_else(|| {
        GeopackageError::MalformedGeometry(format!("reserved envelope indicator code {code}"))
    })?;
    if blob.len() < HEADER_FIXED_LEN + env_len {
        return Err(GeopackageError::MalformedGeometry(
            "blob shorter than its declared envelope".to_string(),
        ));
    }

    let envelope = if env_len == 0 {
        None
    } else {
        let e = &blob[HEADER_FIXED_LEN..HEADER_FIXED_LEN + ENVELOPE_XY_LEN.min(env_len)];
        let minx = read_f64(&e[0..8], little_endian);
        let maxx = read_f64(&e[8..16], little_endian);
        let miny = read_f64(&e[16..24], little_endian);
        let maxy = read_f64(&e[24..32], little_endian);
        Some([minx, miny, maxx, maxy])
    };

    Ok(Decoded {
        srs_id,
        envelope,
        is_empty,
        wkb: &blob[HEADER_FIXED_LEN + env_len..],
    })
}

/// Tracks the enclosing 2D bbox of every coordinate a geometry walk visits —
/// this module's own small copy of the same fold `tellurion-geoparquet`'s
/// `BboxCollector` implements (a shared driver-utility crate for one
/// eight-line struct would cost more than it saves; see that crate's own
/// copy for the identical rationale). `None` after a full walk means the
/// geometry was empty (zero coordinates), which [`encode`] turns into the
/// GPB empty-geometry flag rather than a zero-sized envelope.
#[derive(Default)]
struct BboxCollector {
    bbox: Option<[f64; 4]>,
}

impl BboxCollector {
    fn accumulate(&mut self, x: f64, y: f64) {
        self.bbox = Some(match self.bbox {
            Some([minx, miny, maxx, maxy]) => [minx.min(x), miny.min(y), maxx.max(x), maxy.max(y)],
            None => [x, y, x, y],
        });
    }
}

impl geozero::GeomProcessor for BboxCollector {
    fn xy(&mut self, x: f64, y: f64, _idx: usize) -> geozero::error::Result<()> {
        self.accumulate(x, y);
        Ok(())
    }

    fn empty_point(&mut self, _idx: usize) -> geozero::error::Result<()> {
        Ok(())
    }
}

/// Encodes `wkb` (already-encoded ISO WKB, little-endian) into a full GPB
/// blob for `srs_id`, computing its 2D envelope by re-walking the same
/// coordinates via [`BboxCollector`]. `wkb` must decode cleanly — this is
/// always called immediately after this driver's own WKB encode, never on
/// caller-supplied bytes.
fn encode(srs_id: i32, wkb: &[u8], bbox: Option<[f64; 4]>) -> Vec<u8> {
    let mut buf =
        Vec::with_capacity(HEADER_FIXED_LEN + bbox.map_or(0, |_| ENVELOPE_XY_LEN) + wkb.len());
    buf.extend_from_slice(&MAGIC);
    buf.push(0); // version
    let code: u8 = if bbox.is_some() { 1 } else { 0 };
    // Byte order bit 1 (little-endian); empty flag (bit 4) set exactly when
    // there was no envelope to compute, i.e. an empty geometry.
    let empty_bit: u8 = if bbox.is_none() { 1 } else { 0 };
    let flags: u8 = 0x1 | (code << 1) | (empty_bit << 4);
    buf.push(flags);
    buf.extend_from_slice(&srs_id.to_le_bytes());
    if let Some([minx, miny, maxx, maxy]) = bbox {
        buf.extend_from_slice(&minx.to_le_bytes());
        buf.extend_from_slice(&maxx.to_le_bytes());
        buf.extend_from_slice(&miny.to_le_bytes());
        buf.extend_from_slice(&maxy.to_le_bytes());
    }
    buf.extend_from_slice(wkb);
    buf
}

/// Encodes a GeoJSON geometry object (never a null/absent geometry — the
/// caller binds a plain SQL `NULL` for that case instead, see `write_sql.rs`)
/// into a full GPB blob for `srs_id`: the complete geometry is first
/// normalized from `requested_crs` into the supported storage CRS, then
/// encoded as ISO WKB and wrapped with a matching computed envelope.
pub(crate) fn encode_from_geojson_geometry(
    srs_id: i32,
    geometry: &serde_json::Value,
    requested_crs: tellurion_core::RequestedCrs,
) -> Result<Vec<u8>> {
    let geometry = crate::crs::geometry_for_write(srs_id, geometry, requested_crs)?;

    let mut wkb = Vec::new();
    {
        let mut writer = geozero::wkb::WkbWriter::new(&mut wkb, geozero::wkb::WkbDialect::Wkb);
        geometry.process_geom(&mut writer)?;
    }

    let mut collector = BboxCollector::default();
    geometry.process_geom(&mut collector)?;

    Ok(encode(srs_id, &wkb, collector.bbox))
}

/// Computes a 2D envelope directly from a WKB body by walking its
/// coordinates — the fallback [`functions::envelope_of`] uses for a GPB blob
/// that (unlike anything this driver itself ever writes) carries no header
/// envelope of its own. `Ok(None)` for an empty geometry.
pub(crate) fn envelope_from_wkb(wkb: &[u8]) -> Result<Option<[f64; 4]>> {
    let mut collector = BboxCollector::default();
    geozero::wkb::Wkb(wkb).process_geom(&mut collector)?;
    Ok(collector.bbox)
}

/// A whole GPB blob's 2D envelope, in the blob's own storage CRS —
/// preferring the envelope this driver's own
/// [`encode_from_geojson_geometry`] always stores in the header, and
/// decoding the WKB body only for a blob that carries none (one written by
/// some other tool). `Ok(None)` for a geometry that decodes empty.
///
/// Factored out of `functions::envelope_of` (which is exactly this over a
/// SQL argument) so `#141`'s write-path extent capture reads a stored
/// geometry's envelope through the very same rule the `ST_MinX`/`ST_MaxX`
/// SQL functions and the R*Tree triggers already use — one envelope rule for
/// this driver, not two that could drift.
pub(crate) fn envelope_of_blob(blob: &[u8]) -> Result<Option<[f64; 4]>> {
    let decoded = decode(blob)?;
    if decoded.is_empty {
        return Ok(None);
    }
    match decoded.envelope {
        Some(envelope) => Ok(Some(envelope)),
        None => envelope_from_wkb(decoded.wkb),
    }
}

/// Decodes a GPB blob straight to a bare GeoJSON geometry object, via the
/// same `geozero::wkb::Wkb` reader `tellurion-geoparquet`/`tellurion-iceberg`
/// already use for plain WKB. `Ok(serde_json::Value::Null)` for an empty
/// geometry — geozero's GeoJSON writer has no "empty geometry" object shape
/// of its own to emit for one, and a GeoJSON `null` geometry is exactly what
/// RFC 7946 uses for "no geometry" already.
pub(crate) fn geometry_to_geojson(blob: &[u8]) -> Result<serde_json::Value> {
    let decoded = decode(blob)?;
    if decoded.is_empty {
        return Ok(serde_json::Value::Null);
    }
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut writer = geozero::geojson::GeoJsonWriter::new(&mut buf);
        geozero::wkb::Wkb(decoded.wkb).process_geom(&mut writer)?;
    }
    Ok(serde_json::from_slice(&buf)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point_wkb(x: f64, y: f64) -> Vec<u8> {
        let text = format!(r#"{{"type":"Point","coordinates":[{x},{y}]}}"#);
        let mut buf = Vec::new();
        let mut writer = geozero::wkb::WkbWriter::new(&mut buf, geozero::wkb::WkbDialect::Wkb);
        geozero::geojson::GeoJson(&text)
            .process_geom(&mut writer)
            .unwrap();
        buf
    }

    #[test]
    fn round_trips_a_point_through_encode_and_decode() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [1.5, 2.5]});
        let blob =
            encode_from_geojson_geometry(4326, &geometry, tellurion_core::RequestedCrs::Omitted)
                .unwrap();
        assert_eq!(&blob[0..2], b"GP");

        let decoded = decode(&blob).unwrap();
        assert_eq!(decoded.srs_id, 4326);
        assert!(!decoded.is_empty);
        assert_eq!(decoded.envelope, Some([1.5, 2.5, 1.5, 2.5]));

        let round_tripped = geometry_to_geojson(&blob).unwrap();
        assert_eq!(round_tripped, geometry);
    }

    #[test]
    fn envelope_covers_a_multi_point_geometrys_full_extent() {
        let geometry = serde_json::json!({
            "type": "LineString",
            "coordinates": [[-4.0, 46.0], [4.0, 54.0]]
        });
        let blob =
            encode_from_geojson_geometry(4326, &geometry, tellurion_core::RequestedCrs::Omitted)
                .unwrap();
        let decoded = decode(&blob).unwrap();
        assert_eq!(decoded.envelope, Some([-4.0, 46.0, 4.0, 54.0]));
    }

    #[test]
    fn crs84_to_3857_transforms_every_vertex_before_wkb_and_envelope_encoding() {
        let geometry = serde_json::json!({
            "type": "LineString",
            "coordinates": [[0.0, 0.0], [12.0, 41.0]]
        });
        let blob =
            encode_from_geojson_geometry(3857, &geometry, tellurion_core::RequestedCrs::Crs84)
                .unwrap();
        let decoded = decode(&blob).unwrap();
        let [minx, miny, maxx, maxy] = decoded.envelope.unwrap();
        assert!(minx.abs() < 1.0e-6);
        assert!(miny.abs() < 1.0e-6);
        assert!((maxx - 1_335_833.889_519_282_8).abs() < 1.0e-6);
        assert!((maxy - 5_012_341.663_847_514).abs() < 1.0e-6);

        let round_tripped = geometry_to_geojson(&blob).unwrap();
        let coordinates = round_tripped["coordinates"].as_array().unwrap();
        assert!((coordinates[1][0].as_f64().unwrap() - maxx).abs() < 1.0e-6);
        assert!((coordinates[1][1].as_f64().unwrap() - maxy).abs() < 1.0e-6);
    }

    #[test]
    fn authority_ordered_4326_storage_input_is_normalized_to_internal_xy_order() {
        let geometry = serde_json::json!({"type": "Point", "coordinates": [41.0, 12.0]});
        let blob =
            encode_from_geojson_geometry(4326, &geometry, tellurion_core::RequestedCrs::Storage)
                .unwrap();
        assert_eq!(
            geometry_to_geojson(&blob).unwrap(),
            serde_json::json!({"type": "Point", "coordinates": [12, 41]})
        );
        assert_eq!(
            decode(&blob).unwrap().envelope,
            Some([12.0, 41.0, 12.0, 41.0])
        );
    }

    #[test]
    fn decode_rejects_a_blob_with_no_magic_bytes() {
        assert!(decode(&[0u8; 16]).is_err());
    }

    #[test]
    fn decode_rejects_a_truncated_envelope() {
        let mut blob = vec![b'G', b'P', 0, 0x1 | (1 << 1)];
        blob.extend_from_slice(&4326i32.to_le_bytes());
        // Declares envelope code 1 (32 bytes) but supplies none.
        assert!(decode(&blob).is_err());
    }

    #[test]
    fn geometry_to_geojson_matches_a_raw_wkb_round_trip() {
        let wkb = point_wkb(10.0, 20.0);
        let blob = encode(4326, &wkb, Some([10.0, 20.0, 10.0, 20.0]));
        let geojson = geometry_to_geojson(&blob).unwrap();
        // geozero's `GeoJsonWriter` renders a whole-valued f64 without a
        // decimal point (`10`, not `10.0`), which `serde_json` then parses
        // back as an integer-tagged `Number` — an integer literal here
        // (rather than `10.0`) matches that representation exactly, the same
        // way `round_trips_a_point_through_encode_and_decode` above avoids
        // the question entirely by using non-integral coordinates.
        assert_eq!(
            geojson,
            serde_json::json!({"type": "Point", "coordinates": [10, 20]})
        );
    }
}
