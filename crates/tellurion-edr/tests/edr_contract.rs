use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tellurion_core::{
    AppConfig, AppContext, CatalogSource, CollectionDecl, CoverageGrid, DriverFactory, FeaturePage,
    FeatureSource, FileStyleStore, Filter, ItemsQuery, MokaTileCache, PhysicalCollection,
    RasterSource, RasterWindow, Registry, Resolver, Result as CoreResult, Router as CoreRouter,
    StaticResolver, StorageDecl, StorageDriver, StyleStore, TileCache, TileCoord,
};
use tower::ServiceExt;

/// Combined Feature (granule index S) + Raster (CoverageGrid) test driver
/// simulating a sparse 36-dekad GeoTIFF/COG cube where only a subset of dekads exist.
struct SparseDekadalDriver;

struct SparseDekadalFactory;

impl DriverFactory for SparseDekadalFactory {
    fn name(&self) -> &str {
        "sparse-dekadal"
    }

    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(SparseDekadalDriver))
    }
}

impl StorageDriver for SparseDekadalDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(SparseDekadalDriver)
    }

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::new(SparseDekadalDriver))
    }

    fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
        Some(Arc::new(SparseDekadalDriver))
    }
}

#[async_trait]
impl CatalogSource for SparseDekadalDriver {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: "ndvi_dekadal".to_string(),
            geometry_column: Some("geom".to_string()),
            primary_key: Some("id".to_string()),
            srid: Some(4326),
            geometry_type: Some("POLYGON".to_string()),
        }])
    }
}

#[async_trait]
impl FeatureSource for SparseDekadalDriver {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        // Intentional sparse subset S ⊂ D: only D1 and D3 exist in Jan 2021 (D2 is missing!).
        let all = vec![
            json!({
                "type": "Feature",
                "id": "2021-01-D1",
                "geometry": { "type": "Polygon", "coordinates": [[[12.0, 41.0], [13.0, 41.0], [13.0, 42.0], [12.0, 42.0], [12.0, 41.0]]] },
                "properties": { "dekad_id": "2021-01-D1", "start_datetime": "2021-01-01T00:00:00Z" }
            }),
            json!({
                "type": "Feature",
                "id": "2021-01-D3",
                "geometry": { "type": "Polygon", "coordinates": [[[12.0, 41.0], [13.0, 41.0], [13.0, 42.0], [12.0, 42.0], [12.0, 41.0]]] },
                "properties": { "dekad_id": "2021-01-D3", "start_datetime": "2021-01-21T00:00:00Z" }
            }),
        ];
        // Honor spatial bbox filter (only return if query bbox intersects [12, 41, 13, 42]).
        let features = if let Some(b) = query.bbox {
            if b[2] < 12.0 || b[0] > 13.0 || b[3] < 41.0 || b[1] > 42.0 {
                vec![]
            } else {
                all
            }
        } else {
            all
        };
        Ok(FeaturePage {
            features_geojson: features,
            next_token: None,
            number_matched: None,
        })
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        id: &str,
        _filter: Option<&Filter>,
    ) -> CoreResult<Option<Value>> {
        if id == "2021-01-D1" {
            Ok(Some(json!({
                "type": "Feature",
                "id": "2021-01-D1",
                "geometry": { "type": "Point", "coordinates": [12.5, 41.5] },
                "properties": { "dekad_id": "2021-01-D1" }
            })))
        } else {
            Ok(None)
        }
    }
}

#[async_trait]
impl RasterSource for SparseDekadalDriver {
    async fn raster_tile(
        &self,
        _collection: &CollectionDecl,
        _coord: TileCoord,
    ) -> CoreResult<Option<RasterWindow>> {
        Ok(None)
    }

    async fn sample_points(
        &self,
        _collection: &CollectionDecl,
        points: &[[f64; 2]],
        _band_indices: Option<&[usize]>,
    ) -> CoreResult<Option<Vec<Vec<Option<f64>>>>> {
        Ok(Some(vec![points
            .iter()
            .map(|p| Some(p[0] + p[1]))
            .collect()]))
    }

    async fn sample_grid(
        &self,
        _collection: &CollectionDecl,
        bbox: [f64; 4],
        _max_dim: usize,
        _band_indices: Option<&[usize]>,
    ) -> CoreResult<Option<CoverageGrid>> {
        let w = 4;
        let h = 4;
        Ok(Some(CoverageGrid {
            width: w,
            height: h,
            bbox,
            bands: vec!["ndvi".to_string()],
            values: vec![vec![Some(0.75); w * h]],
            crs: "CRS84".to_string(),
        }))
    }
}

fn test_app() -> axum::Router {
    let yaml = r#"
version: 1
storages:
  - id: sparse
    driver: sparse-dekadal
    url_env: TELLURION_TEST_URL
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: ndvi_dekadal
    title: Sparse Dekadal NDVI Cube
    catalog: default
    storage: sparse
    table: ndvi_dekadal
    geometry: geom
    settings:
      stac:
        cube_dimensions:
          time:
            type: temporal
            extent: ["2021-01-01T00:00:00Z", "2021-12-31T23:59:59Z"]
            size: 36
            variableType: categoricalOrdinal
"#;
    std::env::set_var("TELLURION_TEST_URL", "memory://sparse");
    let config: AppConfig = serde_yaml::from_str(yaml).unwrap();
    let mut registry = Registry::new();
    registry.register(Arc::new(SparseDekadalFactory));
    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    let ctx = Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        None,
        cache,
        style_store,
    ));
    tellurion_edr::standalone_router().with_state(ctx)
}

#[tokio::test]
async fn edr_conformance_collections_and_sparse_cube_with_null_gaps() {
    let app = test_app();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/conformance")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let conf: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 65536).await.unwrap()).unwrap();
    assert!(conf["conformsTo"].as_array().unwrap().len() >= 6);

    // Sparse cube over January 2021: D1 present, D2 missing -> null, D3 present.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/collections/ndvi_dekadal/cube?bbox=12.1,41.1,12.9,41.9&datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        tellurion_edr::COVJSON_MEDIA_TYPE
    );
    let cov: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert_eq!(cov["type"], "Coverage");
    assert_eq!(cov["domain"]["domainType"], "Grid");
    assert_eq!(
        cov["domain"]["axes"]["t"]["values"],
        json!(["2021-01-D1", "2021-01-D2", "2021-01-D3"])
    );
    assert_eq!(
        cov["domain"]["axes"]["t"]["bounds"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let vals = cov["ranges"]["ndvi"]["values"].as_array().unwrap();
    assert_eq!(vals.len(), 3 * 4 * 4);
    // D1 -> 0.75, D2 -> null, D3 -> 0.75
    assert_eq!(vals[0], json!(0.75));
    assert!(
        vals[16].is_null(),
        "missing dekad D2 must emit null in CoverageJSON"
    );
    assert_eq!(vals[32], json!(0.75));
}

#[tokio::test]
async fn edr_area_masks_non_rectangular_polygon_and_supports_advanced_queries() {
    let app = test_app();

    // Diagonal triangle: bottom-right and top-left bbox corners fall outside -> null!
    let uri = "/collections/ndvi_dekadal/area?coords=POLYGON((12.0%2041.0,13.0%2041.0,12.0%2042.0,12.0%2041.0))&datetime=2021-01-01T00:00:00Z/2021-01-10T23:59:59Z";
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cov: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    let vals = cov["ranges"]["ndvi"]["values"].as_array().unwrap();
    assert!(
        vals.iter().any(|v| v.is_null()),
        "polygon masking must null out exterior bbox cells"
    );
    assert!(
        vals.iter().any(|v| !v.is_null()),
        "polygon interior cells must retain values"
    );

    // Position GeoJSON + CRS84/EPSG:3857 + radius + trajectory + corridor + locations
    for path in [
        "/collections/ndvi_dekadal/position?coords=POINT(12.5%2041.5)&f=GeoJSON&crs=EPSG:3857",
        "/collections/ndvi_dekadal/radius?coords=POINT(12.5%2041.5)&within=5&within-units=km&datetime=2021-01-01T00:00:00Z/2021-01-10T23:59:59Z",
        "/collections/ndvi_dekadal/trajectory?coords=LINESTRING(12.2%2041.2,12.8%2041.8)&datetime=2021-01-01T00:00:00Z/2021-01-10T23:59:59Z",
        "/collections/ndvi_dekadal/corridor?coords=LINESTRING(12.2%2041.2,12.8%2041.8)&corridor-width=2&width-units=km&datetime=2021-01-01T00:00:00Z/2021-01-10T23:59:59Z",
        "/collections/ndvi_dekadal/locations",
        "/collections/ndvi_dekadal/locations/2021-01-D1",
        "/collections/ndvi_dekadal/instances",
        "/collections/ndvi_dekadal/items?bbox=12.1,41.1,12.9,41.9&datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z",
    ] {
        let r = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "failed on {path}");
    }

    // Standard /items GeoJSON FeatureCollection on intersecting AOI.
    let resp_in = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/collections/ndvi_dekadal/items?bbox=12.1,41.1,12.9,41.9&datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_in.status(), StatusCode::OK);
    let items_in: Value =
        serde_json::from_slice(&to_bytes(resp_in.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(items_in["type"], "FeatureCollection");
    assert_eq!(items_in["numberReturned"], json!(2));
    let ids_in: Vec<&str> = items_in["features"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["id"].as_str())
        .collect();
    assert_eq!(ids_in, vec!["2021-01-D1", "2021-01-D3"]);

    // CQL2 S_INTERSECTS on /scenes with RFC 6906 Domain profile returns CoverageJSON Domain.
    let resp_domain = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/collections/ndvi_dekadal/scenes?filter=S_INTERSECTS(geom,POLYGON((12.1%2041.1,12.9%2041.1,12.9%2041.9,12.1%2041.9,12.1%2041.1)))&filter-lang=cql2-text&datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z")
                .header(header::ACCEPT, tellurion_edr::COVJSON_DOMAIN_MEDIA_TYPE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_domain.status(), StatusCode::OK);
    assert_eq!(
        resp_domain.headers().get(header::CONTENT_TYPE).unwrap(),
        tellurion_edr::COVJSON_DOMAIN_MEDIA_TYPE
    );
    let dom: Value =
        serde_json::from_slice(&to_bytes(resp_domain.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(dom["type"], "Domain");
    assert_eq!(
        dom["axes"]["t"]["values"],
        json!(["2021-01-D1", "2021-01-D3"])
    );

    // Accept-Profile header on /items also yields CoverageJSON Domain.
    let resp_ap = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/collections/ndvi_dekadal/items?filter=S_INTERSECTS(geom,POLYGON((12.1%2041.1,12.9%2041.1,12.9%2041.9,12.1%2041.9,12.1%2041.1)))&filter-lang=cql2-text&datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z")
                .header("accept-profile", format!("<{}>", tellurion_edr::DOMAIN_PROFILE_URI))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_ap.status(), StatusCode::OK);
    let dom_ap: Value =
        serde_json::from_slice(&to_bytes(resp_ap.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(dom_ap["type"], "Domain");

    // Bare f=CoverageJSON without profile returns full extracted Coverage data (format-orthogonal).
    let resp_covjson = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/collections/ndvi_dekadal/items?filter=S_INTERSECTS(geom,POLYGON((12.1%2041.1,12.9%2041.1,12.9%2041.9,12.1%2041.9,12.1%2041.1)))&filter-lang=cql2-text&datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z&f=CoverageJSON")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_covjson.status(), StatusCode::OK);
    assert_eq!(
        resp_covjson.headers().get(header::CONTENT_TYPE).unwrap(),
        tellurion_edr::COVJSON_MEDIA_TYPE
    );
    let full_cov: Value =
        serde_json::from_slice(&to_bytes(resp_covjson.into_body(), 262144).await.unwrap()).unwrap();
    assert_eq!(full_cov["type"], "Coverage");
    assert!(full_cov["ranges"].is_object());

    // Classic CQL/ECQL INTERSECTS outside footprint returns 0 matching items.
    let resp_out = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/collections/ndvi_dekadal/items?filter=INTERSECTS(geom,POLYGON((0.0%200.0,1.0%200.0,1.0%201.0,0.0%201.0,0.0%200.0)))&datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_out.status(), StatusCode::OK);
    let items_out: Value =
        serde_json::from_slice(&to_bytes(resp_out.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(items_out["numberReturned"], json!(0));

    // Single item lookup: present D1 -> 200, missing D2 -> 404.
    let resp_d1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/collections/ndvi_dekadal/items/2021-01-D1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_d1.status(), StatusCode::OK);
    let resp_d2 = app
        .oneshot(
            Request::builder()
                .uri("/collections/ndvi_dekadal/items/2021-01-D2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_d2.status(), StatusCode::NOT_FOUND);
}
