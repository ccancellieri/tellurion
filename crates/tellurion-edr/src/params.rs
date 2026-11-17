//! Query parameter parsers and validators for OGC API - EDR Part 1 v1.1.

use chrono::{DateTime, NaiveDate, Utc};
use geo_types::{Geometry, LineString, MultiPolygon, Point};
use serde::Deserialize;
use wkt::TryFromWkt;

use crate::crs::SupportedCrs;
use crate::problem::ApiError;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EdrQueryParams {
    pub coords: Option<String>,
    pub bbox: Option<String>,
    pub datetime: Option<String>,
    pub z: Option<String>,
    #[serde(rename = "parameter-name")]
    pub parameter_name: Option<String>,
    pub crs: Option<String>,
    pub f: Option<String>,
    pub profile: Option<String>,
    pub filter: Option<String>,
    #[serde(rename = "filter-lang")]
    pub filter_lang: Option<String>,
    pub intersects: Option<String>,
    pub limit: Option<u32>,
    pub within: Option<f64>,
    #[serde(rename = "within-units")]
    pub within_units: Option<String>,
    #[serde(rename = "corridor-width")]
    pub corridor_width: Option<f64>,
    #[serde(rename = "width-units")]
    pub width_units: Option<String>,
    #[serde(rename = "corridor-height")]
    pub corridor_height: Option<f64>,
    #[serde(rename = "height-units")]
    pub height_units: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    CoverageJson,
    GeoJson,
}

impl OutputFormat {
    pub fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(Self::CoverageJson);
        };
        match s.to_ascii_lowercase().as_str() {
            "coveragejson"
            | "covjson"
            | "application/prs.coverage+json"
            | "application/vnd.cov+json"
            | "json" => Ok(Self::CoverageJson),
            "geojson" | "application/geo+json" => Ok(Self::GeoJson),
            _ => Err(ApiError::unsupported_media_type(format!(
                "Unsupported output format '{s}'; supported: CoverageJSON, GeoJSON"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VerticalSelection {
    Single(f64),
    List(Vec<f64>),
    Interval { min: f64, max: f64 },
}

impl VerticalSelection {
    pub fn parse(raw: Option<&str>) -> Result<Option<Self>, ApiError> {
        let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        if let Some((a, b)) = s.split_once('/') {
            let min: f64 = a
                .trim()
                .parse()
                .map_err(|_| ApiError::bad_request(format!("Invalid z interval start '{a}'")))?;
            let max: f64 = b
                .trim()
                .parse()
                .map_err(|_| ApiError::bad_request(format!("Invalid z interval end '{b}'")))?;
            if min > max {
                return Err(ApiError::bad_request("z interval start must be <= end"));
            }
            return Ok(Some(Self::Interval { min, max }));
        }
        if s.contains(',') {
            let mut vals = Vec::new();
            for part in s.split(',') {
                let v: f64 = part
                    .trim()
                    .parse()
                    .map_err(|_| ApiError::bad_request(format!("Invalid z value '{part}'")))?;
                vals.push(v);
            }
            return Ok(Some(Self::List(vals)));
        }
        let v: f64 = s
            .parse()
            .map_err(|_| ApiError::bad_request(format!("Invalid z value '{s}'")))?;
        Ok(Some(Self::Single(v)))
    }

    pub fn values(&self) -> Vec<f64> {
        match self {
            Self::Single(v) => vec![*v],
            Self::List(vs) => vs.clone(),
            Self::Interval { min, max } => {
                if (*min - *max).abs() < f64::EPSILON {
                    vec![*min]
                } else {
                    vec![*min, *max]
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TemporalRange {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
}

pub(crate) fn parse_iso_instant(raw: &str) -> Result<DateTime<Utc>, ApiError> {
    let s = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = d
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| ApiError::bad_request("Invalid date"))?;
        return Ok(DateTime::from_naive_utc_and_offset(dt, Utc));
    }
    Err(ApiError::bad_request(format!(
        "Invalid ISO 8601 datetime '{s}'"
    )))
}

pub fn parse_datetime(raw: Option<&str>) -> Result<Option<TemporalRange>, ApiError> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if let Some((left, right)) = s.split_once('/') {
        let start = if left.trim() == ".." || left.trim().is_empty() {
            None
        } else {
            Some(parse_iso_instant(left)?)
        };
        let end = if right.trim() == ".." || right.trim().is_empty() {
            None
        } else {
            Some(parse_iso_instant(right)?)
        };
        if let (Some(a), Some(b)) = (start, end) {
            if a > b {
                return Err(ApiError::bad_request("datetime start must be <= end"));
            }
        }
        Ok(Some(TemporalRange { start, end }))
    } else {
        let dt = parse_iso_instant(s)?;
        Ok(Some(TemporalRange {
            start: Some(dt),
            end: Some(dt),
        }))
    }
}

pub fn parse_bbox(raw: Option<&str>) -> Result<[f64; 4], ApiError> {
    let s = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("Missing required query parameter 'bbox'"))?;
    let parts: Vec<f64> = s
        .split(',')
        .map(|p| {
            p.trim()
                .parse::<f64>()
                .map_err(|_| ApiError::bad_request(format!("Invalid bbox ordinate '{p}'")))
        })
        .collect::<Result<_, _>>()?;
    match parts.len() {
        4 => {
            if parts[0] >= parts[2] || parts[1] >= parts[3] {
                return Err(ApiError::bad_request("bbox min must be < max"));
            }
            Ok([parts[0], parts[1], parts[2], parts[3]])
        }
        6 => {
            if parts[0] >= parts[3] || parts[1] >= parts[4] {
                return Err(ApiError::bad_request("bbox min must be < max"));
            }
            Ok([parts[0], parts[1], parts[3], parts[4]])
        }
        _ => Err(ApiError::bad_request(
            "bbox must have 4 or 6 comma-separated numbers",
        )),
    }
}

pub fn parse_wkt_points(raw: Option<&str>) -> Result<Vec<[f64; 2]>, ApiError> {
    let s = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("Missing required query parameter 'coords'"))?;
    let geom: Geometry<f64> = Geometry::try_from_wkt_str(s)
        .map_err(|e| ApiError::bad_request(format!("Invalid WKT in 'coords': {e}")))?;
    match geom {
        Geometry::Point(p) => Ok(vec![[p.x(), p.y()]]),
        Geometry::MultiPoint(mp) if !mp.0.is_empty() => {
            Ok(mp.0.into_iter().map(|p| [p.x(), p.y()]).collect())
        }
        _ => Err(ApiError::bad_request(
            "'coords' must be a WKT POINT or non-empty MULTIPOINT",
        )),
    }
}

pub fn parse_wkt_polygons(raw: Option<&str>) -> Result<MultiPolygon<f64>, ApiError> {
    let s = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("Missing required query parameter 'coords'"))?;
    let geom: Geometry<f64> = Geometry::try_from_wkt_str(s)
        .map_err(|e| ApiError::bad_request(format!("Invalid WKT in 'coords': {e}")))?;
    match geom {
        Geometry::Polygon(poly) => Ok(MultiPolygon::new(vec![poly])),
        Geometry::MultiPolygon(mp) if !mp.0.is_empty() => Ok(mp),
        _ => Err(ApiError::bad_request(
            "'coords' must be a WKT POLYGON or non-empty MULTIPOLYGON",
        )),
    }
}

pub fn parse_wkt_trajectory(
    raw: Option<&str>,
) -> Result<(LineString<f64>, Option<Vec<f64>>), ApiError> {
    let s = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("Missing required query parameter 'coords'"))?;
    let parsed: wkt::Wkt<f64> = s
        .parse()
        .map_err(|e| ApiError::bad_request(format!("Invalid WKT in 'coords': {e}")))?;
    match parsed {
        wkt::Wkt::LineString(ls) => {
            let mut coords = Vec::new();
            let mut z_vals = Vec::new();
            let mut has_z = false;
            for c in ls.coords() {
                coords.push(geo_types::Coord { x: c.x, y: c.y });
                if let Some(z) = c.z {
                    has_z = true;
                    z_vals.push(z);
                }
            }
            if coords.len() < 2 {
                return Err(ApiError::bad_request(
                    "Trajectory LINESTRING must contain at least 2 points",
                ));
            }
            Ok((LineString::new(coords), if has_z { Some(z_vals) } else { None }))
        }
        _ => Err(ApiError::bad_request(
            "'coords' for trajectory/corridor must be a WKT LINESTRING",
        )),
    }
}

pub fn parse_within_meters(within: Option<f64>, units: Option<&str>) -> Result<f64, ApiError> {
    let val = within.ok_or_else(|| ApiError::bad_request("Missing required 'within' parameter"))?;
    if val <= 0.0 || !val.is_finite() {
        return Err(ApiError::bad_request("'within' must be a positive number"));
    }
    let u = units.unwrap_or("km").trim().to_ascii_lowercase();
    let factor = match u.as_str() {
        "m" | "meters" | "metre" | "metres" => 1.0,
        "km" | "kilometers" | "kilometre" | "kilometres" => 1000.0,
        "mi" | "miles" => 1609.344,
        "nm" | "nautical_miles" => 1852.0,
        _ => {
            return Err(ApiError::bad_request(format!(
                "Unsupported within-units '{u}'; supported: m, km, mi, nm"
            )))
        }
    };
    Ok(val * factor)
}

pub fn parse_parameter_names(raw: Option<&str>) -> Option<Vec<String>> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>()
    })
    .filter(|v| !v.is_empty())
}

pub fn parse_crs(raw: Option<&str>) -> Result<SupportedCrs, ApiError> {
    SupportedCrs::parse(raw)
}

pub fn point_from_xy(lon: f64, lat: f64) -> Point<f64> {
    Point::new(lon, lat)
}
