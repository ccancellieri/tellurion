//! Axum HTTP handlers for OGC API - EDR Part 1 v1.1.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use geo::BoundingRect;
use geo_types::{Coord, Geometry, LineString, MultiPoint, MultiPolygon, Point, Polygon, Rect};
use serde::Serialize;
use serde_json::{json, Value};
use tellurion_core::filter::{self, Filter, GeometryLiteral, TemporalOp, TemporalValue};
use tellurion_core::{AppContext, CollectionDecl, ItemsQuery};
use wkt::TryFromWkt;

use crate::models::{
    EdrCollection, EdrCollections, EdrConformance, EdrDataQueryLink, EdrExtent, EdrInstances,
    EdrLandingPage, EdrParameter, Link,
};
use crate::params::{
    parse_bbox, parse_crs, parse_datetime, parse_iso_instant, parse_parameter_names,
    parse_within_meters, parse_wkt_points, parse_wkt_polygons, parse_wkt_trajectory,
    EdrQueryParams, OutputFormat, TemporalRange, VerticalSelection,
};
use crate::problem::ApiError;
use crate::sparse::{
    bbox_of_polygons, build_grid_coverage, build_point_coverage, line_bbox, mask_grid_by_polygons,
    mask_grid_by_radius, resolve_sparse_time_slices, resolve_sparse_time_slices_with_geom,
    sample_grid_with_fallback, sample_points_with_fallback, TimeSlice,
};
use crate::{
    CONFORMANCE_CLASSES, COVJSON_DOMAIN_MEDIA_TYPE, COVJSON_LEGACY_MEDIA_TYPE, COVJSON_MEDIA_TYPE,
    DOMAIN_PROFILE_URI, GEOJSON_MEDIA_TYPE, JSON_MEDIA_TYPE,
};

pub const DEFAULT_TENANT: &str = "public";
pub const DEFAULT_CATALOG: &str = "default";

fn json_response<T: Serialize>(media_type: &'static str, body: &T) -> Response {
    let mut response = (StatusCode::OK, Json(body)).into_response();
    if let Ok(hv) = HeaderValue::from_str(media_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, hv);
    }
    response
}

fn base_path(uri: &OriginalUri) -> String {
    uri.0.path().trim_end_matches('/').to_string()
}

fn extract_scope(params: &BTreeMap<String, String>) -> (&str, &str) {
    let tenant = params
        .get("tenant")
        .map(String::as_str)
        .unwrap_or(DEFAULT_TENANT);
    let catalog = params
        .get("catalog")
        .map(String::as_str)
        .unwrap_or(DEFAULT_CATALOG);
    (tenant, catalog)
}

fn build_collection_dto(decl: &CollectionDecl, coll_base: &str) -> EdrCollection {
    let mut queries = BTreeMap::new();
    for q in [
        "position",
        "area",
        "cube",
        "radius",
        "trajectory",
        "corridor",
        "locations",
        "instances",
    ] {
        queries.insert(
            q.to_string(),
            EdrDataQueryLink {
                link: Link::new(format!("{coll_base}/{q}"), "data", COVJSON_MEDIA_TYPE),
            },
        );
    }
    queries.insert(
        "items".to_string(),
        EdrDataQueryLink {
            link: Link::new(format!("{coll_base}/items"), "items", GEOJSON_MEDIA_TYPE),
        },
    );
    let mut params_map = BTreeMap::new();
    params_map.insert(
        "band_1".to_string(),
        EdrParameter {
            id: "band_1".to_string(),
            kind: "Parameter".to_string(),
            description: Some("Primary raster/cube parameter".to_string()),
            unit: None,
            observed_property: json!({ "label": { "en": "band_1" } }),
        },
    );

    let temporal = decl.settings.stac.as_ref().and_then(|s| {
        s.cube_dimensions.get("time").and_then(|d| {
            d.extent
                .as_ref()
                .map(|ext| json!({ "interval": [ext], "trs": "http://www.opengis.net/def/uom/ISO-8601/0/Gregorian" }))
        })
    });

    EdrCollection {
        id: decl.external_id().to_string(),
        title: Some(decl.external_id().to_string()),
        description: None,
        extent: Some(EdrExtent {
            spatial: Some(json!({ "bbox": [[-180.0, -90.0, 180.0, 90.0]], "crs": "CRS84" })),
            temporal,
            vertical: None,
        }),
        data_queries: Some(queries),
        crs: vec![
            "CRS84".to_string(),
            "EPSG:4326".to_string(),
            "EPSG:3857".to_string(),
        ],
        output_formats: vec!["CoverageJSON".to_string(), "GeoJSON".to_string()],
        parameter_names: Some(params_map),
        links: vec![
            Link::new(coll_base, "self", JSON_MEDIA_TYPE),
            Link::new(format!("{coll_base}/items"), "items", GEOJSON_MEDIA_TYPE),
            Link::new(format!("{coll_base}/scenes"), "scenes", GEOJSON_MEDIA_TYPE),
            Link::new(
                format!("{coll_base}/instances"),
                "instances",
                JSON_MEDIA_TYPE,
            ),
            Link::new(
                format!("{coll_base}/locations"),
                "locations",
                GEOJSON_MEDIA_TYPE,
            ),
        ],
    }
}

pub async fn landing_page(uri: OriginalUri) -> Response {
    let base = base_path(&uri);
    let body = EdrLandingPage {
        title: "Tellurion OGC API - EDR".to_string(),
        description: "OGC API - Environmental Data Retrieval Part 1 v1.1 with sparse multi-item cube support.".to_string(),
        links: vec![
            Link::new(&base, "self", JSON_MEDIA_TYPE),
            Link::new(format!("{base}/conformance"), "conformance", JSON_MEDIA_TYPE),
            Link::new(format!("{base}/collections"), "data", JSON_MEDIA_TYPE),
        ],
    };
    json_response(JSON_MEDIA_TYPE, &body)
}

pub async fn conformance() -> Response {
    let body = EdrConformance {
        conforms_to: CONFORMANCE_CLASSES
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
    };
    json_response(JSON_MEDIA_TYPE, &body)
}

pub async fn list_collections(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    uri: OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let base = base_path(&uri);
    let state = ctx.current();
    let page = state
        .registry
        .list_collections(
            catalog,
            tellurion_core::PageRequest {
                limit: 1000,
                after: None,
            },
        )
        .await?;
    let mut dtos = Vec::new();
    for c in &page.items {
        if state
            .router
            .resolve_raster(tenant, catalog, c.external_id())
            .await
            .is_ok()
        {
            dtos.push(build_collection_dto(
                c,
                &format!("{base}/{}", c.external_id()),
            ));
        }
    }
    let body = EdrCollections {
        collections: dtos,
        links: vec![Link::new(base, "self", JSON_MEDIA_TYPE)],
    };
    Ok(json_response(JSON_MEDIA_TYPE, &body))
}

pub async fn get_collection(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    uri: OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let (decl, _source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let base = base_path(&uri);
    Ok(json_response(
        JSON_MEDIA_TYPE,
        &build_collection_dto(&decl, &base),
    ))
}

pub async fn list_instances(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    uri: OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let (decl, _source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let base = base_path(&uri);
    let inst = build_collection_dto(&decl, &format!("{base}/default"));
    let body = EdrInstances {
        instances: vec![inst],
        links: vec![Link::new(base, "self", JSON_MEDIA_TYPE)],
    };
    Ok(json_response(JSON_MEDIA_TYPE, &body))
}

pub async fn get_instance(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    uri: OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let iid = params
        .get("iid")
        .ok_or_else(|| ApiError::bad_request("Missing instance id"))?;
    if iid != "default" {
        return Err(ApiError::not_found(format!("Instance '{iid}' not found")));
    }
    let (decl, _source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let base = base_path(&uri);
    Ok(json_response(
        JSON_MEDIA_TYPE,
        &build_collection_dto(&decl, &base),
    ))
}

pub async fn list_locations(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let _ = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let features = if let Ok((f_decl, f_source)) = ctx
        .current()
        .router
        .resolve_features(tenant, catalog, cid)
        .await
    {
        f_source
            .items(
                &f_decl,
                &ItemsQuery {
                    limit: 100,
                    ..Default::default()
                },
            )
            .await
            .map(|p| p.features_geojson)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(json_response(
        GEOJSON_MEDIA_TYPE,
        &json!({ "type": "FeatureCollection", "features": features }),
    ))
}

pub async fn get_location(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let loc_id = params
        .get("locId")
        .ok_or_else(|| ApiError::bad_request("Missing location id"))?;
    let _ = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    if let Ok((f_decl, f_source)) = ctx
        .current()
        .router
        .resolve_features(tenant, catalog, cid)
        .await
    {
        if let Ok(Some(feat)) = f_source.item(&f_decl, loc_id, None).await {
            return Ok(json_response(GEOJSON_MEDIA_TYPE, &feat));
        }
    }
    Err(ApiError::not_found(format!(
        "Location '{loc_id}' not found"
    )))
}

pub async fn query_position(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    Query(query): Query<EdrQueryParams>,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let points = parse_wkt_points(query.coords.as_deref())?;
    let temporal = parse_datetime(query.datetime.as_deref())?;
    let _z = VerticalSelection::parse(query.z.as_deref())?;
    let crs = parse_crs(query.crs.as_deref())?;
    let format = OutputFormat::parse(query.f.as_deref())?;

    let (decl, source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let bbox = [
        points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min) - 0.001,
        points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min) - 0.001,
        points
            .iter()
            .map(|p| p[0])
            .fold(f64::NEG_INFINITY, f64::max)
            + 0.001,
        points
            .iter()
            .map(|p| p[1])
            .fold(f64::NEG_INFINITY, f64::max)
            + 0.001,
    ];
    let slices =
        resolve_sparse_time_slices(&ctx, tenant, catalog, &decl, bbox, temporal.as_ref()).await?;
    let sampled = sample_points_with_fallback(source.as_ref(), &decl, &points, None).await?;
    let band_names = vec!["band_1".to_string()];

    match format {
        OutputFormat::GeoJson => {
            let features: Vec<Value> = points
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let out_p = crs.from_crs84(p[0], p[1]);
                    let val = sampled.first().and_then(|r| r.get(i)).copied().flatten();
                    json!({
                        "type": "Feature",
                        "geometry": { "type": "Point", "coordinates": out_p },
                        "properties": {
                            "band_1": val,
                            "time_slices": slices.iter().map(|s| json!({"id": s.label, "available": s.has_granule})).collect::<Vec<_>>()
                        }
                    })
                })
                .collect();
            Ok(json_response(
                GEOJSON_MEDIA_TYPE,
                &json!({ "type": "FeatureCollection", "features": features }),
            ))
        }
        OutputFormat::CoverageJson => {
            let domain_type = if slices.len() > 1 {
                if points.len() > 1 {
                    "MultiPointSeries"
                } else {
                    "PointSeries"
                }
            } else if points.len() > 1 {
                "MultiPoint"
            } else {
                "Point"
            };
            let cov =
                build_point_coverage(&points, &slices, &band_names, &sampled, crs, domain_type);
            Ok(json_response(COVJSON_MEDIA_TYPE, &cov))
        }
    }
}

pub async fn query_area(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    Query(query): Query<EdrQueryParams>,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let polys = parse_wkt_polygons(query.coords.as_deref())?;
    let bbox = bbox_of_polygons(&polys)?;
    let temporal = parse_datetime(query.datetime.as_deref())?;
    let _z = VerticalSelection::parse(query.z.as_deref())?;
    let crs = parse_crs(query.crs.as_deref())?;
    let format = OutputFormat::parse(query.f.as_deref())?;
    if format != OutputFormat::CoverageJson {
        return Err(ApiError::unsupported_media_type(
            "area query supports CoverageJSON output only",
        ));
    }
    let selected = parse_parameter_names(query.parameter_name.as_deref());

    let (decl, source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let poly_geom = geo_types::Geometry::MultiPolygon(polys.clone());
    let slices = resolve_sparse_time_slices_with_geom(
        &ctx,
        tenant,
        catalog,
        &decl,
        bbox,
        Some(&poly_geom),
        temporal.as_ref(),
    )
    .await?;
    let mut grid = sample_grid_with_fallback(source.as_ref(), &decl, bbox, 64, None).await?;
    mask_grid_by_polygons(&mut grid, &polys);
    let cov = build_grid_coverage(&grid, &slices, crs, selected.as_deref());
    Ok(json_response(COVJSON_MEDIA_TYPE, &cov))
}

pub async fn query_cube(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    Query(query): Query<EdrQueryParams>,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let bbox = parse_bbox(query.bbox.as_deref())?;
    let temporal = parse_datetime(query.datetime.as_deref())?;
    let _z = VerticalSelection::parse(query.z.as_deref())?;
    let crs = parse_crs(query.crs.as_deref())?;
    let format = OutputFormat::parse(query.f.as_deref())?;
    if format != OutputFormat::CoverageJson {
        return Err(ApiError::unsupported_media_type(
            "cube query supports CoverageJSON output only",
        ));
    }
    let selected = parse_parameter_names(query.parameter_name.as_deref());

    let (decl, source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let slices =
        resolve_sparse_time_slices(&ctx, tenant, catalog, &decl, bbox, temporal.as_ref()).await?;
    let grid = sample_grid_with_fallback(source.as_ref(), &decl, bbox, 64, None).await?;
    let cov = build_grid_coverage(&grid, &slices, crs, selected.as_deref());
    Ok(json_response(COVJSON_MEDIA_TYPE, &cov))
}

pub async fn query_radius(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    Query(query): Query<EdrQueryParams>,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let pts = parse_wkt_points(query.coords.as_deref())?;
    let center = pts[0];
    let radius_m = parse_within_meters(query.within, query.within_units.as_deref())?;
    let temporal = parse_datetime(query.datetime.as_deref())?;
    let crs = parse_crs(query.crs.as_deref())?;
    let selected = parse_parameter_names(query.parameter_name.as_deref());

    let d_lat = radius_m / 111_320.0;
    let d_lon = d_lat / center[1].to_radians().cos().abs().max(0.1);
    let bbox = [
        center[0] - d_lon,
        center[1] - d_lat,
        center[0] + d_lon,
        center[1] + d_lat,
    ];

    let (decl, source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let slices =
        resolve_sparse_time_slices(&ctx, tenant, catalog, &decl, bbox, temporal.as_ref()).await?;
    let mut grid = sample_grid_with_fallback(source.as_ref(), &decl, bbox, 64, None).await?;
    mask_grid_by_radius(&mut grid, center, radius_m);
    let cov = build_grid_coverage(&grid, &slices, crs, selected.as_deref());
    Ok(json_response(COVJSON_MEDIA_TYPE, &cov))
}

pub async fn query_trajectory(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    Query(query): Query<EdrQueryParams>,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let (line, _z_coords) = parse_wkt_trajectory(query.coords.as_deref())?;
    let pts: Vec<[f64; 2]> = line.0.iter().map(|c| [c.x, c.y]).collect();
    let temporal = parse_datetime(query.datetime.as_deref())?;
    let crs = parse_crs(query.crs.as_deref())?;
    let bbox = line_bbox(&line.0, 0.001);

    let (decl, source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let slices =
        resolve_sparse_time_slices(&ctx, tenant, catalog, &decl, bbox, temporal.as_ref()).await?;
    let sampled = sample_points_with_fallback(source.as_ref(), &decl, &pts, None).await?;
    let cov = build_point_coverage(
        &pts,
        &slices,
        &["band_1".to_string()],
        &sampled,
        crs,
        "Trajectory",
    );
    Ok(json_response(COVJSON_MEDIA_TYPE, &cov))
}

pub async fn query_corridor(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    Query(query): Query<EdrQueryParams>,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let (line, _) = parse_wkt_trajectory(query.coords.as_deref())?;
    let width_m = parse_within_meters(query.corridor_width, query.width_units.as_deref())?;
    let _height = query.corridor_height;
    let temporal = parse_datetime(query.datetime.as_deref())?;
    let crs = parse_crs(query.crs.as_deref())?;
    let selected = parse_parameter_names(query.parameter_name.as_deref());

    let buf_deg = (width_m / 2.0) / 111_320.0;
    let bbox = line_bbox(&line.0, buf_deg.max(0.001));
    let (decl, source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let slices =
        resolve_sparse_time_slices(&ctx, tenant, catalog, &decl, bbox, temporal.as_ref()).await?;
    let grid = sample_grid_with_fallback(source.as_ref(), &decl, bbox, 64, None).await?;
    let cov = build_grid_coverage(&grid, &slices, crs, selected.as_deref());
    Ok(json_response(COVJSON_MEDIA_TYPE, &cov))
}

fn parse_geojson_coords(arr: &Value) -> Option<Vec<Coord<f64>>> {
    let items = arr.as_array()?;
    let mut coords = Vec::with_capacity(items.len());
    for pt in items {
        let pair = pt.as_array()?;
        let x = pair.first()?.as_f64()?;
        let y = pair.get(1)?.as_f64()?;
        coords.push(Coord { x, y });
    }
    Some(coords)
}

fn parse_geojson_polygon(rings_val: &Value) -> Option<Polygon<f64>> {
    let rings = rings_val.as_array()?;
    let exterior = LineString::new(parse_geojson_coords(rings.first()?)?);
    let mut interiors = Vec::new();
    for ring in rings.iter().skip(1) {
        interiors.push(LineString::new(parse_geojson_coords(ring)?));
    }
    Some(Polygon::new(exterior, interiors))
}

fn geojson_to_geometry(val: &Value) -> Result<Geometry<f64>, ApiError> {
    let geom = if val.get("type").and_then(Value::as_str) == Some("Feature") {
        val.get("geometry")
            .ok_or_else(|| ApiError::bad_request("GeoJSON Feature has null geometry"))?
    } else {
        val
    };
    let ty = geom
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("Missing GeoJSON geometry 'type'"))?;
    let coords = geom
        .get("coordinates")
        .ok_or_else(|| ApiError::bad_request("Missing GeoJSON 'coordinates'"))?;

    match ty {
        "Point" => {
            let arr = coords
                .as_array()
                .ok_or_else(|| ApiError::bad_request("Invalid GeoJSON Point coordinates"))?;
            let x = arr.first().and_then(Value::as_f64);
            let y = arr.get(1).and_then(Value::as_f64);
            match (x, y) {
                (Some(x), Some(y)) => Ok(Geometry::Point(Point::new(x, y))),
                _ => Err(ApiError::bad_request("Invalid GeoJSON Point coordinates")),
            }
        }
        "MultiPoint" => parse_geojson_coords(coords)
            .map(|cs| Geometry::MultiPoint(MultiPoint(cs.into_iter().map(Point::from).collect())))
            .ok_or_else(|| ApiError::bad_request("Invalid GeoJSON MultiPoint coordinates")),
        "LineString" => parse_geojson_coords(coords)
            .map(|cs| Geometry::LineString(LineString::new(cs)))
            .ok_or_else(|| ApiError::bad_request("Invalid GeoJSON LineString coordinates")),
        "Polygon" => parse_geojson_polygon(coords)
            .map(Geometry::Polygon)
            .ok_or_else(|| ApiError::bad_request("Invalid GeoJSON Polygon coordinates")),
        "MultiPolygon" => {
            let polys = coords
                .as_array()
                .and_then(|arr| {
                    arr.iter()
                        .map(parse_geojson_polygon)
                        .collect::<Option<Vec<_>>>()
                })
                .ok_or_else(|| ApiError::bad_request("Invalid GeoJSON MultiPolygon coordinates"))?;
            Ok(Geometry::MultiPolygon(MultiPolygon::new(polys)))
        }
        _ => Err(ApiError::bad_request(format!(
            "Unsupported GeoJSON geometry type '{ty}'"
        ))),
    }
}

fn cql_geom_to_geo(lit: &GeometryLiteral) -> Result<Geometry<f64>, ApiError> {
    match lit {
        GeometryLiteral::Bbox([minx, miny, maxx, maxy]) => Ok(Geometry::Rect(Rect::new(
            Coord { x: *minx, y: *miny },
            Coord { x: *maxx, y: *maxy },
        ))),
        GeometryLiteral::Wkt(wkt_geom) => Geometry::try_from_wkt_str(&wkt_geom.to_wkt_text())
            .map_err(|e| ApiError::bad_request(format!("Invalid CQL2 WKT geometry: {e}"))),
        GeometryLiteral::GeoJson(val) => geojson_to_geometry(val),
    }
}

fn extract_filter_constraints(
    filter: &Filter,
    out_geom: &mut Option<Geometry<f64>>,
    out_temp: &mut Option<TemporalRange>,
) -> Result<(), ApiError> {
    match filter {
        Filter::And(items) => {
            for item in items {
                extract_filter_constraints(item, out_geom, out_temp)?;
            }
        }
        Filter::Intersects { geometry, .. } | Filter::Spatial { geometry, .. } => {
            if out_geom.is_none() {
                *out_geom = Some(cql_geom_to_geo(geometry)?);
            }
        }
        Filter::During { start, end, .. } => {
            if out_temp.is_none() {
                *out_temp = Some(TemporalRange {
                    start: Some(parse_iso_instant(start)?),
                    end: Some(parse_iso_instant(end)?),
                });
            }
        }
        Filter::After { instant, .. } => {
            if out_temp.is_none() {
                *out_temp = Some(TemporalRange {
                    start: Some(parse_iso_instant(instant)?),
                    end: None,
                });
            }
        }
        Filter::Before { instant, .. } => {
            if out_temp.is_none() {
                *out_temp = Some(TemporalRange {
                    start: None,
                    end: Some(parse_iso_instant(instant)?),
                });
            }
        }
        Filter::Temporal {
            op:
                TemporalOp::Intersects
                | TemporalOp::Overlaps
                | TemporalOp::OverlappedBy
                | TemporalOp::Contains
                | TemporalOp::Equals,
            value,
            ..
        } if out_temp.is_none() => {
            let range = match value {
                TemporalValue::Instant(s) => {
                    let dt = parse_iso_instant(s)?;
                    TemporalRange {
                        start: Some(dt),
                        end: Some(dt),
                    }
                }
                TemporalValue::Interval(s, e) => TemporalRange {
                    start: Some(parse_iso_instant(s)?),
                    end: Some(parse_iso_instant(e)?),
                },
            };
            *out_temp = Some(range);
        }
        _ => {}
    }
    Ok(())
}

fn bbox_polygon_geometry(b: [f64; 4]) -> Value {
    json!({
        "type": "Polygon",
        "coordinates": [[[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]], [b[0], b[1]]]]
    })
}

fn slice_to_geojson_feature(s: &TimeSlice, fallback_bbox: [f64; 4]) -> Value {
    let bbox = s.matched_bbox.unwrap_or(fallback_bbox);
    let (datetime_str, start_dt, end_dt, time_obj) = match &s.bounds {
        Some(b) => (
            Value::String(format!("{}/{}", b[0], b[1])),
            Value::String(b[0].clone()),
            Value::String(b[1].clone()),
            json!({ "interval": [b[0], b[1]] }),
        ),
        None => (Value::Null, Value::Null, Value::Null, Value::Null),
    };
    json!({
        "type": "Feature",
        "id": s.label,
        "geometry": bbox_polygon_geometry(bbox),
        "time": time_obj,
        "properties": {
            "datetime": datetime_str,
            "start_datetime": start_dt,
            "end_datetime": end_dt,
            "granule_bbox": s.matched_bbox
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemsRepresentation {
    GeoJson,
    CoverageData,
    DomainProfile,
}

fn header_requests_domain_profile(headers: &HeaderMap, query: &EdrQueryParams) -> bool {
    if let Some(p) = query
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if p == DOMAIN_PROFILE_URI || p.eq_ignore_ascii_case("domain") {
            return true;
        }
    }
    if let Some(ap) = headers.get("accept-profile").and_then(|v| v.to_str().ok()) {
        if ap.contains(DOMAIN_PROFILE_URI) {
            return true;
        }
    }
    if let Some(accept) = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) {
        let lower = accept.to_ascii_lowercase();
        if (lower.contains(COVJSON_MEDIA_TYPE) || lower.contains(COVJSON_LEGACY_MEDIA_TYPE))
            && accept.contains(DOMAIN_PROFILE_URI)
        {
            return true;
        }
    }
    false
}

fn negotiate_items_representation(
    headers: &HeaderMap,
    query: &EdrQueryParams,
) -> Result<ItemsRepresentation, ApiError> {
    if header_requests_domain_profile(headers, query) {
        return Ok(ItemsRepresentation::DomainProfile);
    }
    if let Some(raw) = query.f.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let fmt = OutputFormat::parse(Some(raw))?;
        return Ok(match fmt {
            OutputFormat::CoverageJson => ItemsRepresentation::CoverageData,
            OutputFormat::GeoJson => ItemsRepresentation::GeoJson,
        });
    }
    if let Some(accept) = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) {
        let lower = accept.to_ascii_lowercase();
        if lower.contains(COVJSON_MEDIA_TYPE) || lower.contains(COVJSON_LEGACY_MEDIA_TYPE) {
            return Ok(ItemsRepresentation::CoverageData);
        }
    }
    Ok(ItemsRepresentation::GeoJson)
}

fn with_vary_accept_profile(mut resp: Response) -> Response {
    if let Ok(hv) = HeaderValue::from_str("Accept, Accept-Profile") {
        resp.headers_mut()
            .insert(HeaderName::from_static("vary"), hv);
    }
    resp
}

/// Standard OGC API - Coverages Part 1 `GET /collections/{cid}/coverage` endpoint.
pub async fn query_coverage(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    Query(mut query): Query<EdrQueryParams>,
) -> Result<Response, ApiError> {
    if query.bbox.as_deref().is_none_or(|s| s.trim().is_empty()) {
        query.bbox = Some("-180,-90,180,90".to_string());
    }
    query_cube(State(ctx), Path(params), Query(query)).await
}

/// Standard OGC API - Coverages Part 3 `GET /collections/{cid}/scenes/{itemId}/coverage` endpoint.
pub async fn query_scene_coverage(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    Query(query): Query<EdrQueryParams>,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let item_id = params
        .get("itemId")
        .ok_or_else(|| ApiError::bad_request("Missing scene id"))?;
    let crs = parse_crs(query.crs.as_deref())?;
    let selected = parse_parameter_names(query.parameter_name.as_deref());

    let (decl, source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let world = [-180.0, -90.0, 180.0, 90.0];
    let slices = resolve_sparse_time_slices(&ctx, tenant, catalog, &decl, world, None).await?;
    let matched = slices
        .into_iter()
        .find(|s| s.label == *item_id && s.has_granule)
        .ok_or_else(|| ApiError::not_found(format!("Scene '{item_id}' not found in '{cid}'")))?;
    let bbox = if query.bbox.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        parse_bbox(query.bbox.as_deref())?
    } else {
        matched.matched_bbox.unwrap_or(world)
    };
    let grid = sample_grid_with_fallback(source.as_ref(), &decl, bbox, 64, None).await?;
    let cov = build_grid_coverage(&grid, &[matched], crs, selected.as_deref());
    Ok(json_response(COVJSON_MEDIA_TYPE, &cov))
}

/// Standard OGC API - EDR / Features Part 3 / Coverages Part 3 `GET /collections/{cid}/items`
/// and `/scenes` with CQL2 (`S_INTERSECTS` / `INTERSECTS` / `T_INTERSECTS`) and RFC 6906 `profile`
/// (`Domain`-only) vs. format-orthogonal `f=CoverageJSON` (full extracted Coverage data).
pub async fn list_items(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
    Query(query): Query<EdrQueryParams>,
    headers: HeaderMap,
    uri: OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let mut temporal = parse_datetime(query.datetime.as_deref())?;

    let mut filter_geom: Option<Geometry<f64>> =
        if let Some(wkt_str) = query
            .coords
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(
                Geometry::try_from_wkt_str(wkt_str)
                    .map_err(|e| ApiError::bad_request(format!("Invalid WKT in 'coords': {e}")))?,
            )
        } else if let Some(raw_intersects) = query
            .intersects
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if raw_intersects.starts_with('{') {
                let val: Value = serde_json::from_str(raw_intersects).map_err(|e| {
                    ApiError::bad_request(format!("Invalid GeoJSON in 'intersects': {e}"))
                })?;
                Some(geojson_to_geometry(&val)?)
            } else {
                Some(Geometry::try_from_wkt_str(raw_intersects).map_err(|e| {
                    ApiError::bad_request(format!("Invalid WKT in 'intersects': {e}"))
                })?)
            }
        } else {
            None
        };

    if let Some(raw_filter) = query
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let lang = query.filter_lang.as_deref().unwrap_or("cql2-text");
        let parsed = filter::parse(lang, raw_filter)
            .map_err(|e| ApiError::bad_request(format!("Invalid CQL2 filter: {e}")))?;
        extract_filter_constraints(&parsed, &mut filter_geom, &mut temporal)?;
    }

    let query_bbox = if query.bbox.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        parse_bbox(query.bbox.as_deref())?
    } else if let Some(geom) = &filter_geom {
        let rect = geom.bounding_rect().ok_or_else(|| {
            ApiError::bad_request("Spatial filter geometry has empty bounding box")
        })?;
        [rect.min().x, rect.min().y, rect.max().x, rect.max().y]
    } else {
        [-180.0, -90.0, 180.0, 90.0]
    };

    let (decl, source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let slices = resolve_sparse_time_slices_with_geom(
        &ctx,
        tenant,
        catalog,
        &decl,
        query_bbox,
        filter_geom.as_ref(),
        temporal.as_ref(),
    )
    .await?;

    let mut populated: Vec<TimeSlice> = slices.into_iter().filter(|s| s.has_granule).collect();
    let number_matched = populated.len();
    if let Some(limit) = query.limit {
        if limit == 0 {
            return Err(ApiError::bad_request("limit must be >= 1"));
        }
        populated.truncate(limit as usize);
    }

    match negotiate_items_representation(&headers, &query)? {
        ItemsRepresentation::DomainProfile => {
            let values: Vec<&str> = populated.iter().map(|s| s.label.as_str()).collect();
            let bounds: Vec<&[String; 2]> =
                populated.iter().filter_map(|s| s.bounds.as_ref()).collect();
            let t_axis = if bounds.len() == values.len() && !bounds.is_empty() {
                json!({ "values": values, "bounds": bounds })
            } else {
                json!({ "values": values })
            };
            return Ok(with_vary_accept_profile(json_response(
                COVJSON_DOMAIN_MEDIA_TYPE,
                &json!({
                    "type": "Domain",
                    "domainType": "Grid",
                    "axes": {
                        "t": t_axis,
                        "x": { "start": query_bbox[0], "stop": query_bbox[2], "num": 1 },
                        "y": { "start": query_bbox[1], "stop": query_bbox[3], "num": 1 }
                    },
                    "referencing": [
                        {
                            "coordinates": ["x", "y"],
                            "system": {
                                "type": "GeographicCRS",
                                "id": "http://www.opengis.net/def/crs/OGC/1.3/CRS84"
                            }
                        },
                        {
                            "coordinates": ["t"],
                            "system": {
                                "type": "TemporalRS",
                                "calendar": "Gregorian"
                            }
                        }
                    ]
                }),
            )));
        }
        ItemsRepresentation::CoverageData => {
            let crs = parse_crs(query.crs.as_deref())?;
            let selected = parse_parameter_names(query.parameter_name.as_deref());
            let grid =
                sample_grid_with_fallback(source.as_ref(), &decl, query_bbox, 64, None).await?;
            let cov = build_grid_coverage(&grid, &populated, crs, selected.as_deref());
            return Ok(with_vary_accept_profile(json_response(
                COVJSON_MEDIA_TYPE,
                &cov,
            )));
        }
        ItemsRepresentation::GeoJson => {}
    }

    let items_base = base_path(&uri);
    let coll_base = items_base
        .strip_suffix("/items")
        .or_else(|| items_base.strip_suffix("/scenes"))
        .unwrap_or(&items_base)
        .to_string();
    let features: Vec<Value> = populated
        .iter()
        .map(|s| slice_to_geojson_feature(s, query_bbox))
        .collect();
    Ok(with_vary_accept_profile(json_response(
        GEOJSON_MEDIA_TYPE,
        &json!({
            "type": "FeatureCollection",
            "features": features,
            "numberMatched": number_matched,
            "numberReturned": features.len(),
            "links": [
                Link::new(items_base, "self", GEOJSON_MEDIA_TYPE),
                Link::new(coll_base, "collection", JSON_MEDIA_TYPE)
            ]
        }),
    )))
}

/// Standard OGC API - EDR Part 1 v1.1 `GET /collections/{cid}/items/{itemId}` endpoint.
pub async fn get_item(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    let (tenant, catalog) = extract_scope(&params);
    let cid = params
        .get("cid")
        .ok_or_else(|| ApiError::bad_request("Missing collection id"))?;
    let item_id = params
        .get("itemId")
        .ok_or_else(|| ApiError::bad_request("Missing item id"))?;

    let (decl, _source) = ctx
        .current()
        .router
        .resolve_raster(tenant, catalog, cid)
        .await?;
    let world = [-180.0, -90.0, 180.0, 90.0];
    let slices = resolve_sparse_time_slices(&ctx, tenant, catalog, &decl, world, None).await?;
    let matched = slices
        .iter()
        .find(|s| s.label == *item_id && s.has_granule)
        .ok_or_else(|| ApiError::not_found(format!("Item '{item_id}' not found in '{cid}'")))?;
    Ok(json_response(
        GEOJSON_MEDIA_TYPE,
        &slice_to_geojson_feature(matched, world),
    ))
}
