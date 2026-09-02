//! Ephemeral, anonymous public remote-source inspection routes.
//!
//! This module is intentionally outside the configured control/router graph:
//! a source is held only in this process, only for the browser session that
//! registered it, and is never addressable through a tenant or catalog path.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::extract::{DefaultBodyLimit, OriginalUri, Path, RawQuery};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{middleware, Extension, Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use tellurion_cog::CogDriverFactory;
use tellurion_core::{
    CatalogSource, CollectionDecl, FeatureSource, ItemsQuery, RasterSource, TileCoord, TileSource,
};
use tellurion_geoparquet::{GeoparquetBackend, GeoparquetInput};
use tellurion_http_source::{PublicHttpsGateway, RangeObject, SourceSession};
use tellurion_render::{encode_rgba_to_png, render_mvt_to_png, RenderStyle};
use tellurion_shapefile::{ArchiveLimits, ArchiveSpool, ShapefileBackend};

const COOKIE_NAME: &str = "__Host-tellurion-demo";
const LOCAL_COOKIE_NAME: &str = "tellurion-demo-local";
const ARCHIVE_ROOT_ENV: &str = "TELLURION_PUBLIC_DEMO_ARCHIVE_ROOT";
const SESSION_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_LIVE_SOURCES: usize = 3;
const MAX_CONCURRENT_OPERATIONS: usize = 2;
const OPERATION_QUEUE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BODY_BYTES: usize = 2 * 1024;
const MAX_ZOOM: u8 = 22;
/// Process-wide admission ceiling for anonymous sessions. This keeps expiry
/// bookkeeping and the background sweep bounded even under browser churn.
const MAX_LIVE_SESSIONS: usize = 128;
const REAPER_INTERVAL: Duration = Duration::from_secs(30);
const RENDER_TILE_SIZE_PX: u32 = 256;
const DEFAULT_POINT_RADIUS_PX: f32 = 3.0;
const GEOJSON_MEDIA_TYPE: &str = "application/geo+json";
const MVT_MEDIA_TYPE: &str = "application/vnd.mapbox-vector-tile";

#[derive(Clone)]
struct DemoRegistry {
    registrar: Arc<dyn DemoRegistrar>,
    state: Arc<DemoState>,
}

struct DemoState {
    clock: Arc<dyn Clock>,
    sessions: Mutex<HashMap<String, Arc<DemoSession>>>,
    session_slots: Arc<Semaphore>,
    archive_spool: Option<DemoArchiveSpool>,
}

struct DemoArchiveSpool {
    spool: ArchiveSpool,
    root: DemoArchiveRoot,
}

enum DemoArchiveRoot {
    Temporary(tempfile::TempDir),
    External(std::path::PathBuf),
}

impl DemoArchiveRoot {
    fn path(&self) -> &std::path::Path {
        match self {
            Self::Temporary(root) => root.path(),
            Self::External(root) => root,
        }
    }
}

impl DemoArchiveSpool {
    fn configured(root: Option<std::path::PathBuf>) -> Result<Option<Self>, &'static str> {
        let Some(parent) = root else {
            return Ok(Self::new());
        };
        std::fs::create_dir_all(&parent).map_err(|_| "archive spool parent unavailable")?;
        let root = tempfile::Builder::new()
            .prefix("tellurion-public-shapefile-")
            .tempdir_in(parent)
            .map_err(|_| "archive spool root unavailable")?;
        let spool = ArchiveSpool::new(root.path(), ArchiveLimits::default())
            .map_err(|_| "archive spool unavailable")?;
        Ok(Some(Self {
            spool,
            root: DemoArchiveRoot::Temporary(root),
        }))
    }

    fn new() -> Option<Self> {
        let root = tempfile::Builder::new()
            .prefix("tellurion-public-shapefile-")
            .tempdir()
            .ok()?;
        let spool = ArchiveSpool::new(root.path(), ArchiveLimits::default()).ok()?;
        Some(Self {
            spool,
            root: DemoArchiveRoot::Temporary(root),
        })
    }

    fn in_directory(root: std::path::PathBuf) -> Option<Self> {
        let spool = ArchiveSpool::new(&root, ArchiveLimits::default()).ok()?;
        Some(Self {
            spool,
            root: DemoArchiveRoot::External(root),
        })
    }
}

#[doc(hidden)]
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

#[async_trait]
#[doc(hidden)]
pub trait DemoRegistrar: Send + Sync {
    fn open_session(&self) -> SourceSession;
    async fn register(
        &self,
        session: &SourceSession,
        raw_url: &str,
    ) -> Result<Arc<dyn RangeObject>, ()>;
}

struct GatewayRegistrar(PublicHttpsGateway);

#[async_trait]
impl DemoRegistrar for GatewayRegistrar {
    fn open_session(&self) -> SourceSession {
        self.0.open_session()
    }

    async fn register(
        &self,
        session: &SourceSession,
        raw_url: &str,
    ) -> Result<Arc<dyn RangeObject>, ()> {
        self.0
            .register_range_object(session, raw_url)
            .await
            .map_err(|_| ())
    }
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

struct DemoSession {
    _slot: OwnedSemaphorePermit,
    source_session: SourceSession,
    created_at: Instant,
    operations: Arc<Semaphore>,
    sources: Mutex<HashMap<String, Arc<DemoSource>>>,
}

struct DemoSource {
    raster: Option<Arc<dyn RasterSource>>,
    features: Option<Arc<dyn FeatureSource>>,
    tiles: Option<Arc<dyn TileSource>>,
    collection: CollectionDecl,
    metadata: SourceMetadata,
}

#[derive(Clone, Serialize)]
struct SourceMetadata {
    id: String,
    format: &'static str,
    transport: &'static str,
    revision: &'static str,
    capability_state: &'static str,
    extent: Option<[f64; 4]>,
    geometry_type: Option<String>,
    srid: Option<i32>,
    number_matched: Option<u64>,
    properties: Vec<String>,
    attribution: &'static str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterRequest {
    url: String,
    #[serde(default)]
    format: Option<SourceFormat>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SourceFormat {
    Cog,
    Geoparquet,
    ShapefileZip,
}

#[derive(Serialize)]
struct SourceResponse {
    #[serde(flatten)]
    metadata: SourceMetadata,
    limits: DemoLimits,
    links: DemoLinks,
}

#[derive(Serialize)]
struct DemoLimits {
    expires_in_seconds: u64,
    max_live_sources: usize,
    max_concurrent_operations: usize,
}

#[derive(Serialize)]
struct DemoLinks {
    self_href: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    items_href: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mvt_tile_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tile_template: Option<String>,
}

impl DemoRegistry {
    fn new() -> Self {
        let archive_root = std::env::var_os(ARCHIVE_ROOT_ENV).map(std::path::PathBuf::from);
        Self::with_components_reaper_and_spool(
            Arc::new(GatewayRegistrar(PublicHttpsGateway::new())),
            Arc::new(SystemClock),
            REAPER_INTERVAL,
            DemoArchiveSpool::configured(archive_root)
                .expect("configured public demo archive root must be writable"),
        )
    }

    // Kept private so deterministic tests can control expiry without opening
    // a public path to supply a RangeObject or modify the gateway transport.
    #[cfg(test)]
    fn with_components(registrar: Arc<dyn DemoRegistrar>, clock: Arc<dyn Clock>) -> Self {
        Self::with_components_and_reaper(registrar, clock, REAPER_INTERVAL)
    }

    #[cfg(test)]
    fn with_components_and_reaper(
        registrar: Arc<dyn DemoRegistrar>,
        clock: Arc<dyn Clock>,
        interval: Duration,
    ) -> Self {
        Self::with_components_reaper_and_spool(registrar, clock, interval, DemoArchiveSpool::new())
    }

    fn with_components_reaper_and_spool(
        registrar: Arc<dyn DemoRegistrar>,
        clock: Arc<dyn Clock>,
        interval: Duration,
        archive_spool: Option<DemoArchiveSpool>,
    ) -> Self {
        let registry = Self {
            registrar,
            state: Arc::new(DemoState {
                clock,
                sessions: Mutex::new(HashMap::new()),
                session_slots: Arc::new(Semaphore::new(MAX_LIVE_SESSIONS)),
                archive_spool,
            }),
        };
        registry.start_reaper(interval);
        registry
    }

    fn start_reaper(&self, interval: Duration) {
        let state = Arc::downgrade(&self.state);
        // The task owns only a Weak reference, so it ends when the router and
        // its registry are dropped rather than keeping retired demo state alive.
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(state) = state.upgrade() else {
                    return;
                };
                let now = state.clock.now();
                let mut sessions = state.sessions.lock().await;
                // Session admission is capped, making this sweep bounded.
                sessions.retain(|_, session| !expired(session, now));
                drop(sessions);
                if let Some(archive) = &state.archive_spool {
                    archive.spool.cleanup_unused().await;
                }
            }
        });
    }

    async fn existing_session(&self, headers: &HeaderMap) -> Option<(String, Arc<DemoSession>)> {
        let now = self.state.clock.now();
        let supplied = session_cookie(headers)?;
        let mut sessions = self.state.sessions.lock().await;
        sessions.retain(|_, session| !expired(session, now));
        sessions
            .get(&supplied)
            .map(|session| (supplied, Arc::clone(session)))
    }

    async fn reserve_session(&self) -> Result<(String, Arc<DemoSession>), ()> {
        let now = self.state.clock.now();
        self.state
            .sessions
            .lock()
            .await
            .retain(|_, session| !expired(session, now));
        let slot = Arc::clone(&self.state.session_slots)
            .try_acquire_owned()
            .map_err(|_| ())?;

        let id = Uuid::new_v4().simple().to_string();
        let session = Arc::new(DemoSession {
            _slot: slot,
            source_session: self.registrar.open_session(),
            created_at: now,
            operations: Arc::new(Semaphore::new(MAX_CONCURRENT_OPERATIONS)),
            sources: Mutex::new(HashMap::new()),
        });
        Ok((id, session))
    }

    async fn publish_session(&self, id: String, session: Arc<DemoSession>) {
        self.state.sessions.lock().await.insert(id, session);
    }

    async fn source_for_request(
        &self,
        headers: &HeaderMap,
        source_id: &str,
    ) -> Option<(Arc<DemoSession>, Arc<DemoSource>)> {
        let (_, session) = self.existing_session(headers).await?;
        let sources = session.sources.lock().await;
        let source = Arc::clone(sources.get(source_id)?);
        drop(sources);
        Some((session, source))
    }
}

fn expired(session: &DemoSession, now: Instant) -> bool {
    now.saturating_duration_since(session.created_at) >= SESSION_TTL
}

/// Returns the feature-gated demo routes. The caller merges this router into
/// the server only when `public-demo` is enabled.
pub fn router() -> Router<Arc<tellurion_core::AppContext>> {
    router_with_registry(DemoRegistry::new())
}

fn router_with_registry(registry: DemoRegistry) -> Router<Arc<tellurion_core::AppContext>> {
    Router::new()
        .route("/demo/sources", post(register_source))
        .route(
            "/demo/sources/{source_id}",
            get(get_source).delete(delete_source),
        )
        .route("/demo/sources/{source_id}/items", get(list_items))
        .route(
            "/demo/sources/{source_id}/items/{feature_id}",
            get(get_item),
        )
        .route(
            "/demo/sources/{source_id}/tiles/WebMercatorQuad/{z}/{y}/{x}",
            get(tile),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(Extension(registry))
        .layer(middleware::from_fn(always_private))
}

async fn register_source(
    Extension(registry): Extension<DemoRegistry>,
    headers: HeaderMap,
    OriginalUri(request_uri): OriginalUri,
    Json(body): Json<RegisterRequest>,
) -> Response {
    let Some(cookie_kind) = same_origin_kind(&headers) else {
        return private(StatusCode::FORBIDDEN.into_response());
    };
    let (session_id, session, minted) = match registry.existing_session(&headers).await {
        Some((id, session)) => (id, session, false),
        None => match registry.reserve_session().await {
            Ok((id, session)) => (id, session, true),
            Err(()) => return private(StatusCode::TOO_MANY_REQUESTS.into_response()),
        },
    };
    let response = register_source_inner(&registry, &session, request_uri.path(), body).await;
    let registered = response.status().is_success();
    if minted && registered {
        registry
            .publish_session(session_id.clone(), Arc::clone(&session))
            .await;
    }
    let mut response = private(response);
    if minted && registered {
        set_session_cookie(response.headers_mut(), &session_id, cookie_kind);
    }
    response
}

async fn register_source_inner(
    registry: &DemoRegistry,
    session: &Arc<DemoSession>,
    request_path: &str,
    body: RegisterRequest,
) -> Response {
    if body.url.len() > MAX_BODY_BYTES {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    if session.sources.lock().await.len() >= MAX_LIVE_SOURCES {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let Ok(_permit) = acquire_operation(session).await else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };

    let Some(format) = body.format.or_else(|| detect_source_format(&body.url)) else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    if matches!(format, SourceFormat::ShapefileZip) && registry.state.archive_spool.is_none() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    // The gateway keeps the locator private. This module deliberately maps
    // every registration failure to a fixed status, never a driver/gateway
    // error whose wording could contain a user-supplied URL.
    let Ok(object) = registry
        .registrar
        .register(&session.source_session, &body.url)
        .await
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let Ok(source) = inspect_source(registry, format, object).await else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    let source_id = source.metadata.id.clone();
    let mut sources = session.sources.lock().await;
    if sources.len() >= MAX_LIVE_SOURCES {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let metadata = source.metadata.clone();
    sources.insert(source_id, Arc::new(source));
    source_response(
        &metadata,
        request_path,
        registry.state.clock.now(),
        session.created_at,
    )
}

fn detect_source_format(raw_url: &str) -> Option<SourceFormat> {
    let name = raw_url.rsplit('/').next()?.to_ascii_lowercase();
    if name.ends_with(".parquet") {
        Some(SourceFormat::Geoparquet)
    } else if name.ends_with(".zip") {
        Some(SourceFormat::ShapefileZip)
    } else if name.ends_with(".tif") || name.ends_with(".tiff") {
        Some(SourceFormat::Cog)
    } else {
        None
    }
}

async fn inspect_source(
    registry: &DemoRegistry,
    format: SourceFormat,
    object: Arc<dyn RangeObject>,
) -> Result<DemoSource, ()> {
    match format {
        SourceFormat::Cog => inspect_cog(object).await,
        SourceFormat::Geoparquet => {
            let backend = Arc::new(GeoparquetBackend::from_input(GeoparquetInput::Remote(
                Arc::clone(&object),
            )));
            inspect_vector(
                object,
                Arc::clone(&backend) as Arc<dyn CatalogSource>,
                backend.clone() as Arc<dyn FeatureSource>,
                backend as Arc<dyn TileSource>,
                "geoparquet",
                "range-native",
            )
            .await
        }
        SourceFormat::ShapefileZip => {
            let archive = registry.state.archive_spool.as_ref().ok_or(())?;
            let files = archive
                .spool
                .materialize(object.clone())
                .await
                .map_err(|_| ())?;
            let backend = Arc::new(ShapefileBackend::new(files));
            inspect_vector(
                object,
                Arc::clone(&backend) as Arc<dyn CatalogSource>,
                backend.clone() as Arc<dyn FeatureSource>,
                backend as Arc<dyn TileSource>,
                "shapefile-zip",
                "bounded-zip-spool",
            )
            .await
        }
    }
}

async fn inspect_vector(
    object: Arc<dyn RangeObject>,
    catalog: Arc<dyn CatalogSource>,
    features: Arc<dyn FeatureSource>,
    tiles: Arc<dyn TileSource>,
    format: &'static str,
    transport: &'static str,
) -> Result<DemoSource, ()> {
    let mut collections = catalog.collections().await.map_err(|_| ())?;
    if collections.len() != 1 {
        return Err(());
    }
    let physical = collections.pop().ok_or(())?;
    if physical.srid != Some(4326) {
        return Err(());
    }
    let extent = catalog
        .extent(&physical)
        .await
        .map_err(|_| ())?
        .map(|value| value.bbox);
    let number_matched = catalog.row_estimate(&physical).await.map_err(|_| ())?;
    let attributes = catalog
        .attribute_schema(&physical)
        .await
        .map_err(|_| ())?
        .unwrap_or_default();
    let id = object.handle().as_str().to_owned();
    let mut collection: CollectionDecl = serde_json::from_value(serde_json::json!({
        "id": id,
        "catalog": "demo",
        "storage": "ephemeral",
        "table": physical.name,
        "geometry": physical.geometry_column,
        "pk": physical.primary_key,
        "tiles": { "minzoom": 0, "maxzoom": MAX_ZOOM, "caps": {} }
    }))
    .map_err(|_| ())?;
    collection.row_estimate = number_matched;
    collection.srid = physical.srid;
    collection.attribute_columns = Some(attributes.clone());
    Ok(DemoSource {
        raster: None,
        features: Some(features),
        tiles: Some(tiles),
        collection,
        metadata: SourceMetadata {
            id,
            format,
            transport,
            revision: "strong",
            capability_state: "ready",
            extent,
            geometry_type: physical.geometry_type,
            srid: physical.srid,
            number_matched,
            properties: attributes.into_iter().map(|column| column.name).collect(),
            attribution: "Remote source supplied by this browser session",
        },
    })
}

async fn inspect_cog(object: Arc<dyn RangeObject>) -> Result<DemoSource, ()> {
    let driver = CogDriverFactory::new().build_range_object(Arc::clone(&object));
    let catalog = driver.catalog_source();
    let mut physical = catalog.collections().await.map_err(|_| ())?;
    let physical = physical.pop().ok_or(())?;
    let extent = catalog
        .extent(&physical)
        .await
        .map_err(|_| ())?
        .map(|value| value.bbox);
    let raster = driver.raster_source().ok_or(())?;
    let id = object.handle().as_str().to_owned();
    let collection: CollectionDecl = serde_json::from_value(serde_json::json!({
        "id": id,
        "kind": "raster",
        "catalog": "demo",
        "storage": "ephemeral",
        "tiles": { "minzoom": 0, "maxzoom": MAX_ZOOM, "caps": {} }
    }))
    .map_err(|_| ())?;
    Ok(DemoSource {
        raster: Some(raster),
        features: None,
        tiles: None,
        collection,
        metadata: SourceMetadata {
            id,
            // Metadata parsing validates a tiled GeoTIFF, but not every COG
            // layout invariant. Calling it a full COG would overclaim.
            format: "tiled-geotiff",
            transport: "range-native",
            revision: "strong",
            capability_state: "ready",
            extent,
            geometry_type: None,
            srid: None,
            number_matched: None,
            properties: Vec::new(),
            attribution: "Remote source supplied by this browser session",
        },
    })
}

async fn get_source(
    Extension(registry): Extension<DemoRegistry>,
    headers: HeaderMap,
    Path(source_id): Path<String>,
    OriginalUri(request_uri): OriginalUri,
) -> Response {
    let Some((session, source)) = registry.source_for_request(&headers, &source_id).await else {
        return private(StatusCode::NOT_FOUND.into_response());
    };
    private(source_response(
        &source.metadata,
        request_uri.path(),
        registry.state.clock.now(),
        session.created_at,
    ))
}

async fn delete_source(
    Extension(registry): Extension<DemoRegistry>,
    headers: HeaderMap,
    Path(source_id): Path<String>,
) -> Response {
    if !same_origin(&headers) {
        return private(StatusCode::FORBIDDEN.into_response());
    }
    let Some((_, session)) = registry.existing_session(&headers).await else {
        return private(StatusCode::NOT_FOUND.into_response());
    };
    let removed = session.sources.lock().await.remove(&source_id);
    private(if removed.is_some() {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    })
}

struct DemoItemsQuery {
    query: ItemsQuery,
    limit: Option<u32>,
    bbox: Option<String>,
}

fn parse_items_query(raw: Option<&str>) -> Result<DemoItemsQuery, ()> {
    let mut limit = None;
    let mut bbox = None;
    let mut token = None;
    for (name, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        match name.as_ref() {
            "limit" if limit.is_none() => {
                let parsed = value.parse::<u32>().map_err(|_| ())?;
                if !(1..=tellurion_features::MAX_LIMIT).contains(&parsed) {
                    return Err(());
                }
                limit = Some(parsed);
            }
            "bbox" if bbox.is_none() => {
                tellurion_core::query_params::parse_bbox(&value).map_err(|_| ())?;
                bbox = Some(value.into_owned());
            }
            "token" if token.is_none() => {
                if value.is_empty()
                    || value.len() > 32
                    || !value.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Err(());
                }
                token = Some(value.into_owned());
            }
            _ => return Err(()),
        }
    }
    let parsed_bbox = bbox
        .as_deref()
        .map(tellurion_core::query_params::parse_bbox)
        .transpose()
        .map_err(|_| ())?;
    Ok(DemoItemsQuery {
        query: ItemsQuery {
            limit: limit.unwrap_or(tellurion_features::DEFAULT_LIMIT),
            bbox: parsed_bbox,
            token,
            ..ItemsQuery::default()
        },
        limit,
        bbox,
    })
}

fn items_href(
    path: &str,
    query: &DemoItemsQuery,
    token: Option<&str>,
    include_current_token: bool,
) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    if let Some(limit) = query.limit {
        serializer.append_pair("limit", &limit.to_string());
    }
    if let Some(bbox) = &query.bbox {
        serializer.append_pair("bbox", bbox);
    }
    let token = token.or_else(|| {
        include_current_token
            .then_some(query.query.token.as_deref())
            .flatten()
    });
    if let Some(token) = token {
        serializer.append_pair("token", token);
    }
    let encoded = serializer.finish();
    if encoded.is_empty() {
        path.to_owned()
    } else {
        format!("{path}?{encoded}")
    }
}

#[derive(Serialize)]
struct DemoLink {
    href: String,
    rel: &'static str,
    #[serde(rename = "type")]
    media_type: &'static str,
}

#[derive(Serialize)]
struct DemoFeatureCollection {
    #[serde(rename = "type")]
    type_: &'static str,
    #[serde(rename = "numberReturned")]
    number_returned: u64,
    #[serde(rename = "numberMatched", skip_serializing_if = "Option::is_none")]
    number_matched: Option<u64>,
    features: Vec<serde_json::Value>,
    links: Vec<DemoLink>,
}

async fn list_items(
    Extension(registry): Extension<DemoRegistry>,
    headers: HeaderMap,
    Path(source_id): Path<String>,
    OriginalUri(request_uri): OriginalUri,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let Some((session, source)) = registry.source_for_request(&headers, &source_id).await else {
        return private(StatusCode::NOT_FOUND.into_response());
    };
    let Some(features) = &source.features else {
        return private(StatusCode::NOT_FOUND.into_response());
    };
    let Ok(query) = parse_items_query(raw_query.as_deref()) else {
        return private(StatusCode::BAD_REQUEST.into_response());
    };
    let Ok(_permit) = acquire_operation(&session).await else {
        return private(StatusCode::TOO_MANY_REQUESTS.into_response());
    };
    let Ok(page) = features.items(&source.collection, &query.query).await else {
        return private(StatusCode::BAD_GATEWAY.into_response());
    };
    let path = request_uri.path();
    let mut links = vec![DemoLink {
        href: items_href(path, &query, None, true),
        rel: "self",
        media_type: GEOJSON_MEDIA_TYPE,
    }];
    if let Some(next) = page.next_token.as_deref() {
        links.push(DemoLink {
            href: items_href(path, &query, Some(next), false),
            rel: "next",
            media_type: GEOJSON_MEDIA_TYPE,
        });
    }
    let mut response = Json(DemoFeatureCollection {
        type_: "FeatureCollection",
        number_returned: page.features_geojson.len() as u64,
        number_matched: page.number_matched,
        features: page.features_geojson,
        links,
    })
    .into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(GEOJSON_MEDIA_TYPE),
    );
    private(response)
}

async fn get_item(
    Extension(registry): Extension<DemoRegistry>,
    headers: HeaderMap,
    Path((source_id, feature_id)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let Some((session, source)) = registry.source_for_request(&headers, &source_id).await else {
        return private(StatusCode::NOT_FOUND.into_response());
    };
    let Some(features) = &source.features else {
        return private(StatusCode::NOT_FOUND.into_response());
    };
    if raw_query.is_some_and(|query| !query.is_empty()) {
        return private(StatusCode::BAD_REQUEST.into_response());
    }
    if feature_id.is_empty()
        || feature_id.len() > 32
        || !feature_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return private(StatusCode::BAD_REQUEST.into_response());
    }
    let Ok(_permit) = acquire_operation(&session).await else {
        return private(StatusCode::TOO_MANY_REQUESTS.into_response());
    };
    let item = match features.item(&source.collection, &feature_id, None).await {
        Ok(Some(item)) => item,
        Ok(None) => return private(StatusCode::NOT_FOUND.into_response()),
        Err(_) => return private(StatusCode::BAD_GATEWAY.into_response()),
    };
    let mut response = Json(item).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(GEOJSON_MEDIA_TYPE),
    );
    private(response)
}

#[derive(Clone, Copy)]
enum DemoTileFormat {
    Mvt,
    Png,
}

fn parse_tile_request(z: &str, y: &str, x: &str) -> Result<(TileCoord, DemoTileFormat), ()> {
    let (column, format) = if let Some(column) = x.strip_suffix(".mvt") {
        (column, DemoTileFormat::Mvt)
    } else if let Some(column) = x.strip_suffix(".png") {
        (column, DemoTileFormat::Png)
    } else {
        return Err(());
    };
    parse_web_mercator_quad(z, y, column)
        .map(|coord| (coord, format))
        .ok_or(())
}

async fn tile(
    Extension(registry): Extension<DemoRegistry>,
    headers: HeaderMap,
    Path((source_id, z, y, x)): Path<(String, String, String, String)>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let Some((session, source)) = registry.source_for_request(&headers, &source_id).await else {
        return private(StatusCode::NOT_FOUND.into_response());
    };
    if raw_query.is_some_and(|query| !query.is_empty()) {
        return private(StatusCode::BAD_REQUEST.into_response());
    }
    let Ok(_permit) = acquire_operation(&session).await else {
        return private(StatusCode::TOO_MANY_REQUESTS.into_response());
    };
    let Ok((coord, format)) = parse_tile_request(&z, &y, &x) else {
        return private(StatusCode::BAD_REQUEST.into_response());
    };
    let response = if let Some(raster) = &source.raster {
        if matches!(format, DemoTileFormat::Mvt) {
            return private(StatusCode::BAD_REQUEST.into_response());
        }
        raster_tile_response(raster, &source.collection, coord).await
    } else if let Some(tiles) = &source.tiles {
        vector_tile_response(tiles, &source.collection, coord, format).await
    } else {
        StatusCode::NOT_FOUND.into_response()
    };
    private(response)
}

async fn raster_tile_response(
    raster: &Arc<dyn RasterSource>,
    collection: &CollectionDecl,
    coord: TileCoord,
) -> Response {
    match raster.raster_tile(collection, coord).await {
        Ok(Some(window)) => match tokio::task::spawn_blocking(move || {
            encode_rgba_to_png(&window.rgba, window.width, window.height)
        })
        .await
        {
            Ok(Ok(png)) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, HeaderValue::from_static("image/png"))],
                png,
            )
                .into_response(),
            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

async fn vector_tile_response(
    tiles: &Arc<dyn TileSource>,
    collection: &CollectionDecl,
    coord: TileCoord,
    format: DemoTileFormat,
) -> Response {
    let bytes = match tiles.mvt_tile(collection, coord, None).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(tellurion_core::Error::Invalid(_))
        | Err(tellurion_core::Error::CapabilityUnsupported { .. }) => {
            return StatusCode::UNPROCESSABLE_ENTITY.into_response()
        }
        Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
    };
    match format {
        DemoTileFormat::Mvt => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static(MVT_MEDIA_TYPE),
            )],
            bytes,
        )
            .into_response(),
        DemoTileFormat::Png => {
            let style = match RenderStyle::new(
                &collection.style.fill,
                &collection.style.stroke,
                collection.style.stroke_width as f32,
                DEFAULT_POINT_RADIUS_PX,
            ) {
                Ok(style) => style,
                Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };
            match tokio::task::spawn_blocking(move || {
                render_mvt_to_png(bytes.as_ref(), &style, RENDER_TILE_SIZE_PX)
            })
            .await
            {
                Ok(Ok(png)) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, HeaderValue::from_static("image/png"))],
                    png,
                )
                    .into_response(),
                _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    }
}

async fn acquire_operation(session: &DemoSession) -> Result<OwnedSemaphorePermit, ()> {
    // Browsers request a viewport worth of raster or vector tiles together.
    // Keep only two source operations active, but let that bounded burst wait
    // behind them instead of turning ordinary map startup into missing tiles.
    // The outer server concurrency limit bounds the number of queued requests.
    tokio::time::timeout(
        OPERATION_QUEUE_TIMEOUT,
        session.operations.clone().acquire_owned(),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}

fn parse_web_mercator_quad(z: &str, y: &str, x: &str) -> Option<TileCoord> {
    let z: u8 = z.parse().ok()?;
    if z > MAX_ZOOM {
        return None;
    }
    let y: u32 = y.parse().ok()?;
    let x: u32 = x.parse().ok()?;
    let width = 1_u64.checked_shl(u32::from(z))?;
    (u64::from(x) < width && u64::from(y) < width).then_some(TileCoord { z, x, y })
}

fn source_response(
    metadata: &SourceMetadata,
    request_path: &str,
    now: Instant,
    created_at: Instant,
) -> Response {
    let source_id = &metadata.id;
    let self_href = if request_path.ends_with(source_id) {
        request_path.to_owned()
    } else {
        format!("/demo/sources/{source_id}")
    };
    let remaining = SESSION_TTL.saturating_sub(now.saturating_duration_since(created_at));
    let base = format!("/demo/sources/{source_id}");
    let vector = metadata.geometry_type.is_some();
    Json(SourceResponse {
        metadata: metadata.clone(),
        limits: DemoLimits {
            expires_in_seconds: remaining.as_secs(),
            max_live_sources: MAX_LIVE_SOURCES,
            max_concurrent_operations: MAX_CONCURRENT_OPERATIONS,
        },
        links: DemoLinks {
            self_href,
            items_href: vector.then(|| format!("{base}/items")),
            item_template: vector.then(|| format!("{base}/items/{{featureId}}")),
            mvt_tile_template: vector
                .then(|| format!("{base}/tiles/WebMercatorQuad/{{z}}/{{y}}/{{x}}.mvt")),
            tile_template: Some(format!(
                "{base}/tiles/WebMercatorQuad/{{z}}/{{y}}/{{x}}.png"
            )),
        },
    })
    .into_response()
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let local_allowed = exactly_one_header(headers, header::HOST)
        .and_then(|host| canonical_authority(host, 443))
        .is_some_and(|(host, _)| is_loopback_host(&host));
    [COOKIE_NAME, LOCAL_COOKIE_NAME]
        .into_iter()
        .take(if local_allowed { 2 } else { 1 })
        .find_map(|accepted_name| {
            headers
                .get_all(header::COOKIE)
                .iter()
                .filter_map(|header| header.to_str().ok())
                .flat_map(|value| value.split(';'))
                .filter_map(|part| part.trim().split_once('='))
                .find_map(|(name, value)| {
                    (name == accepted_name && valid_session_id(value)).then(|| value.to_owned())
                })
        })
}

fn valid_session_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn set_session_cookie(headers: &mut HeaderMap, session_id: &str, kind: SessionCookieKind) {
    let value = match kind {
        SessionCookieKind::Secure => {
            format!("{COOKIE_NAME}={session_id}; Path=/; Secure; HttpOnly; SameSite=Strict")
        }
        SessionCookieKind::LoopbackHttp => {
            format!("{LOCAL_COOKIE_NAME}={session_id}; Path=/; HttpOnly; SameSite=Strict")
        }
    };
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::try_from(value).expect("fixed cookie syntax"),
    );
}

fn same_origin(headers: &HeaderMap) -> bool {
    same_origin_kind(headers).is_some()
}

#[derive(Clone, Copy)]
enum SessionCookieKind {
    Secure,
    LoopbackHttp,
}

fn same_origin_kind(headers: &HeaderMap) -> Option<SessionCookieKind> {
    let origin = exactly_one_header(headers, header::ORIGIN)?;
    let host = exactly_one_header(headers, header::HOST)?;
    if origin.contains('?') || origin.contains('#') {
        return None;
    }
    let Ok(origin) = origin.parse::<axum::http::Uri>() else {
        return None;
    };
    let (default_port, kind) = match origin.scheme_str() {
        Some(scheme) if scheme.eq_ignore_ascii_case("https") => (443, SessionCookieKind::Secure),
        Some(scheme) if scheme.eq_ignore_ascii_case("http") => {
            (80, SessionCookieKind::LoopbackHttp)
        }
        _ => return None,
    };
    if origin.path() != "/" {
        return None;
    }
    let origin_authority = origin.authority()?;
    let origin_authority = canonical_authority(origin_authority.as_str(), default_port)?;
    if default_port == 80 && !is_loopback_host(&origin_authority.0) {
        return None;
    }
    (Some(origin_authority) == canonical_authority(host, default_port)).then_some(kind)
}

fn exactly_one_header(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

fn canonical_authority(value: &str, default_port: u16) -> Option<(String, u16)> {
    if value.contains('@') || value.is_empty() {
        return None;
    }
    let authority = value.parse::<axum::http::uri::Authority>().ok()?;
    let host = authority.host();
    if host.is_empty() {
        return None;
    }
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
        .to_ascii_lowercase();
    Some((host, authority.port_u16().unwrap_or(default_port)))
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn private(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

async fn always_private(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    private(next.run(request).await)
}

/// Outermost application-layer protection for every response whose requested
/// path is `/demo` or below it, including responses created by Axum or Tower
/// before the nested demo router can run.
pub async fn private_demo_responses(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let demo_path = request.uri().path() == "/demo" || request.uri().path().starts_with("/demo/");
    let response = next.run(request).await;
    if demo_path {
        private(response)
    } else {
        response
    }
}

/// Narrow construction and observation seams used by the standalone route
/// contract. They do not expose source locators or configured-router mutation.
#[doc(hidden)]
pub mod test_support {
    use super::*;

    pub enum ArchiveSpoolMode {
        Temporary,
        Directory(std::path::PathBuf),
        Unavailable,
    }

    pub struct DemoHarness {
        app: Router,
        registry: DemoRegistry,
        archive_root: Option<std::path::PathBuf>,
    }

    impl DemoHarness {
        pub fn new(
            registrar: Arc<dyn DemoRegistrar>,
            clock: Arc<dyn Clock>,
            reaper_interval: Duration,
            context: Arc<tellurion_core::AppContext>,
            spool_mode: ArchiveSpoolMode,
        ) -> Result<Self, &'static str> {
            let archive_spool = match spool_mode {
                ArchiveSpoolMode::Temporary => {
                    Some(DemoArchiveSpool::new().ok_or("archive spool unavailable")?)
                }
                ArchiveSpoolMode::Directory(root) => {
                    Some(DemoArchiveSpool::in_directory(root).ok_or("archive spool unavailable")?)
                }
                ArchiveSpoolMode::Unavailable => None,
            };
            let archive_root = archive_spool
                .as_ref()
                .map(|archive| archive.root.path().to_path_buf());
            let registry = DemoRegistry::with_components_reaper_and_spool(
                registrar,
                clock,
                reaper_interval,
                archive_spool,
            );
            let app = router_with_registry(registry.clone()).with_state(context);
            Ok(Self {
                app,
                registry,
                archive_root,
            })
        }

        pub fn app(&self) -> Router {
            self.app.clone()
        }

        pub fn archive_root(&self) -> Option<&std::path::Path> {
            self.archive_root.as_deref()
        }

        pub async fn session_count(&self) -> usize {
            self.registry.state.sessions.lock().await.len()
        }

        pub async fn source_count(&self) -> usize {
            let sessions = self.registry.state.sessions.lock().await;
            let mut count = 0;
            for session in sessions.values() {
                count += session.sources.lock().await.len();
            }
            count
        }

        pub async fn replace_feature_source(
            &self,
            cookie: &str,
            source_id: &str,
            features: Arc<dyn FeatureSource>,
        ) -> bool {
            let Some((_, session_id)) = cookie.split_once('=') else {
                return false;
            };
            let session = {
                let sessions = self.registry.state.sessions.lock().await;
                sessions.get(session_id).cloned()
            };
            let Some(session) = session else {
                return false;
            };
            let mut sources = session.sources.lock().await;
            let Some(source) = sources.get(source_id) else {
                return false;
            };
            let replacement = DemoSource {
                raster: source.raster.clone(),
                features: Some(features),
                tiles: source.tiles.clone(),
                collection: source.collection.clone(),
                metadata: source.metadata.clone(),
            };
            sources.insert(source_id.to_owned(), Arc::new(replacement));
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use bytes::Bytes;
    use std::ops::Range;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use tower::ServiceExt;

    use tellurion_core::{
        AppConfig, AppContext, FeaturePage, FileStyleStore, MokaTileCache, PhysicalCollection,
        Registry, Resolver, Router as CoreRouter, StaticResolver, StyleStore, TileCache,
    };
    use tellurion_http_source::{ContentIdentity, SourceError, SourceHandle};

    struct TestClock(StdMutex<Instant>);

    impl TestClock {
        fn new(now: Instant) -> Self {
            Self(StdMutex::new(now))
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

    struct FixtureRangeObject {
        handle: SourceHandle,
        identity: ContentIdentity,
        bytes: Bytes,
        dropped: Arc<AtomicUsize>,
    }

    impl FixtureRangeObject {
        fn new(index: usize, dropped: Arc<AtomicUsize>) -> Self {
            let bytes = Bytes::from_static(include_bytes!(
                "../../tellurion-cog/tests/fixtures/tiled_rgb.tif"
            ));
            let length = bytes.len() as u64;
            Self {
                handle: SourceHandle::new(format!("fixture-{index:032x}")),
                identity: ContentIdentity::StrongEtag {
                    source_key: [7; 32],
                    revision_key: [9; 32],
                    length,
                },
                bytes,
                dropped,
            }
        }
    }

    impl Drop for FixtureRangeObject {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl RangeObject for FixtureRangeObject {
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
            "fixture.tif"
        }

        async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError> {
            Ok(self.bytes.slice(range.start as usize..range.end as usize))
        }
    }

    struct FixtureRegistrar {
        sessions: PublicHttpsGateway,
        registrations: AtomicUsize,
        fail: AtomicBool,
        dropped: Arc<AtomicUsize>,
    }

    impl FixtureRegistrar {
        fn new() -> Self {
            Self {
                sessions: PublicHttpsGateway::new(),
                registrations: AtomicUsize::new(0),
                fail: AtomicBool::new(false),
                dropped: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl DemoRegistrar for FixtureRegistrar {
        fn open_session(&self) -> SourceSession {
            self.sessions.open_session()
        }

        async fn register(
            &self,
            _session: &SourceSession,
            _raw_url: &str,
        ) -> Result<Arc<dyn RangeObject>, ()> {
            let index = self.registrations.fetch_add(1, Ordering::SeqCst);
            (!self.fail.load(Ordering::SeqCst))
                .then(|| {
                    Arc::new(FixtureRangeObject::new(index, Arc::clone(&self.dropped)))
                        as Arc<dyn RangeObject>
                })
                .ok_or(())
        }
    }

    struct BlockingRegistrar {
        sessions: PublicHttpsGateway,
        entered: tokio::sync::Notify,
    }

    struct CatalogWithSrid(Option<i32>);

    #[async_trait]
    impl CatalogSource for CatalogWithSrid {
        async fn collections(&self) -> tellurion_core::Result<Vec<PhysicalCollection>> {
            Ok(vec![PhysicalCollection {
                name: "fixture".to_owned(),
                geometry_column: Some("geometry".to_owned()),
                primary_key: Some("id".to_owned()),
                srid: self.0,
                geometry_type: Some("POINT".to_owned()),
            }])
        }
    }

    struct UnusedVectorSource;

    #[async_trait]
    impl FeatureSource for UnusedVectorSource {
        async fn items(
            &self,
            _collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> tellurion_core::Result<FeaturePage> {
            unreachable!("CRS admission must happen before feature access")
        }

        async fn item(
            &self,
            _collection: &CollectionDecl,
            _id: &str,
            _filter: Option<&tellurion_core::Filter>,
        ) -> tellurion_core::Result<Option<serde_json::Value>> {
            unreachable!("CRS admission must happen before feature access")
        }
    }

    #[async_trait]
    impl TileSource for UnusedVectorSource {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&tellurion_core::Filter>,
        ) -> tellurion_core::Result<Option<Bytes>> {
            unreachable!("CRS admission must happen before tile access")
        }
    }

    impl BlockingRegistrar {
        fn new() -> Self {
            Self {
                sessions: PublicHttpsGateway::new(),
                entered: tokio::sync::Notify::new(),
            }
        }
    }

    #[async_trait]
    impl DemoRegistrar for BlockingRegistrar {
        fn open_session(&self) -> SourceSession {
            self.sessions.open_session()
        }

        async fn register(
            &self,
            _session: &SourceSession,
            _raw_url: &str,
        ) -> Result<Arc<dyn RangeObject>, ()> {
            self.entered.notify_one();
            std::future::pending().await
        }
    }

    fn test_app() -> Router {
        test_app_with_registry(DemoRegistry::new())
    }

    fn test_app_with_registry(registry: DemoRegistry) -> Router {
        let config = AppConfig::default();
        config.validate().unwrap();
        let storage_registry = Registry::new();
        let core_router = CoreRouter::build(&config, &storage_registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            core_router,
            resolver,
            None,
            cache,
            style_store,
        ));
        router_with_registry(registry).with_state(ctx)
    }

    fn fixture_app() -> (Router, DemoRegistry, Arc<TestClock>, Arc<FixtureRegistrar>) {
        let clock = Arc::new(TestClock::new(Instant::now()));
        let registrar = Arc::new(FixtureRegistrar::new());
        let registry = DemoRegistry::with_components(registrar.clone(), clock.clone());
        (
            test_app_with_registry(registry.clone()),
            registry,
            clock,
            registrar,
        )
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

    struct RegisteredFixture {
        cookie: Option<String>,
        id: String,
        json: String,
    }

    async fn register_fixture(app: &Router, cookie: Option<&str>) -> RegisteredFixture {
        let mut request = same_origin_request(
            "POST",
            "/demo/sources",
            Body::from(r#"{"url":"https://example.com/secret/fixture.tif"}"#),
        );
        if let Some(cookie) = cookie {
            request
                .headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
        }
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let minted_cookie = response.headers().get(header::SET_COOKIE).map(|value| {
            value
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned()
        });
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        RegisteredFixture {
            cookie: minted_cookie,
            id: json["id"].as_str().unwrap().to_owned(),
            json: json.to_string(),
        }
    }

    #[test]
    fn configured_archive_spool_uses_the_deployment_root() {
        let parent = tempfile::tempdir().unwrap();
        let spool = DemoArchiveSpool::configured(Some(parent.path().to_path_buf()))
            .unwrap()
            .unwrap();
        let process_root = spool.root.path().to_path_buf();

        assert_eq!(process_root.parent(), Some(parent.path()));
        assert_ne!(process_root, parent.path());
        drop(spool);
        assert!(!process_root.exists());
    }

    #[test]
    fn configured_archive_spool_refuses_an_unusable_deployment_root() {
        let parent = tempfile::tempdir().unwrap();
        let file = parent.path().join("not-a-directory");
        std::fs::write(&file, b"occupied").unwrap();

        assert!(DemoArchiveSpool::configured(Some(file)).is_err());
    }

    #[test]
    fn only_valid_web_mercator_quad_coordinates_are_accepted() {
        assert_eq!(
            parse_web_mercator_quad("2", "3", "3"),
            Some(TileCoord { z: 2, x: 3, y: 3 })
        );
        assert_eq!(parse_web_mercator_quad("2", "4", "0"), None);
        assert_eq!(parse_web_mercator_quad("23", "0", "0"), None);
    }

    #[test]
    fn session_cookie_is_strictly_opaque() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-tellurion-demo=0123456789abcdef0123456789abcdef"),
        );
        assert!(session_cookie(&headers).is_some());
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-tellurion-demo=not-valid"),
        );
        assert!(session_cookie(&headers).is_none());
    }

    #[test]
    fn minted_cookie_uses_host_only_secure_attributes() {
        let mut headers = HeaderMap::new();
        set_session_cookie(
            &mut headers,
            "0123456789abcdef0123456789abcdef",
            SessionCookieKind::Secure,
        );
        assert_eq!(
            headers.get(header::SET_COOKIE).unwrap(),
            "__Host-tellurion-demo=0123456789abcdef0123456789abcdef; Path=/; Secure; HttpOnly; SameSite=Strict"
        );
    }

    #[tokio::test]
    async fn loopback_http_uses_a_host_bound_cookie_without_secure() {
        let (app, _, _, _) = fixture_app();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/demo/sources")
                    .header(header::HOST, "127.0.0.1:8080")
                    .header(header::ORIGIN, "http://127.0.0.1:8080")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"url":"https://example.com/secret/fixture.tif"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(set_cookie.starts_with("tellurion-demo-local="));
        assert!(!set_cookie.contains("; Secure"));
        assert!(set_cookie.ends_with("; Path=/; HttpOnly; SameSite=Strict"));
        let cookie = set_cookie.split(';').next().unwrap().to_owned();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let id = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let local = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/demo/sources/{id}"))
                    .header(header::HOST, "127.0.0.1:8080")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(local.status(), StatusCode::OK);

        let remote = app
            .oneshot(
                Request::builder()
                    .uri(format!("/demo/sources/{id}"))
                    .header(header::HOST, "demo.example")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(remote.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unauthenticated_demo_reads_and_control_paths_are_not_addressable() {
        let app = test_app();
        for path in ["/demo/sources/not-a-source", "/config/demo"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(
                response.headers().get(header::CACHE_CONTROL).unwrap(),
                "private, no-store"
            );
        }
    }

    #[tokio::test]
    async fn cross_origin_registration_is_rejected_without_echoing_the_locator() {
        let locator = "https://example.com/private/path.tif";
        let response = test_app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/demo/sources")
                    .header(header::HOST, "demo.example")
                    .header(header::ORIGIN, "https://elsewhere.example")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"url":"{locator}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "private, no-store"
        );
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains(locator));
    }

    #[tokio::test]
    async fn oversized_or_wrong_content_type_posts_do_not_reach_a_gateway() {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/demo/sources")
                    .header(header::HOST, "demo.example")
                    .header(header::ORIGIN, "https://demo.example")
                    .header(header::CONTENT_TYPE, "text/plain")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "private, no-store"
        );

        let oversized = format!(
            r#"{{"url":"https://example.com/{}.tif"}}"#,
            "x".repeat(MAX_BODY_BYTES)
        );
        let response = test_app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/demo/sources")
                    .header(header::HOST, "demo.example")
                    .header(header::ORIGIN, "https://demo.example")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(oversized))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "private, no-store"
        );
    }

    #[tokio::test]
    async fn successful_registration_is_session_scoped_and_redacts_the_locator() {
        let (app, _, _, _) = fixture_app();
        let first = register_fixture(&app, None).await;
        let first_cookie = first.cookie.as_deref().unwrap();
        assert!(!first
            .json
            .contains("https://example.com/secret/fixture.tif"));
        assert!(first.json.contains("tiled-geotiff"));
        assert!(first.json.contains("range-native"));
        assert!(first.json.contains("strong"));

        let second = register_fixture(&app, None).await;
        let second_cookie = second.cookie.as_deref().unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/demo/sources/{}", first.id))
                    .header(header::COOKIE, second_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/demo/sources/{}", first.id))
                    .header(header::COOKIE, first_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let text =
            String::from_utf8_lossy(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .into_owned();
        assert!(!text.contains("https://example.com/secret/fixture.tif"));
    }

    #[tokio::test]
    async fn registered_cog_tile_matches_the_driver_png_and_is_private() {
        let (app, registry, _, _) = fixture_app();
        let registered = register_fixture(&app, None).await;
        let cookie = registered.cookie.as_deref().unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/demo/sources/{}/tiles/WebMercatorQuad/0/0/0.png",
                        registered.id
                    ))
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/png"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "private, no-store"
        );
        let actual = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        let session_id = cookie.split_once('=').unwrap().1;
        let session = registry
            .state
            .sessions
            .lock()
            .await
            .get(session_id)
            .unwrap()
            .clone();
        let source = session
            .sources
            .lock()
            .await
            .get(&registered.id)
            .unwrap()
            .clone();
        let window = source
            .raster
            .as_ref()
            .unwrap()
            .raster_tile(&source.collection, TileCoord { z: 0, x: 0, y: 0 })
            .await
            .unwrap()
            .unwrap();
        let expected = encode_rgba_to_png(&window.rgba, window.width, window.height).unwrap();
        assert_eq!(actual.as_ref(), expected.as_slice());
    }

    #[tokio::test]
    async fn fix_round_one_raster_tiles_refuse_every_query() {
        let (app, _, _, _) = fixture_app();
        let registered = register_fixture(&app, None).await;
        let cookie = registered.cookie.as_deref().unwrap();
        let locator = "https://example.com/secret/fixture.tif";
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/demo/sources/{}/tiles/WebMercatorQuad/0/0/0.png?crs=EPSG%3A3857",
                        registered.id
                    ))
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains(locator));
    }

    #[tokio::test]
    async fn fix_round_one_projected_and_unknown_catalog_metadata_are_refused() {
        for srid in [Some(3857), None] {
            let dropped = Arc::new(AtomicUsize::new(0));
            let object: Arc<dyn RangeObject> = Arc::new(FixtureRangeObject::new(0, dropped));
            let catalog: Arc<dyn CatalogSource> = Arc::new(CatalogWithSrid(srid));
            let vector = Arc::new(UnusedVectorSource);
            let result = inspect_vector(
                object,
                catalog,
                vector.clone() as Arc<dyn FeatureSource>,
                vector as Arc<dyn TileSource>,
                "fixture",
                "test",
            )
            .await;
            assert!(result.is_err(), "public demo admitted SRID {srid:?}");
        }
    }

    #[tokio::test]
    async fn expiry_purges_state_and_source_cap_rejects_the_fourth() {
        let (app, registry, clock, _) = fixture_app();
        let first = register_fixture(&app, None).await;
        let cookie = first.cookie.as_deref().unwrap().to_owned();
        let _second = register_fixture(&app, Some(&cookie)).await;
        let _third = register_fixture(&app, Some(&cookie)).await;
        let request = same_origin_request(
            "POST",
            "/demo/sources",
            Body::from(r#"{"url":"https://example.com/fourth.tif"}"#),
        );
        let mut request = request;
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        clock.advance(SESSION_TTL);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/demo/sources/{}", first.id))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert!(registry.state.sessions.lock().await.is_empty());
    }

    #[tokio::test]
    async fn delete_removes_a_source_and_requires_same_origin() {
        let (app, _, _, _) = fixture_app();
        let registered = register_fixture(&app, None).await;
        let cookie = registered.cookie.as_deref().unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/demo/sources/{}", registered.id))
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let mut request = same_origin_request(
            "DELETE",
            &format!("/demo/sources/{}", registered.id),
            Body::empty(),
        );
        request
            .headers_mut()
            .insert(header::COOKIE, cookie.parse().unwrap());
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/demo/sources/{}", registered.id))
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn operation_slots_queue_a_browser_tile_burst() {
        let (app, registry, _, _) = fixture_app();
        let registered = register_fixture(&app, None).await;
        let cookie = registered.cookie.as_deref().unwrap();
        let session_id = cookie.split_once('=').unwrap().1;
        let session = registry
            .state
            .sessions
            .lock()
            .await
            .get(session_id)
            .unwrap()
            .clone();
        let first = session.operations.clone().try_acquire_owned().unwrap();
        let second = session.operations.clone().try_acquire_owned().unwrap();
        let request = Request::builder()
            .uri(format!(
                "/demo/sources/{}/tiles/WebMercatorQuad/0/0/0.png",
                registered.id
            ))
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap();
        let queued = tokio::spawn(app.oneshot(request));
        tokio::task::yield_now().await;
        assert!(
            !queued.is_finished(),
            "a normal map tile burst must wait for bounded capacity instead of returning 429"
        );

        drop(first);
        let response = tokio::time::timeout(Duration::from_secs(1), queued)
            .await
            .expect("queued tile should start when one operation slot is released")
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        drop(second);
    }

    #[tokio::test]
    async fn failed_registration_releases_its_unadvertised_session() {
        let clock = Arc::new(TestClock::new(Instant::now()));
        let registrar = Arc::new(FixtureRegistrar::new());
        registrar.fail.store(true, Ordering::SeqCst);
        let registry = DemoRegistry::with_components(registrar, clock);
        let app = test_app_with_registry(registry.clone());
        let response = app
            .oneshot(same_origin_request(
                "POST",
                "/demo/sources",
                Body::from(r#"{"url":"https://example.com/failed.tif"}"#),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        assert!(registry.state.sessions.lock().await.is_empty());
    }

    #[tokio::test]
    async fn cancelled_registration_releases_its_unadvertised_session() {
        let clock = Arc::new(TestClock::new(Instant::now()));
        let registrar = Arc::new(BlockingRegistrar::new());
        let registry = DemoRegistry::with_components(registrar.clone(), clock);
        let app = test_app_with_registry(registry.clone());
        let request = same_origin_request(
            "POST",
            "/demo/sources",
            Body::from(r#"{"url":"https://example.com/slow.tif"}"#),
        );

        let registration = tokio::spawn(app.oneshot(request));
        registrar.entered.notified().await;
        registration.abort();
        let error = registration.await.unwrap_err();
        assert!(error.is_cancelled());
        assert!(registry.state.sessions.lock().await.is_empty());
    }

    #[test]
    fn same_origin_accepts_https_and_loopback_http_authorities() {
        let cases = [
            ("https://DEMO.example", "demo.EXAMPLE:443", true),
            ("https://demo.example:8443", "DEMO.example:8443", true),
            ("https://demo.example:8443", "demo.example", false),
            ("https://[2001:db8::1]", "[2001:DB8::1]:443", true),
            ("http://demo.example", "demo.example", false),
            ("http://127.0.0.1:18080", "127.0.0.1:18080", true),
            ("http://localhost:8080", "LOCALHOST:8080", true),
            ("http://[::1]:8080", "[::1]:8080", true),
            ("http://localhost", "localhost:80", true),
            ("http://localhost", "localhost:443", false),
            ("https://user@demo.example", "demo.example", false),
            ("https://demo.example/path", "demo.example", false),
            ("https://demo.example?query=1", "demo.example", false),
            ("https://demo.example#fragment", "demo.example", false),
        ];
        for (origin, host, expected) in cases {
            let mut headers = HeaderMap::new();
            headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
            headers.insert(header::HOST, HeaderValue::from_str(host).unwrap());
            assert_eq!(same_origin(&headers), expected, "{origin} / {host}");
        }

        let mut duplicate_origin = HeaderMap::new();
        duplicate_origin.append(
            header::ORIGIN,
            HeaderValue::from_static("https://demo.example"),
        );
        duplicate_origin.append(
            header::ORIGIN,
            HeaderValue::from_static("https://demo.example"),
        );
        duplicate_origin.insert(header::HOST, HeaderValue::from_static("demo.example"));
        assert!(!same_origin(&duplicate_origin));

        let mut duplicate_host = HeaderMap::new();
        duplicate_host.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://demo.example"),
        );
        duplicate_host.append(header::HOST, HeaderValue::from_static("demo.example"));
        duplicate_host.append(header::HOST, HeaderValue::from_static("demo.example"));
        assert!(!same_origin(&duplicate_host));
    }

    #[tokio::test]
    async fn reaper_removes_expired_sources_without_another_request_and_stops_with_router() {
        let clock = Arc::new(TestClock::new(Instant::now()));
        let registrar = Arc::new(FixtureRegistrar::new());
        let registry = DemoRegistry::with_components_and_reaper(
            registrar.clone(),
            clock.clone(),
            Duration::from_millis(5),
        );
        let app = test_app_with_registry(registry.clone());
        let registered = register_fixture(&app, None).await;
        let cookie = registered.cookie.as_deref().unwrap();
        let session_id = cookie.split_once('=').unwrap().1;
        let session = registry
            .state
            .sessions
            .lock()
            .await
            .get(session_id)
            .unwrap()
            .clone();
        let session_weak = Arc::downgrade(&session);
        drop(session);

        clock.advance(SESSION_TTL);
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(session_weak.upgrade().is_none());
        assert_eq!(registrar.dropped.load(Ordering::SeqCst), 1);

        let state_weak = Arc::downgrade(&registry.state);
        drop(app);
        drop(registry);
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(state_weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn session_admission_is_atomic_and_rejects_new_browser_sessions() {
        let (app, registry, _, _) = fixture_app();
        let mut workers = tokio::task::JoinSet::new();
        for _ in 0..=MAX_LIVE_SESSIONS {
            let registry = registry.clone();
            workers.spawn(async move {
                let Ok((id, session)) = registry.reserve_session().await else {
                    return false;
                };
                registry.publish_session(id, session).await;
                true
            });
        }
        let mut admitted = 0;
        let mut rejected = 0;
        while let Some(result) = workers.join_next().await {
            if result.unwrap() {
                admitted += 1;
            } else {
                rejected += 1;
            }
        }
        assert_eq!(admitted, MAX_LIVE_SESSIONS);
        assert_eq!(rejected, 1);
        assert_eq!(
            registry.state.sessions.lock().await.len(),
            MAX_LIVE_SESSIONS
        );

        let response = app
            .oneshot(same_origin_request(
                "POST",
                "/demo/sources",
                Body::from(r#"{"url":"https://example.com/cap.tif"}"#),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "private, no-store"
        );
    }
}
