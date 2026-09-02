#![cfg(feature = "public-demo")]

use std::{
    io::{Cursor, Write},
    ops::Range,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use bytes::Bytes;
use geozero::mvt::{Message, Tile};
use tellurion::public_demo::{
    test_support::{ArchiveSpoolMode, DemoHarness},
    Clock, DemoRegistrar,
};
use tellurion_core::{
    AppConfig, AppContext, CollectionDecl, FeaturePage, FeatureSource, FileStyleStore, ItemsQuery,
    MokaTileCache, Registry, Resolver, Router as CoreRouter, StaticResolver, StyleStore, TileCache,
};
use tellurion_http_source::{
    ContentIdentity, PublicHttpsGateway, RangeObject, SourceError, SourceHandle, SourceSession,
};
use tower::ServiceExt;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

const PARQUET_URL: &str = "https://data.example.test/demo.parquet";
const SHAPEFILE_URL: &str = "https://data.example.test/demo.zip";
const PROJECTED_SHAPEFILE_URL: &str = "https://data.example.test/projected.zip";
const SESSION_TTL: Duration = Duration::from_secs(15 * 60);

struct TestClock(Mutex<Instant>);

impl TestClock {
    fn new() -> Self {
        Self(Mutex::new(Instant::now()))
    }

    fn advance(&self, duration: Duration) {
        *self.0.lock().unwrap() += duration;
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap()
    }
}

struct VectorRangeObject {
    bytes: Bytes,
    handle: SourceHandle,
    identity: ContentIdentity,
    display_name: String,
}

#[async_trait]
impl RangeObject for VectorRangeObject {
    fn handle(&self) -> &SourceHandle {
        &self.handle
    }

    fn identity(&self) -> &ContentIdentity {
        &self.identity
    }

    fn length(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError> {
        Ok(self.bytes.slice(range.start as usize..range.end as usize))
    }
}

struct VectorRegistrar {
    gateway: PublicHttpsGateway,
    opened: Mutex<Vec<SourceSession>>,
    parquet: Bytes,
    shapefile: Bytes,
    projected_shapefile: Bytes,
    sequence: AtomicUsize,
}

impl VectorRegistrar {
    fn new() -> Self {
        Self {
            gateway: PublicHttpsGateway::new(),
            opened: Mutex::new(Vec::new()),
            parquet: Bytes::from_static(include_bytes!(
                "../../tellurion-geoparquet/tests/fixtures/tiny.parquet"
            )),
            shapefile: Bytes::from(shapefile_archive_with_prj(
                b"GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]]",
            )),
            projected_shapefile: Bytes::from(shapefile_archive_with_prj(
                b"PROJCS[\"WGS 84 / Pseudo-Mercator\",GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]],AUTHORITY[\"EPSG\",\"3857\"]]",
            )),
            sequence: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl DemoRegistrar for VectorRegistrar {
    fn open_session(&self) -> SourceSession {
        let session = self.gateway.open_session();
        self.opened.lock().unwrap().push(session.clone());
        session
    }

    async fn register(
        &self,
        session: &SourceSession,
        raw_url: &str,
    ) -> Result<Arc<dyn RangeObject>, ()> {
        if !self
            .opened
            .lock()
            .unwrap()
            .iter()
            .any(|opened| opened.same_session(session))
        {
            return Err(());
        }
        let index = self.sequence.fetch_add(1, Ordering::SeqCst);
        let (bytes, display_name) = if raw_url.ends_with(".parquet") {
            (self.parquet.clone(), "demo.parquet")
        } else if raw_url.ends_with("projected.zip") {
            (self.projected_shapefile.clone(), "projected.zip")
        } else if raw_url.ends_with(".zip") {
            (self.shapefile.clone(), "demo.zip")
        } else {
            return Err(());
        };
        let length = bytes.len() as u64;
        let discriminator = u8::try_from(index % 250 + 1).unwrap();
        Ok(Arc::new(VectorRangeObject {
            bytes,
            handle: SourceHandle::new(format!("vector-{index:025x}")),
            identity: ContentIdentity::StrongEtag {
                source_key: [discriminator; 32],
                revision_key: [discriminator.wrapping_add(1); 32],
                length,
            },
            display_name: display_name.to_owned(),
        }))
    }
}

struct RegisteredVector {
    cookie: String,
    id: String,
    json: serde_json::Value,
}

fn context() -> Arc<AppContext> {
    let config = AppConfig::default();
    config.validate().unwrap();
    let registry = Registry::new();
    let router = CoreRouter::build(&config, &registry).unwrap();
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    Arc::new(AppContext::new(
        config,
        router,
        resolver,
        None,
        cache,
        style_store,
    ))
}

fn harness(
    registrar: Arc<VectorRegistrar>,
    clock: Arc<TestClock>,
    ctx: Arc<AppContext>,
    spool_mode: ArchiveSpoolMode,
) -> DemoHarness {
    DemoHarness::new(registrar, clock, Duration::from_millis(1), ctx, spool_mode).unwrap()
}

fn same_origin_request(method: &str, path: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "demo.example")
        .header(header::ORIGIN, "https://demo.example")
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .unwrap()
}

async fn register_vector(app: &Router, url: &str, format: &str) -> RegisteredVector {
    register_vector_request(app, url, Some(format)).await
}

async fn register_vector_request(
    app: &Router,
    url: &str,
    format: Option<&str>,
) -> RegisteredVector {
    let body = match format {
        Some(format) => serde_json::json!({ "url": url, "format": format }),
        None => serde_json::json!({ "url": url }),
    };
    let response = app
        .clone()
        .oneshot(same_origin_request(
            "POST",
            "/demo/sources",
            Body::from(body.to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    RegisteredVector {
        cookie,
        id: json["id"].as_str().unwrap().to_owned(),
        json,
    }
}

#[tokio::test]
async fn omitted_format_is_inferred_for_supported_vector_extensions() {
    let harness = harness(
        Arc::new(VectorRegistrar::new()),
        Arc::new(TestClock::new()),
        context(),
        ArchiveSpoolMode::Temporary,
    );
    let app = harness.app();

    let parquet = register_vector_request(&app, PARQUET_URL, None).await;
    assert_eq!(parquet.json["format"], "geoparquet");
    assert_eq!(parquet.json["transport"], "range-native");

    let shapefile = register_vector_request(&app, SHAPEFILE_URL, None).await;
    assert_eq!(shapefile.json["format"], "shapefile-zip");
    assert_eq!(shapefile.json["transport"], "bounded-zip-spool");
}

async fn get(app: &Router, path: String, cookie: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn geoparquet_router_is_session_bound_and_leaves_app_context_unchanged() {
    let registrar = Arc::new(VectorRegistrar::new());
    let foreign = PublicHttpsGateway::new().open_session();
    assert!(registrar.register(&foreign, PARQUET_URL).await.is_err());

    let clock = Arc::new(TestClock::new());
    let ctx = context();
    let before = ctx.current();
    assert_eq!(before.router.collection_count(), 0);
    let harness = harness(
        registrar,
        clock,
        Arc::clone(&ctx),
        ArchiveSpoolMode::Temporary,
    );
    let app = harness.app();
    let registered = register_vector(&app, PARQUET_URL, "geoparquet").await;
    assert_eq!(registered.json["format"], "geoparquet");
    assert!(!registered.json.to_string().contains(PARQUET_URL));
    assert_eq!(harness.session_count().await, 1);
    assert_eq!(harness.source_count().await, 1);
    let after = ctx.current();
    assert!(Arc::ptr_eq(&before, &after));
    assert_eq!(after.router.collection_count(), 0);

    let items_path = format!("/demo/sources/{}/items?limit=2", registered.id);
    let response = get(&app, items_path.clone(), &registered.cookie).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/geo+json"
    );
    let page: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(page["numberReturned"], 2);
    assert_eq!(page["numberMatched"], 5);
    assert_eq!(page["features"][0]["id"], "0");
    assert_eq!(page["features"][1]["id"], "1");
    assert_eq!(page["links"][1]["href"], format!("{items_path}&token=1"));

    let bbox = get(
        &app,
        format!(
            "/demo/sources/{}/items?limit=1&bbox=-5,45,1,51",
            registered.id
        ),
        &registered.cookie,
    )
    .await;
    let bbox_page: serde_json::Value =
        serde_json::from_slice(&to_bytes(bbox.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(bbox_page["numberMatched"], 3);

    let item = get(
        &app,
        format!("/demo/sources/{}/items/0", registered.id),
        &registered.cookie,
    )
    .await;
    assert_eq!(item.status(), StatusCode::OK);
    let item: serde_json::Value =
        serde_json::from_slice(&to_bytes(item.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(item["id"], "0");

    let mvt = get(
        &app,
        format!(
            "/demo/sources/{}/tiles/WebMercatorQuad/2/1/1.mvt",
            registered.id
        ),
        &registered.cookie,
    )
    .await;
    assert_eq!(mvt.status(), StatusCode::OK);
    assert_eq!(
        mvt.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/vnd.mapbox-vector-tile"
    );
    let mvt = to_bytes(mvt.into_body(), usize::MAX).await.unwrap();
    let decoded = Tile::decode(mvt.as_ref()).expect("public demo returns valid MVT protobuf");
    assert_eq!(decoded.layers.len(), 1);
    assert_eq!(decoded.layers[0].name, registered.id);
    assert!(!decoded.layers[0].features.is_empty());

    let png = get(
        &app,
        format!(
            "/demo/sources/{}/tiles/WebMercatorQuad/2/1/1.png",
            registered.id
        ),
        &registered.cookie,
    )
    .await;
    assert_eq!(png.status(), StatusCode::OK);
    assert!(to_bytes(png.into_body(), usize::MAX)
        .await
        .unwrap()
        .starts_with(&[0x89, b'P', b'N', b'G']));

    for path in [
        format!("/demo/sources/{}/items?datetime=2024-01-01", registered.id),
        format!("/demo/sources/{}/items/0?crs=EPSG%3A3857", registered.id),
        format!(
            "/demo/sources/{}/tiles/WebMercatorQuad/2/1/1.mvt?filter=name%3Dnorth",
            registered.id
        ),
        format!(
            "/demo/sources/{}/tiles/WebMercatorQuad/2/1/1.png?datetime=2024-01-01",
            registered.id
        ),
    ] {
        let response = get(&app, path, &registered.cookie).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "private, no-store"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains(PARQUET_URL));
    }

    let other = register_vector(&app, SHAPEFILE_URL, "shapefile-zip").await;
    let isolated = get(
        &app,
        format!("/demo/sources/{}/items", registered.id),
        &other.cookie,
    )
    .await;
    assert_eq!(isolated.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn shapefile_router_renders_and_reaper_removes_its_private_spool() {
    let root = tempfile::tempdir().unwrap();
    let registrar = Arc::new(VectorRegistrar::new());
    let clock = Arc::new(TestClock::new());
    let harness = harness(
        registrar,
        Arc::clone(&clock),
        context(),
        ArchiveSpoolMode::Directory(root.path().to_path_buf()),
    );
    let app = harness.app();
    let registered = register_vector(&app, SHAPEFILE_URL, "shapefile-zip").await;

    let items = get(
        &app,
        format!("/demo/sources/{}/items?limit=1", registered.id),
        &registered.cookie,
    )
    .await;
    let page: serde_json::Value =
        serde_json::from_slice(&to_bytes(items.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(page["numberReturned"], 1);
    assert_eq!(page["numberMatched"], 2);
    assert_eq!(page["features"][0]["properties"]["name"], "north");

    for suffix in ["2/1/2.mvt", "2/1/2.png"] {
        let tile = get(
            &app,
            format!(
                "/demo/sources/{}/tiles/WebMercatorQuad/{suffix}",
                registered.id
            ),
            &registered.cookie,
        )
        .await;
        assert_eq!(tile.status(), StatusCode::OK);
        let bytes = to_bytes(tile.into_body(), usize::MAX).await.unwrap();
        if suffix.ends_with(".png") {
            assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        } else {
            assert!(!bytes.is_empty());
        }
    }

    let spool_root = harness.archive_root().unwrap();
    assert!(std::fs::read_dir(spool_root).unwrap().next().is_some());
    clock.advance(SESSION_TTL);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if harness.session_count().await == 0
                && std::fs::read_dir(spool_root).unwrap().next().is_none()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("expired sessions and their spool directories must be reaped");
}

#[tokio::test]
async fn projected_shapefile_and_unavailable_spool_are_fixed_private_refusals() {
    let registrar = Arc::new(VectorRegistrar::new());
    let available = harness(
        registrar,
        Arc::new(TestClock::new()),
        context(),
        ArchiveSpoolMode::Temporary,
    );
    let response = available
        .app()
        .oneshot(same_origin_request(
            "POST",
            "/demo/sources",
            Body::from(
                serde_json::json!({
                    "url": PROJECTED_SHAPEFILE_URL,
                    "format": "shapefile-zip"
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    assert!(
        !String::from_utf8_lossy(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
            .contains(PROJECTED_SHAPEFILE_URL)
    );
    assert_eq!(available.session_count().await, 0);
    assert_eq!(available.source_count().await, 0);

    let unavailable = harness(
        Arc::new(VectorRegistrar::new()),
        Arc::new(TestClock::new()),
        context(),
        ArchiveSpoolMode::Unavailable,
    );
    let response = unavailable
        .app()
        .oneshot(same_origin_request(
            "POST",
            "/demo/sources",
            Body::from(
                serde_json::json!({
                    "url": SHAPEFILE_URL,
                    "format": "shapefile-zip"
                })
                .to_string(),
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "private, no-store"
    );
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    assert!(
        !String::from_utf8_lossy(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
            .contains(SHAPEFILE_URL)
    );
    assert_eq!(unavailable.session_count().await, 0);
}

struct UnknownCountFeatures;

#[async_trait]
impl FeatureSource for UnknownCountFeatures {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        _query: &ItemsQuery,
    ) -> tellurion_core::Result<FeaturePage> {
        Ok(FeaturePage {
            features_geojson: vec![serde_json::json!({
                "type": "Feature",
                "id": "stable",
                "geometry": null,
                "properties": {}
            })],
            number_matched: None,
            next_token: None,
        })
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        _id: &str,
        _filter: Option<&tellurion_core::Filter>,
    ) -> tellurion_core::Result<Option<serde_json::Value>> {
        Ok(None)
    }
}

#[tokio::test]
async fn items_route_omits_an_unknown_number_matched() {
    let harness = harness(
        Arc::new(VectorRegistrar::new()),
        Arc::new(TestClock::new()),
        context(),
        ArchiveSpoolMode::Temporary,
    );
    let app = harness.app();
    let registered = register_vector(&app, PARQUET_URL, "geoparquet").await;
    assert!(
        harness
            .replace_feature_source(
                &registered.cookie,
                &registered.id,
                Arc::new(UnknownCountFeatures),
            )
            .await
    );

    let response = get(
        &app,
        format!("/demo/sources/{}/items", registered.id),
        &registered.cookie,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["numberReturned"], 1);
    assert!(body.get("numberMatched").is_none());
}

fn shapefile_archive_with_prj(prj: &[u8]) -> Vec<u8> {
    let records = [(10.0, 10.0, "north"), (20.0, 20.0, "east")];
    let (shp, shx) = point_shape_files(&records);
    let dbf = point_dbf(&records);
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in [
        ("dataset.shp", shp),
        ("dataset.shx", shx),
        ("dataset.dbf", dbf),
        ("dataset.prj", prj.to_vec()),
    ] {
        writer.start_file(name, options).unwrap();
        writer.write_all(&bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn point_shape_files(records: &[(f64, f64, &str)]) -> (Vec<u8>, Vec<u8>) {
    let bbox = records.iter().fold(
        [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ],
        |[minx, miny, maxx, maxy], (x, y, _)| {
            [minx.min(*x), miny.min(*y), maxx.max(*x), maxy.max(*y)]
        },
    );
    let mut shp = shape_header(100 + records.len() * 28, bbox);
    let mut shx = shape_header(100 + records.len() * 8, bbox);
    let mut offset = 50_u32;
    for (index, (x, y, _)) in records.iter().enumerate() {
        shp.extend_from_slice(&u32::try_from(index + 1).unwrap().to_be_bytes());
        shp.extend_from_slice(&10_u32.to_be_bytes());
        shp.extend_from_slice(&1_i32.to_le_bytes());
        shp.extend_from_slice(&x.to_le_bytes());
        shp.extend_from_slice(&y.to_le_bytes());
        shx.extend_from_slice(&offset.to_be_bytes());
        shx.extend_from_slice(&10_u32.to_be_bytes());
        offset += 14;
    }
    (shp, shx)
}

fn shape_header(byte_len: usize, bbox: [f64; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(100);
    out.extend_from_slice(&9994_i32.to_be_bytes());
    out.extend_from_slice(&[0; 20]);
    out.extend_from_slice(&u32::try_from(byte_len / 2).unwrap().to_be_bytes());
    out.extend_from_slice(&1000_i32.to_le_bytes());
    out.extend_from_slice(&1_i32.to_le_bytes());
    for value in bbox {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&[0; 32]);
    out
}

fn point_dbf(records: &[(f64, f64, &str)]) -> Vec<u8> {
    let mut out = vec![0x03, 126, 1, 1];
    out.extend_from_slice(&u32::try_from(records.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&65_u16.to_le_bytes());
    out.extend_from_slice(&21_u16.to_le_bytes());
    out.extend_from_slice(&[0; 20]);
    let mut field = [0_u8; 32];
    field[..4].copy_from_slice(b"name");
    field[11] = b'C';
    field[16] = 20;
    out.extend_from_slice(&field);
    out.push(0x0d);
    for (_, _, name) in records {
        out.push(b' ');
        out.extend_from_slice(name.as_bytes());
        out.extend(std::iter::repeat_n(b' ', 20 - name.len()));
    }
    out.push(0x1a);
    out
}
