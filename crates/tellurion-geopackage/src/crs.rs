//! The embedded driver's deliberately narrow, dependency-free CRS transform.
//! GeoPackage writes and the 4326-backed tile path share these exact
//! spherical Web Mercator formulas so neither path can drift from the other.

use geo::MapCoordsInPlace;
use geozero::ToGeo;
use tellurion_core::RequestedCrs;

use crate::error::{GeopackageError, Result};

const WEB_MERCATOR_RADIUS: f64 = 6_378_137.0;
const WEB_MERCATOR_MAX_LAT: f64 = 85.051_128_78;

pub(crate) fn lonlat_to_web_mercator(lon: f64, lat: f64) -> (f64, f64) {
    let lat = lat.clamp(-WEB_MERCATOR_MAX_LAT, WEB_MERCATOR_MAX_LAT);
    let x = lon.to_radians() * WEB_MERCATOR_RADIUS;
    let y = (std::f64::consts::FRAC_PI_4 + lat.to_radians() / 2.0)
        .tan()
        .ln()
        * WEB_MERCATOR_RADIUS;
    (x, y)
}

pub(crate) fn web_mercator_to_lonlat(x: f64, y: f64) -> (f64, f64) {
    let lon = (x / WEB_MERCATOR_RADIUS).to_degrees();
    let lat =
        (2.0 * (y / WEB_MERCATOR_RADIUS).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
    (lon, lat)
}

/// A storage-CRS bbox (`[minx, miny, maxx, maxy]`) expressed in CRS84
/// (`#142`), or `None` when this driver cannot express `storage_srid` in
/// CRS84 at all.
///
/// The two answerable cases are exactly the two CRSs this whole driver knows
/// — [`ensure_write_srid`] and `driver::mvt_tile_inner` already refuse
/// everything else by name — and they are answered with the very formulas
/// above rather than a second copy of them:
///
/// - `4326`: identity. GeoPackage, like PostGIS, stores 4326 geometry
///   longitude-first whatever EPSG:4326's authority axis order says, so no
///   swap belongs here. This is the case
///   `tellurion_core::crs::crs84_literals_need_transform` calls "no
///   transform needed", and it is the one a CRS84 deployment travels — its
///   extents come out byte-for-byte the numbers it stored.
/// - `3857`: [`web_mercator_to_lonlat`] applied to the two corners. Web
///   Mercator is monotonic and axis-separable in both directions, so mapping
///   the corners maps the box exactly — this is the reprojected outline of
///   an axis-aligned box, not an approximation of one.
///
/// Anything else answers `None`, and the caller records
/// `ObligationExtent::Unrecorded` — an honest "this storage cannot say",
/// which the invalidation consumer degrades conservatively on, rather than a
/// projected box relabelled as degrees.
pub(crate) fn bbox_to_crs84(storage_srid: i32, bbox: [f64; 4]) -> Option<[f64; 4]> {
    let [minx, miny, maxx, maxy] = bbox;
    match storage_srid {
        4326 => Some(bbox),
        3857 => {
            let (minlon, minlat) = web_mercator_to_lonlat(minx, miny);
            let (maxlon, maxlat) = web_mercator_to_lonlat(maxx, maxy);
            Some([minlon, minlat, maxlon, maxlat])
        }
        _ => None,
    }
}

pub(crate) fn ensure_write_srid(
    collection: &str,
    storage_srid: Option<i32>,
    requested_crs: RequestedCrs,
    has_geometry: bool,
) -> Result<i32> {
    let srid = storage_srid.unwrap_or(4326);
    if !has_geometry || requested_crs == RequestedCrs::Storage || matches!(srid, 4326 | 3857) {
        Ok(srid)
    } else {
        Err(GeopackageError::UnsupportedWriteCrs {
            collection: collection.to_string(),
            found: storage_srid,
        })
    }
}

pub(crate) fn geometry_for_write(
    storage_srid: i32,
    geometry: &serde_json::Value,
    requested_crs: RequestedCrs,
) -> Result<geo_types::Geometry<f64>> {
    let text = geometry.to_string();
    let mut geometry: geo_types::Geometry<f64> = geozero::geojson::GeoJson(&text)
        .to_geo()
        .map_err(GeopackageError::Geozero)?;

    match (requested_crs, storage_srid) {
        (RequestedCrs::Omitted | RequestedCrs::Crs84, 4326) => {}
        (RequestedCrs::Omitted | RequestedCrs::Crs84, 3857) => {
            geometry.map_coords_in_place(|coord| {
                let (x, y) = lonlat_to_web_mercator(coord.x, coord.y);
                geo_types::Coord { x, y }
            });
        }
        (RequestedCrs::Storage, 4326) => geometry.map_coords_in_place(|coord| geo_types::Coord {
            x: coord.y,
            y: coord.x,
        }),
        (RequestedCrs::Storage, _) => {}
        (RequestedCrs::Omitted | RequestedCrs::Crs84, _) => {
            unreachable!("ensure_write_srid validates transformations into the storage SRID")
        }
    }
    Ok(geometry)
}
