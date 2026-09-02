//! GeoTIFF georeferencing: the `ModelPixelScaleTag`/`ModelTiepointTag` pixel
//! transform, and the CRS declared via `GeoKeyDirectoryTag` (GeoTIFF spec
//! 6.3). First-slice scope, deliberately narrow (`#37`): only an
//! in-directory (`TIFFTagLocation == 0`) short-valued `GTModelTypeGeoKey`/
//! `GeographicTypeGeoKey`/`ProjectedCSTypeGeoKey` is read — a key stored
//! indirectly via `GeoDoubleParamsTag`/`GeoAsciiParamsTag` is refused by
//! name rather than silently ignored. And only a *Geographic* model whose
//! `GeographicTypeGeoKey` is exactly EPSG:4326 (WGS84 lon/lat degrees) can
//! be related to CRS84 without a reprojection this slice never implements
//! (see `reader.rs`'s module doc for the decision) — every other CRS is
//! still reported honestly (its own EPSG code), just not usable for tile
//! serving here.

use crate::error::{CogError, Result};

const GT_MODEL_TYPE_GEO_KEY: u16 = 1024;
const GEOGRAPHIC_TYPE_GEO_KEY: u16 = 2048;
const PROJECTED_CS_TYPE_GEO_KEY: u16 = 3072;

const MODEL_TYPE_PROJECTED: u16 = 1;
const MODEL_TYPE_GEOGRAPHIC: u16 = 2;
const USER_DEFINED: u16 = 32767;

/// The pixel -> geographic-coordinate affine transform (axis-aligned only,
/// per `ModelPixelScaleTag`/`ModelTiepointTag` — `ModelTransformationTag`'s
/// general 4x4 matrix is out of scope, refused by [`CogError::Unsupported`]
/// wherever it is the only georeferencing tag present).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoTransform {
    /// Geographic X (longitude, degrees) at pixel `(0, 0)`.
    pub origin_x: f64,
    /// Geographic Y (latitude, degrees) at pixel `(0, 0)`.
    pub origin_y: f64,
    /// Degrees per pixel, X axis (always positive).
    pub pixel_scale_x: f64,
    /// Degrees per pixel, Y axis (always positive; a raster's Y pixel
    /// coordinate increases southward, so geographic Y at pixel row `py` is
    /// `origin_y - py * pixel_scale_y`).
    pub pixel_scale_y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrsInfo {
    pub epsg: Option<u32>,
    /// Whether this CRS is exactly EPSG:4326 (WGS84 geographic) — the only
    /// CRS this first slice can relate to CRS84 (identity transform, since
    /// GeoTIFF's raster convention already stores X=longitude, Y=latitude in
    /// that order, matching CRS84's own axis order).
    pub is_wgs84_geographic: bool,
}

/// Builds the pixel transform from `ModelPixelScaleTag`'s raw double values
/// (`[sx, sy, sz]`, `sz` unused here) and `ModelTiepointTag`'s (`[i, j, k,
/// x, y, z]` — only the first tiepoint is read; a GeoTIFF with more than one
/// tiepoint is a non-affine ("ground control point") georeferencing scheme
/// this slice does not implement, but the tags's own tag-presence check
/// below can't distinguish that from a single tiepoint, so it is accepted
/// as-is, matching the affine reading every COG generator in practice
/// produces).
pub fn parse_geo_transform(pixel_scale: &[f64], tiepoint: &[f64]) -> Result<GeoTransform> {
    if pixel_scale.len() < 2 {
        return Err(CogError::Unsupported(
            "ModelPixelScaleTag is missing or has fewer than 2 values".to_string(),
        ));
    }
    if tiepoint.len() < 6 {
        return Err(CogError::Unsupported(
            "ModelTiepointTag is missing or has fewer than 6 values".to_string(),
        ));
    }
    let (scale_x, scale_y) = (pixel_scale[0], pixel_scale[1]);
    if !(scale_x > 0.0 && scale_y > 0.0) {
        return Err(CogError::Unsupported(format!(
            "ModelPixelScaleTag has a non-positive scale ({scale_x}, {scale_y})"
        )));
    }
    let (tie_i, tie_j, tie_x, tie_y) = (tiepoint[0], tiepoint[1], tiepoint[3], tiepoint[4]);
    Ok(GeoTransform {
        origin_x: tie_x - tie_i * scale_x,
        origin_y: tie_y + tie_j * scale_y,
        pixel_scale_x: scale_x,
        pixel_scale_y: scale_y,
    })
}

/// Parses `GeoKeyDirectoryTag`'s raw SHORT array (widened to `u32` by the
/// `tiff` crate's generic tag accessor) into a [`CrsInfo`]. See this
/// module's own doc for exactly which GeoTIFF spec features are in scope.
pub fn parse_crs(directory: &[u32]) -> Result<CrsInfo> {
    if directory.len() < 4 {
        return Err(CogError::Unsupported(
            "GeoKeyDirectoryTag is missing or too short for its own header".to_string(),
        ));
    }
    let num_keys = directory[3] as usize;

    let mut model_type: Option<u16> = None;
    let mut geographic_epsg: Option<u16> = None;
    let mut projected_epsg: Option<u16> = None;

    for entry in 0..num_keys {
        let base = 4 + entry * 4;
        let Some(fields) = directory.get(base..base + 4) else {
            return Err(CogError::Unsupported(format!(
                "GeoKeyDirectoryTag declares {num_keys} keys but its array is too short to hold entry {entry}"
            )));
        };
        let key_id = fields[0] as u16;
        let tag_location = fields[1] as u16;
        let value_offset = fields[3] as u16;

        let target = match key_id {
            GT_MODEL_TYPE_GEO_KEY => Some(("GTModelTypeGeoKey", &mut model_type)),
            GEOGRAPHIC_TYPE_GEO_KEY => Some(("GeographicTypeGeoKey", &mut geographic_epsg)),
            PROJECTED_CS_TYPE_GEO_KEY => Some(("ProjectedCSTypeGeoKey", &mut projected_epsg)),
            _ => None,
        };
        let Some((name, slot)) = target else {
            continue;
        };
        if tag_location != 0 {
            return Err(CogError::Unsupported(format!(
                "{name} is stored indirectly via tag {tag_location} (GeoDoubleParamsTag/GeoAsciiParamsTag); only in-directory short values are supported"
            )));
        }
        *slot = Some(value_offset);
    }

    match model_type {
        Some(MODEL_TYPE_GEOGRAPHIC) => {
            let epsg = geographic_epsg.ok_or_else(|| {
                CogError::Unsupported(
                    "GTModelTypeGeoKey is Geographic but GeographicTypeGeoKey is absent"
                        .to_string(),
                )
            })?;
            if epsg == USER_DEFINED {
                return Err(CogError::Unsupported(
                    "GeographicTypeGeoKey is user-defined (32767); only a concrete EPSG code is supported"
                        .to_string(),
                ));
            }
            Ok(CrsInfo {
                epsg: Some(u32::from(epsg)),
                is_wgs84_geographic: epsg == 4326,
            })
        }
        Some(MODEL_TYPE_PROJECTED) => {
            let epsg = projected_epsg.ok_or_else(|| {
                CogError::Unsupported(
                    "GTModelTypeGeoKey is Projected but ProjectedCSTypeGeoKey is absent"
                        .to_string(),
                )
            })?;
            if epsg == USER_DEFINED {
                return Err(CogError::Unsupported(
                    "ProjectedCSTypeGeoKey is user-defined (32767); only a concrete EPSG code is supported"
                        .to_string(),
                ));
            }
            Ok(CrsInfo {
                epsg: Some(u32::from(epsg)),
                is_wgs84_geographic: false,
            })
        }
        Some(other) => Err(CogError::Unsupported(format!(
            "GTModelTypeGeoKey value {other} is neither Geographic (2) nor Projected (1)"
        ))),
        None => Err(CogError::Unsupported(
            "GeoKeyDirectoryTag does not declare GTModelTypeGeoKey".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo_key_directory(entries: &[(u16, u16, u16, u16)]) -> Vec<u32> {
        let mut directory = vec![1, 1, 0, entries.len() as u32];
        for (key_id, tag_location, count, value_offset) in entries {
            directory.push(u32::from(*key_id));
            directory.push(u32::from(*tag_location));
            directory.push(u32::from(*count));
            directory.push(u32::from(*value_offset));
        }
        directory
    }

    #[test]
    fn parses_a_wgs84_geographic_crs() {
        let directory = geo_key_directory(&[(1024, 0, 1, 2), (2048, 0, 1, 4326)]);
        let crs = parse_crs(&directory).unwrap();
        assert_eq!(crs.epsg, Some(4326));
        assert!(crs.is_wgs84_geographic);
    }

    #[test]
    fn a_non_4326_geographic_crs_is_reported_but_not_wgs84() {
        let directory = geo_key_directory(&[(1024, 0, 1, 2), (2048, 0, 1, 4269)]);
        let crs = parse_crs(&directory).unwrap();
        assert_eq!(crs.epsg, Some(4269));
        assert!(!crs.is_wgs84_geographic);
    }

    #[test]
    fn a_projected_crs_is_reported_but_not_wgs84_geographic() {
        let directory = geo_key_directory(&[(1024, 0, 1, 1), (3072, 0, 1, 32633)]);
        let crs = parse_crs(&directory).unwrap();
        assert_eq!(crs.epsg, Some(32633));
        assert!(!crs.is_wgs84_geographic);
    }

    #[test]
    fn missing_model_type_key_is_unsupported() {
        let directory = geo_key_directory(&[(2048, 0, 1, 4326)]);
        assert!(matches!(
            parse_crs(&directory),
            Err(CogError::Unsupported(_))
        ));
    }

    #[test]
    fn indirect_geographic_type_key_is_unsupported() {
        // TIFFTagLocation = 34737 (GeoAsciiParamsTag) instead of 0.
        let directory = geo_key_directory(&[(1024, 0, 1, 2), (2048, 34737, 1, 0)]);
        assert!(matches!(
            parse_crs(&directory),
            Err(CogError::Unsupported(_))
        ));
    }

    #[test]
    fn too_short_directory_is_unsupported_not_a_panic() {
        assert!(matches!(
            parse_crs(&[1, 1, 0]),
            Err(CogError::Unsupported(_))
        ));
    }

    #[test]
    fn declared_key_count_past_the_array_end_is_unsupported_not_a_panic() {
        let mut directory = geo_key_directory(&[(1024, 0, 1, 2)]);
        directory[3] = 5; // claims 5 keys, only 1 is actually present
        assert!(matches!(
            parse_crs(&directory),
            Err(CogError::Unsupported(_))
        ));
    }

    #[test]
    fn parses_the_pixel_transform_from_scale_and_tiepoint() {
        let transform =
            parse_geo_transform(&[0.01, 0.01, 0.0], &[0.0, 0.0, 0.0, -10.0, 50.0, 0.0]).unwrap();
        assert_eq!(transform.origin_x, -10.0);
        assert_eq!(transform.origin_y, 50.0);
        assert_eq!(transform.pixel_scale_x, 0.01);
        assert_eq!(transform.pixel_scale_y, 0.01);
    }

    #[test]
    fn a_non_origin_tiepoint_is_projected_back_to_pixel_zero_zero() {
        // Tiepoint anchors pixel (10, 20) to geo (0.0, 60.0); pixel (0,0)'s
        // geo coordinate is offset backward by that pixel amount.
        let transform =
            parse_geo_transform(&[0.1, 0.1, 0.0], &[10.0, 20.0, 0.0, 0.0, 60.0, 0.0]).unwrap();
        assert_eq!(transform.origin_x, -1.0);
        assert_eq!(transform.origin_y, 62.0);
    }

    #[test]
    fn zero_pixel_scale_is_unsupported() {
        assert!(matches!(
            parse_geo_transform(&[0.0, 0.01, 0.0], &[0.0, 0.0, 0.0, -10.0, 50.0, 0.0]),
            Err(CogError::Unsupported(_))
        ));
    }

    #[test]
    fn missing_tiepoint_is_unsupported_not_a_panic() {
        assert!(matches!(
            parse_geo_transform(&[0.01, 0.01, 0.0], &[]),
            Err(CogError::Unsupported(_))
        ));
    }
}
