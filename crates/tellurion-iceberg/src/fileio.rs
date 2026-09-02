//! This driver's `FileIO` layer (`#123`): the `iceberg::io::Storage`
//! implementation that resolves every manifest, manifest list and data file
//! an Iceberg table names.
//!
//! ## Why this exists rather than `iceberg-rust`'s own storage factories
//!
//! `iceberg-rust` ships S3/GCS/ADLS backends of its own, and wiring them
//! would have been fewer lines. It would also have pulled the opendal/AWS
//! SDK chain into this workspace and left TWO independent S3
//! implementations compiled into one binary — while
//! `tellurion_core::objectstore` already carries an `S3ObjectStore` signed
//! by the workspace's own hand-rolled SigV4 (`sigv4.rs`), no vendor SDK, no
//! C toolchain, already exercised by the managed-asset lane down to
//! multipart upload. That is the same dependency-weight test the Zarr
//! driver applied when it declined a crate costing ~125 transitive crates
//! plus a C compression library, and it fails here for the same reason. So
//! this module is a thin adapter: `iceberg::io::Storage` on the outside,
//! `tellurion_core::PathAddressedObjectStore` on the inside.
//!
//! ## What that covers, and what it does not
//!
//! Everything that speaks the **S3 protocol** — AWS S3, MinIO, Ceph RGW,
//! Cloudflare R2 — which is the large majority of real Iceberg
//! deployments. Plus the local filesystem, delegated unchanged to
//! `iceberg`'s own [`LocalFsStorage`], so a table that worked before this
//! slice reads through byte-for-byte the same code as before.
//!
//! **Not** GCS and **not** ADLS. Their native APIs are not the S3 protocol
//! and this workspace has no client for either. They are refused BY NAME —
//! [`StorageRoute::resolve`] names the scheme it found and says it is not
//! implemented — at table load, before a single byte is served. Never a
//! silent fallback to another backend, never a generic "failed to read
//! file" 500 a thousand requests later.
//!
//! ## Read-only, structurally
//!
//! [`Storage::write`]/[`writer`](Storage::writer)/[`delete`](Storage::delete)/
//! [`delete_prefix`](Storage::delete_prefix) refuse by name on every scheme,
//! local filesystem included. This driver serves; ingest owns all DDL and
//! all physical layout, and a serving process that can delete a data file
//! is a serving process that can corrupt a table. The refusal is what makes
//! that structural rather than merely unexercised.
//!
//! ## Credentials
//!
//! Read from the environment, once, at table-load time, out of the two
//! variables the storage locator NAMES (`s3_access_key_env`/
//! `s3_secret_key_env`, see `location.rs`). No credential is ever read from
//! `config.yaml`, held in a config struct, or carried in the serialized
//! form of anything in this module — see [`ObjectStoreStorage`]'s own
//! `Serialize` note.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use iceberg::io::{
    FileMetadata, FileRead, FileWrite, InputFile, LocalFsStorage, OutputFile, Storage,
    StorageConfig, StorageFactory,
};
use iceberg::{Error as IcebergError, ErrorKind, Result as IcebergResult};
use serde::{Deserialize, Serialize};
use tellurion_core::{PathAddressedObjectStore, S3ObjectStore};

/// The S3 URI schemes this driver answers to. `s3a`/`s3n` are the Hadoop
/// connector spellings; plenty of tables in the wild were written with a
/// Hadoop-based engine and carry them verbatim in their metadata. All three
/// name the same protocol.
const S3_SCHEMES: [&str; 3] = ["s3", "s3a", "s3n"];

/// Schemes this driver recognizes and deliberately does NOT implement,
/// paired with the human name its refusal uses. Recognizing them is the
/// point: an operator who points this driver at a GCS table gets told
/// "'gs' — Google Cloud Storage — is not supported by the iceberg driver",
/// not "unknown scheme", and certainly not a silent fallback.
const UNSUPPORTED_SCHEMES: [(&str, &str); 7] = [
    ("gs", "Google Cloud Storage"),
    ("gcs", "Google Cloud Storage"),
    ("abfs", "Azure Data Lake Storage"),
    ("abfss", "Azure Data Lake Storage"),
    ("adl", "Azure Data Lake Storage"),
    ("wasb", "Azure Blob Storage"),
    ("wasbs", "Azure Blob Storage"),
];

/// Where one Iceberg path resolves to — the whole of this driver's storage
/// dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StorageRoute {
    /// A `file://` URI, or a bare absolute path (Iceberg metadata written
    /// by a local-filesystem engine carries both spellings). Served by
    /// `iceberg`'s own [`LocalFsStorage`], unchanged.
    LocalFs,
    /// `s3://{bucket}/{key}` (or `s3a`/`s3n`). `key` is the complete
    /// in-bucket key, never re-prefixed.
    S3 { bucket: String, key: String },
}

/// Why a path could not be routed — always a named scheme, never a generic
/// failure. Rendered into `IcebergDriverError` by `driver.rs` (an
/// `Error::Config` refusal at load time) and into an `iceberg::Error` by
/// [`ObjectStoreStorage`] (the backstop, for a data file whose scheme
/// differs from the table location's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnsupportedScheme {
    /// Recognized, deliberately unimplemented: GCS or ADLS.
    Known { scheme: String, product: String },
    /// Not a scheme this driver has ever heard of.
    Unknown { scheme: String },
}

impl std::fmt::Display for UnsupportedScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known { scheme, product } => write!(
                f,
                "scheme '{scheme}://' ({product}) is not supported by the iceberg driver, which \
                 implements the local filesystem and the S3 protocol only (AWS S3, MinIO, Ceph, \
                 R2); {product} would need a native client this workspace does not have"
            ),
            Self::Unknown { scheme } => write!(
                f,
                "scheme '{scheme}://' is not a storage scheme the iceberg driver recognizes; it \
                 implements the local filesystem ('file://') and the S3 protocol ('s3://', \
                 's3a://', 's3n://') only"
            ),
        }
    }
}

impl StorageRoute {
    /// Routes one absolute Iceberg path. The ONLY place a scheme decides
    /// anything in this driver — `driver.rs`'s load-time check and
    /// [`ObjectStoreStorage`]'s per-read dispatch both come through here,
    /// so the two can never disagree about what is supported.
    pub(crate) fn resolve(path: &str) -> std::result::Result<Self, UnsupportedScheme> {
        let Some((scheme, rest)) = path.split_once("://") else {
            // No scheme at all: a bare filesystem path. Iceberg metadata
            // written against a local warehouse carries these, and they
            // read exactly as they did before this slice existed.
            return Ok(Self::LocalFs);
        };
        let lowered = scheme.to_ascii_lowercase();
        if lowered == "file" {
            return Ok(Self::LocalFs);
        }
        if S3_SCHEMES.contains(&lowered.as_str()) {
            let (bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
            return Ok(Self::S3 {
                bucket: bucket.to_string(),
                key: key.to_string(),
            });
        }
        if let Some((_, product)) = UNSUPPORTED_SCHEMES
            .iter()
            .find(|(name, _)| *name == lowered)
        {
            return Err(UnsupportedScheme::Known {
                scheme: lowered,
                product: (*product).to_string(),
            });
        }
        Err(UnsupportedScheme::Unknown { scheme: lowered })
    }

    /// `true` for a route needing the locator's `s3_*` declarations — what
    /// `driver.rs` asks before demanding them, so a local-filesystem table
    /// is never asked for an endpoint it has no use for.
    pub(crate) fn needs_s3_declaration(&self) -> bool {
        matches!(self, Self::S3 { .. })
    }
}

/// Already-resolved S3 connection facts, credentials included — built by
/// `driver.rs` from the locator's `s3_*` declarations plus the two
/// environment variables those declarations NAME. Never serialized, never
/// logged, never `Debug`-printed: `S3ObjectStore`'s own hand-written
/// `Debug` is the only thing that ever renders any of this, and it omits
/// both keys.
pub(crate) struct S3Connection {
    pub endpoint: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
}

/// The `StorageFactory` this driver hands to `RestCatalogBuilder`.
///
/// `iceberg::io::FileIO` invokes a factory exactly once, lazily, and caches
/// the single `Arc<dyn Storage>` it returns for every path thereafter — so
/// the ONE storage this builds has to be able to route every scheme itself,
/// which is what [`ObjectStoreStorage`] does.
///
/// `#[typetag::serde]` is mandatory, not decorative: `iceberg` declares both
/// `Storage` and `StorageFactory` as typetag traits so a `FileIO` can cross
/// a process boundary, and an impl without the attribute does not compile.
/// Nothing in this driver ever actually serializes one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ObjectStoreStorageFactory {
    /// `None` for a local-filesystem-only deployment — the shape every
    /// pre-`#123` config produces, and the shape that must keep behaving
    /// identically. `Some` never means "use S3"; it means "S3 is available
    /// if a path asks for it".
    #[serde(skip)]
    s3: Option<Arc<S3Connection>>,
}

impl ObjectStoreStorageFactory {
    pub(crate) fn new(s3: Option<Arc<S3Connection>>) -> Self {
        Self { s3 }
    }
}

#[typetag::serde]
impl StorageFactory for ObjectStoreStorageFactory {
    fn build(&self, _config: &StorageConfig) -> IcebergResult<Arc<dyn Storage>> {
        // `StorageConfig` is ignored on purpose. It carries whatever
        // properties a REST catalog handed back in its `config` block —
        // including, on some servers, vended S3 credentials. This driver
        // takes its credentials from the environment variables the operator
        // named and nowhere else, so a catalog server cannot redirect this
        // process's reads or its credentials by answering `GET /v1/config`
        // with properties of its choosing.
        Ok(Arc::new(ObjectStoreStorage {
            s3: self.s3.clone(),
            stores: Arc::new(Mutex::new(HashMap::new())),
            local: LocalFsStorage::new(),
        }))
    }
}

/// The single `Storage` every read in this driver goes through.
///
/// `Serialize`/`Deserialize` are derived only to satisfy `iceberg`'s typetag
/// requirement (see [`ObjectStoreStorageFactory`]); every field is
/// `#[serde(skip)]`, so the serialized form of this type is empty. That is
/// deliberate — a credential must not be able to leave this process inside a
/// serialized `FileIO`, and the way to guarantee that is for there to be
/// nothing to write.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ObjectStoreStorage {
    #[serde(skip)]
    s3: Option<Arc<S3Connection>>,
    /// One `S3ObjectStore` per bucket, built on first use. An Iceberg table
    /// normally lives in one bucket, but its metadata is free to name
    /// several, and each needs its own store (the bucket is part of the
    /// path-style URL every request signs).
    #[serde(skip)]
    stores: Arc<Mutex<HashMap<String, Arc<S3ObjectStore>>>>,
    #[serde(skip)]
    local: LocalFsStorage,
}

impl std::fmt::Debug for S3Connection {
    /// Hand-written for the same reason `S3ObjectStore`'s own `Debug` is: a
    /// stray `{:?}` in a log line must not print a secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Connection")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

impl ObjectStoreStorage {
    /// The per-bucket `S3ObjectStore`, built once and reused — so one
    /// `reqwest::Client` (and therefore one connection pool) serves every
    /// read against that bucket, rather than one per file.
    fn s3_store(&self, bucket: &str) -> IcebergResult<Arc<S3ObjectStore>> {
        let connection = self.s3.as_ref().ok_or_else(|| {
            IcebergError::new(
                ErrorKind::Unexpected,
                format!(
                    "iceberg storage location resolves to 's3://{bucket}', but this storage \
                     declares no S3 connection (s3_endpoint/s3_region/s3_access_key_env/\
                     s3_secret_key_env)"
                ),
            )
        })?;
        let mut stores = self
            .stores
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = stores.get(bucket) {
            return Ok(Arc::clone(existing));
        }
        let store = S3ObjectStore::for_path_reads(
            &connection.endpoint,
            bucket,
            &connection.region,
            &connection.access_key,
            &connection.secret_key,
        )
        .map_err(|err| {
            IcebergError::new(
                ErrorKind::Unexpected,
                format!("cannot address S3 bucket '{bucket}': {err}"),
            )
        })?;
        let store = Arc::new(store);
        stores.insert(bucket.to_string(), Arc::clone(&store));
        Ok(store)
    }

    /// Resolves `path` to a route, turning an unsupported scheme into an
    /// `iceberg::Error` that carries [`UnsupportedScheme`]'s own wording —
    /// the same sentence `driver.rs`'s load-time refusal prints, so the two
    /// surfaces never describe the same situation two different ways.
    fn route(&self, path: &str) -> IcebergResult<StorageRoute> {
        StorageRoute::resolve(path).map_err(|refusal| {
            IcebergError::new(
                ErrorKind::FeatureUnsupported,
                format!("cannot read '{path}': {refusal}"),
            )
        })
    }

    /// Every write-shaped verb's single refusal. See this module's
    /// "Read-only, structurally".
    fn refuse_write(verb: &str, path: &str) -> IcebergError {
        IcebergError::new(
            ErrorKind::FeatureUnsupported,
            format!(
                "the iceberg driver is read-only and never writes to a table's storage: \
                 refusing to {verb} '{path}'. Ingest owns all DDL and all physical layout."
            ),
        )
    }

    async fn s3_metadata(
        &self,
        bucket: &str,
        key: &str,
        path: &str,
    ) -> IcebergResult<FileMetadata> {
        let store = self.s3_store(bucket)?;
        let metadata = store
            .head_path(key)
            .await
            .map_err(|err| s3_error(path, "HEAD", &err))?
            .ok_or_else(|| {
                IcebergError::new(ErrorKind::Unexpected, format!("'{path}' does not exist"))
            })?;
        // A store that answered HEAD but reported no `Content-Length` has
        // told us nothing about the size. Reporting `0` here would make
        // Iceberg's Parquet reader read an empty footer and fail somewhere
        // far away; naming the gap is the honest answer.
        let size = metadata.size.ok_or_else(|| {
            IcebergError::new(
                ErrorKind::Unexpected,
                format!("object store reported no Content-Length for '{path}'"),
            )
        })?;
        Ok(FileMetadata { size })
    }
}

fn s3_error(path: &str, verb: &str, err: &tellurion_core::ObjectStoreError) -> IcebergError {
    IcebergError::new(
        ErrorKind::Unexpected,
        format!("object store {verb} for '{path}' failed: {err}"),
    )
}

#[async_trait]
#[typetag::serde]
impl Storage for ObjectStoreStorage {
    async fn exists(&self, path: &str) -> IcebergResult<bool> {
        match self.route(path)? {
            StorageRoute::LocalFs => self.local.exists(path).await,
            StorageRoute::S3 { bucket, key } => {
                let store = self.s3_store(&bucket)?;
                Ok(store
                    .head_path(&key)
                    .await
                    .map_err(|err| s3_error(path, "HEAD", &err))?
                    .is_some())
            }
        }
    }

    async fn metadata(&self, path: &str) -> IcebergResult<FileMetadata> {
        match self.route(path)? {
            StorageRoute::LocalFs => self.local.metadata(path).await,
            StorageRoute::S3 { bucket, key } => self.s3_metadata(&bucket, &key, path).await,
        }
    }

    async fn read(&self, path: &str) -> IcebergResult<Bytes> {
        match self.route(path)? {
            StorageRoute::LocalFs => self.local.read(path).await,
            StorageRoute::S3 { bucket, key } => {
                let store = self.s3_store(&bucket)?;
                store
                    .get_path(&key)
                    .await
                    .map_err(|err| s3_error(path, "GET", &err))?
                    .ok_or_else(|| {
                        IcebergError::new(ErrorKind::Unexpected, format!("'{path}' does not exist"))
                    })
            }
        }
    }

    async fn reader(&self, path: &str) -> IcebergResult<Box<dyn FileRead>> {
        match self.route(path)? {
            StorageRoute::LocalFs => self.local.reader(path).await,
            StorageRoute::S3 { bucket, key } => Ok(Box::new(S3FileRead {
                store: self.s3_store(&bucket)?,
                key,
                path: path.to_string(),
            })),
        }
    }

    async fn write(&self, path: &str, _bs: Bytes) -> IcebergResult<()> {
        Err(Self::refuse_write("write", path))
    }

    async fn writer(&self, path: &str) -> IcebergResult<Box<dyn FileWrite>> {
        Err(Self::refuse_write("open a writer for", path))
    }

    async fn delete(&self, path: &str) -> IcebergResult<()> {
        Err(Self::refuse_write("delete", path))
    }

    async fn delete_prefix(&self, path: &str) -> IcebergResult<()> {
        Err(Self::refuse_write("delete everything under", path))
    }

    fn new_input(&self, path: &str) -> IcebergResult<InputFile> {
        Ok(InputFile::new(Arc::new(self.clone()), path.to_string()))
    }

    fn new_output(&self, path: &str) -> IcebergResult<OutputFile> {
        // `OutputFile` is a handle, not a write — but handing one back for
        // a driver that refuses every write verb would be a promise this
        // module cannot keep, discovered only at the moment bytes are
        // pushed. Refuse where the intent is stated.
        Err(Self::refuse_write("open an output file for", path))
    }
}

/// The continuous-read handle Iceberg's Parquet reader drives: one HTTP
/// `Range` request per call, against the same per-bucket `S3ObjectStore`
/// (and therefore the same connection pool) every other read on this table
/// already uses.
#[derive(Debug)]
struct S3FileRead {
    store: Arc<S3ObjectStore>,
    key: String,
    path: String,
}

#[async_trait]
impl FileRead for S3FileRead {
    async fn read(&self, range: Range<u64>) -> IcebergResult<Bytes> {
        let length = range.end.saturating_sub(range.start);
        let bytes = self
            .store
            .get_path_range(&self.key, range.start, length)
            .await
            .map_err(|err| s3_error(&self.path, "ranged GET", &err))?
            .ok_or_else(|| {
                IcebergError::new(
                    ErrorKind::Unexpected,
                    format!("'{}' does not exist", self.path),
                )
            })?;
        // A store that ignored `Range` and answered with the whole object
        // would otherwise feed the Parquet reader bytes from the wrong
        // offset — a silent misread, the worst possible failure here.
        // Checking the length turns it into a named one.
        if bytes.len() as u64 != length {
            return Err(IcebergError::new(
                ErrorKind::Unexpected,
                format!(
                    "object store returned {} bytes for a {length}-byte range of '{}' \
                     (offset {}) — it did not honor the Range request",
                    bytes.len(),
                    self.path,
                    range.start
                ),
            ));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_path_routes_to_the_local_filesystem() {
        assert_eq!(
            StorageRoute::resolve("/warehouse/geo/points/metadata/v1.json").unwrap(),
            StorageRoute::LocalFs
        );
    }

    #[test]
    fn a_file_uri_routes_to_the_local_filesystem() {
        assert_eq!(
            StorageRoute::resolve("file:///warehouse/geo/points/data/x.parquet").unwrap(),
            StorageRoute::LocalFs
        );
    }

    #[test]
    fn an_s3_uri_splits_into_bucket_and_whole_in_bucket_key() {
        assert_eq!(
            StorageRoute::resolve("s3://lake/warehouse/geo/points/data/x.parquet").unwrap(),
            StorageRoute::S3 {
                bucket: "lake".to_string(),
                key: "warehouse/geo/points/data/x.parquet".to_string(),
            }
        );
    }

    #[test]
    fn the_hadoop_s3_scheme_spellings_route_to_the_same_protocol() {
        for scheme in ["s3a", "s3n", "S3", "S3A"] {
            assert_eq!(
                StorageRoute::resolve(&format!("{scheme}://lake/a/b.parquet")).unwrap(),
                StorageRoute::S3 {
                    bucket: "lake".to_string(),
                    key: "a/b.parquet".to_string(),
                },
                "scheme {scheme}"
            );
        }
    }

    #[test]
    fn gcs_is_refused_by_name_naming_the_scheme_and_the_product() {
        for scheme in ["gs", "gcs"] {
            let refusal =
                StorageRoute::resolve(&format!("{scheme}://lake/a/b.parquet")).unwrap_err();
            let message = refusal.to_string();
            assert!(
                message.contains(&format!("'{scheme}://'")),
                "refusal must name the scheme it found, got: {message}"
            );
            assert!(
                message.contains("Google Cloud Storage"),
                "refusal must name the product, got: {message}"
            );
            assert!(
                message.contains("not supported"),
                "refusal must say it is not supported, got: {message}"
            );
        }
    }

    #[test]
    fn adls_is_refused_by_name_naming_the_scheme_and_the_product() {
        for (scheme, product) in [
            ("abfs", "Azure Data Lake Storage"),
            ("abfss", "Azure Data Lake Storage"),
            ("adl", "Azure Data Lake Storage"),
            ("wasb", "Azure Blob Storage"),
            ("wasbs", "Azure Blob Storage"),
        ] {
            let refusal =
                StorageRoute::resolve(&format!("{scheme}://container/a/b.parquet")).unwrap_err();
            let message = refusal.to_string();
            assert!(
                message.contains(&format!("'{scheme}://'")) && message.contains(product),
                "refusal must name both scheme and product, got: {message}"
            );
        }
    }

    #[test]
    fn an_unheard_of_scheme_is_refused_by_name_too_never_silently_treated_as_a_path() {
        let refusal = StorageRoute::resolve("hdfs://namenode/a/b.parquet").unwrap_err();
        assert_eq!(
            refusal,
            UnsupportedScheme::Unknown {
                scheme: "hdfs".to_string()
            }
        );
        assert!(refusal.to_string().contains("'hdfs://'"));
    }

    #[test]
    fn only_an_s3_route_asks_for_the_s3_declarations() {
        assert!(!StorageRoute::LocalFs.needs_s3_declaration());
        assert!(StorageRoute::S3 {
            bucket: "b".to_string(),
            key: "k".to_string()
        }
        .needs_s3_declaration());
    }

    #[tokio::test]
    async fn every_write_shaped_verb_is_refused_by_name_on_every_scheme() {
        let storage = ObjectStoreStorage::default();
        for path in ["/warehouse/a.parquet", "s3://lake/a.parquet"] {
            let write = storage.write(path, Bytes::new()).await.unwrap_err();
            assert!(write.to_string().contains("read-only"), "got: {write}");
            // `Box<dyn FileWrite>` is not `Debug`, so this one cannot use
            // `unwrap_err()` the way its neighbours do.
            let Err(writer) = storage.writer(path).await else {
                panic!("writer() must refuse, it returned a writer for {path}");
            };
            assert!(writer.to_string().contains("read-only"), "got: {writer}");
            let delete = storage.delete(path).await.unwrap_err();
            assert!(delete.to_string().contains("read-only"), "got: {delete}");
            let delete_prefix = storage.delete_prefix(path).await.unwrap_err();
            assert!(
                delete_prefix.to_string().contains("read-only"),
                "got: {delete_prefix}"
            );
            let output = storage.new_output(path).unwrap_err();
            assert!(output.to_string().contains("read-only"), "got: {output}");
        }
    }

    #[tokio::test]
    async fn an_s3_path_with_no_declared_connection_refuses_rather_than_falling_back_to_disk() {
        let storage = ObjectStoreStorage::default();
        let err = storage.exists("s3://lake/a.parquet").await.unwrap_err();
        assert!(
            err.to_string().contains("declares no S3 connection"),
            "got: {err}"
        );
    }

    #[test]
    fn the_serialized_form_of_a_storage_carries_nothing() {
        // The credential-containment claim in this module's own docs,
        // checked rather than asserted in prose: every field is
        // `#[serde(skip)]`, so a `FileIO` that crossed a process boundary
        // would carry no endpoint, no region, and above all no keys.
        let storage = ObjectStoreStorage::default();
        let json = serde_json::to_string(&storage).unwrap();
        assert_eq!(json, "{}");
    }
}
