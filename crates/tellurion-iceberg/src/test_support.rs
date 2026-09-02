//! Test-only fixture support (`#[cfg(test)]`, see `lib.rs`): builds a real,
//! disk-backed Iceberg table via Iceberg's own in-process memory catalog
//! plus real writer/append calls, then serves that table over a real HTTP
//! loopback listener speaking just enough of the REST catalog protocol
//! (`GET /v1/config`, `GET /v1/namespaces/{namespace}/tables/{table}`) for
//! this crate's own `RestCatalog` client to load it. No test in this crate
//! ever reaches a real network service — see `Cargo.toml`'s dev-dependency
//! docs for why a single committed binary fixture can't stand in for this
//! (Iceberg metadata embeds absolute file paths and a fresh UUID per
//! commit).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, BinaryArray, Float64Array, Int32Array, RecordBatch, StringArray,
    TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, TimeUnit};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use iceberg::io::{LocalFsStorage, LocalFsStorageFactory};
use iceberg::memory::{MemoryCatalogBuilder, MEMORY_CATALOG_WAREHOUSE};
use iceberg::spec::{
    DataFile, DataFileFormat, Datum, NestedField, PrimitiveLiteral, PrimitiveType, Schema, Type,
};
use iceberg::table::Table;
use iceberg::transaction::{ApplyTransactionAction, Transaction};
use iceberg::writer::base_writer::data_file_writer::DataFileWriterBuilder;
use iceberg::writer::file_writer::location_generator::{
    DefaultFileNameGenerator, DefaultLocationGenerator,
};
use iceberg::writer::file_writer::rolling_writer::RollingFileWriterBuilder;
use iceberg::writer::file_writer::ParquetWriterBuilder;
use iceberg::writer::{IcebergWriter, IcebergWriterBuilder};
use iceberg::{Catalog, CatalogBuilder, NamespaceIdent, TableCreation, TableIdent};
use parquet::arrow::PARQUET_FIELD_ID_META_KEY;
use parquet::file::properties::WriterProperties;
use tellurion_core::CollectionDecl;

use crate::driver::IcebergBackend;
use crate::location::IcebergLocation;

const NAMESPACE: &str = "geo";
const TABLE: &str = "points";
const GEOMETRY_COLUMN: &str = "geom";
const BBOX_COLUMNS: &str = "bbox_xmin,bbox_ymin,bbox_xmax,bbox_ymax";
/// Optional `timestamptz` attribute column, present on every fixture row —
/// declared as this collection's `datetime` column only by the tests that
/// need it (`TestFixture::collection_decl_with_datetime`); every other test
/// leaves it as an ordinary, unreferenced attribute, exactly like `name`.
pub(crate) const DATETIME_COLUMN: &str = "observed_at";

/// A running fixture: a real Iceberg table backed by a tempdir, served over
/// a real HTTP loopback listener. `_warehouse` just needs to outlive the
/// fixture (dropping it deletes the tempdir); the spawned server task is
/// abandoned when the owning `#[tokio::test]` runtime shuts down — nothing
/// in it holds a resource beyond `_warehouse`'s own tempdir.
pub(crate) struct TestFixture {
    server_addr: SocketAddr,
    _warehouse: tempfile::TempDir,
    pub(crate) table: Table,
    pub(crate) catalog: Arc<dyn Catalog>,
}

impl TestFixture {
    /// One data file, two rows — enough for the catalog-introspection
    /// tests (extent, row estimate, attribute schema).
    pub(crate) async fn build() -> Self {
        let (catalog, table, warehouse, server_addr) = start_fixture_table().await;
        let batch = build_batch(vec![
            (
                1,
                Some("west"),
                point_wkb(-3.0, 48.0),
                [-3.0, 48.0, -3.0, 48.0],
                "2020-01-01T00:00:00Z",
            ),
            (
                2,
                Some("east"),
                point_wkb(-2.0, 49.0),
                [-2.0, 49.0, -2.0, 49.0],
                "2020-01-02T00:00:00Z",
            ),
        ]);
        let data_files = write_batch(&table, "initial", batch).await;
        let table = commit_append(catalog.as_ref(), &table, data_files).await;
        Self {
            server_addr,
            _warehouse: warehouse,
            table,
            catalog,
        }
    }

    /// Two separate appends -> two data files with non-overlapping bbox
    /// column stats, so bbox pushdown can prune one entirely and paging has
    /// more than one file to walk across.
    pub(crate) async fn two_disjoint_files() -> Self {
        let (catalog, table, warehouse, server_addr) = start_fixture_table().await;

        let west = build_batch(vec![(
            1,
            Some("west"),
            point_wkb(-3.0, 48.0),
            [-3.0, 48.0, -3.0, 48.0],
            "2020-01-01T00:00:00Z",
        )]);
        let data_files = write_batch(&table, "west", west).await;
        let table = commit_append(catalog.as_ref(), &table, data_files).await;

        let east = build_batch(vec![(
            2,
            Some("east"),
            point_wkb(10.0, 48.0),
            [10.0, 48.0, 10.0, 48.0],
            "2020-01-02T00:00:00Z",
        )]);
        let data_files = write_batch(&table, "east", east).await;
        let table = commit_append(catalog.as_ref(), &table, data_files).await;

        Self {
            server_addr,
            _warehouse: warehouse,
            table,
            catalog,
        }
    }

    /// Two separate appends, two rows each -> four rows total, disjoint
    /// bbox/`id`-range stats per file, one null `name`. Built specifically
    /// to make CQL2 attribute filtering and datetime interval filtering
    /// observable (`#45` slice 2): a `Compare`/`In`/`IsNull` predicate on
    /// `id`/`name` can prune the whole non-matching file by column stats
    /// alone, and each row's own `observed_at` spans a distinct interval a
    /// `datetime` query can narrow to.
    ///
    /// | file | id | name      | observed_at            |
    /// |------|----|-----------|-------------------------|
    /// | west | 1  | "west-a"  | 2020-01-01T00:00:00Z    |
    /// | west | 2  | "west-b"  | 2020-06-01T00:00:00Z    |
    /// | east | 3  | `None`    | 2021-01-01T00:00:00Z    |
    /// | east | 4  | "east-b"  | 2021-06-01T00:00:00Z    |
    pub(crate) async fn four_rows_two_files() -> Self {
        let (catalog, table, warehouse, server_addr) = start_fixture_table().await;

        let west = build_batch(vec![
            (
                1,
                Some("west-a"),
                point_wkb(-3.0, 48.0),
                [-3.0, 48.0, -3.0, 48.0],
                "2020-01-01T00:00:00Z",
            ),
            (
                2,
                Some("west-b"),
                point_wkb(-2.5, 48.5),
                [-2.5, 48.5, -2.5, 48.5],
                "2020-06-01T00:00:00Z",
            ),
        ]);
        let data_files = write_batch(&table, "west", west).await;
        let table = commit_append(catalog.as_ref(), &table, data_files).await;

        let east = build_batch(vec![
            (
                3,
                None,
                point_wkb(10.0, 48.0),
                [10.0, 48.0, 10.0, 48.0],
                "2021-01-01T00:00:00Z",
            ),
            (
                4,
                Some("east-b"),
                point_wkb(10.5, 48.5),
                [10.5, 48.5, 10.5, 48.5],
                "2021-06-01T00:00:00Z",
            ),
        ]);
        let data_files = write_batch(&table, "east", east).await;
        let table = commit_append(catalog.as_ref(), &table, data_files).await;

        Self {
            server_addr,
            _warehouse: warehouse,
            table,
            catalog,
        }
    }

    /// Commits one more row directly against the fixture's own backing
    /// catalog — on-disk only, never through a `backend()` already loaded
    /// from this fixture. Used by the snapshot-pinning test to prove an
    /// already-loaded backend never observes it.
    pub(crate) async fn append_more_rows_on_disk_only(&self) {
        let batch = build_batch(vec![(
            3,
            Some("later"),
            point_wkb(0.0, 0.0),
            [0.0, 0.0, 0.0, 0.0],
            "2022-01-01T00:00:00Z",
        )]);
        let data_files = write_batch(&self.table, "later", batch).await;
        commit_append(self.catalog.as_ref(), &self.table, data_files).await;
    }

    pub(crate) fn backend(&self) -> IcebergBackend {
        IcebergBackend::new(self.location())
    }

    pub(crate) fn location(&self) -> IcebergLocation {
        self.location_with(TABLE, GEOMETRY_COLUMN, BBOX_COLUMNS)
    }

    pub(crate) fn location_for_missing_table(&self, table: &str) -> IcebergLocation {
        self.location_with(table, GEOMETRY_COLUMN, BBOX_COLUMNS)
    }

    pub(crate) fn location_with_geometry_column(&self, geometry: &str) -> IcebergLocation {
        self.location_with(TABLE, geometry, BBOX_COLUMNS)
    }

    pub(crate) fn location_with_bbox_xmin(&self, xmin: &str) -> IcebergLocation {
        let bbox = format!("{xmin},bbox_ymin,bbox_xmax,bbox_ymax");
        self.location_with(TABLE, GEOMETRY_COLUMN, &bbox)
    }

    fn location_with(&self, table: &str, geometry: &str, bbox: &str) -> IcebergLocation {
        let raw = format!(
            "http://{}?namespace={NAMESPACE}&table={table}&geometry={geometry}&bbox={bbox}",
            self.server_addr,
        );
        IcebergLocation::parse(&raw).unwrap()
    }

    pub(crate) fn collection_decl(&self) -> CollectionDecl {
        // `items`/`item` never actually read this driver's `CollectionDecl`
        // parameter beyond `id`/`datetime` (same "ignored, table identity
        // comes from the storage locator" shape flatgeobuf's/geoparquet's
        // own drivers use for everything else) — the only fields that
        // matter to `serde_yaml` here are the ones the type requires at all.
        serde_yaml::from_str(&format!("id: {TABLE}\ncatalog: default\nstorage: main\n")).unwrap()
    }

    /// `collection_decl()` with `datetime: {column}` declared — the operator
    /// declaration `compile_datetime_predicate` requires (`#45` slice 2: see
    /// `driver.rs`'s "CQL2 and datetime pushdown" docs for why Iceberg
    /// itself never derives one). Pass a column that isn't
    /// [`DATETIME_COLUMN`] (e.g. `"id"`) to exercise the wrong-type refusal.
    pub(crate) fn collection_decl_with_datetime(&self, column: &str) -> CollectionDecl {
        serde_yaml::from_str(&format!(
            "id: {TABLE}\ncatalog: default\nstorage: main\ndatetime: {column}\n"
        ))
        .unwrap()
    }
}

async fn start_fixture_table() -> (Arc<dyn Catalog>, Table, tempfile::TempDir, SocketAddr) {
    let warehouse_dir = tempfile::tempdir().unwrap();
    let warehouse = warehouse_dir.path().to_str().unwrap().to_string();

    let catalog = MemoryCatalogBuilder::default()
        .with_storage_factory(Arc::new(LocalFsStorageFactory))
        .load(
            "memory",
            HashMap::from([(MEMORY_CATALOG_WAREHOUSE.to_string(), warehouse)]),
        )
        .await
        .unwrap();

    let namespace = NamespaceIdent::new(NAMESPACE.to_string());
    catalog
        .create_namespace(&namespace, HashMap::new())
        .await
        .unwrap();

    let creation = TableCreation::builder()
        .name(TABLE.to_string())
        .schema(table_schema())
        .build();
    let table = catalog.create_table(&namespace, creation).await.unwrap();

    let catalog: Arc<dyn Catalog> = Arc::new(catalog);
    let server_addr = spawn_rest_server(Arc::clone(&catalog)).await;

    (catalog, table, warehouse_dir, server_addr)
}

fn table_schema() -> Schema {
    Schema::builder()
        .with_schema_id(0)
        .with_fields(vec![
            NestedField::required(1, "id", Type::Primitive(PrimitiveType::Int)).into(),
            NestedField::optional(2, "name", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::required(3, GEOMETRY_COLUMN, Type::Primitive(PrimitiveType::Binary))
                .into(),
            NestedField::required(4, "bbox_xmin", Type::Primitive(PrimitiveType::Double)).into(),
            NestedField::required(5, "bbox_ymin", Type::Primitive(PrimitiveType::Double)).into(),
            NestedField::required(6, "bbox_xmax", Type::Primitive(PrimitiveType::Double)).into(),
            NestedField::required(7, "bbox_ymax", Type::Primitive(PrimitiveType::Double)).into(),
            NestedField::required(
                8,
                DATETIME_COLUMN,
                Type::Primitive(PrimitiveType::Timestamptz),
            )
            .into(),
        ])
        .build()
        .unwrap()
}

/// Spins up a loopback HTTP server on an OS-assigned port, speaking just
/// enough of the Iceberg REST catalog protocol for `RestCatalogBuilder`
/// (this crate's own production client) to resolve a `load_table` call:
/// `GET /v1/config` (always empty overrides/defaults — this fixture never
/// exercises server-side config overrides) and `GET /v1/namespaces/
/// {namespace}/tables/{table}` (delegates straight to the real, in-process
/// catalog this fixture already built the table through, so "the server"
/// and "the catalog" always agree). The spawned task outlives this
/// function; it is abandoned, not joined, when the test's runtime shuts
/// down.
async fn spawn_rest_server(catalog: Arc<dyn Catalog>) -> SocketAddr {
    let app = Router::new()
        .route("/v1/config", get(get_config))
        .route("/v1/namespaces/{namespace}/tables/{table}", get(get_table))
        .with_state(catalog);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

async fn get_config() -> Json<serde_json::Value> {
    Json(serde_json::json!({"overrides": {}, "defaults": {}}))
}

async fn get_table(
    State(catalog): State<Arc<dyn Catalog>>,
    Path((namespace, table)): Path<(String, String)>,
) -> Response {
    let ident = TableIdent::new(NamespaceIdent::new(namespace), table);
    match catalog.load_table(&ident).await {
        Ok(loaded) => Json(serde_json::json!({
            "metadata-location": loaded.metadata_location(),
            "metadata": loaded.metadata(),
            "config": {},
        }))
        .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn field_with_id(name: &str, data_type: DataType, nullable: bool, field_id: i32) -> Field {
    Field::new(name, data_type, nullable).with_metadata(HashMap::from([(
        PARQUET_FIELD_ID_META_KEY.to_string(),
        field_id.to_string(),
    )]))
}

/// UTC offset string this fixture uses uniformly for its `observed_at`
/// column, on both the Arrow field's own `DataType::Timestamp` and every
/// array built against it — Arrow requires an exact match between the two.
const ARROW_TZ: &str = "+00:00";

fn arrow_fixture_schema() -> arrow_schema::Schema {
    arrow_schema::Schema::new(vec![
        field_with_id("id", DataType::Int32, false, 1),
        field_with_id("name", DataType::Utf8, true, 2),
        field_with_id(GEOMETRY_COLUMN, DataType::Binary, false, 3),
        field_with_id("bbox_xmin", DataType::Float64, false, 4),
        field_with_id("bbox_ymin", DataType::Float64, false, 5),
        field_with_id("bbox_xmax", DataType::Float64, false, 6),
        field_with_id("bbox_ymax", DataType::Float64, false, 7),
        field_with_id(
            DATETIME_COLUMN,
            DataType::Timestamp(TimeUnit::Microsecond, Some(ARROW_TZ.into())),
            false,
            8,
        ),
    ])
}

/// Encodes a minimal ISO WKB 2D `Point` — little-endian byte order, geometry
/// type `1`, then the two 8-byte coordinates. Enough for this crate's own
/// fixtures; not a general WKB writer.
pub(crate) fn point_wkb(x: f64, y: f64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(21);
    buf.push(1u8);
    buf.extend_from_slice(&1u32.to_le_bytes());
    buf.extend_from_slice(&x.to_le_bytes());
    buf.extend_from_slice(&y.to_le_bytes());
    buf
}

/// Microseconds since the Unix epoch for an RFC 3339 instant — routed
/// through `iceberg::spec::Datum::timestamptz_from_str`, the exact same
/// parser `driver.rs`'s own `datetime_datum` compiles a request's
/// `datetime`/CQL2 temporal literal through, rather than adding a `chrono`
/// dev-dependency this crate doesn't otherwise need.
fn timestamptz_micros(rfc3339: &str) -> i64 {
    match Datum::timestamptz_from_str(rfc3339).unwrap().literal() {
        PrimitiveLiteral::Long(micros) => *micros,
        other => unreachable!("timestamptz_from_str always produces a Long literal, got {other:?}"),
    }
}

/// `(id, name, WKB geometry, [xmin, ymin, xmax, ymax], observed_at as RFC
/// 3339 text)` — one fixture row.
type FixtureRow<'a> = (i32, Option<&'a str>, Vec<u8>, [f64; 4], &'a str);

fn build_batch(rows: Vec<FixtureRow<'_>>) -> RecordBatch {
    let ids: Vec<i32> = rows.iter().map(|row| row.0).collect();
    let names: Vec<Option<&str>> = rows.iter().map(|row| row.1).collect();
    let geoms: Vec<&[u8]> = rows.iter().map(|row| row.2.as_slice()).collect();
    let xmins: Vec<f64> = rows.iter().map(|row| row.3[0]).collect();
    let ymins: Vec<f64> = rows.iter().map(|row| row.3[1]).collect();
    let xmaxs: Vec<f64> = rows.iter().map(|row| row.3[2]).collect();
    let ymaxs: Vec<f64> = rows.iter().map(|row| row.3[3]).collect();
    let observed_ats: Vec<i64> = rows.iter().map(|row| timestamptz_micros(row.4)).collect();

    let id_arr: ArrayRef = Arc::new(Int32Array::from(ids));
    let name_arr: ArrayRef = Arc::new(StringArray::from(names));
    let geom_arr: ArrayRef = Arc::new(BinaryArray::from(geoms));
    let xmin_arr: ArrayRef = Arc::new(Float64Array::from(xmins));
    let ymin_arr: ArrayRef = Arc::new(Float64Array::from(ymins));
    let xmax_arr: ArrayRef = Arc::new(Float64Array::from(xmaxs));
    let ymax_arr: ArrayRef = Arc::new(Float64Array::from(ymaxs));
    let observed_at_arr: ArrayRef =
        Arc::new(TimestampMicrosecondArray::from(observed_ats).with_timezone(ARROW_TZ));

    RecordBatch::try_new(
        Arc::new(arrow_fixture_schema()),
        vec![
            id_arr,
            name_arr,
            geom_arr,
            xmin_arr,
            ymin_arr,
            xmax_arr,
            ymax_arr,
            observed_at_arr,
        ],
    )
    .unwrap()
}

/// `prefix` must be distinct across every call sharing one fixture's
/// warehouse directory — `DefaultFileNameGenerator` numbers files from `0`
/// per instance, not per warehouse, so two single-shot generators (one per
/// `write_batch` call, as this fixture builder makes them) would otherwise
/// both name their first file `<prefix>-00000.parquet` and collide on
/// commit.
async fn write_batch(table: &Table, prefix: &str, batch: RecordBatch) -> Vec<DataFile> {
    let location_gen = DefaultLocationGenerator::new(table.metadata().clone()).unwrap();
    let file_name_gen =
        DefaultFileNameGenerator::new(format!("data-{prefix}"), None, DataFileFormat::Parquet);
    let writer_schema = Arc::new(table.metadata().current_schema().as_ref().clone());
    let parquet_writer_builder =
        ParquetWriterBuilder::new(WriterProperties::builder().build(), writer_schema);
    let rolling_writer_builder = RollingFileWriterBuilder::new_with_default_file_size(
        parquet_writer_builder,
        table.file_io().clone(),
        location_gen,
        file_name_gen,
    );
    let mut writer = DataFileWriterBuilder::new(rolling_writer_builder)
        .build(None)
        .await
        .unwrap();
    writer.write(batch).await.unwrap();
    writer.close().await.unwrap()
}

async fn commit_append(catalog: &dyn Catalog, table: &Table, data_files: Vec<DataFile>) -> Table {
    let tx = Transaction::new(table);
    let action = tx.fast_append().add_data_files(data_files);
    let tx = action.apply(tx).unwrap();
    tx.commit(catalog).await.unwrap()
}

// ---------------------------------------------------------------------------
// S3-backed fixture (`#123`)
// ---------------------------------------------------------------------------
//
// Everything below builds the same Iceberg table as the fixtures above, but
// with its warehouse rooted at an `s3://` URI, so the driver under test
// reads every manifest and data file through this crate's own S3 `FileIO`
// (`fileio.rs`) over a real HTTP loopback listener speaking the S3 protocol.
//
// The two halves are deliberately asymmetric, and that asymmetry is what
// makes the test worth anything:
//
// - the WRITE half (building the fixture table) uses [`FakeS3Storage`], a
//   test-only `iceberg::io::Storage` that maps `s3://{bucket}/{key}` to a
//   file under a tempdir and writes it directly. It never speaks HTTP.
//   Production code has no write path at all — `ObjectStoreStorage` refuses
//   every write verb by name — so a fixture cannot be built through it.
// - the READ half (the driver under test) uses the real production
//   `ObjectStoreStorage`, over real HTTP, against [`spawn_s3_server`], which
//   serves that same tempdir and records every request it answers.
//
// So the bytes the driver reads are genuinely fetched over the S3 protocol,
// signed with the workspace's own SigV4, from paths an Iceberg writer
// actually minted — none of it simulated on the read side.

/// Bucket name every S3 fixture uses. Arbitrary; it only has to appear
/// verbatim in the table metadata and in the request paths the fake store
/// receives, which is exactly what the tests assert.
pub(crate) const S3_BUCKET: &str = "tellurion-iceberg-test";

/// Region every S3 fixture signs for. Never validated by the fake store —
/// SigV4's own correctness is `tellurion_core::sigv4`'s test suite's job,
/// against AWS's published worked examples — but it must reach the signer,
/// which the `Credential=.../{region}/s3/aws4_request` assertion checks.
const S3_REGION: &str = "us-east-1";

const S3_ACCESS_KEY: &str = "tellurion-test-access-key";
const S3_SECRET_KEY: &str = "tellurion-test-secret-key";

/// One request the fake S3 store answered — what the assertions read.
#[derive(Debug, Clone)]
pub(crate) struct S3Request {
    pub method: String,
    pub path: String,
    pub range: Option<String>,
    pub authorization: Option<String>,
}

#[derive(Clone)]
struct FakeS3State {
    root: std::path::PathBuf,
    requests: Arc<std::sync::Mutex<Vec<S3Request>>>,
}

/// A test-only `iceberg::io::Storage` that resolves `s3://{bucket}/{key}` to
/// `{root}/{bucket}/{key}` on disk and reads/writes it directly — the WRITE
/// half described above. Deliberately NOT the production storage: production
/// refuses every write verb, so the only way to lay down a fixture table
/// whose metadata carries `s3://` paths is a writer that understands those
/// paths without going through HTTP.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct FakeS3Storage {
    root: String,
}

impl FakeS3Storage {
    /// Strips whatever `<scheme>://` prefix a path carries and resolves the
    /// remainder under `root`. Scheme-agnostic on purpose: the same writer
    /// lays down the `gs://` and `abfss://` fixtures the boot-refusal tests
    /// need, which the production storage would (correctly) refuse to touch.
    fn resolve(&self, path: &str) -> String {
        let rest = path.split_once("://").map(|(_, rest)| rest).unwrap_or(path);
        format!("{}/{rest}", self.root)
    }
}

#[async_trait::async_trait]
#[typetag::serde]
impl iceberg::io::Storage for FakeS3Storage {
    async fn exists(&self, path: &str) -> iceberg::Result<bool> {
        LocalFsStorage::new().exists(&self.resolve(path)).await
    }
    async fn metadata(&self, path: &str) -> iceberg::Result<iceberg::io::FileMetadata> {
        LocalFsStorage::new().metadata(&self.resolve(path)).await
    }
    async fn read(&self, path: &str) -> iceberg::Result<bytes::Bytes> {
        LocalFsStorage::new().read(&self.resolve(path)).await
    }
    async fn reader(&self, path: &str) -> iceberg::Result<Box<dyn iceberg::io::FileRead>> {
        LocalFsStorage::new().reader(&self.resolve(path)).await
    }
    async fn write(&self, path: &str, bs: bytes::Bytes) -> iceberg::Result<()> {
        LocalFsStorage::new().write(&self.resolve(path), bs).await
    }
    async fn writer(&self, path: &str) -> iceberg::Result<Box<dyn iceberg::io::FileWrite>> {
        LocalFsStorage::new().writer(&self.resolve(path)).await
    }
    async fn delete(&self, path: &str) -> iceberg::Result<()> {
        LocalFsStorage::new().delete(&self.resolve(path)).await
    }
    async fn delete_prefix(&self, path: &str) -> iceberg::Result<()> {
        LocalFsStorage::new()
            .delete_prefix(&self.resolve(path))
            .await
    }
    fn new_input(&self, path: &str) -> iceberg::Result<iceberg::io::InputFile> {
        Ok(iceberg::io::InputFile::new(
            Arc::new(self.clone()),
            path.to_string(),
        ))
    }
    fn new_output(&self, path: &str) -> iceberg::Result<iceberg::io::OutputFile> {
        Ok(iceberg::io::OutputFile::new(
            Arc::new(self.clone()),
            path.to_string(),
        ))
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct FakeS3StorageFactory {
    root: String,
}

#[typetag::serde]
impl iceberg::io::StorageFactory for FakeS3StorageFactory {
    fn build(
        &self,
        _config: &iceberg::io::StorageConfig,
    ) -> iceberg::Result<Arc<dyn iceberg::io::Storage>> {
        Ok(Arc::new(FakeS3Storage {
            root: self.root.clone(),
        }))
    }
}

/// A running S3-backed fixture. Beyond [`TestFixture`]'s own pieces it owns
/// the fake object store's request log, and the two environment variables
/// the locator NAMES for credentials — set here rather than in the test so
/// every S3 fixture gets its own uniquely named pair and parallel tests in
/// this binary cannot observe each other's.
pub(crate) struct S3TestFixture {
    server_addr: SocketAddr,
    s3_addr: SocketAddr,
    _warehouse: tempfile::TempDir,
    requests: Arc<std::sync::Mutex<Vec<S3Request>>>,
    access_key_env: String,
    secret_key_env: String,
}

impl S3TestFixture {
    /// The same two-row, one-data-file table [`TestFixture::build`] makes,
    /// with every file addressed `s3://{S3_BUCKET}/warehouse/...`.
    pub(crate) async fn build() -> Self {
        Self::on_scheme("s3").await
    }

    /// The same table with its warehouse rooted at an arbitrary URI scheme
    /// — `gs`, `abfss`, `hdfs`. Nothing here validates the scheme (the
    /// fixture's own writer is scheme-agnostic by design); the whole point
    /// is to hand the production driver a table it must refuse BY NAME.
    pub(crate) async fn on_scheme(scheme: &str) -> Self {
        Self::with_rows(
            scheme,
            vec![
                (
                    1,
                    Some("west"),
                    point_wkb(-3.0, 48.0),
                    [-3.0, 48.0, -3.0, 48.0],
                    "2020-01-01T00:00:00Z",
                ),
                (
                    2,
                    Some("east"),
                    point_wkb(-2.0, 49.0),
                    [-2.0, 49.0, -2.0, 49.0],
                    "2020-01-02T00:00:00Z",
                ),
            ],
        )
        .await
    }

    async fn with_rows(scheme: &str, rows: Vec<FixtureRow<'_>>) -> Self {
        let warehouse_dir = tempfile::tempdir().unwrap();
        let root = warehouse_dir.path().to_str().unwrap().to_string();

        let catalog = MemoryCatalogBuilder::default()
            .with_storage_factory(Arc::new(FakeS3StorageFactory { root: root.clone() }))
            .load(
                "memory",
                HashMap::from([(
                    MEMORY_CATALOG_WAREHOUSE.to_string(),
                    format!("{scheme}://{S3_BUCKET}/warehouse"),
                )]),
            )
            .await
            .unwrap();

        let namespace = NamespaceIdent::new(NAMESPACE.to_string());
        catalog
            .create_namespace(&namespace, HashMap::new())
            .await
            .unwrap();
        let creation = TableCreation::builder()
            .name(TABLE.to_string())
            .schema(table_schema())
            .build();
        let table = catalog.create_table(&namespace, creation).await.unwrap();

        let data_files = write_batch(&table, "initial", build_batch(rows)).await;
        commit_append(&catalog, &table, data_files).await;

        let catalog: Arc<dyn Catalog> = Arc::new(catalog);
        let server_addr = spawn_rest_server(Arc::clone(&catalog)).await;
        let (s3_addr, requests) = spawn_s3_server(warehouse_dir.path().to_path_buf()).await;

        // A unique variable pair per fixture instance: this crate's tests
        // run in parallel threads of one process, and a shared name would
        // let one fixture's teardown blank another's credential mid-read.
        let unique = format!("{}_{}", std::process::id(), s3_addr.port());
        let access_key_env = format!("TELLURION_ICEBERG_TEST_ACCESS_KEY_{unique}");
        let secret_key_env = format!("TELLURION_ICEBERG_TEST_SECRET_KEY_{unique}");
        // Safety: `std::env::set_var` is not thread-safe against a
        // concurrent reader in the same process. These two names are unique
        // per fixture (above) and are read exactly once, by this fixture's
        // own `resolve_s3_connection` call, after this line.
        std::env::set_var(&access_key_env, S3_ACCESS_KEY);
        std::env::set_var(&secret_key_env, S3_SECRET_KEY);

        Self {
            server_addr,
            s3_addr,
            _warehouse: warehouse_dir,
            requests,
            access_key_env,
            secret_key_env,
        }
    }

    pub(crate) fn backend(&self) -> IcebergBackend {
        IcebergBackend::new(self.location())
    }

    /// The locator a production `config.yaml` would hold in this storage's
    /// `url_env` variable: catalog URI, the four column declarations, and
    /// the four `s3_*` declarations — endpoint and region literally, the
    /// two credentials by ENVIRONMENT VARIABLE NAME only.
    pub(crate) fn location(&self) -> IcebergLocation {
        IcebergLocation::parse(&format!(
            "http://{}?namespace={NAMESPACE}&table={TABLE}&geometry={GEOMETRY_COLUMN}\
             &bbox={BBOX_COLUMNS}&s3_endpoint=http://{}&s3_region={S3_REGION}\
             &s3_access_key_env={}&s3_secret_key_env={}",
            self.server_addr, self.s3_addr, self.access_key_env, self.secret_key_env,
        ))
        .unwrap()
    }

    /// The same locator with one `s3_*` key dropped — for proving the
    /// missing-declaration refusal names it.
    pub(crate) fn location_without(&self, dropped: &str) -> IcebergLocation {
        let full = format!(
            "http://{}?namespace={NAMESPACE}&table={TABLE}&geometry={GEOMETRY_COLUMN}\
             &bbox={BBOX_COLUMNS}&s3_endpoint=http://{}&s3_region={S3_REGION}\
             &s3_access_key_env={}&s3_secret_key_env={}",
            self.server_addr, self.s3_addr, self.access_key_env, self.secret_key_env,
        );
        let (base, query) = full.split_once('?').unwrap();
        let kept: Vec<&str> = query
            .split('&')
            .filter(|pair| !pair.starts_with(&format!("{dropped}=")))
            .collect();
        IcebergLocation::parse(&format!("{base}?{}", kept.join("&"))).unwrap()
    }

    /// Every request the fake object store answered, in order.
    pub(crate) fn s3_requests(&self) -> Vec<S3Request> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn collection_decl(&self) -> CollectionDecl {
        serde_yaml::from_str(&format!("id: {TABLE}\ncatalog: default\nstorage: main\n")).unwrap()
    }

    pub(crate) fn access_key(&self) -> &str {
        S3_ACCESS_KEY
    }

    pub(crate) fn region(&self) -> &str {
        S3_REGION
    }
}

impl Drop for S3TestFixture {
    fn drop(&mut self) {
        // Safety: the two names are unique to this fixture (see
        // `with_rows`), so nothing else in this process can be reading them.
        std::env::remove_var(&self.access_key_env);
        std::env::remove_var(&self.secret_key_env);
    }
}

/// A loopback listener speaking the read half of the S3 protocol against
/// `root`: `GET`/`HEAD /{bucket}/{key...}`, honoring `Range` with a real
/// `206 Partial Content`, and recording every request for the assertions.
///
/// Deliberately does NOT verify the SigV4 signature. Reimplementing the
/// signer here to check it against itself would prove nothing;
/// `tellurion_core::sigv4`'s own suite already checks it against AWS's
/// published worked examples. What this fixture proves instead is that the
/// driver really did sign — the `Authorization` header it recorded names
/// the algorithm, the access key the LOCATOR pointed at, and the region —
/// and that the bytes came over HTTP rather than off local disk.
async fn spawn_s3_server(
    root: std::path::PathBuf,
) -> (SocketAddr, Arc<std::sync::Mutex<Vec<S3Request>>>) {
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let state = FakeS3State {
        root,
        requests: Arc::clone(&requests),
    };
    let app = Router::new()
        .route("/{bucket}/{*key}", get(serve_object).head(serve_object))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, requests)
}

async fn serve_object(
    State(state): State<FakeS3State>,
    axum::extract::Path((bucket, key)): axum::extract::Path<(String, String)>,
    method: axum::http::Method,
    headers: axum::http::HeaderMap,
) -> Response {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let range = header("range");
    state
        .requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(S3Request {
            method: method.as_str().to_string(),
            path: format!("/{bucket}/{key}"),
            range: range.clone(),
            authorization: header("authorization"),
        });

    let path = state.root.join(&bucket).join(&key);
    let Ok(bytes) = std::fs::read(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if method == axum::http::Method::HEAD {
        // Content-Length set explicitly against an empty body, exactly as a
        // real store answers HEAD — this is the header
        // `S3ObjectStore::head_path` reads a size out of.
        return axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_LENGTH, bytes.len())
            .body(axum::body::Body::empty())
            .unwrap();
    }

    let Some(range) = range else {
        return (StatusCode::OK, bytes).into_response();
    };
    // `bytes=<first>-<last>`, inclusive at both ends — the only spelling
    // `S3ObjectStore::get_path_range` ever sends.
    let spec = range.trim_start_matches("bytes=");
    let (first, last) = spec.split_once('-').expect("a bytes=first-last range");
    let first: usize = first.parse().expect("a numeric range start");
    let last: usize = last.parse().expect("a numeric range end");
    let total = bytes.len();
    let start = first.min(total);
    let end = (last + 1).min(total);
    let slice = bytes[start..end.max(start)].to_vec();
    axum::http::Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(
            axum::http::header::CONTENT_RANGE,
            format!("bytes {start}-{}/{total}", end.saturating_sub(1)),
        )
        .body(axum::body::Body::from(slice))
        .unwrap()
}
