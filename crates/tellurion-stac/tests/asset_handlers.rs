//! HTTP-level tests for the assets-and-object-storage proposal: a fake,
//! in-memory `AssetRecordStore` (via `tellurion_core::InMemoryAssetRecordStore`,
//! the `test-support` feature), a real `FsObjectStore` against a temp
//! directory for the first slice's `fs` profile, and a real `S3ObjectStore`
//! against an in-process loopback mock (`MockS3` below) for the `s3`
//! profile's `presigned-upload` (second slice), `resumable-upload` (this
//! slice's own real multipart-upload signing), and `download-redirect`
//! (fourth slice) classes — driven through the real
//! `tellurion_core::Router` and the real axum router this crate exports —
//! no database involved (the database-backed `AssetRecordStore`
//! implementation itself is covered by `tellurion-postgis`'s own
//! `tests/assets_live.rs`; this file proves the HTTP contract: status
//! codes, JSON shapes, header parsing). Mirrors `tests/handlers.rs`'s own
//! fake-driver style.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::Response;
use serde_json::{json, Value};
use tower::ServiceExt;

use tellurion_core::{
    AppConfig, AppContext, AssetRecordStore, AttributeColumn, CatalogSource, CollectionDecl,
    DriverFactory, FeaturePage, FeatureSource, FileStyleStore, Filter, InMemoryAssetRecordStore,
    ItemsQuery, MokaTileCache, PhysicalCollection, Registry, Resolver, Result as CoreResult,
    Router as CoreRouter, SpatialExtent, StaticResolver, StorageDecl, StorageDriver, StyleStore,
    TileCache,
};

// -- a tiny in-process S3-compatible mock (presigned-upload class tests) --
//
// The same loopback-`TcpListener` idiom `tellurion_core::objectstore`'s own
// `s3_tests` module uses for the `S3ObjectStore` adapter's own tests
// (never a mocking crate) — this file needs one too, to drive the real
// `put_asset_presign`/`get_asset_presign`/`post_asset_finalize` HTTP
// handlers against a real `s3`-profile object store rather than only the
// `fs` profile the rest of this file exercises. A presigned URL is still
// never dereferenced over the network by any test below — `seed` places
// bytes directly at the request-target a real client's out-of-band `PUT`
// would have written to, the same "simulate the transfer directly"
// convention `tellurion_core::asset`'s own presign test suite uses.
/// One in-progress multipart upload this mock is tracking — this file's own
/// counterpart to `tellurion_core::objectstore`'s own `s3_tests::
/// MultipartUpload` (a separate integration-test binary can't reuse that
/// crate-private type, so the shape is duplicated rather than shared).
struct MultipartUpload {
    object_path: String,
    parts: std::collections::HashMap<i32, Vec<u8>>,
}

/// Fault-injection hook for the successor-admission race: armed by a test
/// right before it triggers a real digest-mismatch cleanup delete, this
/// parks that DELETE's handler thread between "request received" and
/// "response sent" so the test can drive a concurrent request into the
/// exact window a slow network delete leaves open in production, instead of
/// hoping for a timing accident. `parked_tx` is `Some` until the first
/// (and only) plain DELETE this gate ever sees takes it — every DELETE
/// after that passes straight through, armed or not.
struct DeleteGate {
    parked_tx: Option<tokio::sync::oneshot::Sender<()>>,
    release_rx: std::sync::mpsc::Receiver<()>,
}

struct MockS3 {
    addr: std::net::SocketAddr,
    objects: Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>,
    // Kept alive here (not just in the listener thread's own clone) for
    // symmetry with `objects` above — no test in this file reads either
    // field directly, only through the real HTTP round trip.
    #[allow(dead_code)]
    multipart: Arc<std::sync::Mutex<std::collections::HashMap<String, MultipartUpload>>>,
    #[allow(dead_code)]
    next_upload_id: Arc<std::sync::Mutex<u64>>,
    delete_gate: Arc<std::sync::Mutex<Option<DeleteGate>>>,
}

impl MockS3 {
    fn spawn() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binds a loopback port");
        let addr = listener.local_addr().expect("bound listener has an addr");
        let objects = Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            String,
            Vec<u8>,
        >::new()));
        let multipart = Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            String,
            MultipartUpload,
        >::new()));
        let next_upload_id = Arc::new(std::sync::Mutex::new(0u64));
        let delete_gate = Arc::new(std::sync::Mutex::new(None::<DeleteGate>));
        let objects_for_thread = Arc::clone(&objects);
        let multipart_for_thread = Arc::clone(&multipart);
        let next_upload_id_for_thread = Arc::clone(&next_upload_id);
        let delete_gate_for_thread = Arc::clone(&delete_gate);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                Self::handle_one(
                    stream,
                    &objects_for_thread,
                    &multipart_for_thread,
                    &next_upload_id_for_thread,
                    &delete_gate_for_thread,
                );
            }
        });
        Self {
            addr,
            objects,
            multipart,
            next_upload_id,
            delete_gate,
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn seed(&self, path: &str, bytes: &[u8]) {
        self.objects
            .lock()
            .unwrap()
            .insert(path.to_string(), bytes.to_vec());
    }

    fn contains(&self, path: &str) -> bool {
        self.objects.lock().unwrap().contains_key(path)
    }

    /// Arms the plain-DELETE pause described on [`DeleteGate`] and returns
    /// the two handles a test needs to drive it: a receiver that resolves
    /// once the mock's own handler thread is genuinely parked on the
    /// DELETE it just received (never a fixed sleep guessing at that), and
    /// a sender the test uses to let that parked thread continue once it
    /// has exercised the window.
    fn arm_delete_pause(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (parked_tx, parked_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *self.delete_gate.lock().unwrap() = Some(DeleteGate {
            parked_tx: Some(parked_tx),
            release_rx,
        });
        (parked_rx, release_tx)
    }

    /// Splits a raw (still percent-encoded) query string into decoded
    /// `(key, value)` pairs — the multipart-upload verbs' own request
    /// shape (`?uploads`/`?partNumber=N&uploadId=...`/`?uploadId=...`)
    /// needs this; the plain put/get/head/delete verbs this mock already
    /// spoke never carried a query string at all.
    fn parse_query(raw_query: &str) -> Vec<(String, String)> {
        raw_query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                (key.to_string(), value.to_string())
            })
            .collect()
    }

    /// `CompleteMultipartUpload`'s own request body: every `<Part>`'s own
    /// `<PartNumber>`, in document order.
    fn parse_complete_multipart_body(xml: &str) -> Vec<i32> {
        let mut part_numbers = Vec::new();
        let mut rest = xml;
        while let Some(start) = rest.find("<PartNumber>") {
            let after = &rest[start + "<PartNumber>".len()..];
            let Some(end) = after.find("</PartNumber>") else {
                break;
            };
            if let Ok(number) = after[..end].parse::<i32>() {
                part_numbers.push(number);
            }
            rest = &after[end..];
        }
        part_numbers
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_one(
        mut stream: std::net::TcpStream,
        objects: &Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>,
        multipart: &Arc<std::sync::Mutex<std::collections::HashMap<String, MultipartUpload>>>,
        next_upload_id: &Arc<std::sync::Mutex<u64>>,
        delete_gate: &Arc<std::sync::Mutex<Option<DeleteGate>>>,
    ) {
        use std::io::{BufRead, BufReader, Read, Write};
        let mut reader = BufReader::new(stream.try_clone().expect("clone loopback stream"));
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();

        let mut content_length: usize = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line
                .strip_prefix("Content-Length: ")
                .or_else(|| line.strip_prefix("content-length: "))
            {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            let _ = reader.read_exact(&mut body);
        }

        let (base_path, raw_query) = path.split_once('?').unwrap_or((path.as_str(), ""));
        let base_path = base_path.to_string();
        let query = Self::parse_query(raw_query);
        let has = |name: &str| query.iter().any(|(k, _)| k == name);
        let query_value = |name: &str| {
            query
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };

        match method.as_str() {
            "POST" if has("uploads") => {
                let mut counter = next_upload_id.lock().unwrap();
                *counter += 1;
                let upload_id = format!("mock-upload-{}", *counter);
                drop(counter);
                multipart.lock().unwrap().insert(
                    upload_id.clone(),
                    MultipartUpload {
                        object_path: base_path.clone(),
                        parts: std::collections::HashMap::new(),
                    },
                );
                let xml = format!(
                    "<InitiateMultipartUploadResult><UploadId>{upload_id}</UploadId>\
                     </InitiateMultipartUploadResult>"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
                    xml.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
            "PUT" if has("partNumber") && has("uploadId") => {
                let upload_id = query_value("uploadId").unwrap_or_default();
                let part_number: i32 = query_value("partNumber")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                let mut in_progress = multipart.lock().unwrap();
                match in_progress.get_mut(&upload_id) {
                    Some(entry) => {
                        entry.parts.insert(part_number, body.clone());
                        let etag = format!("\"etag-{upload_id}-{part_number}\"");
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nETag: {etag}\r\nContent-Length: 0\r\n\
                             Connection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    None => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\
                              Connection: close\r\n\r\n",
                        );
                    }
                }
            }
            "POST" if has("uploadId") => {
                let upload_id = query_value("uploadId").unwrap_or_default();
                let mut in_progress = multipart.lock().unwrap();
                match in_progress.remove(&upload_id) {
                    Some(entry) => {
                        let body_str = String::from_utf8_lossy(&body);
                        let mut assembled = Vec::new();
                        for part_number in Self::parse_complete_multipart_body(&body_str) {
                            if let Some(part_bytes) = entry.parts.get(&part_number) {
                                assembled.extend_from_slice(part_bytes);
                            }
                        }
                        objects.lock().unwrap().insert(entry.object_path, assembled);
                        let xml = "<CompleteMultipartUploadResult>\
                                   <ETag>\"final\"</ETag></CompleteMultipartUploadResult>";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
                            xml.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    None => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\
                              Connection: close\r\n\r\n",
                        );
                    }
                }
            }
            "DELETE" if has("uploadId") => {
                let upload_id = query_value("uploadId").unwrap_or_default();
                multipart.lock().unwrap().remove(&upload_id);
                let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
            }
            "PUT" => {
                objects.lock().unwrap().insert(path, body);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            "HEAD" => match objects.lock().unwrap().get(&path).cloned() {
                Some(bytes) => {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                None => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            },
            "GET" => match objects.lock().unwrap().get(&path).cloned() {
                Some(bytes) => {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.write_all(&bytes);
                }
                None => {
                    let _ = stream.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            },
            "DELETE" => {
                // The successor-admission race's own fault-injection point
                // (`DeleteGate`'s own doc): a test that armed this parks
                // the response here, between "the mismatch cleanup's
                // DELETE landed at the mock" and "the client sees it
                // complete" — precisely the network round trip a stale
                // delete takes in production. Taken out of the mutex
                // entirely (never held across the blocking `recv` below) —
                // this mock only ever runs one connection at a time on this
                // one thread, so nothing else could contend for the lock
                // regardless, but there is no reason to hold it longer than
                // the single `Option::take` needs. A gate armed for one
                // DELETE is consumed by it; any later DELETE this mock sees
                // finds `None` and passes straight through.
                let gate = delete_gate.lock().unwrap().take();
                if let Some(mut gate) = gate {
                    if let Some(parked_tx) = gate.parked_tx.take() {
                        let _ = parked_tx.send(());
                        let _ = gate.release_rx.recv();
                    }
                }
                objects.lock().unwrap().remove(&path);
                let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
            }
            _ => {
                let _ = stream.write_all(
                    b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        }
    }
}

struct DemoCatalog(&'static str);

#[async_trait::async_trait]
impl CatalogSource for DemoCatalog {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        Ok(vec![PhysicalCollection {
            name: self.0.to_string(),
            geometry_column: Some("geom".to_string()),
            primary_key: Some("id".to_string()),
            srid: Some(4326),
            geometry_type: None,
        }])
    }
    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        Ok(Some(SpatialExtent {
            bbox: [-5.0, 45.0, 5.0, 55.0],
        }))
    }
    async fn attribute_schema(
        &self,
        _physical: &PhysicalCollection,
    ) -> CoreResult<Option<Vec<AttributeColumn>>> {
        Ok(Some(vec![]))
    }
}

struct EmptyFeatureSource;

#[async_trait::async_trait]
impl FeatureSource for EmptyFeatureSource {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        _query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        Ok(FeaturePage {
            features_geojson: vec![],
            number_matched: Some(0),
            next_token: None,
        })
    }
    async fn item(
        &self,
        _collection: &CollectionDecl,
        _id: &str,
        _filter: Option<&Filter>,
    ) -> CoreResult<Option<Value>> {
        Ok(None)
    }
}

/// `FeatureSource` + `AssetRecordStore`, backed by an in-memory fake — the
/// collection this file's asset tests exercise.
struct AssetCapableDriver {
    table: &'static str,
    assets: Arc<InMemoryAssetRecordStore>,
}

#[async_trait::async_trait]
impl AssetRecordStore for AssetCapableDriver {
    async fn register(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
        new_record: tellurion_core::NewAssetRecord,
    ) -> CoreResult<tellurion_core::AssetRecord> {
        self.assets
            .register(collection, item_id, key, new_record)
            .await
    }
    async fn get(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
    ) -> CoreResult<Option<tellurion_core::AssetRecord>> {
        self.assets.get(collection, item_id, key).await
    }
    async fn finalize(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
        outcome: tellurion_core::FinalizeOutcome,
    ) -> CoreResult<tellurion_core::AssetRecord> {
        self.assets
            .finalize(collection, item_id, key, outcome)
            .await
    }
    async fn delete(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
    ) -> CoreResult<Option<tellurion_core::AssetRecord>> {
        self.assets.delete(collection, item_id, key).await
    }
    async fn list(
        &self,
        collection: &CollectionDecl,
    ) -> CoreResult<Vec<tellurion_core::AssetRecordEntry>> {
        self.assets.list(collection).await
    }
    async fn item_assets(
        &self,
        collection: &CollectionDecl,
        item_ids: &[String],
    ) -> CoreResult<Vec<tellurion_core::AssetRecordEntry>> {
        self.assets.item_assets(collection, item_ids).await
    }
}

impl StorageDriver for AssetCapableDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(DemoCatalog(self.table))
    }
    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::new(EmptyFeatureSource) as Arc<dyn FeatureSource>)
    }
    fn asset_record_store(&self) -> Option<Arc<dyn AssetRecordStore>> {
        None // overridden below via the wrapping Arc, see `build_ctx`
    }
}

/// A driver with `FeatureSource` but deliberately no `AssetRecordStore` at
/// all — proves the "refusal by name" path when the anchor driver lacks the
/// capability entirely (distinct from "table not provisioned", which the
/// database-backed driver alone can exercise; see `tellurion-postgis::
/// tests::assets_live`).
struct BareDriver;

impl StorageDriver for BareDriver {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::new(DemoCatalog("bare"))
    }
    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::new(EmptyFeatureSource) as Arc<dyn FeatureSource>)
    }
}

/// Wraps [`AssetCapableDriver`] so `asset_record_store` can actually
/// advertise `Some` — a plain struct can't return `Some(Arc::new(self))`
/// from a `&self` method, so the capability lives on a thin `Arc`-holding
/// wrapper instead, the same shape `PostgisDriverImpl`/`PostgisBackend`
/// split in the real driver.
struct AssetCapableDriverImpl {
    inner: Arc<AssetCapableDriver>,
}

impl StorageDriver for AssetCapableDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        self.inner.catalog_source()
    }
    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        self.inner.feature_source()
    }
    fn asset_record_store(&self) -> Option<Arc<dyn AssetRecordStore>> {
        Some(Arc::clone(&self.inner) as Arc<dyn AssetRecordStore>)
    }
}

struct AssetCapableFactory;

impl DriverFactory for AssetCapableFactory {
    fn name(&self) -> &str {
        "fake-assets"
    }
    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(AssetCapableDriverImpl {
            inner: Arc::new(AssetCapableDriver {
                table: "demo",
                assets: Arc::new(InMemoryAssetRecordStore::default()),
            }),
        }))
    }
}

struct BareFactory;

impl DriverFactory for BareFactory {
    fn name(&self) -> &str {
        "fake-bare"
    }
    fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        Ok(Arc::new(BareDriver))
    }
}

fn build_config_yaml(object_store_root: &str) -> String {
    format!(
        r#"
storages:
  - id: main
    driver: fake-assets
    url_env: DATABASE_URL
  - id: bare
    driver: fake-bare
    url_env: DATABASE_URL
object_stores:
  - id: store1
    profile: fs
    root: {object_store_root}
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    object_store: store1
    settings:
      max_asset_bytes: 20
      asset_media_types: ["image/png"]
      stac:
        assets:
          declared-thumb:
            href: "https://example.test/declared.png"
            type: image/png
  - id: demo-no-store
    catalog: default
    storage: main
    table: demo-no-store
    geometry: geom
    pk: id
  - id: bare
    catalog: default
    storage: bare
    table: bare
    geometry: geom
    pk: id
"#
    )
}

/// [`build_config_yaml`] plus one `s3`-profile `object_store` (`s3store`,
/// pointed at a [`MockS3`] endpoint) and one collection (`s3demo`) declared
/// against it — the `presigned-upload` conformance class's own fixture.
/// `s3demo` reuses storage `main`/table `demo` (the fake driver always
/// reports its physical collection as `"demo"` regardless of which
/// `CollectionDecl` routes to it — see `AssetCapableFactory::build`), so
/// only the collection `id` (and hence the URL path) differs from `demo`
/// itself.
///
/// `access_key_env`/`secret_key_env` are caller-chosen (not a shared
/// constant): `object_stores` entries are built eagerly at `Router::build`
/// time regardless of which collection a given test actually exercises, so
/// every test using this fixture must have its own environment variable
/// present for the whole `build_ctx` call — a shared name across tests
/// running concurrently in this same integration-test binary would race on
/// which test's `std::env::set_var`/`remove_var` wins.
fn build_config_yaml_with_s3(
    object_store_root: &str,
    s3_endpoint: &str,
    access_key_env: &str,
    secret_key_env: &str,
) -> String {
    format!(
        r#"
storages:
  - id: main
    driver: fake-assets
    url_env: DATABASE_URL
  - id: bare
    driver: fake-bare
    url_env: DATABASE_URL
object_stores:
  - id: store1
    profile: fs
    root: {object_store_root}
  - id: s3store
    profile: s3
    endpoint: {s3_endpoint}
    bucket: photos
    region: us-east-1
    access_key_env: {access_key_env}
    secret_key_env: {secret_key_env}
    presign_expiry_s: 300
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    object_store: store1
    settings:
      max_asset_bytes: 20
      asset_media_types: ["image/png"]
      stac:
        assets:
          declared-thumb:
            href: "https://example.test/declared.png"
            type: image/png
  - id: s3demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    object_store: s3store
    settings:
      max_asset_bytes: 1000
      asset_media_types: ["image/png"]
  - id: bare
    catalog: default
    storage: bare
    table: bare
    geometry: geom
    pk: id
"#
    )
}

fn build_ctx(config_yaml: &str) -> Arc<AppContext> {
    let config: AppConfig = serde_yaml::from_str(config_yaml).unwrap();
    config.validate().unwrap();

    let mut registry = Registry::new();
    registry.register(Arc::new(AssetCapableFactory));
    registry.register(Arc::new(BareFactory));

    let core_router = CoreRouter::build(&config, &registry).unwrap();
    let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1024));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    Arc::new(AppContext::new(
        config,
        core_router,
        resolver,
        None,
        cache,
        style_store,
    ))
}

fn build_app(ctx: Arc<AppContext>) -> axum::Router {
    tellurion_stac::router().with_state(ctx)
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn body_bytes(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

fn repr_digest_header(bytes: &[u8]) -> String {
    let digest = tellurion_core::compute_sha256(bytes);
    format!("sha-256=:{}:", tellurion_core::encode_base64(&digest.value))
}

async fn put_metadata(
    app: &axum::Router,
    path: &str,
    digest_header: Option<&str>,
    body: Value,
) -> Response {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(digest) = digest_header {
        builder = builder.header("repr-digest", digest);
    }
    let request = builder.body(Body::from(body.to_string())).unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn put_data(app: &axum::Router, path: &str, bytes: &[u8]) -> Response {
    let request = Request::builder()
        .method("PUT")
        .uri(path)
        .body(Body::from(bytes.to_vec()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn get(app: &axum::Router, path: &str) -> Response {
    let request = Request::builder().uri(path).body(Body::empty()).unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn delete(app: &axum::Router, path: &str) -> Response {
    let request = Request::builder()
        .method("DELETE")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn post(app: &axum::Router, path: &str) -> Response {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

/// `PATCH .../data/uploads` with the `Upload-Offset` request header the
/// resumable-upload append verb requires (`asset_handlers.rs`'s own doc).
async fn patch_chunk(app: &axum::Router, path: &str, offset: u64, bytes: &[u8]) -> Response {
    let request = Request::builder()
        .method("PATCH")
        .uri(path)
        .header("upload-offset", offset.to_string())
        .body(Body::from(bytes.to_vec()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

/// The exact request-target (path only, no query string) an `S3ObjectStore`
/// presigned URL points at — `MockS3::seed` needs this, not the full signed
/// URL, since these tests never dereference the URL over the network (see
/// `MockS3`'s own doc).
fn presign_path(href: &str) -> String {
    let after_scheme = href.split_once("://").map(|(_, rest)| rest).unwrap_or(href);
    let after_host = after_scheme
        .split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or("");
    let path_only = after_host.split('?').next().unwrap_or(after_host);
    format!("/{path_only}")
}

/// Register -> upload -> available round trip, with the digest verified,
/// at collection level.
#[tokio::test]
async fn managed_round_trip_at_collection_level() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let payload = b"hello";
    let digest_header = repr_digest_header(payload);

    let register = put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&digest_header),
        json!({"type": "image/png", "title": "Thumbnail", "file:size": payload.len()}),
    )
    .await;
    assert_eq!(register.status(), StatusCode::OK);
    let body = body_json(register).await;
    assert_eq!(body["status"], "pending");

    let upload = put_data(&app, "/collections/demo/assets/thumb/data", payload).await;
    assert_eq!(upload.status(), StatusCode::OK);
    let body = body_json(upload).await;
    assert_eq!(body["status"], "available");

    let metadata = get(&app, "/collections/demo/assets/thumb").await;
    assert_eq!(metadata.status(), StatusCode::OK);
    let body = body_json(metadata).await;
    assert_eq!(body["status"], "available");
    assert!(body["href"].as_str().unwrap().ends_with("/data"));

    let data = get(&app, "/collections/demo/assets/thumb/data").await;
    assert_eq!(data.status(), StatusCode::OK);
    assert_eq!(
        data.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    assert_eq!(body_bytes(data).await, payload);
}

/// The identical round trip at item level, on a different route, proving
/// the two scopes are independent (the same key at collection level is
/// untouched).
#[tokio::test]
async fn managed_round_trip_at_item_level() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let payload = b"item bytes";
    let digest_header = repr_digest_header(payload);

    let register = put_metadata(
        &app,
        "/collections/demo/items/feature-1/assets/thumb",
        Some(&digest_header),
        json!({"type": "image/png", "file:size": payload.len()}),
    )
    .await;
    assert_eq!(register.status(), StatusCode::OK);

    let upload = put_data(
        &app,
        "/collections/demo/items/feature-1/assets/thumb/data",
        payload,
    )
    .await;
    assert_eq!(upload.status(), StatusCode::OK);

    let data = get(&app, "/collections/demo/items/feature-1/assets/thumb/data").await;
    assert_eq!(data.status(), StatusCode::OK);
    assert_eq!(body_bytes(data).await, payload);

    // The collection-level key of the same name was never registered.
    let collection_level = get(&app, "/collections/demo/assets/thumb").await;
    assert_eq!(collection_level.status(), StatusCode::NOT_FOUND);
}

/// A digest mismatch fails the asset by name: a `422` naming the mismatch,
/// the record durably `failed`, and nothing written to the object store.
#[tokio::test]
async fn digest_mismatch_fails_the_asset_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let declared_digest = repr_digest_header(b"expected bytes");
    put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&declared_digest),
        json!({"type": "image/png", "file:size": 14}),
    )
    .await;

    let upload = put_data(
        &app,
        "/collections/demo/assets/thumb/data",
        b"different!!!!!",
    )
    .await;
    assert_eq!(upload.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_json(upload).await;
    assert_eq!(body["status"], 422);

    let metadata = get(&app, "/collections/demo/assets/thumb").await;
    let body = body_json(metadata).await;
    assert_eq!(body["status"], "failed");
    assert!(body["status_detail"].is_string());

    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "a digest mismatch must never leave bytes on disk"
    );
}

/// A second registration at the same key with a different declaration
/// refuses `409`.
#[tokio::test]
async fn conflicting_registration_refuses_409() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let first = put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&repr_digest_header(b"aaaaa")),
        json!({"type": "image/png", "file:size": 5}),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);

    let second = put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&repr_digest_header(b"bbbbb")),
        json!({"type": "image/png", "file:size": 5}),
    )
    .await;
    assert_eq!(second.status(), StatusCode::CONFLICT);
}

/// A media type outside the collection's configured allow-list refuses
/// `415`.
#[tokio::test]
async fn media_type_outside_the_allow_list_refuses_415() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let response = put_metadata(
        &app,
        "/collections/demo/assets/bad",
        Some(&repr_digest_header(b"aaaaa")),
        json!({"type": "application/x-executable", "file:size": 5}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

/// A declared size over the collection's configured cap refuses `413`,
/// before any storage I/O — the existing streamed-length body-cap
/// machinery, reused for the declared-size check at registration.
#[tokio::test]
async fn declared_size_over_the_cap_refuses_413() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    // `settings.max_asset_bytes: 20` in the fixture config.
    let response = put_metadata(
        &app,
        "/collections/demo/assets/big",
        Some(&repr_digest_header(b"aaaaa")),
        json!({"type": "image/png", "file:size": 999}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// A remote asset registers available with no byte lifecycle; deleting it
/// removes only the record.
#[tokio::test]
async fn remote_asset_register_and_delete_never_touches_the_object_store() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let register = put_metadata(
        &app,
        "/collections/demo/assets/external",
        None,
        json!({"href": "https://example.test/x.tif", "type": "image/png"}),
    )
    .await;
    assert_eq!(register.status(), StatusCode::OK);
    let body = body_json(register).await;
    assert_eq!(body["status"], "available");
    assert_eq!(body["href"], "https://example.test/x.tif");

    let removed = delete(&app, "/collections/demo/assets/external").await;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);

    let after = get(&app, "/collections/demo/assets/external").await;
    assert_eq!(after.status(), StatusCode::NOT_FOUND);

    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "a remote asset's delete must never touch the object store"
    );
}

/// A config-declared asset is visible on this same read surface (declared
/// assets ARE remote assets that happen to live in config), always
/// available, and read-only through this API.
#[tokio::test]
async fn declared_assets_are_unified_onto_the_read_surface_and_are_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let read = get(&app, "/collections/demo/assets/declared-thumb").await;
    assert_eq!(read.status(), StatusCode::OK);
    let body = body_json(read).await;
    assert_eq!(body["status"], "available");
    assert_eq!(body["href"], "https://example.test/declared.png");

    let put = put_metadata(
        &app,
        "/collections/demo/assets/declared-thumb",
        None,
        json!({"href": "https://example.test/overwrite.png"}),
    )
    .await;
    assert_eq!(put.status(), StatusCode::CONFLICT);

    let del = delete(&app, "/collections/demo/assets/declared-thumb").await;
    assert_eq!(del.status(), StatusCode::CONFLICT);
}

/// A collection whose anchor driver never advertises `AssetRecordStore` at
/// all refuses by name (`CapabilityUnsupported` -> `404` naming
/// `'assets'`), never a panic or a 500.
#[tokio::test]
async fn a_driver_with_no_asset_capability_refuses_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let response = get(&app, "/collections/bare/assets/anything").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    assert!(
        body["detail"].as_str().unwrap().contains("assets"),
        "detail was: {body}"
    );
}

// -- presigned-upload conformance class ---------------------------------

/// `fs`-profile collections refuse the `presigned-upload` class by name —
/// `fs` has no URL space of its own to mint a signed URL against
/// (`tellurion_core::objectstore::ObjectStore::as_presigned`'s own doc).
/// Both the upload negotiation (`PUT .../data/presign`) and the download
/// negotiation (`GET .../data/presign`) refuse the identical way.
#[tokio::test]
async fn fs_profile_presign_is_refused_by_name() {
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_FS_REFUSAL",
        "test-access",
    );
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_SECRET_KEY_FS_REFUSAL",
        "test-secret",
    );
    let dir = tempfile::tempdir().unwrap();
    let mock = MockS3::spawn();
    let ctx = build_ctx(&build_config_yaml_with_s3(
        &dir.path().to_string_lossy(),
        &mock.endpoint(),
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_FS_REFUSAL",
        "TELLURION_STAC_TEST_S3_SECRET_KEY_FS_REFUSAL",
    ));
    let app = build_app(ctx);

    put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&repr_digest_header(b"hello")),
        json!({"type": "image/png", "file:size": 5}),
    )
    .await;

    let put_presign = Request::builder()
        .method("PUT")
        .uri("/collections/demo/assets/thumb/data/presign")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(put_presign).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    assert!(
        body["detail"]
            .as_str()
            .unwrap()
            .contains("presigned-upload"),
        "detail was: {body}"
    );

    let get_presign = get(&app, "/collections/demo/assets/thumb/data/presign").await;
    assert_eq!(get_presign.status(), StatusCode::NOT_FOUND);

    std::env::remove_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_FS_REFUSAL");
    std::env::remove_var("TELLURION_STAC_TEST_S3_SECRET_KEY_FS_REFUSAL");
}

/// The full presigned-upload round trip through the real HTTP handlers and
/// a real `S3ObjectStore` signing real requests against a loopback mock:
/// register (pending) -> negotiate a presigned `PUT` -> the client's
/// out-of-band transfer (simulated via `MockS3::seed` at the presigned
/// URL's own path, never by dereferencing the URL — see `MockS3`'s own
/// doc) -> finalize verifies via a real signed `HEAD` -> available.
#[tokio::test]
async fn presigned_upload_round_trip_register_presign_finalize_available() {
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_ROUND_TRIP",
        "test-access",
    );
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_SECRET_KEY_ROUND_TRIP",
        "test-secret",
    );
    let dir = tempfile::tempdir().unwrap();
    let mock = MockS3::spawn();
    let ctx = build_ctx(&build_config_yaml_with_s3(
        &dir.path().to_string_lossy(),
        &mock.endpoint(),
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_ROUND_TRIP",
        "TELLURION_STAC_TEST_S3_SECRET_KEY_ROUND_TRIP",
    ));
    let app = build_app(ctx);

    let payload = b"presigned round trip bytes";
    let register = put_metadata(
        &app,
        "/collections/s3demo/assets/thumb",
        Some(&repr_digest_header(payload)),
        json!({"type": "image/png", "file:size": payload.len()}),
    )
    .await;
    assert_eq!(register.status(), StatusCode::OK);
    let body = body_json(register).await;
    assert_eq!(body["status"], "pending");

    let put_presign = Request::builder()
        .method("PUT")
        .uri("/collections/s3demo/assets/thumb/data/presign")
        .body(Body::empty())
        .unwrap();
    let presign_response = app.clone().oneshot(put_presign).await.unwrap();
    assert_eq!(presign_response.status(), StatusCode::OK);
    let presign_body = body_json(presign_response).await;
    assert_eq!(presign_body["method"], "PUT");
    assert_eq!(presign_body["expires_in_s"], 300);
    let href = presign_body["href"].as_str().unwrap().to_string();
    assert!(href.contains("X-Amz-Signature="), "href was: {href}");
    assert!(href.contains("X-Amz-Expires=300"), "href was: {href}");

    // The client's own out-of-band transfer, simulated by placing bytes at
    // the exact target the presigned URL names — this test never
    // dereferences `href` itself.
    mock.seed(&presign_path(&href), payload);

    let finalize = post(&app, "/collections/s3demo/assets/thumb/finalize").await;
    assert_eq!(finalize.status(), StatusCode::OK);
    let finalize_body = body_json(finalize).await;
    assert_eq!(finalize_body["status"], "available");

    let metadata = get(&app, "/collections/s3demo/assets/thumb").await;
    let metadata_body = body_json(metadata).await;
    assert_eq!(metadata_body["status"], "available");

    // The read-side presign (download) negotiation works identically for
    // an available managed asset on the same `s3`-profile store.
    let get_presign = get(&app, "/collections/s3demo/assets/thumb/data/presign").await;
    assert_eq!(get_presign.status(), StatusCode::OK);
    let get_presign_body = body_json(get_presign).await;
    assert_eq!(get_presign_body["method"], "GET");

    std::env::remove_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_ROUND_TRIP");
    std::env::remove_var("TELLURION_STAC_TEST_S3_SECRET_KEY_ROUND_TRIP");
}

/// Finalizing a presigned upload the client never actually transferred
/// fails by name: the real signed `HEAD` against the mock reports the
/// object absent, and the asset flips `pending` -> `failed` (never left
/// dangling in `pending`).
#[tokio::test]
async fn finalize_presigned_upload_fails_by_name_when_nothing_was_uploaded() {
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_FAILS_BY_NAME",
        "test-access",
    );
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_SECRET_KEY_FAILS_BY_NAME",
        "test-secret",
    );
    let dir = tempfile::tempdir().unwrap();
    let mock = MockS3::spawn();
    let ctx = build_ctx(&build_config_yaml_with_s3(
        &dir.path().to_string_lossy(),
        &mock.endpoint(),
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_FAILS_BY_NAME",
        "TELLURION_STAC_TEST_S3_SECRET_KEY_FAILS_BY_NAME",
    ));
    let app = build_app(ctx);

    put_metadata(
        &app,
        "/collections/s3demo/assets/thumb",
        Some(&repr_digest_header(b"never uploaded")),
        json!({"type": "image/png", "file:size": 14}),
    )
    .await;

    // No `presign` call needed here — finalize's own `HEAD` is what fails,
    // regardless of whether the client ever negotiated a URL first.
    let finalize = post(&app, "/collections/s3demo/assets/thumb/finalize").await;
    assert_eq!(finalize.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let metadata = get(&app, "/collections/s3demo/assets/thumb").await;
    let body = body_json(metadata).await;
    assert_eq!(body["status"], "failed");

    std::env::remove_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_FAILS_BY_NAME");
    std::env::remove_var("TELLURION_STAC_TEST_S3_SECRET_KEY_FAILS_BY_NAME");
}

// -- resumable-upload conformance class ----------------------------------

/// Full resumable round trip against the real `fs` store, through the real
/// HTTP handlers: register (pending) -> create the upload resource ->
/// append in two separate chunks -> complete -> available, with the digest
/// verified exactly like the direct-upload lane's own round trip.
#[tokio::test]
async fn resumable_round_trip_register_create_append_complete_available() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let payload = b"resumable!!";
    let digest_header = repr_digest_header(payload);

    let register = put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&digest_header),
        json!({"type": "image/png", "file:size": payload.len()}),
    )
    .await;
    assert_eq!(register.status(), StatusCode::OK);

    let create = post(&app, "/collections/demo/assets/thumb/data/uploads").await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let create_body = body_json(create).await;
    assert_eq!(create_body["offset"], 0);

    let first = patch_chunk(
        &app,
        "/collections/demo/assets/thumb/data/uploads",
        0,
        &payload[..5],
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(body_json(first).await["offset"], 5);

    let second = patch_chunk(
        &app,
        "/collections/demo/assets/thumb/data/uploads",
        5,
        &payload[5..],
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(body_json(second).await["offset"], payload.len() as u64);

    let complete = post(&app, "/collections/demo/assets/thumb/data/uploads/complete").await;
    assert_eq!(complete.status(), StatusCode::OK);
    let complete_body = body_json(complete).await;
    assert_eq!(complete_body["status"], "available");

    let data = get(&app, "/collections/demo/assets/thumb/data").await;
    assert_eq!(data.status(), StatusCode::OK);
    assert_eq!(body_bytes(data).await, payload);
}

/// `GET .../data/uploads` (offset probe, HEAD-style) reports the true
/// accumulated length mid-upload — before any chunk, right after the first,
/// and `404` once nothing is in progress at all.
#[tokio::test]
async fn offset_probe_reports_progress_mid_upload() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&repr_digest_header(b"abcdefgh")),
        json!({"type": "image/png", "file:size": 8}),
    )
    .await;

    let before_create = get(&app, "/collections/demo/assets/thumb/data/uploads").await;
    assert_eq!(before_create.status(), StatusCode::NOT_FOUND);

    post(&app, "/collections/demo/assets/thumb/data/uploads").await;
    let just_created = get(&app, "/collections/demo/assets/thumb/data/uploads").await;
    assert_eq!(just_created.status(), StatusCode::OK);
    assert_eq!(body_json(just_created).await["offset"], 0);

    patch_chunk(
        &app,
        "/collections/demo/assets/thumb/data/uploads",
        0,
        b"abcd",
    )
    .await;
    let mid_upload = get(&app, "/collections/demo/assets/thumb/data/uploads").await;
    assert_eq!(mid_upload.status(), StatusCode::OK);
    assert_eq!(body_json(mid_upload).await["offset"], 4);
}

/// An append whose `Upload-Offset` header names a position past what has
/// actually accumulated (a gap) is refused `409`, named "out-of-order".
#[tokio::test]
async fn out_of_order_append_refuses_409_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&repr_digest_header(b"abcdefgh")),
        json!({"type": "image/png", "file:size": 8}),
    )
    .await;
    post(&app, "/collections/demo/assets/thumb/data/uploads").await;
    patch_chunk(
        &app,
        "/collections/demo/assets/thumb/data/uploads",
        0,
        b"ab",
    )
    .await;

    let response = patch_chunk(&app, "/collections/demo/assets/thumb/data/uploads", 6, b"x").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = body_json(response).await;
    assert!(
        body["detail"].as_str().unwrap().contains("out-of-order"),
        "detail was: {body}"
    );
}

/// An append whose `Upload-Offset` header names a position the server has
/// already moved past (a retry of a stale position) is refused `409`,
/// named "stale".
#[tokio::test]
async fn stale_offset_append_refuses_409_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&repr_digest_header(b"abcdefgh")),
        json!({"type": "image/png", "file:size": 8}),
    )
    .await;
    post(&app, "/collections/demo/assets/thumb/data/uploads").await;
    patch_chunk(
        &app,
        "/collections/demo/assets/thumb/data/uploads",
        0,
        b"ab",
    )
    .await;

    let response = patch_chunk(&app, "/collections/demo/assets/thumb/data/uploads", 0, b"x").await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = body_json(response).await;
    assert!(
        body["detail"].as_str().unwrap().contains("stale"),
        "detail was: {body}"
    );
}

/// An append that would push the accumulated total past the asset's own
/// declared size is refused `413` mid-stream, never buffered past the cap —
/// the resumable-upload counterpart of `declared_size_over_the_cap_refuses_413`,
/// which covers the up-front (registration-time) half of the same rule.
#[tokio::test]
async fn append_exceeding_the_declared_total_refuses_413_mid_stream() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    // `file:size: 10`, comfortably under the fixture's own
    // `max_asset_bytes: 20` cap — this test is about the per-asset declared
    // size, not the collection-wide cap.
    put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&repr_digest_header(b"0123456789")),
        json!({"type": "image/png", "file:size": 10}),
    )
    .await;
    post(&app, "/collections/demo/assets/thumb/data/uploads").await;
    let first = patch_chunk(
        &app,
        "/collections/demo/assets/thumb/data/uploads",
        0,
        b"012345",
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);

    // 6 bytes already accumulated + a 5-byte chunk would total 11, past the
    // 10-byte declared size.
    let second = patch_chunk(
        &app,
        "/collections/demo/assets/thumb/data/uploads",
        6,
        b"67890",
    )
    .await;
    assert_eq!(second.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // Never buffered past the cap — the offset is exactly where the refused
    // append found it.
    let probe = get(&app, "/collections/demo/assets/thumb/data/uploads").await;
    assert_eq!(body_json(probe).await["offset"], 6);
}

/// Deleting an incomplete upload discards it (idempotently); a fresh upload
/// on the same still-`pending` asset then starts clean and completes
/// normally.
#[tokio::test]
async fn deleting_an_incomplete_upload_lets_a_fresh_one_start_clean() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let payload = b"final bytes";
    let digest_header = repr_digest_header(payload);
    put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&digest_header),
        json!({"type": "image/png", "file:size": payload.len()}),
    )
    .await;

    post(&app, "/collections/demo/assets/thumb/data/uploads").await;
    patch_chunk(
        &app,
        "/collections/demo/assets/thumb/data/uploads",
        0,
        b"stale junk!",
    )
    .await;

    let deleted = delete(&app, "/collections/demo/assets/thumb/data/uploads").await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    // Idempotent — deleting again (nothing left) is still `204`.
    let deleted_again = delete(&app, "/collections/demo/assets/thumb/data/uploads").await;
    assert_eq!(deleted_again.status(), StatusCode::NO_CONTENT);
    let probe = get(&app, "/collections/demo/assets/thumb/data/uploads").await;
    assert_eq!(probe.status(), StatusCode::NOT_FOUND);

    // The asset itself is untouched — still pending — so a fresh upload on
    // the same key starts clean and completes normally.
    let recreate = post(&app, "/collections/demo/assets/thumb/data/uploads").await;
    assert_eq!(recreate.status(), StatusCode::CREATED);
    assert_eq!(body_json(recreate).await["offset"], 0);
    patch_chunk(
        &app,
        "/collections/demo/assets/thumb/data/uploads",
        0,
        payload,
    )
    .await;
    let complete = post(&app, "/collections/demo/assets/thumb/data/uploads/complete").await;
    assert_eq!(complete.status(), StatusCode::OK);
    assert_eq!(body_json(complete).await["status"], "available");
}

/// A digest mismatch at completion fails the asset by name: `422`, the
/// record durably `failed`, and nothing written to the object store — the
/// same contract `digest_mismatch_fails_the_asset_by_name` already proves
/// for the direct-upload lane, since `complete_resumable_upload` delegates
/// into the exact same verification.
#[tokio::test]
async fn digest_mismatch_at_complete_fails_the_asset_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let declared_digest = repr_digest_header(b"expected bytes");
    put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&declared_digest),
        json!({"type": "image/png", "file:size": 14}),
    )
    .await;
    post(&app, "/collections/demo/assets/thumb/data/uploads").await;
    patch_chunk(
        &app,
        "/collections/demo/assets/thumb/data/uploads",
        0,
        b"different!!!!!",
    )
    .await;

    let complete = post(&app, "/collections/demo/assets/thumb/data/uploads/complete").await;
    assert_eq!(complete.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let metadata = get(&app, "/collections/demo/assets/thumb").await;
    let body = body_json(metadata).await;
    assert_eq!(body["status"], "failed");
    assert!(body["status_detail"].is_string());

    assert!(
        std::fs::read_dir(dir.path()).unwrap().all(|entry| {
            // Only real completed objects count — the now-consumed
            // `.upload` staging file, if any survived, would also show up
            // here.
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".upload")
        }),
        "a digest mismatch must never leave the object written on disk"
    );
}

/// A digest mismatch on the `s3`-profile resumable-upload completion path
/// deletes the bytes `take_upload`'s own `CompleteMultipartUpload` already
/// committed to the real key before this function ever gets a chance to
/// check them — the `s3`-backed counterpart of
/// `digest_mismatch_at_complete_fails_the_asset_by_name`, and the one
/// regression guard that actually drives the interaction between
/// `S3ObjectStore` and `asset::finish_upload` this defect lived in end to
/// end: for `fs` a mismatch's delete is a no-op cleanup of something never
/// written; for `s3` it is the only thing standing between "digest
/// mismatch" and wrong bytes staying readable at this asset's key forever.
#[tokio::test]
async fn s3_profile_digest_mismatch_at_complete_deletes_the_already_committed_bytes() {
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_S3_DIGEST_MISMATCH",
        "test-access",
    );
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_SECRET_KEY_S3_DIGEST_MISMATCH",
        "test-secret",
    );
    let dir = tempfile::tempdir().unwrap();
    let mock = MockS3::spawn();
    let ctx = build_ctx(&build_config_yaml_with_s3(
        &dir.path().to_string_lossy(),
        &mock.endpoint(),
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_S3_DIGEST_MISMATCH",
        "TELLURION_STAC_TEST_S3_SECRET_KEY_S3_DIGEST_MISMATCH",
    ));
    let app = build_app(ctx);

    let declared_digest = repr_digest_header(b"expected bytes");
    put_metadata(
        &app,
        "/collections/s3demo/assets/thumb",
        Some(&declared_digest),
        json!({"type": "image/png", "file:size": 14}),
    )
    .await;

    // Learn the real backing key without touching the asset's own state:
    // presigning is idempotent and side-effect-free (`asset::presign_upload`'s
    // own doc), so calling it ahead of the resumable-upload transport this
    // test actually exercises never interferes with what follows.
    let presign = Request::builder()
        .method("PUT")
        .uri("/collections/s3demo/assets/thumb/data/presign")
        .body(Body::empty())
        .unwrap();
    let presign_response = app.clone().oneshot(presign).await.unwrap();
    let href = body_json(presign_response).await["href"]
        .as_str()
        .unwrap()
        .to_string();
    let object_path = presign_path(&href);

    post(&app, "/collections/s3demo/assets/thumb/data/uploads").await;
    patch_chunk(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads",
        0,
        b"different!!!!!",
    )
    .await;

    let complete = post(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads/complete",
    )
    .await;
    assert_eq!(complete.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let metadata = get(&app, "/collections/s3demo/assets/thumb").await;
    let body = body_json(metadata).await;
    assert_eq!(body["status"], "failed");
    assert!(body["status_detail"].is_string());

    assert!(
        !mock.contains(&object_path),
        "take_upload's own CompleteMultipartUpload already committed the wrong bytes at the \
         real key; the digest-mismatch cleanup must delete them, the same invariant every \
         other transport gets for free"
    );

    std::env::remove_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_S3_DIGEST_MISMATCH");
    std::env::remove_var("TELLURION_STAC_TEST_S3_SECRET_KEY_S3_DIGEST_MISMATCH");
}

/// The successor-admission race reproduced end to end through the real HTTP
/// handlers and a real network round trip, rather than by calling store
/// internals directly the way `tellurion_core::objectstore::s3_tests::
/// resumable_s3_successor_upload_stays_refused_while_a_mismatched_attempt_is_still_verifying`
/// already does at that level: a first attempt uploads wrong bytes and
/// completes; its own digest-mismatch cleanup issues a real `DELETE` this
/// test pauses via `MockS3::arm_delete_pause`'s fault-injection hook. While
/// that `DELETE` is parked — the wrong bytes already committed at the real
/// key, the record still `pending` because `finish_upload` only marks it
/// `failed` once this very delete returns — a second, legitimate
/// `create_resumable_upload` for the very same key must stay refused. Proves
/// the "verifying" hold survives an actual network round trip, not merely
/// the in-process call stack that issues it, which is all the pre-fix code
/// ever protected.
#[tokio::test]
async fn s3_profile_successor_upload_stays_refused_while_a_mismatched_completion_deletes_over_the_wire(
) {
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_SUCCESSOR_RACE",
        "test-access",
    );
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_SECRET_KEY_SUCCESSOR_RACE",
        "test-secret",
    );
    let dir = tempfile::tempdir().unwrap();
    let mock = MockS3::spawn();
    let ctx = build_ctx(&build_config_yaml_with_s3(
        &dir.path().to_string_lossy(),
        &mock.endpoint(),
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_SUCCESSOR_RACE",
        "TELLURION_STAC_TEST_S3_SECRET_KEY_SUCCESSOR_RACE",
    ));
    let app = build_app(ctx);

    let correct = b"the right eventual bytes";
    let wrong: Vec<u8> = correct.iter().map(|byte| byte.wrapping_add(1)).collect();

    put_metadata(
        &app,
        "/collections/s3demo/assets/thumb",
        Some(&repr_digest_header(correct)),
        json!({"type": "image/png", "file:size": correct.len()}),
    )
    .await;

    post(&app, "/collections/s3demo/assets/thumb/data/uploads").await;
    patch_chunk(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads",
        0,
        &wrong,
    )
    .await;

    let (parked_rx, release_tx) = mock.arm_delete_pause();
    let complete_app = app.clone();
    let complete_task = tokio::spawn(async move {
        post(
            &complete_app,
            "/collections/s3demo/assets/thumb/data/uploads/complete",
        )
        .await
    });

    // Wait until attempt one's own mismatch cleanup has actually reached
    // the real DELETE and is parked there — never a fixed sleep guessing at
    // it. The wrong bytes are already committed at the real key
    // (`take_upload`'s own `CompleteMultipartUpload`), but the record is
    // still `pending`: `finish_upload` only marks it `failed` once this
    // very delete returns.
    parked_rx.await.expect("mock parked on the DELETE");

    let mid_flight = get(&app, "/collections/s3demo/assets/thumb").await;
    assert_eq!(body_json(mid_flight).await["status"], "pending");

    // The key assertion: a second attempt must stay refused while that
    // DELETE is still in flight over the wire — not merely while the
    // in-process call issuing it is still on the stack.
    let successor = post(&app, "/collections/s3demo/assets/thumb/data/uploads").await;
    assert_eq!(successor.status(), StatusCode::CONFLICT);

    release_tx.send(()).expect("release the parked DELETE");
    let complete = complete_task.await.expect("complete task did not panic");
    assert_eq!(complete.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let metadata = get(&app, "/collections/s3demo/assets/thumb").await;
    assert_eq!(body_json(metadata).await["status"], "failed");

    std::env::remove_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_SUCCESSOR_RACE");
    std::env::remove_var("TELLURION_STAC_TEST_S3_SECRET_KEY_SUCCESSOR_RACE");
}

/// Full resumable round trip against a real `S3ObjectStore` signing real
/// multipart-upload requests against the loopback mock, through the real
/// HTTP handlers: register (pending) -> create the upload resource ->
/// append in two chunks -> complete -> available — the `s3`-backed
/// counterpart of `resumable_round_trip_register_create_append_complete_available`.
/// Stays well under `S3_MULTIPART_PART_FLOOR` (real threshold-crossing is
/// proven store-level, in `tellurion_core::objectstore::S3ObjectStore`'s
/// own test suite, where a 5&nbsp;MiB-plus payload is cheap to build and
/// drive against a loopback mock without an HTTP framework in the way) —
/// this test's job is the HTTP wiring, not the part math.
#[tokio::test]
async fn s3_profile_resumable_round_trip_register_create_append_complete_available() {
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_RESUMABLE_ROUND_TRIP",
        "test-access",
    );
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_SECRET_KEY_RESUMABLE_ROUND_TRIP",
        "test-secret",
    );
    let dir = tempfile::tempdir().unwrap();
    let mock = MockS3::spawn();
    let ctx = build_ctx(&build_config_yaml_with_s3(
        &dir.path().to_string_lossy(),
        &mock.endpoint(),
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_RESUMABLE_ROUND_TRIP",
        "TELLURION_STAC_TEST_S3_SECRET_KEY_RESUMABLE_ROUND_TRIP",
    ));
    let app = build_app(ctx);

    let payload = b"resumable via real s3 multipart!!";
    let register = put_metadata(
        &app,
        "/collections/s3demo/assets/thumb",
        Some(&repr_digest_header(payload)),
        json!({"type": "image/png", "file:size": payload.len()}),
    )
    .await;
    assert_eq!(register.status(), StatusCode::OK);

    let create = post(&app, "/collections/s3demo/assets/thumb/data/uploads").await;
    assert_eq!(create.status(), StatusCode::CREATED);
    assert_eq!(body_json(create).await["offset"], 0);

    let first = patch_chunk(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads",
        0,
        &payload[..10],
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(body_json(first).await["offset"], 10);

    let second = patch_chunk(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads",
        10,
        &payload[10..],
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(body_json(second).await["offset"], payload.len() as u64);

    let complete = post(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads/complete",
    )
    .await;
    assert_eq!(complete.status(), StatusCode::OK);
    assert_eq!(body_json(complete).await["status"], "available");

    // `s3demo` also earns the `download-redirect` class, so `GET .../data`
    // answers `307` to a presigned `GET` rather than proxying bytes — the
    // metadata resource is what proves the real upload landed.
    let data = get(&app, "/collections/s3demo/assets/thumb/data").await;
    assert_eq!(data.status(), StatusCode::TEMPORARY_REDIRECT);
    let metadata = get(&app, "/collections/s3demo/assets/thumb").await;
    assert_eq!(body_json(metadata).await["status"], "available");

    std::env::remove_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_RESUMABLE_ROUND_TRIP");
    std::env::remove_var("TELLURION_STAC_TEST_S3_SECRET_KEY_RESUMABLE_ROUND_TRIP");
}

/// Offset mismatch (both directions) on an `s3`-profile resumable upload
/// answers the identical named `409`s the `fs` profile's own
/// `out_of_order_append_refuses_409_by_name`/`stale_offset_append_refuses_
/// 409_by_name` already prove — the offset CAS lives inside
/// `S3ObjectStore::append_upload` itself, not duplicated per profile at the
/// HTTP layer.
#[tokio::test]
async fn s3_profile_resumable_offset_mismatch_refuses_409_by_name() {
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_OFFSET_MISMATCH",
        "test-access",
    );
    std::env::set_var(
        "TELLURION_STAC_TEST_S3_SECRET_KEY_OFFSET_MISMATCH",
        "test-secret",
    );
    let dir = tempfile::tempdir().unwrap();
    let mock = MockS3::spawn();
    let ctx = build_ctx(&build_config_yaml_with_s3(
        &dir.path().to_string_lossy(),
        &mock.endpoint(),
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_OFFSET_MISMATCH",
        "TELLURION_STAC_TEST_S3_SECRET_KEY_OFFSET_MISMATCH",
    ));
    let app = build_app(ctx);

    put_metadata(
        &app,
        "/collections/s3demo/assets/thumb",
        Some(&repr_digest_header(b"abcdefgh")),
        json!({"type": "image/png", "file:size": 8}),
    )
    .await;
    post(&app, "/collections/s3demo/assets/thumb/data/uploads").await;
    patch_chunk(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads",
        0,
        b"ab",
    )
    .await;

    let out_of_order = patch_chunk(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads",
        6,
        b"x",
    )
    .await;
    assert_eq!(out_of_order.status(), StatusCode::CONFLICT);

    let stale = patch_chunk(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads",
        0,
        b"x",
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    std::env::remove_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_OFFSET_MISMATCH");
    std::env::remove_var("TELLURION_STAC_TEST_S3_SECRET_KEY_OFFSET_MISMATCH");
}

/// `DELETE .../data/uploads` on an `s3`-profile resumable upload really
/// aborts the underlying multipart upload (proven directly, with the mock
/// asserting the abort arrived, in
/// `tellurion_core::objectstore::S3ObjectStore`'s own
/// `abandon_upload_aborts_the_multipart_upload_on_the_store`) — this is the
/// HTTP-level counterpart: deleting an incomplete upload lets a fresh one
/// on the same still-`pending` asset start clean and complete normally.
#[tokio::test]
async fn s3_profile_abandoning_an_incomplete_upload_lets_a_fresh_one_start_clean() {
    std::env::set_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_ABANDON", "test-access");
    std::env::set_var("TELLURION_STAC_TEST_S3_SECRET_KEY_ABANDON", "test-secret");
    let dir = tempfile::tempdir().unwrap();
    let mock = MockS3::spawn();
    let ctx = build_ctx(&build_config_yaml_with_s3(
        &dir.path().to_string_lossy(),
        &mock.endpoint(),
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_ABANDON",
        "TELLURION_STAC_TEST_S3_SECRET_KEY_ABANDON",
    ));
    let app = build_app(ctx);

    let payload = b"clean restart";
    put_metadata(
        &app,
        "/collections/s3demo/assets/thumb",
        Some(&repr_digest_header(payload)),
        json!({"type": "image/png", "file:size": payload.len()}),
    )
    .await;
    post(&app, "/collections/s3demo/assets/thumb/data/uploads").await;
    patch_chunk(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads",
        0,
        b"stale junk",
    )
    .await;

    let deleted = delete(&app, "/collections/s3demo/assets/thumb/data/uploads").await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let probe = get(&app, "/collections/s3demo/assets/thumb/data/uploads").await;
    assert_eq!(probe.status(), StatusCode::NOT_FOUND);

    let recreate = post(&app, "/collections/s3demo/assets/thumb/data/uploads").await;
    assert_eq!(recreate.status(), StatusCode::CREATED);
    assert_eq!(body_json(recreate).await["offset"], 0);
    patch_chunk(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads",
        0,
        payload,
    )
    .await;
    let complete = post(
        &app,
        "/collections/s3demo/assets/thumb/data/uploads/complete",
    )
    .await;
    assert_eq!(complete.status(), StatusCode::OK);
    assert_eq!(body_json(complete).await["status"], "available");

    std::env::remove_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_ABANDON");
    std::env::remove_var("TELLURION_STAC_TEST_S3_SECRET_KEY_ABANDON");
}

// -- download-redirect conformance class (`s3`-profile stores only) -----

/// An `available` managed asset on an `s3`-profile collection: `GET
/// .../data` answers `307 Temporary Redirect` with a `Location` header
/// shaped like a real presigned `GET` URL — never proxies bytes through
/// this server. The URL's exact signature is time-dependent (this handler
/// signs against `SystemTime::now()`, the same choice every other presign
/// call site in this module already makes — `s3_presign_shape_is_deterministic_at_a_fixed_clock`
/// in `tellurion-core::objectstore` is the golden, clock-fixed test for the
/// signing primitive itself), so this asserts the URL's *shape* — the
/// mock's own host, the SigV4 query-parameter family, and a signature that
/// differs from the earlier upload presign's own.
#[tokio::test]
async fn download_redirect_answers_307_with_a_presigned_get_location_for_an_available_s3_asset() {
    std::env::set_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_REDIRECT", "test-access");
    std::env::set_var("TELLURION_STAC_TEST_S3_SECRET_KEY_REDIRECT", "test-secret");
    let dir = tempfile::tempdir().unwrap();
    let mock = MockS3::spawn();
    let ctx = build_ctx(&build_config_yaml_with_s3(
        &dir.path().to_string_lossy(),
        &mock.endpoint(),
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_REDIRECT",
        "TELLURION_STAC_TEST_S3_SECRET_KEY_REDIRECT",
    ));
    let app = build_app(ctx);

    let payload = b"redirect me";
    put_metadata(
        &app,
        "/collections/s3demo/assets/thumb",
        Some(&repr_digest_header(payload)),
        json!({"type": "image/png", "file:size": payload.len()}),
    )
    .await;
    let put_presign = Request::builder()
        .method("PUT")
        .uri("/collections/s3demo/assets/thumb/data/presign")
        .body(Body::empty())
        .unwrap();
    let presign_response = app.clone().oneshot(put_presign).await.unwrap();
    let put_href = body_json(presign_response).await["href"]
        .as_str()
        .unwrap()
        .to_string();
    mock.seed(&presign_path(&put_href), payload);
    let finalize = post(&app, "/collections/s3demo/assets/thumb/finalize").await;
    assert_eq!(finalize.status(), StatusCode::OK);

    let redirect = get(&app, "/collections/s3demo/assets/thumb/data").await;
    assert_eq!(redirect.status(), StatusCode::TEMPORARY_REDIRECT);
    let location = redirect
        .headers()
        .get(header::LOCATION)
        .expect("a 307 redirect carries a Location header")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        location.starts_with(&mock.endpoint()),
        "location was: {location}"
    );
    assert!(location.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
    assert!(location.contains("X-Amz-SignedHeaders=host"));
    assert!(location.contains("X-Amz-Signature="));
    assert_ne!(
        location, put_href,
        "the download redirect must sign a GET, never reuse the earlier upload PUT's own URL"
    );

    std::env::remove_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_REDIRECT");
    std::env::remove_var("TELLURION_STAC_TEST_S3_SECRET_KEY_REDIRECT");
}

/// A `pending` managed asset on an `s3`-profile collection never redirects
/// — `404`, the same named refusal the `fs` profile's own byte-proxy path
/// already gives a pending asset (nothing exists at the target yet, on
/// either profile).
#[tokio::test]
async fn pending_asset_on_s3_does_not_redirect() {
    std::env::set_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_PENDING", "test-access");
    std::env::set_var("TELLURION_STAC_TEST_S3_SECRET_KEY_PENDING", "test-secret");
    let dir = tempfile::tempdir().unwrap();
    let mock = MockS3::spawn();
    let ctx = build_ctx(&build_config_yaml_with_s3(
        &dir.path().to_string_lossy(),
        &mock.endpoint(),
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_PENDING",
        "TELLURION_STAC_TEST_S3_SECRET_KEY_PENDING",
    ));
    let app = build_app(ctx);

    put_metadata(
        &app,
        "/collections/s3demo/assets/thumb",
        Some(&repr_digest_header(b"hello")),
        json!({"type": "image/png", "file:size": 5}),
    )
    .await;

    let response = get(&app, "/collections/s3demo/assets/thumb/data").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    std::env::remove_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_PENDING");
    std::env::remove_var("TELLURION_STAC_TEST_S3_SECRET_KEY_PENDING");
}

/// A `failed` managed asset (a presigned finalize that found nothing at
/// the target) never redirects either — the same `404` by name, not just
/// `pending`.
#[tokio::test]
async fn failed_asset_on_s3_does_not_redirect() {
    std::env::set_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_FAILED", "test-access");
    std::env::set_var("TELLURION_STAC_TEST_S3_SECRET_KEY_FAILED", "test-secret");
    let dir = tempfile::tempdir().unwrap();
    let mock = MockS3::spawn();
    let ctx = build_ctx(&build_config_yaml_with_s3(
        &dir.path().to_string_lossy(),
        &mock.endpoint(),
        "TELLURION_STAC_TEST_S3_ACCESS_KEY_FAILED",
        "TELLURION_STAC_TEST_S3_SECRET_KEY_FAILED",
    ));
    let app = build_app(ctx);

    put_metadata(
        &app,
        "/collections/s3demo/assets/thumb",
        Some(&repr_digest_header(b"hello")),
        json!({"type": "image/png", "file:size": 5}),
    )
    .await;
    // Finalize without ever transferring bytes — the store's own HEAD sees
    // nothing, so this fails the asset by name (`asset::
    // finalize_presigned_upload`'s own doc).
    let finalize = post(&app, "/collections/s3demo/assets/thumb/finalize").await;
    assert_eq!(finalize.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let response = get(&app, "/collections/s3demo/assets/thumb/data").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    std::env::remove_var("TELLURION_STAC_TEST_S3_ACCESS_KEY_FAILED");
    std::env::remove_var("TELLURION_STAC_TEST_S3_SECRET_KEY_FAILED");
}

// -- reconcile (read-only report) ----------------------------------------

/// The report is empty when every managed record's object is genuinely
/// present and nothing unclaimed sits in the store.
#[tokio::test]
async fn reconcile_report_is_empty_on_a_consistent_store() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let payload = b"hello";
    put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&repr_digest_header(payload)),
        json!({"type": "image/png", "file:size": payload.len()}),
    )
    .await;
    let upload = put_data(&app, "/collections/demo/assets/thumb/data", payload).await;
    assert_eq!(upload.status(), StatusCode::OK);

    let report = get(&app, "/collections/demo/assets/reconcile").await;
    assert_eq!(report.status(), StatusCode::OK);
    let body = body_json(report).await;
    assert_eq!(body["broken"], json!([]));
    assert_eq!(body["orphaned"], json!([]));
}

/// An `available` record whose object was removed straight from the store
/// (bypassing this API entirely) is named as `broken`, by key and item id.
#[tokio::test]
async fn reconcile_report_names_a_missing_object_as_broken() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let payload = b"hello";
    put_metadata(
        &app,
        "/collections/demo/assets/thumb",
        Some(&repr_digest_header(payload)),
        json!({"type": "image/png", "file:size": payload.len()}),
    )
    .await;
    let upload = put_data(&app, "/collections/demo/assets/thumb/data", payload).await;
    assert_eq!(upload.status(), StatusCode::OK);

    // The store's own root now holds exactly one object — this asset's own
    // — since this test's `dir` is otherwise empty; remove it directly,
    // bypassing the API, the same way a lost bucket object or a bypassed
    // delete would.
    let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(entries.len(), 1, "exactly one object on disk");
    std::fs::remove_file(entries.into_iter().next().unwrap().unwrap().path()).unwrap();

    let report = get(&app, "/collections/demo/assets/reconcile").await;
    assert_eq!(report.status(), StatusCode::OK);
    let body = body_json(report).await;
    let broken = body["broken"].as_array().unwrap();
    assert_eq!(broken.len(), 1, "body was: {body}");
    assert_eq!(broken[0]["key"], "thumb");
    assert!(broken[0]["item_id"].is_null());
    assert_eq!(body["orphaned"], json!([]));
}

/// A stray object with no record at all, and a leftover resumable-upload
/// `.upload` staging file with no record either, are both named as
/// `orphaned` — the staging file distinguished by its own `staging: true`.
#[tokio::test]
async fn reconcile_report_names_orphans_including_a_leftover_upload_staging_file() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    // Two distinct, well-formed (but never registered) UUIDs — this test
    // needs no real randomness, just two names that parse as `Uuid`s and
    // differ from each other.
    std::fs::write(
        dir.path().join("11111111-2222-3333-4444-555555555555"),
        b"junk",
    )
    .unwrap();
    std::fs::write(
        dir.path()
            .join("66666666-7777-8888-9999-aaaaaaaaaaaa.upload"),
        b"half-uploaded",
    )
    .unwrap();

    let report = get(&app, "/collections/demo/assets/reconcile").await;
    assert_eq!(report.status(), StatusCode::OK);
    let body = body_json(report).await;
    assert_eq!(body["broken"], json!([]));
    let orphaned = body["orphaned"].as_array().unwrap();
    assert_eq!(orphaned.len(), 2, "body was: {body}");
    let staging_flags: std::collections::HashSet<bool> = orphaned
        .iter()
        .map(|entry| entry["staging"].as_bool().unwrap())
        .collect();
    assert_eq!(
        staging_flags,
        std::collections::HashSet::from([true, false]),
        "one plain orphan, one staging orphan"
    );
}

/// A collection with `AssetRecordStore` but no `object_store` at all
/// refuses reconcile by name (`"managed-storage"`) — there is nothing to
/// list against.
#[tokio::test]
async fn reconcile_refuses_by_name_when_the_collection_has_no_object_store() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = build_ctx(&build_config_yaml(&dir.path().to_string_lossy()));
    let app = build_app(ctx);

    let response = get(&app, "/collections/demo-no-store/assets/reconcile").await;
    assert_ne!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(
        body["detail"].as_str().unwrap().contains("managed-storage"),
        "detail was: {body}"
    );
}
