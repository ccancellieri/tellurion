//! Pure-Rust CRS normalization and coordinate transformations for OGC API - EDR.

use crate::problem::ApiError;

pub const CRS84_URI: &str = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";
pub const EPSG_4326_URI: &str = "http://www.opengis.net/def/crs/EPSG/0/4326";
pub const EPSG_3857_URI: &str = "http://www.opengis.net/def/crs/EPSG/0/3857";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedCrs {
    Crs84,
    Epsg4326,
    Epsg3857,
}

impl SupportedCrs {
    pub fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(Self::Crs84);
        };
        let norm = s.to_ascii_uppercase();
        if norm == "CRS84"
            || norm == "OGC:CRS84"
            || norm == "http://www.opengis.net/def/crs/OGC/1.3/CRS84".to_ascii_uppercase()
            || norm == "https://www.opengis.net/def/crs/OGC/1.3/CRS84".to_ascii_uppercase()
        {
            Ok(Self::Crs84)
        } else if norm == "EPSG:4326" || norm.ends_with("/EPSG/0/4326") {
            Ok(Self::Epsg4326)
        } else if norm == "EPSG:3857" || norm == "EPSG:900913" || norm.ends_with("/EPSG/0/3857") {
            Ok(Self::Epsg3857)
        } else {
            Err(ApiError::bad_request(format!(
                "Unsupported CRS '{s}'; supported: CRS84, EPSG:4326, EPSG:3857"
            )))
        }
    }

    pub fn uri(self) -> &'static str {
        match self {
            Self::Crs84 => CRS84_URI,
            Self::Epsg4326 => EPSG_4326_URI,
            Self::Epsg3857 => EPSG_3857_URI,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Crs84 => "CRS84",
            Self::Epsg4326 => "EPSG:4326",
            Self::Epsg3857 => "EPSG:3857",
        }
    }

    /// Transform a point from `CRS84` `(lon, lat)` into `self` output coordinates.
    pub fn from_crs84(self, lon: f64, lat: f64) -> [f64; 2] {
        match self {
            Self::Crs84 => [lon, lat],
            Self::Epsg4326 => [lat, lon],
            Self::Epsg3857 => {
                const R: f64 = 6_378_137.0;
                let clamped_lat = lat.clamp(-85.051_128_78, 85.051_128_78);
                let x = lon.to_radians() * R;
                let y = ((std::f64::consts::FRAC_PI_4 + clamped_lat.to_radians() / 2.0)
                    .tan()
                    .ln())
                    * R;
                [x, y]
            }
        }
    }

    /// Transform a `[minx, miny, maxx, maxy]` bbox from `CRS84` into `self`.
    pub fn bbox_from_crs84(self, bbox: [f64; 4]) -> [f64; 4] {
        match self {
            Self::Crs84 => bbox,
            Self::Epsg4326 => [bbox[1], bbox[0], bbox[3], bbox[2]],
            Self::Epsg3857 => {
                let sw = self.from_crs84(bbox[0], bbox[1]);
                let ne = self.from_crs84(bbox[2], bbox[3]);
                [sw[0], sw[1], ne[0], ne[1]]
            }
        }
    }
}

/// Great-circle Haversine distance in meters between two CRS84 `(lon, lat)` points.
pub fn haversine_meters(a: [f64; 2], b: [f64; 2]) -> f64 {
    const EARTH_RADIUS_M: f64 = 6_371_008.8;
    let lat1 = a[1].to_radians();
    let lat2 = b[1].to_radians();
    let dlat = (b[1] - a[1]).to_radians();
    let dlon = (b[0] - a[0]).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().min(1.0).asin()
}
