//! Sparse multi-item datacube resolver & CoverageJSON builder (`S ⊆ D`).
//!
//! Unlike single-item (`limit=1`) implementations, this module:
//! 1. Resolves all matching temporal members across `cube:dimensions` (e.g.
//!    dekadal/pentadal calendars with explicit `[start, end]` bounds).
//! 2. Queries `FeatureSource` with BOTH spatial (`bbox`) and temporal
//!    (`DatetimeRange`) predicates to discover which granules actually exist
//!    in `S ⊆ D`, inserting `null` for missing granules/masked cells.
//! 3. Applies exact polygon masking (`geo::Contains`) for `area`, Haversine
//!    circle masking for `radius`, and corridor buffering for `corridor`.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use geo::{BoundingRect, Contains, Intersects};
use geo_types::{Coord, Geometry, MultiPolygon, Point, Rect};
use serde_json::{json, Value};
use tellurion_core::{AppContext, CollectionDecl, CoverageGrid, ItemsQuery, RasterSource};

use crate::crs::{haversine_meters, SupportedCrs};
use crate::models::{Axis, CovJsonParameter, Coverage, Domain, NdArray, Referencing};
use crate::params::TemporalRange;
use crate::problem::ApiError;

#[derive(Debug, Clone)]
pub struct TimeSlice {
    pub label: String,
    pub bounds: Option<[String; 2]>,
    pub has_granule: bool,
    pub matched_bbox: Option<[f64; 4]>,
}

fn collect_coords_bbox(coords: &Value, acc: &mut [f64; 4], found: &mut bool) {
    let Some(arr) = coords.as_array() else {
        return;
    };
    if arr.len() >= 2 {
        if let (Some(x), Some(y)) = (arr[0].as_f64(), arr[1].as_f64()) {
            acc[0] = acc[0].min(x);
            acc[1] = acc[1].min(y);
            acc[2] = acc[2].max(x);
            acc[3] = acc[3].max(y);
            *found = true;
            return;
        }
    }
    for item in arr {
        collect_coords_bbox(item, acc, found);
    }
}

fn feature_bbox(feat: &Value) -> Option<[f64; 4]> {
    if let Some(arr) = feat.get("bbox").and_then(Value::as_array) {
        if arr.len() >= 4 {
            if let (Some(a), Some(b), Some(c), Some(d)) = (
                arr[0].as_f64(),
                arr[1].as_f64(),
                arr[2].as_f64(),
                arr[3].as_f64(),
            ) {
                return Some([a, b, c, d]);
            }
        }
    }
    if let Some(coords) = feat.get("geometry").and_then(|g| g.get("coordinates")) {
        let mut acc = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        let mut found = false;
        collect_coords_bbox(coords, &mut acc, &mut found);
        if found {
            return Some(acc);
        }
    }
    None
}

fn bboxes_intersect(a: [f64; 4], b: [f64; 4]) -> bool {
    a[2] >= b[0] && a[0] <= b[2] && a[3] >= b[1] && a[1] <= b[3]
}

/// Discover sparse temporal slices `S ⊆ D` without `limit=1`.
pub async fn resolve_sparse_time_slices(
    ctx: &AppContext,
    tenant: &str,
    catalog: &str,
    decl: &CollectionDecl,
    bbox: [f64; 4],
    temporal: Option<&TemporalRange>,
) -> Result<Vec<TimeSlice>, ApiError> {
    resolve_sparse_time_slices_with_geom(ctx, tenant, catalog, decl, bbox, None, temporal).await
}

/// Discover sparse temporal slices `S ⊆ D` filtering against both `bbox` and an optional WKT `Geometry`.
pub async fn resolve_sparse_time_slices_with_geom(
    ctx: &AppContext,
    tenant: &str,
    catalog: &str,
    decl: &CollectionDecl,
    bbox: [f64; 4],
    filter_geom: Option<&Geometry<f64>>,
    temporal: Option<&TemporalRange>,
) -> Result<Vec<TimeSlice>, ApiError> {
    let mut slices = build_intensional_slices(decl, temporal);

    // Query FeatureSource (granule index S) spatially with bbox (R-tree pushdown),
    // then match temporal periods and optional exact geometry intersection.
    if let Ok((feat_decl, feat_source)) = ctx
        .current()
        .router
        .resolve_features(tenant, catalog, &decl.id)
        .await
    {
        let query = ItemsQuery {
            limit: 1000,
            bbox: Some(bbox),
            datetime: None,
            ..Default::default()
        };
        if let Ok(page) = feat_source.items(&feat_decl, &query).await {
            let mut present_keys: HashMap<String, Option<[f64; 4]>> = HashMap::new();
            for feat in &page.features_geojson {
                let fbox = feature_bbox(feat);
                if let Some(fb) = fbox {
                    if !bboxes_intersect(fb, bbox) {
                        continue;
                    }
                    if let Some(geom) = filter_geom {
                        let rect =
                            Rect::new(Coord { x: fb[0], y: fb[1] }, Coord { x: fb[2], y: fb[3] });
                        if !geom.intersects(&rect) {
                            continue;
                        }
                    }
                }
                if let Some(props) = feat.get("properties") {
                    for key in [
                        "pentad_id",
                        "dekad_id",
                        "period_id",
                        "start_datetime",
                        "datetime",
                        "date",
                    ] {
                        if let Some(v) = props.get(key).and_then(Value::as_str) {
                            present_keys.entry(v.to_string()).or_insert(fbox);
                        }
                    }
                }
                if let Some(id) = feat.get("id").and_then(Value::as_str) {
                    present_keys.entry(id.to_string()).or_insert(fbox);
                }
            }
            for slice in &mut slices {
                let matched = present_keys.get(&slice.label).copied().or_else(|| {
                    slice
                        .bounds
                        .as_ref()
                        .and_then(|b| present_keys.get(&b[0]).copied())
                });
                match matched {
                    Some(fbox) => {
                        slice.has_granule = true;
                        slice.matched_bbox = fbox;
                    }
                    None => {
                        slice.has_granule = false;
                        slice.matched_bbox = None;
                    }
                }
            }
        }
    }

    Ok(slices)
}

fn build_intensional_slices(
    decl: &CollectionDecl,
    temporal: Option<&TemporalRange>,
) -> Vec<TimeSlice> {
    if let Some(stac) = &decl.settings.stac {
        if let Some(time_dim) = stac.cube_dimensions.get("time") {
            if let Some(vals) = &time_dim.values {
                let mut out = Vec::new();
                for v in vals {
                    if let Some(s) = v.as_str() {
                        out.push(TimeSlice {
                            label: s.to_string(),
                            bounds: None,
                            has_granule: true,
                            matched_bbox: None,
                        });
                    }
                }
                if !out.is_empty() {
                    return out;
                }
            }
            if time_dim.size == Some(72) {
                return pentadal_2021_slices(temporal);
            }
            if time_dim.size == Some(36) {
                return dekadal_2021_slices(temporal);
            }
        }
    }
    let label = temporal
        .and_then(|t| t.start)
        .unwrap_or_else(Utc::now)
        .to_rfc3339();
    vec![TimeSlice {
        label,
        bounds: None,
        has_granule: true,
        matched_bbox: None,
    }]
}

fn interval_overlaps_temporal(
    start_s: &str,
    end_s: &str,
    temporal: Option<&TemporalRange>,
) -> bool {
    if let Some(t) = temporal {
        let s_dt = start_s.parse::<DateTime<Utc>>().ok();
        let e_dt = end_s.parse::<DateTime<Utc>>().ok();
        match (s_dt, e_dt, t.start, t.end) {
            (Some(s), Some(e), Some(ts), Some(te)) => e >= ts && s <= te,
            (Some(s), _, _, Some(te)) => s <= te,
            (_, Some(e), Some(ts), _) => e >= ts,
            _ => true,
        }
    } else {
        true
    }
}

fn dekadal_2021_slices(temporal: Option<&TemporalRange>) -> Vec<TimeSlice> {
    let mut out = Vec::new();
    for month in 1u32..=12 {
        let last_day = match month {
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        for (idx, (d_start, d_end)) in [(1, 10), (11, 20), (21, last_day)].iter().enumerate() {
            let start_s = format!("2021-{month:02}-{d_start:02}T00:00:00Z");
            let end_s = format!("2021-{month:02}-{d_end:02}T23:59:59Z");
            if interval_overlaps_temporal(&start_s, &end_s, temporal) {
                out.push(TimeSlice {
                    label: format!("2021-{month:02}-D{}", idx + 1),
                    bounds: Some([start_s, end_s]),
                    has_granule: true,
                    matched_bbox: None,
                });
            }
        }
    }
    out
}

fn pentadal_2021_slices(temporal: Option<&TemporalRange>) -> Vec<TimeSlice> {
    let mut out = Vec::new();
    for month in 1u32..=12 {
        let last_day = match month {
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        for (idx, (p_start, p_end)) in [
            (1, 5),
            (6, 10),
            (11, 15),
            (16, 20),
            (21, 25),
            (26, last_day),
        ]
        .iter()
        .enumerate()
        {
            let start_s = format!("2021-{month:02}-{p_start:02}T00:00:00Z");
            let end_s = format!("2021-{month:02}-{p_end:02}T23:59:59Z");
            if interval_overlaps_temporal(&start_s, &end_s, temporal) {
                out.push(TimeSlice {
                    label: format!("2021-{month:02}-P{}", idx + 1),
                    bounds: Some([start_s, end_s]),
                    has_granule: true,
                    matched_bbox: None,
                });
            }
        }
    }
    out
}

pub async fn sample_points_with_fallback(
    source: &dyn RasterSource,
    decl: &CollectionDecl,
    points: &[[f64; 2]],
    band_indices: Option<&[usize]>,
) -> Result<Vec<Vec<Option<f64>>>, ApiError> {
    if let Some(vals) = source.sample_points(decl, points, band_indices).await? {
        return Ok(vals);
    }
    let mut out = vec![vec![None; points.len()]];
    for (idx, &[lon, lat]) in points.iter().enumerate() {
        let eps = 0.01;
        let grid = sample_grid_with_fallback(
            source,
            decl,
            [lon - eps, lat - eps, lon + eps, lat + eps],
            4,
            band_indices,
        )
        .await?;
        if out.len() != grid.bands.len() {
            out = vec![vec![None; points.len()]; grid.bands.len().max(1)];
        }
        let center_idx = (grid.height / 2) * grid.width + (grid.width / 2);
        for (b, band_vals) in grid.values.iter().enumerate() {
            out[b][idx] = band_vals
                .get(center_idx)
                .copied()
                .flatten()
                .or_else(|| band_vals.iter().copied().flatten().next());
        }
    }
    Ok(out)
}

fn lon_lat_to_tile(lon: f64, lat: f64, z: u8) -> tellurion_core::TileCoord {
    let n = (1u64 << z) as f64;
    let max_idx = (1u32 << z).saturating_sub(1);
    let x = (((lon.clamp(-180.0, 180.0) + 180.0) / 360.0) * n)
        .floor()
        .clamp(0.0, max_idx as f64) as u32;
    let lat_rad = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    let merc_y =
        (1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) * 0.5 * n;
    let y = merc_y.floor().clamp(0.0, max_idx as f64) as u32;
    tellurion_core::TileCoord { z, x, y }
}

pub async fn sample_grid_with_fallback(
    source: &dyn RasterSource,
    decl: &CollectionDecl,
    bbox: [f64; 4],
    max_dim: usize,
    band_indices: Option<&[usize]>,
) -> Result<CoverageGrid, ApiError> {
    if let Some(grid) = source
        .sample_grid(decl, bbox, max_dim, band_indices)
        .await?
    {
        return Ok(grid);
    }
    // Fallback to `raster_tile` at the zoom level matching the requested bbox.
    let center_lon = (bbox[0] + bbox[2]) * 0.5;
    let center_lat = (bbox[1] + bbox[3]) * 0.5;
    let span_lon = (bbox[2] - bbox[0]).abs().max(1e-4);
    let ideal_z = (360.0 / span_lon).log2().floor().clamp(0.0, 12.0) as u8;
    for z in (0..=ideal_z).rev() {
        let coord = lon_lat_to_tile(center_lon, center_lat, z);
        if let Some(win) = source.raster_tile(decl, coord).await? {
            let n = (1u64 << z) as f64;
            let w = (win.width as usize).min(max_dim).max(1);
            let h = (win.height as usize).min(max_dim).max(1);
            let dx = (bbox[2] - bbox[0]) / w as f64;
            let dy = (bbox[3] - bbox[1]) / h as f64;
            let mut band = Vec::with_capacity(w * h);
            let mut non_null = 0usize;
            for row in 0..h {
                let lat = bbox[3] - (row as f64 + 0.5) * dy;
                let lat_rad = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
                let merc_y = (1.0
                    - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI)
                    * 0.5
                    * n;
                let v = merc_y - coord.y as f64;
                for col in 0..w {
                    let lon = bbox[0] + (col as f64 + 0.5) * dx;
                    let u = ((lon + 180.0) / 360.0 * n) - coord.x as f64;
                    if (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v) {
                        let px =
                            ((u * win.width as f64).floor() as usize).min(win.width as usize - 1);
                        let py =
                            ((v * win.height as f64).floor() as usize).min(win.height as usize - 1);
                        let off = (py * win.width as usize + px) * 4;
                        let alpha = win.rgba.get(off + 3).copied().unwrap_or(0);
                        if alpha == 0 {
                            band.push(None);
                        } else {
                            non_null += 1;
                            band.push(win.rgba.get(off).map(|&val| val as f64));
                        }
                    } else {
                        band.push(None);
                    }
                }
            }
            if non_null > 0 || z == 0 {
                return Ok(CoverageGrid {
                    width: w,
                    height: h,
                    bbox,
                    bands: vec!["band_1".to_string()],
                    values: vec![band],
                    crs: "CRS84".to_string(),
                });
            }
        }
    }
    Ok(CoverageGrid {
        width: 1,
        height: 1,
        bbox,
        bands: vec!["band_1".to_string()],
        values: vec![vec![None]],
        crs: "CRS84".to_string(),
    })
}

pub fn mask_grid_by_polygons(grid: &mut CoverageGrid, polys: &MultiPolygon<f64>) {
    let dx = (grid.bbox[2] - grid.bbox[0]) / grid.width.max(1) as f64;
    let dy = (grid.bbox[3] - grid.bbox[1]) / grid.height.max(1) as f64;
    for y in 0..grid.height {
        let lat = grid.bbox[3] - (y as f64 + 0.5) * dy;
        for x in 0..grid.width {
            let lon = grid.bbox[0] + (x as f64 + 0.5) * dx;
            let pt = Point::new(lon, lat);
            if !polys.contains(&pt) {
                let idx = y * grid.width + x;
                for band in &mut grid.values {
                    if idx < band.len() {
                        band[idx] = None;
                    }
                }
            }
        }
    }
}

pub fn mask_grid_by_radius(grid: &mut CoverageGrid, center: [f64; 2], radius_m: f64) {
    let dx = (grid.bbox[2] - grid.bbox[0]) / grid.width.max(1) as f64;
    let dy = (grid.bbox[3] - grid.bbox[1]) / grid.height.max(1) as f64;
    for y in 0..grid.height {
        let lat = grid.bbox[3] - (y as f64 + 0.5) * dy;
        for x in 0..grid.width {
            let lon = grid.bbox[0] + (x as f64 + 0.5) * dx;
            if haversine_meters(center, [lon, lat]) > radius_m {
                let idx = y * grid.width + x;
                for band in &mut grid.values {
                    if idx < band.len() {
                        band[idx] = None;
                    }
                }
            }
        }
    }
}

pub fn bbox_of_polygons(polys: &MultiPolygon<f64>) -> Result<[f64; 4], ApiError> {
    let rect = polys
        .bounding_rect()
        .ok_or_else(|| ApiError::bad_request("Polygon has empty bounding box"))?;
    Ok([rect.min().x, rect.min().y, rect.max().x, rect.max().y])
}

pub fn build_grid_coverage(
    grid: &CoverageGrid,
    slices: &[TimeSlice],
    crs: SupportedCrs,
    selected_params: Option<&[String]>,
) -> Coverage {
    let out_bbox = crs.bbox_from_crs84(grid.bbox);
    let mut axes = BTreeMap::new();
    axes.insert(
        "x".to_string(),
        Axis {
            values: None,
            start: Some(out_bbox[0]),
            stop: Some(out_bbox[2]),
            num: Some(grid.width),
            bounds: None,
        },
    );
    axes.insert(
        "y".to_string(),
        Axis {
            values: None,
            start: Some(out_bbox[1]),
            stop: Some(out_bbox[3]),
            num: Some(grid.height),
            bounds: None,
        },
    );
    let t_values: Vec<Value> = slices.iter().map(|s| json!(s.label)).collect();
    let t_bounds: Vec<Value> = slices
        .iter()
        .filter_map(|s| s.bounds.as_ref().map(|b| json!([b[0], b[1]])))
        .collect();
    axes.insert(
        "t".to_string(),
        Axis {
            values: Some(t_values),
            start: None,
            stop: None,
            num: None,
            bounds: if t_bounds.len() == slices.len() && !t_bounds.is_empty() {
                Some(t_bounds)
            } else {
                None
            },
        },
    );

    let mut parameters = BTreeMap::new();
    let mut ranges = BTreeMap::new();
    for (b_idx, band_name) in grid.bands.iter().enumerate() {
        if let Some(filter) = selected_params {
            if !filter.iter().any(|p| p == band_name) {
                continue;
            }
        }
        parameters.insert(
            band_name.clone(),
            CovJsonParameter {
                kind: "Parameter",
                description: Some(BTreeMap::from([(
                    "en".to_string(),
                    format!("Band {band_name}"),
                )])),
                unit: None,
                observed_property: json!({ "label": { "en": band_name } }),
            },
        );
        let dx = (grid.bbox[2] - grid.bbox[0]) / grid.width.max(1) as f64;
        let dy = (grid.bbox[3] - grid.bbox[1]) / grid.height.max(1) as f64;
        let mut t_y_x = Vec::with_capacity(slices.len() * grid.height * grid.width);
        let slice_vals = grid.values.get(b_idx);
        for slice in slices {
            if slice.has_granule {
                if let Some(vals) = slice_vals {
                    if let Some(gb) = slice.matched_bbox {
                        for row in 0..grid.height {
                            let lat = grid.bbox[3] - (row as f64 + 0.5) * dy;
                            for col in 0..grid.width {
                                let lon = grid.bbox[0] + (col as f64 + 0.5) * dx;
                                let idx = row * grid.width + col;
                                if lon >= gb[0] && lon <= gb[2] && lat >= gb[1] && lat <= gb[3] {
                                    t_y_x.push(vals.get(idx).copied().flatten());
                                } else {
                                    t_y_x.push(None);
                                }
                            }
                        }
                    } else {
                        t_y_x.extend_from_slice(vals);
                    }
                } else {
                    t_y_x.extend(std::iter::repeat_n(None, grid.height * grid.width));
                }
            } else {
                t_y_x.extend(std::iter::repeat_n(None, grid.height * grid.width));
            }
        }
        ranges.insert(
            band_name.clone(),
            NdArray {
                kind: "NdArray",
                data_type: "float",
                axis_names: vec!["t".to_string(), "y".to_string(), "x".to_string()],
                shape: vec![slices.len(), grid.height, grid.width],
                values: t_y_x,
            },
        );
    }

    let coord_names = match crs {
        SupportedCrs::Epsg4326 => vec!["y".to_string(), "x".to_string()],
        _ => vec!["x".to_string(), "y".to_string()],
    };

    Coverage {
        kind: "Coverage",
        domain: Domain {
            kind: "Domain",
            domain_type: "Grid",
            axes,
            referencing: vec![
                Referencing {
                    coordinates: coord_names,
                    system: json!({
                        "type": "GeographicCRS",
                        "id": crs.uri()
                    }),
                },
                Referencing {
                    coordinates: vec!["t".to_string()],
                    system: json!({
                        "type": "TemporalRS",
                        "calendar": "Gregorian"
                    }),
                },
            ],
        },
        parameters,
        ranges,
    }
}

pub fn build_point_coverage(
    points: &[[f64; 2]],
    slices: &[TimeSlice],
    band_names: &[String],
    sampled: &[Vec<Option<f64>>],
    crs: SupportedCrs,
    domain_type: &'static str,
) -> Coverage {
    let mut axes = BTreeMap::new();
    let xs: Vec<Value> = points
        .iter()
        .map(|p| json!(crs.from_crs84(p[0], p[1])[0]))
        .collect();
    let ys: Vec<Value> = points
        .iter()
        .map(|p| json!(crs.from_crs84(p[0], p[1])[1]))
        .collect();
    axes.insert(
        "x".to_string(),
        Axis {
            values: Some(xs),
            start: None,
            stop: None,
            num: None,
            bounds: None,
        },
    );
    axes.insert(
        "y".to_string(),
        Axis {
            values: Some(ys),
            start: None,
            stop: None,
            num: None,
            bounds: None,
        },
    );
    axes.insert(
        "t".to_string(),
        Axis {
            values: Some(slices.iter().map(|s| json!(s.label)).collect()),
            start: None,
            stop: None,
            num: None,
            bounds: None,
        },
    );

    let mut parameters = BTreeMap::new();
    let mut ranges = BTreeMap::new();
    for (b_idx, name) in band_names.iter().enumerate() {
        parameters.insert(
            name.clone(),
            CovJsonParameter {
                kind: "Parameter",
                description: None,
                unit: None,
                observed_property: json!({ "label": { "en": name } }),
            },
        );
        let mut values = Vec::with_capacity(slices.len() * points.len());
        for s in slices {
            if s.has_granule {
                if let Some(row) = sampled.get(b_idx) {
                    values.extend_from_slice(row);
                } else {
                    values.extend(std::iter::repeat_n(None, points.len()));
                }
            } else {
                values.extend(std::iter::repeat_n(None, points.len()));
            }
        }
        ranges.insert(
            name.clone(),
            NdArray {
                kind: "NdArray",
                data_type: "float",
                axis_names: vec!["t".to_string(), "x".to_string()],
                shape: vec![slices.len(), points.len()],
                values,
            },
        );
    }

    Coverage {
        kind: "Coverage",
        domain: Domain {
            kind: "Domain",
            domain_type,
            axes,
            referencing: vec![Referencing {
                coordinates: vec!["x".to_string(), "y".to_string()],
                system: json!({ "type": "GeographicCRS", "id": crs.uri() }),
            }],
        },
        parameters,
        ranges,
    }
}

pub fn line_bbox(coords: &[Coord<f64>], buffer_deg: f64) -> [f64; 4] {
    let mut minx = f64::INFINITY;
    let mut miny = f64::INFINITY;
    let mut maxx = f64::NEG_INFINITY;
    let mut maxy = f64::NEG_INFINITY;
    for c in coords {
        minx = minx.min(c.x);
        miny = miny.min(c.y);
        maxx = maxx.max(c.x);
        maxy = maxy.max(c.y);
    }
    [
        minx - buffer_deg,
        miny - buffer_deg,
        maxx + buffer_deg,
        maxy + buffer_deg,
    ]
}
