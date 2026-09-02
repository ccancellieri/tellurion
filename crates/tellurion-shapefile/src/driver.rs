//! Read-only Shapefile Features over a [`ValidatedShapefile`].
//!
//! The archive layer establishes the only file boundary this module accepts;
//! it never opens an unvalidated companion set.  `.shx` supplies the stable
//! physical record order used for `fid` and page cursors, but not spatial
//! envelopes.  A bbox request therefore decodes every record and is refused
//! before decoding when either configured scan limit would be crossed.
//!
//! DBF strings follow a supported `.cpg` label through `shapefile`'s
//! `encoding_rs` feature.  With no `.cpg` (or an unsupported label), the
//! upstream DBF reader's documented `UnicodeLossy` UTF-8 fallback is used.

use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tellurion_core::{
    heuristics, AttributeColumn, CatalogSource, CollectionDecl, DriverFactory, Error as CoreError,
    FeaturePage, FeatureSource, ItemsQuery, PhysicalCollection, Result as CoreResult,
    SpatialExtent, StorageDecl, StorageDriver, TileCoord, TileSource, DEFAULT_TILE_VERTEX_BUDGET,
};
use tellurion_http_source::{
    ContentIdentity, RangeObject, SourceError, SourceErrorKind, SourceHandle,
};
use tellurion_vector_tile::{
    encode_tile, tile_envelope_3857, SourceCrs, TileFeature, TileRequest, TileScalar,
};
use tokio::sync::OnceCell;

use crate::{crs, ArchiveLimits, ArchiveSpool, ValidatedShapefile};

const GEOMETRY_FIELD: &str = "geometry";
const PRIMARY_KEY_FIELD: &str = "fid";
const MVT_EXTENT: u32 = 4096;

/// Default public scan ceiling for exact O(n) bbox filtering.
const DEFAULT_SCAN_RECORDS: u64 = 100_000;
const DEFAULT_SCAN_BYTES: u64 = 64 * 1024 * 1024;

/// Both limits apply before a bbox scan starts, so a request cannot receive a
/// partial result merely because an archive is too large to scan safely.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanLimits {
    pub max_records: u64,
    pub max_bytes: u64,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_records: DEFAULT_SCAN_RECORDS,
            max_bytes: DEFAULT_SCAN_BYTES,
        }
    }
}

/// Public construction seam for a local or already-materialized archive.
pub struct ShapefileBackend {
    files: ValidatedShapefile,
    limits: ScanLimits,
    preflight: OnceCell<Arc<Preflight>>,
    metadata: OnceCell<Arc<Metadata>>,
}

impl ShapefileBackend {
    pub fn new(files: ValidatedShapefile) -> Self {
        Self {
            files,
            limits: ScanLimits::default(),
            preflight: OnceCell::new(),
            metadata: OnceCell::new(),
        }
    }

    pub fn with_scan_limits(&self, limits: ScanLimits) -> Self {
        Self {
            files: self.files.clone(),
            limits,
            preflight: OnceCell::new(),
            metadata: OnceCell::new(),
        }
    }

    async fn preflight(&self) -> DriverResult<Arc<Preflight>> {
        let cached = self
            .preflight
            .get_or_try_init(|| async {
                let files = self.files.clone();
                run_blocking(move || read_preflight(&files).map(Arc::new)).await
            })
            .await?;
        Ok(Arc::clone(cached))
    }

    async fn metadata(&self) -> DriverResult<Arc<Metadata>> {
        let cached = self
            .metadata
            .get_or_try_init(|| async {
                let files = self.files.clone();
                run_blocking(move || read_metadata(&files).map(Arc::new)).await
            })
            .await?;
        Ok(Arc::clone(cached))
    }

    async fn items_inner(&self, query: &ItemsQuery) -> DriverResult<FeaturePage> {
        if query.datetime.is_some() {
            return Err(DriverError::DatetimeUnsupported);
        }
        if query.filter.is_some() {
            return Err(DriverError::FilterUnsupported);
        }
        let token = parse_token(query.token.as_deref())?;
        if query.bbox.is_some() {
            let preflight = self.preflight().await?;
            if preflight.records > self.limits.max_records
                || preflight.shp_bytes > self.limits.max_bytes
            {
                return Err(DriverError::ScanLimitExceeded {
                    max_records: self.limits.max_records,
                    max_bytes: self.limits.max_bytes,
                });
            }
        }
        let metadata = self.metadata().await?;
        let files = self.files.clone();
        let bbox = query.bbox;
        let limit = query.limit;
        run_blocking(move || read_items(&files, metadata.records, token, limit, bbox)).await
    }

    async fn item_inner(&self, id: &str) -> DriverResult<Option<Value>> {
        let Ok(index) = id.parse::<u64>() else {
            return Ok(None);
        };
        let metadata = self.metadata().await?;
        if index >= metadata.records {
            return Ok(None);
        }
        let files = self.files.clone();
        run_blocking(move || read_item(&files, index)).await
    }

    async fn mvt_tile_inner(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
    ) -> CoreResult<Option<bytes::Bytes>> {
        let tile_envelope =
            tile_envelope_3857(coord).map_err(|error| CoreError::Invalid(error.to_string()))?;
        let preflight = self.preflight().await.map_err(CoreError::from)?;
        if preflight.srid != 4326 {
            return Err(unsupported_tile_crs(collection));
        }
        if preflight.records > self.limits.max_records
            || preflight.shp_bytes > self.limits.max_bytes
        {
            return Err(unsupported_tile_scan_budget(collection));
        }
        let metadata = self.metadata().await.map_err(CoreError::from)?;
        if metadata.srid != Some(4326) {
            return Err(unsupported_tile_crs(collection));
        }

        let feature_cap = usize::try_from(heuristics::effective_feature_cap(
            &collection.tiles.caps,
            coord.z,
            collection.row_estimate,
        ))
        .unwrap_or(usize::MAX);
        let files = self.files.clone();
        let query_bbox = web_mercator_envelope_to_crs84(tile_envelope);
        let selected_properties = collection.tile_properties.clone();
        let features = run_blocking(move || {
            read_tile_features(&files, query_bbox, &selected_properties, feature_cap)
        })
        .await
        .map_err(CoreError::from)?;
        let request = TileRequest::new(
            coord,
            collection.external_id(),
            collection.tile_properties.clone(),
            feature_cap,
            collection
                .settings
                .tile_vertex_budget
                .unwrap_or(DEFAULT_TILE_VERTEX_BUDGET),
            MVT_EXTENT,
            SourceCrs::Crs84,
        );
        encode_tile(request, features.into_iter().map(Ok))
            .map_err(|error| CoreError::Storage(Box::new(error)))
    }
}

/// Registers a local ZIP archive driver. Remote registration materializes and
/// validates first, then constructs [`ShapefileBackend`] directly; local ZIPs
/// go through the exact same `ArchiveSpool::materialize` validation path.
#[derive(Default)]
pub struct ShapefileDriverFactory;

impl ShapefileDriverFactory {
    pub fn new() -> Self {
        Self
    }
}

impl DriverFactory for ShapefileDriverFactory {
    fn name(&self) -> &str {
        "shapefile"
    }

    fn build(&self, decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
        let path = std::env::var(&decl.url_env).map_err(|_| {
            CoreError::Config(format!(
                "storage '{}': environment variable '{}' is not set",
                decl.id, decl.url_env
            ))
        })?;
        let root = tempfile::Builder::new()
            .prefix("tellurion-shapefile-")
            .tempdir()
            .map_err(|_| {
                CoreError::Config("could not create Shapefile archive working directory".into())
            })?;
        let object = Arc::new(
            LocalArchive::new(PathBuf::from(path))
                .map_err(|_| CoreError::Config("local Shapefile archive is unavailable".into()))?,
        );
        let spool = ArchiveSpool::new(root.path(), ArchiveLimits::default()).map_err(|_| {
            CoreError::Config("could not initialize Shapefile archive validation".into())
        })?;
        Ok(Arc::new(ShapefileDriverImpl {
            backend: Arc::new(LocalBackend {
                object,
                spool,
                _root: Arc::new(root),
                backend: OnceCell::new(),
            }),
        }))
    }
}

struct ShapefileDriverImpl {
    backend: Arc<LocalBackend>,
}

impl StorageDriver for ShapefileDriverImpl {
    fn catalog_source(&self) -> Arc<dyn CatalogSource> {
        Arc::clone(&self.backend) as Arc<dyn CatalogSource>
    }
    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn FeatureSource>)
    }
    fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
        Some(Arc::clone(&self.backend) as Arc<dyn TileSource>)
    }
}

struct LocalBackend {
    object: Arc<LocalArchive>,
    spool: ArchiveSpool,
    _root: Arc<tempfile::TempDir>,
    backend: OnceCell<Arc<ShapefileBackend>>,
}

impl LocalBackend {
    async fn resolved(&self) -> DriverResult<Arc<ShapefileBackend>> {
        let backend = self
            .backend
            .get_or_try_init(|| async {
                let files = self
                    .spool
                    .materialize(Arc::clone(&self.object) as Arc<dyn RangeObject>)
                    .await
                    .map_err(DriverError::from)?;
                Ok::<_, DriverError>(Arc::new(ShapefileBackend::new(files)))
            })
            .await?;
        Ok(Arc::clone(backend))
    }
}

#[async_trait]
impl CatalogSource for LocalBackend {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        self.resolved()
            .await
            .map_err(CoreError::from)?
            .collections()
            .await
    }
    async fn extent(&self, physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        self.resolved()
            .await
            .map_err(CoreError::from)?
            .extent(physical)
            .await
    }
    async fn row_estimate(&self, physical: &PhysicalCollection) -> CoreResult<Option<u64>> {
        self.resolved()
            .await
            .map_err(CoreError::from)?
            .row_estimate(physical)
            .await
    }
    async fn attribute_schema(
        &self,
        physical: &PhysicalCollection,
    ) -> CoreResult<Option<Vec<AttributeColumn>>> {
        self.resolved()
            .await
            .map_err(CoreError::from)?
            .attribute_schema(physical)
            .await
    }
}

#[async_trait]
impl FeatureSource for LocalBackend {
    async fn items(
        &self,
        collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        self.resolved()
            .await
            .map_err(CoreError::from)?
            .items(collection, query)
            .await
    }
    async fn item(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&tellurion_core::Filter>,
    ) -> CoreResult<Option<Value>> {
        self.resolved()
            .await
            .map_err(CoreError::from)?
            .item(collection, id, filter)
            .await
    }
}

#[async_trait]
impl TileSource for LocalBackend {
    async fn mvt_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&tellurion_core::Filter>,
    ) -> CoreResult<Option<bytes::Bytes>> {
        if filter.is_some() {
            return Err(CoreError::Invalid(
                "Shapefile driver does not support the 'filter' parameter".to_string(),
            ));
        }
        self.resolved()
            .await
            .map_err(CoreError::from)?
            .mvt_tile(collection, coord, None)
            .await
    }

    fn tile_capable(&self, collection: &CollectionDecl) -> bool {
        self.backend
            .get()
            .is_none_or(|backend| backend.tile_capable(collection))
    }
}

struct LocalArchive {
    path: PathBuf,
    handle: SourceHandle,
    identity: ContentIdentity,
}

impl LocalArchive {
    fn new(path: PathBuf) -> std::io::Result<Self> {
        let length = fs::metadata(&path)?.len();
        Ok(Self {
            path,
            handle: SourceHandle::new("local-shapefile"),
            identity: ContentIdentity::StrongEtag {
                source_key: [0; 32],
                revision_key: [0; 32],
                length,
            },
        })
    }
}

#[async_trait]
impl RangeObject for LocalArchive {
    fn handle(&self) -> &SourceHandle {
        &self.handle
    }
    fn identity(&self) -> &ContentIdentity {
        &self.identity
    }
    fn length(&self) -> u64 {
        match self.identity {
            ContentIdentity::StrongEtag { length, .. } => length,
        }
    }
    fn display_name(&self) -> &str {
        "local Shapefile archive"
    }
    async fn get_range(&self, range: Range<u64>) -> Result<bytes::Bytes, SourceError> {
        if range.start >= range.end || range.end > self.length() {
            return Err(SourceError::for_handle(
                SourceErrorKind::Range,
                &self.handle,
            ));
        }
        let path = self.path.clone();
        let handle = self.handle.clone();
        tokio::task::spawn_blocking(move || {
            let mut file = fs::File::open(path)
                .map_err(|_| SourceError::for_handle(SourceErrorKind::Transport, &handle))?;
            file.seek(SeekFrom::Start(range.start))
                .map_err(|_| SourceError::for_handle(SourceErrorKind::Transport, &handle))?;
            let mut bytes = vec![
                0;
                usize::try_from(range.end - range.start).map_err(|_| {
                    SourceError::for_handle(SourceErrorKind::Range, &handle)
                })?
            ];
            file.read_exact(&mut bytes)
                .map_err(|_| SourceError::for_handle(SourceErrorKind::Transport, &handle))?;
            Ok(bytes::Bytes::from(bytes))
        })
        .await
        .map_err(|_| SourceError::for_handle(SourceErrorKind::Transport, &self.handle))?
    }
}

struct Metadata {
    name: String,
    records: u64,
    geometry_type: Option<String>,
    srid: Option<i32>,
    extent: Option<[f64; 4]>,
    attributes: Vec<AttributeColumn>,
}

struct Preflight {
    records: u64,
    shp_bytes: u64,
    srid: i32,
}

#[async_trait]
impl CatalogSource for ShapefileBackend {
    async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
        let metadata = self.metadata().await.map_err(CoreError::from)?;
        Ok(vec![PhysicalCollection {
            name: metadata.name.clone(),
            geometry_column: Some(GEOMETRY_FIELD.into()),
            primary_key: Some(PRIMARY_KEY_FIELD.into()),
            srid: metadata.srid,
            geometry_type: metadata.geometry_type.clone(),
        }])
    }

    async fn extent(&self, _physical: &PhysicalCollection) -> CoreResult<Option<SpatialExtent>> {
        Ok(self
            .metadata()
            .await
            .map_err(CoreError::from)?
            .extent
            .map(|bbox| SpatialExtent { bbox }))
    }

    async fn row_estimate(&self, _physical: &PhysicalCollection) -> CoreResult<Option<u64>> {
        Ok(Some(
            self.metadata().await.map_err(CoreError::from)?.records,
        ))
    }

    async fn attribute_schema(
        &self,
        _physical: &PhysicalCollection,
    ) -> CoreResult<Option<Vec<AttributeColumn>>> {
        Ok(Some(
            self.metadata()
                .await
                .map_err(CoreError::from)?
                .attributes
                .clone(),
        ))
    }
}

#[async_trait]
impl FeatureSource for ShapefileBackend {
    async fn items(
        &self,
        _collection: &CollectionDecl,
        query: &ItemsQuery,
    ) -> CoreResult<FeaturePage> {
        self.items_inner(query).await.map_err(Into::into)
    }

    async fn item(
        &self,
        _collection: &CollectionDecl,
        id: &str,
        _filter: Option<&tellurion_core::Filter>,
    ) -> CoreResult<Option<Value>> {
        self.item_inner(id).await.map_err(Into::into)
    }
}

#[async_trait]
impl TileSource for ShapefileBackend {
    async fn mvt_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&tellurion_core::Filter>,
    ) -> CoreResult<Option<bytes::Bytes>> {
        if filter.is_some() {
            return Err(CoreError::Invalid(
                "Shapefile driver does not support the 'filter' parameter".to_string(),
            ));
        }
        self.mvt_tile_inner(collection, coord).await
    }

    fn tile_capable(&self, _collection: &CollectionDecl) -> bool {
        self.metadata
            .get()
            .is_none_or(|metadata| metadata.srid == Some(4326))
    }
}

fn read_metadata(files: &ValidatedShapefile) -> DriverResult<Metadata> {
    let shx_records = validate_shx_records(&files.shp, &files.shx)?;
    let reader = shapefile::ShapeReader::from_path(&files.shp).map_err(DriverError::read)?;
    let records = u64::try_from(reader.shape_count().map_err(DriverError::read)?)
        .map_err(|_| DriverError::InvalidRecordCount)?;
    let mut physical_reader =
        shapefile::ShapeReader::new(fs::File::open(&files.shp).map_err(DriverError::read)?)
            .map_err(DriverError::read)?;
    let mut physical_shapes = 0usize;
    let mut observed_extent: Option<[f64; 4]> = None;
    for shape in physical_reader.iter_shapes() {
        physical_shapes += 1;
        let shape = shape.map_err(DriverError::read)?;
        if let Some(geometry) = geometry(&shape)? {
            let [minx, miny, maxx, maxy] = geometry_bbox(&geometry);
            observed_extent = Some(match observed_extent {
                Some([left, bottom, right, top]) => [
                    left.min(minx),
                    bottom.min(miny),
                    right.max(maxx),
                    top.max(maxy),
                ],
                None => [minx, miny, maxx, maxy],
            });
        }
    }
    let dbf_records = validate_dbf_physical_rows(&files.dbf)?;
    if records != shx_records
        || records != u64::try_from(physical_shapes).map_err(|_| DriverError::InvalidRecordCount)?
        || records != dbf_records
    {
        return Err(DriverError::RecordCountMismatch);
    }
    let header = reader.header();
    if header.shape_type == shapefile::ShapeType::Multipatch {
        return Err(DriverError::MultipatchUnsupported);
    }
    let srid = crs::epsg(files.prj.as_deref()).ok_or(DriverError::UnsupportedCrs)?;
    let extent = (srid == 4326).then_some(observed_extent).flatten();
    let attributes = read_attributes(&files.dbf, files.cpg.as_deref())?;
    Ok(Metadata {
        name: files
            .shp
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("dataset")
            .to_owned(),
        records,
        geometry_type: geometry_type(header.shape_type),
        srid: Some(srid),
        extent,
        attributes,
    })
}

fn read_preflight(files: &ValidatedShapefile) -> DriverResult<Preflight> {
    let shp_bytes = fs::metadata(&files.shp).map_err(DriverError::read)?.len();
    let shx_bytes = fs::metadata(&files.shx).map_err(DriverError::read)?.len();
    let entries_bytes = shx_bytes.checked_sub(100).ok_or(DriverError::Read)?;
    if entries_bytes % 8 != 0 {
        return Err(DriverError::Read);
    }
    let srid = crs::epsg(files.prj.as_deref()).ok_or(DriverError::UnsupportedCrs)?;
    Ok(Preflight {
        records: entries_bytes / 8,
        shp_bytes,
        srid,
    })
}

fn validate_shx_records(shp: &Path, shx: &Path) -> DriverResult<u64> {
    let shp = fs::read(shp).map_err(DriverError::read)?;
    let shx = fs::read(shx).map_err(DriverError::read)?;
    if shp.len() < 100 || shx.len() < 100 || (shx.len() - 100) % 8 != 0 {
        return Err(DriverError::Read);
    }

    let mut shp_offset = 100usize;
    for entry in shx[100..].chunks_exact(8) {
        let offset_words = u32::from_be_bytes(entry[..4].try_into().map_err(DriverError::read)?);
        let length_words = u32::from_be_bytes(entry[4..].try_into().map_err(DriverError::read)?);
        let expected_words = u32::try_from(shp_offset / 2).map_err(|_| DriverError::Read)?;
        if offset_words != expected_words {
            return Err(DriverError::Read);
        }
        let record_header_end = shp_offset.checked_add(8).ok_or(DriverError::Read)?;
        let record_header = shp
            .get(shp_offset..record_header_end)
            .ok_or(DriverError::Read)?;
        let shp_length_words =
            u32::from_be_bytes(record_header[4..].try_into().map_err(DriverError::read)?);
        if length_words != shp_length_words {
            return Err(DriverError::Read);
        }
        let body_bytes = usize::try_from(length_words)
            .map_err(|_| DriverError::Read)?
            .checked_mul(2)
            .ok_or(DriverError::Read)?;
        shp_offset = record_header_end
            .checked_add(body_bytes)
            .ok_or(DriverError::Read)?;
        if shp_offset > shp.len() {
            return Err(DriverError::Read);
        }
    }
    if shp_offset != shp.len() {
        return Err(DriverError::Read);
    }
    u64::try_from((shx.len() - 100) / 8).map_err(|_| DriverError::InvalidRecordCount)
}

#[derive(Clone)]
struct DbfField {
    name: String,
    field_type: u8,
    offset: usize,
    width: usize,
}

struct DbfLayout {
    count: usize,
    header: usize,
    row: usize,
    fields: Vec<DbfField>,
}

fn dbf_layout(bytes: &[u8]) -> DriverResult<DbfLayout> {
    if bytes.len() < 33 {
        return Err(DriverError::Read);
    }
    let count = usize::try_from(u32::from_le_bytes(
        bytes[4..8].try_into().map_err(DriverError::read)?,
    ))
    .map_err(|_| DriverError::InvalidRecordCount)?;
    let header = usize::from(u16::from_le_bytes(
        bytes[8..10].try_into().map_err(DriverError::read)?,
    ));
    let row = usize::from(u16::from_le_bytes(
        bytes[10..12].try_into().map_err(DriverError::read)?,
    ));
    if row == 0 || header < 33 || header > bytes.len() || (header - 33) % 32 != 0 {
        return Err(DriverError::Read);
    }
    if bytes.get(header - 1) != Some(&0x0d) {
        return Err(DriverError::Read);
    }

    let mut fields = Vec::new();
    let mut offset = 1usize;
    for field in bytes[32..header - 1].chunks_exact(32) {
        let width = usize::from(field[16]);
        let end = offset.checked_add(width).ok_or(DriverError::Read)?;
        if end > row {
            return Err(DriverError::Read);
        }
        fields.push(DbfField {
            name: String::from_utf8_lossy(&field[..11])
                .trim_end_matches('\0')
                .to_owned(),
            field_type: field[11],
            offset,
            width,
        });
        offset = end;
    }
    if offset != row {
        return Err(DriverError::Read);
    }
    let rows_end = header
        .checked_add(count.checked_mul(row).ok_or(DriverError::Read)?)
        .ok_or(DriverError::Read)?;
    if rows_end > bytes.len() {
        return Err(DriverError::Read);
    }
    Ok(DbfLayout {
        count,
        header,
        row,
        fields,
    })
}

fn validate_dbf_physical_rows(dbf: &Path) -> DriverResult<u64> {
    let bytes = fs::read(dbf).map_err(DriverError::read)?;
    let layout = dbf_layout(&bytes)?;
    for field in &layout.fields {
        match field.field_type {
            b'M' => return Err(DriverError::MemoUnsupported),
            // dbase exposes these as binary floats, losing the original fixed
            // decimal spelling.  Refuse rather than silently changing IDs or
            // high-scale decimal values on the JSON wire.
            b'B' | b'Y' if field.width < 8 => return Err(DriverError::Read),
            _ => {}
        }
    }
    for index in 0..layout.count {
        let start = layout
            .header
            .checked_add(index.checked_mul(layout.row).ok_or(DriverError::Read)?)
            .ok_or(DriverError::Read)?;
        let record = bytes
            .get(start..start.checked_add(layout.row).ok_or(DriverError::Read)?)
            .ok_or(DriverError::Read)?;
        if record.first() == Some(&b'*') {
            return Err(DriverError::DeletedRow);
        }
        for field in &layout.fields {
            if !matches!(field.field_type, b'B' | b'Y') {
                continue;
            }
            let value = record
                .get(field.offset..field.offset + field.width)
                .and_then(|value| value.get(..8))
                .and_then(|value| value.try_into().ok())
                .map(f64::from_le_bytes)
                .ok_or(DriverError::Read)?;
            if !value.is_finite() {
                return Err(DriverError::BinaryNonFinite);
            }
        }
    }
    u64::try_from(layout.count).map_err(|_| DriverError::InvalidRecordCount)
}

fn read_attributes(dbf: &Path, cpg: Option<&Path>) -> DriverResult<Vec<AttributeColumn>> {
    let encoding = cpg
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|name| shapefile::dbase::encoding::DynEncoding::from_name(&name));
    let reader = match encoding {
        Some(encoding) => shapefile::dbase::ReaderBuilder::new()
            .with_encoding(encoding)
            .open(dbf),
        None => shapefile::dbase::Reader::from_path(dbf),
    }
    .map_err(DriverError::read)?;
    Ok(reader
        .fields()
        .iter()
        .map(|field| AttributeColumn {
            name: field.name().to_owned(),
            sql_type: dbase_type(field.field_type()),
        })
        .collect())
}

fn read_items(
    files: &ValidatedShapefile,
    records: u64,
    token: Option<u64>,
    limit: u32,
    bbox: Option<[f64; 4]>,
) -> DriverResult<FeaturePage> {
    let mut reader = shapefile::Reader::from_path(&files.shp).map_err(DriverError::read)?;
    let raw_numeric = raw_numeric_rows(&files.dbf)?;
    let start = token.unwrap_or(0);
    let mut features = Vec::new();
    let mut matched = 0u64;
    let mut next = None;
    for (index, result) in reader.iter_shapes_and_records().enumerate() {
        let index = u64::try_from(index).map_err(|_| DriverError::InvalidRecordCount)?;
        let (shape, record) = result.map_err(DriverError::read)?;
        let geometry = geometry(&shape)?;
        if bbox.is_some_and(|bbox| {
            !geometry
                .as_ref()
                .is_some_and(|geometry| geometry_intersects_bbox(geometry, bbox))
        }) {
            continue;
        }
        matched += 1;
        if index < start {
            continue;
        }
        if features.len() == usize::try_from(limit).unwrap_or(usize::MAX) {
            next.get_or_insert_with(|| index.to_string());
            continue;
        }
        features.push(feature(
            index,
            geometry,
            record,
            raw_numeric.get(index as usize),
        ));
    }
    Ok(FeaturePage {
        features_geojson: features,
        number_matched: bbox.map(|_| matched).or(Some(records)),
        next_token: next,
    })
}

fn read_item(files: &ValidatedShapefile, target: u64) -> DriverResult<Option<Value>> {
    let mut reader = shapefile::Reader::from_path(&files.shp).map_err(DriverError::read)?;
    reader
        .seek(usize::try_from(target).map_err(|_| DriverError::InvalidRecordCount)?)
        .map_err(DriverError::read)?;
    match reader.iter_shapes_and_records().next() {
        Some(Ok((shape, record))) => Ok(Some(feature(
            target,
            geometry(&shape)?,
            record,
            raw_numeric_rows(&files.dbf)?.get(target as usize),
        ))),
        Some(Err(error)) => Err(DriverError::read(error)),
        None => Ok(None),
    }
}

fn read_tile_features(
    files: &ValidatedShapefile,
    bbox: [f64; 4],
    selected_properties: &[String],
    feature_cap: usize,
) -> DriverResult<Vec<TileFeature>> {
    if feature_cap == 0 {
        return Ok(Vec::new());
    }
    let mut reader = shapefile::Reader::from_path(&files.shp).map_err(DriverError::read)?;
    let raw_numeric = raw_numeric_rows(&files.dbf)?;
    let mut features = Vec::with_capacity(feature_cap);
    for (index, result) in reader.iter_shapes_and_records().enumerate() {
        if features.len() == feature_cap {
            break;
        }
        let index = u64::try_from(index).map_err(|_| DriverError::InvalidRecordCount)?;
        let (shape, record) = result.map_err(DriverError::read)?;
        let Some(geometry_json) = geometry(&shape)? else {
            continue;
        };
        if !geometry_intersects_bbox(&geometry_json, bbox) {
            continue;
        }
        let geometry = geometry_to_geo_types(&geometry_json)?;
        let properties = feature_properties(record, raw_numeric.get(index as usize));
        let mut tile_properties = Vec::with_capacity(selected_properties.len());
        for name in selected_properties {
            let Some(value) = properties.get(name) else {
                continue;
            };
            tile_properties.push((name.clone(), tile_scalar(value)?));
        }
        features.push(TileFeature::new(
            index.to_string(),
            geometry,
            tile_properties,
        ));
    }
    Ok(features)
}

fn web_mercator_envelope_to_crs84([minx, miny, maxx, maxy]: [f64; 4]) -> [f64; 4] {
    const WEB_MERCATOR_RADIUS: f64 = 6_378_137.0;
    let inverse = |x: f64, y: f64| {
        let lon = x.to_degrees() / WEB_MERCATOR_RADIUS;
        let lat = (2.0 * (y / WEB_MERCATOR_RADIUS).exp().atan() - std::f64::consts::FRAC_PI_2)
            .to_degrees();
        (lon, lat)
    };
    let (min_lon, min_lat) = inverse(minx, miny);
    let (max_lon, max_lat) = inverse(maxx, maxy);
    [min_lon, min_lat, max_lon, max_lat]
}

fn unsupported_tile_crs(collection: &CollectionDecl) -> CoreError {
    CoreError::CapabilityUnsupported {
        collection: collection.id.clone(),
        capability: "tiles:crs84".to_string(),
    }
}

fn unsupported_tile_scan_budget(collection: &CollectionDecl) -> CoreError {
    CoreError::CapabilityUnsupported {
        collection: collection.id.clone(),
        capability: "tiles:scan-budget".to_string(),
    }
}

fn feature(
    index: u64,
    geometry: Option<Value>,
    record: shapefile::dbase::Record,
    raw_numeric: Option<&Map<String, Value>>,
) -> Value {
    json!({ "type": "Feature", "id": index.to_string(), "geometry": geometry, "properties": feature_properties(record, raw_numeric) })
}

fn feature_properties(
    record: shapefile::dbase::Record,
    raw_numeric: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let mut properties = record
        .into_iter()
        .map(|(name, value)| (name, dbase_value(value)))
        .collect::<Map<_, _>>();
    if let Some(raw_numeric) = raw_numeric {
        properties.extend(raw_numeric.clone());
    }
    properties
}

fn tile_scalar(value: &Value) -> DriverResult<TileScalar> {
    match value {
        Value::Null => Ok(TileScalar::Null),
        Value::Bool(value) => Ok(TileScalar::Bool(*value)),
        Value::String(value) => Ok(TileScalar::String(value.clone())),
        Value::Number(value) => value
            .as_i64()
            .map(TileScalar::Signed)
            .or_else(|| value.as_u64().map(TileScalar::Unsigned))
            .or_else(|| value.as_f64().map(TileScalar::Float))
            .ok_or(DriverError::TileProperty),
        Value::Array(_) | Value::Object(_) => Err(DriverError::TileProperty),
    }
}

fn raw_numeric_rows(dbf: &Path) -> DriverResult<Vec<Map<String, Value>>> {
    let bytes = fs::read(dbf).map_err(DriverError::read)?;
    let layout = dbf_layout(&bytes)?;
    (0..layout.count)
        .map(|index| {
            let start = layout
                .header
                .checked_add(index.checked_mul(layout.row).ok_or(DriverError::Read)?)
                .ok_or(DriverError::Read)?;
            let record = bytes
                .get(start..start.checked_add(layout.row).ok_or(DriverError::Read)?)
                .ok_or(DriverError::Read)?;
            Ok(layout
                .fields
                .iter()
                .filter(|field| matches!(field.field_type, b'N' | b'F'))
                .map(|field| {
                    let value = record
                        .get(field.offset..field.offset + field.width)
                        .and_then(|value| std::str::from_utf8(value).ok())
                        .map(str::trim)
                        .filter(|v| !v.is_empty() && !v.bytes().all(|byte| byte == b'*'))
                        .map(|v| Value::String(v.to_owned()))
                        .unwrap_or(Value::Null);
                    (field.name.clone(), value)
                })
                .collect())
        })
        .collect::<DriverResult<Vec<_>>>()
}

fn geometry(shape: &shapefile::Shape) -> DriverResult<Option<Value>> {
    use shapefile::Shape;
    let point = |point: &dyn dyn_xy::PointLike| {
        Value::Array(point.coordinates().into_iter().map(Value::from).collect())
    };
    Ok(match shape {
        Shape::NullShape => None,
        Shape::Point(value) => Some(json!({ "type": "Point", "coordinates": [value.x, value.y] })),
        Shape::PointM(value) => Some(json!({ "type": "Point", "coordinates": [value.x, value.y] })),
        Shape::PointZ(value) => {
            Some(json!({ "type": "Point", "coordinates": [value.x, value.y, value.z] }))
        }
        Shape::Polyline(value) => line_geometry(value.parts(), &point),
        Shape::PolylineM(value) => line_geometry(value.parts(), &point),
        Shape::PolylineZ(value) => line_geometry(value.parts(), &point),
        Shape::Polygon(value) => polygon_geometry(value.rings(), &point)?,
        Shape::PolygonM(value) => polygon_geometry(value.rings(), &point)?,
        Shape::PolygonZ(value) => polygon_geometry(value.rings(), &point)?,
        Shape::Multipoint(value) => Some(
            json!({ "type": "MultiPoint", "coordinates": value.points().iter().map(|value| point(value)).collect::<Vec<_>>() }),
        ),
        Shape::MultipointM(value) => Some(
            json!({ "type": "MultiPoint", "coordinates": value.points().iter().map(|value| point(value)).collect::<Vec<_>>() }),
        ),
        Shape::MultipointZ(value) => Some(
            json!({ "type": "MultiPoint", "coordinates": value.points().iter().map(|value| point(value)).collect::<Vec<_>>() }),
        ),
        Shape::Multipatch(_) => None,
    })
}

fn geometry_to_geo_types(geometry: &Value) -> DriverResult<geo_types::Geometry<f64>> {
    fn coordinate(value: &Value) -> DriverResult<geo_types::Coord<f64>> {
        let values = value.as_array().ok_or(DriverError::Read)?;
        let x = values
            .first()
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .ok_or(DriverError::Read)?;
        let y = values
            .get(1)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .ok_or(DriverError::Read)?;
        Ok(geo_types::Coord { x, y })
    }

    fn line(value: &Value) -> DriverResult<geo_types::LineString<f64>> {
        value
            .as_array()
            .ok_or(DriverError::Read)?
            .iter()
            .map(coordinate)
            .collect::<DriverResult<Vec<_>>>()
            .map(geo_types::LineString)
    }

    fn polygon(value: &Value) -> DriverResult<geo_types::Polygon<f64>> {
        let rings = value.as_array().ok_or(DriverError::Read)?;
        let (exterior, interiors) = rings.split_first().ok_or(DriverError::Read)?;
        Ok(geo_types::Polygon::new(
            line(exterior)?,
            interiors
                .iter()
                .map(line)
                .collect::<DriverResult<Vec<_>>>()?,
        ))
    }

    let coordinates = &geometry["coordinates"];
    match geometry["type"].as_str() {
        Some("Point") => Ok(geo_types::Geometry::Point(geo_types::Point::from(
            coordinate(coordinates)?,
        ))),
        Some("MultiPoint") => Ok(geo_types::Geometry::MultiPoint(geo_types::MultiPoint(
            coordinates
                .as_array()
                .ok_or(DriverError::Read)?
                .iter()
                .map(coordinate)
                .map(|coordinate| coordinate.map(geo_types::Point::from))
                .collect::<DriverResult<Vec<_>>>()?,
        ))),
        Some("LineString") => Ok(geo_types::Geometry::LineString(line(coordinates)?)),
        Some("MultiLineString") => Ok(geo_types::Geometry::MultiLineString(
            geo_types::MultiLineString(
                coordinates
                    .as_array()
                    .ok_or(DriverError::Read)?
                    .iter()
                    .map(line)
                    .collect::<DriverResult<Vec<_>>>()?,
            ),
        )),
        Some("Polygon") => Ok(geo_types::Geometry::Polygon(polygon(coordinates)?)),
        Some("MultiPolygon") => Ok(geo_types::Geometry::MultiPolygon(geo_types::MultiPolygon(
            coordinates
                .as_array()
                .ok_or(DriverError::Read)?
                .iter()
                .map(polygon)
                .collect::<DriverResult<Vec<_>>>()?,
        ))),
        _ => Err(DriverError::Read),
    }
}

fn line_geometry<T: dyn_xy::PointLike>(
    parts: &[Vec<T>],
    point: &impl Fn(&dyn dyn_xy::PointLike) -> Value,
) -> Option<Value> {
    let coordinates = parts
        .iter()
        .map(|part| part.iter().map(|value| point(value)).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    Some(if coordinates.len() == 1 {
        json!({ "type": "LineString", "coordinates": coordinates[0] })
    } else {
        json!({ "type": "MultiLineString", "coordinates": coordinates })
    })
}

fn polygon_geometry<T: dyn_xy::PointLike>(
    rings: &[shapefile::PolygonRing<T>],
    point: &impl Fn(&dyn dyn_xy::PointLike) -> Value,
) -> DriverResult<Option<Value>> {
    struct Polygon {
        coordinates: Vec<Vec<Value>>,
        ring: Vec<(f64, f64)>,
    }

    let mut polygons = Vec::new();
    let mut holes = Vec::new();
    for ring in rings {
        let coordinates = ring
            .points()
            .iter()
            .map(|value| point(value))
            .collect::<Vec<_>>();
        let raw_ring = ring
            .points()
            .iter()
            .map(|value| (value.x(), value.y()))
            .collect::<Vec<_>>();
        match ring {
            shapefile::PolygonRing::Outer(_) => polygons.push(Polygon {
                coordinates: vec![coordinates],
                ring: raw_ring,
            }),
            shapefile::PolygonRing::Inner(_) => holes.push((coordinates, raw_ring)),
        }
    }

    for (coordinates, hole) in holes {
        let mut candidates = polygons
            .iter()
            .enumerate()
            .filter_map(|(index, polygon)| {
                ring_contains_ring(&polygon.ring, &hole).then_some(index)
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return Err(DriverError::OrphanPolygonRing);
        }
        candidates.sort_by(|left, right| {
            ring_area(&polygons[*left].ring).total_cmp(&ring_area(&polygons[*right].ring))
        });
        let selected = candidates[0];
        if candidates.iter().skip(1).any(|index| {
            ring_area(&polygons[*index].ring) == ring_area(&polygons[selected].ring)
                || !ring_contains_ring(&polygons[*index].ring, &polygons[selected].ring)
        }) {
            return Err(DriverError::AmbiguousPolygonRing);
        }
        polygons[selected].coordinates.push(coordinates);
    }

    let mut polygons = polygons
        .into_iter()
        .map(|polygon| polygon.coordinates)
        .collect::<Vec<_>>();
    match polygons.len() {
        0 => Ok(None),
        1 => Ok(Some(
            json!({ "type": "Polygon", "coordinates": polygons.pop().unwrap() }),
        )),
        _ => Ok(Some(
            json!({ "type": "MultiPolygon", "coordinates": polygons }),
        )),
    }
}

fn ring_area(ring: &[(f64, f64)]) -> f64 {
    ring.windows(2)
        .map(|window| window[0].0 * window[1].1 - window[1].0 * window[0].1)
        .sum::<f64>()
        .abs()
}

fn ring_contains_ring(outer: &[(f64, f64)], inner: &[(f64, f64)]) -> bool {
    inner.iter().all(|&point| point_in_raw_ring(outer, point))
        && inner.windows(2).all(|inner_edge| {
            outer.windows(2).all(|outer_edge| {
                !segments_intersect(inner_edge[0], inner_edge[1], outer_edge[0], outer_edge[1])
            })
        })
}

fn point_in_raw_ring(ring: &[(f64, f64)], point: (f64, f64)) -> bool {
    let mut inside = false;
    for edge in ring.windows(2) {
        let ((x1, y1), (x2, y2)) = (edge[0], edge[1]);
        if (y1 > point.1) != (y2 > point.1) && point.0 < (x2 - x1) * (point.1 - y1) / (y2 - y1) + x1
        {
            inside = !inside;
        }
    }
    inside
}

fn geometry_bbox(geometry: &Value) -> [f64; 4] {
    fn visit(value: &Value, bbox: &mut Option<[f64; 4]>) {
        match value {
            Value::Array(values)
                if values.len() >= 2 && values[0].is_number() && values[1].is_number() =>
            {
                let x = values[0].as_f64().unwrap();
                let y = values[1].as_f64().unwrap();
                *bbox = Some(match *bbox {
                    Some([minx, miny, maxx, maxy]) => {
                        [minx.min(x), miny.min(y), maxx.max(x), maxy.max(y)]
                    }
                    None => [x, y, x, y],
                });
            }
            Value::Array(values) => {
                for value in values {
                    visit(value, bbox);
                }
            }
            _ => {}
        }
    }
    let mut bbox = None;
    visit(&geometry["coordinates"], &mut bbox);
    bbox.unwrap_or([
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ])
}

fn geometry_intersects_bbox(geometry: &Value, bbox: [f64; 4]) -> bool {
    let coordinate = |value: &Value| {
        value
            .as_array()
            .and_then(|v| Some((v.first()?.as_f64()?, v.get(1)?.as_f64()?)))
    };
    let line_hits = |line: &[Value]| {
        line.windows(2).any(|pair| {
            coordinate(&pair[0])
                .zip(coordinate(&pair[1]))
                .is_some_and(|(a, b)| segment_hits_bbox(a, b, bbox))
        }) || line
            .iter()
            .any(|p| coordinate(p).is_some_and(|p| point_in_bbox(p, bbox)))
    };
    match geometry["type"].as_str() {
        Some("Point") => {
            coordinate(&geometry["coordinates"]).is_some_and(|p| point_in_bbox(p, bbox))
        }
        Some("MultiPoint") => geometry["coordinates"].as_array().is_some_and(|p| {
            p.iter()
                .any(|v| coordinate(v).is_some_and(|p| point_in_bbox(p, bbox)))
        }),
        Some("LineString") => geometry["coordinates"]
            .as_array()
            .is_some_and(|v| line_hits(v)),
        Some("MultiLineString") => geometry["coordinates"].as_array().is_some_and(|v| {
            v.iter()
                .filter_map(Value::as_array)
                .any(|line| line_hits(line))
        }),
        Some("Polygon") => polygon_hits_bbox(
            geometry["coordinates"].as_array(),
            bbox,
            &coordinate,
            &line_hits,
        ),
        Some("MultiPolygon") => geometry["coordinates"].as_array().is_some_and(|v| {
            v.iter()
                .any(|p| polygon_hits_bbox(p.as_array(), bbox, &coordinate, &line_hits))
        }),
        _ => false,
    }
}

fn polygon_hits_bbox(
    rings: Option<&Vec<Value>>,
    bbox: [f64; 4],
    coordinate: &impl Fn(&Value) -> Option<(f64, f64)>,
    line_hits: &impl Fn(&[Value]) -> bool,
) -> bool {
    let Some(rings) = rings else { return false };
    if rings
        .iter()
        .filter_map(Value::as_array)
        .any(|ring| line_hits(ring))
    {
        return true;
    }
    let corners = [
        (bbox[0], bbox[1]),
        (bbox[0], bbox[3]),
        (bbox[2], bbox[1]),
        (bbox[2], bbox[3]),
    ];
    corners
        .into_iter()
        .any(|p| point_in_polygon(p, rings, coordinate))
}

fn point_in_polygon(
    p: (f64, f64),
    rings: &[Value],
    coordinate: &impl Fn(&Value) -> Option<(f64, f64)>,
) -> bool {
    let Some(outer) = rings.first().and_then(Value::as_array) else {
        return false;
    };
    if !point_in_ring(p, outer, coordinate) {
        return false;
    }
    !rings
        .iter()
        .skip(1)
        .filter_map(Value::as_array)
        .any(|ring| point_in_ring(p, ring, coordinate))
}

fn point_in_ring(
    p: (f64, f64),
    ring: &[Value],
    coordinate: &impl Fn(&Value) -> Option<(f64, f64)>,
) -> bool {
    ring.windows(2)
        .filter_map(|pair| coordinate(&pair[0]).zip(coordinate(&pair[1])))
        .fold(false, |inside, ((x1, y1), (x2, y2))| {
            inside ^ ((y1 > p.1) != (y2 > p.1) && p.0 < (x2 - x1) * (p.1 - y1) / (y2 - y1) + x1)
        })
}

fn point_in_bbox((x, y): (f64, f64), [minx, miny, maxx, maxy]: [f64; 4]) -> bool {
    x >= minx && x <= maxx && y >= miny && y <= maxy
}

fn segment_hits_bbox(a: (f64, f64), b: (f64, f64), bbox: [f64; 4]) -> bool {
    if point_in_bbox(a, bbox) || point_in_bbox(b, bbox) {
        return true;
    }
    let corners = [
        (bbox[0], bbox[1]),
        (bbox[2], bbox[1]),
        (bbox[2], bbox[3]),
        (bbox[0], bbox[3]),
    ];
    corners
        .windows(2)
        .any(|e| segments_intersect(a, b, e[0], e[1]))
        || segments_intersect(a, b, corners[3], corners[0])
}
fn segments_intersect(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> bool {
    fn cross(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
        (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
    }
    let ab_c = cross(a, b, c);
    let ab_d = cross(a, b, d);
    let cd_a = cross(c, d, a);
    let cd_b = cross(c, d, b);
    (ab_c == 0.0 && point_on_segment(c, a, b))
        || (ab_d == 0.0 && point_on_segment(d, a, b))
        || (cd_a == 0.0 && point_on_segment(a, c, d))
        || (cd_b == 0.0 && point_on_segment(b, c, d))
        || (ab_c.signum() != ab_d.signum() && cd_a.signum() != cd_b.signum())
}
fn point_on_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> bool {
    p.0 >= a.0.min(b.0) && p.0 <= a.0.max(b.0) && p.1 >= a.1.min(b.1) && p.1 <= a.1.max(b.1)
}

fn parse_token(token: Option<&str>) -> DriverResult<Option<u64>> {
    token
        .map(|token| token.parse().map_err(|_| DriverError::InvalidToken))
        .transpose()
}

fn geometry_type(shape_type: shapefile::ShapeType) -> Option<String> {
    use shapefile::ShapeType::*;
    Some(
        match shape_type {
            Point | PointM | PointZ => "POINT",
            Polyline | PolylineM | PolylineZ => "LINESTRING",
            Polygon | PolygonM | PolygonZ => "POLYGON",
            Multipoint | MultipointM | MultipointZ => "MULTIPOINT",
            Multipatch | NullShape => return None,
        }
        .into(),
    )
}

fn dbase_type(field: shapefile::dbase::FieldType) -> String {
    use shapefile::dbase::FieldType::*;
    match field {
        Character | Memo => "text",
        Numeric | Float => "text",
        Double | Currency => "double precision",
        Logical => "boolean",
        Date => "date",
        DateTime => "timestamp without time zone",
        Integer => "integer",
    }
    .into()
}

fn dbase_value(value: shapefile::dbase::FieldValue) -> Value {
    use shapefile::dbase::FieldValue::*;
    match value {
        Character(value) => value.map(Value::String).unwrap_or(Value::Null),
        Numeric(value) => value
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Logical(value) => value.map(Value::Bool).unwrap_or(Value::Null),
        Date(value) => value
            .map(|value| {
                Value::String(format!(
                    "{:04}-{:02}-{:02}",
                    value.year(),
                    value.month(),
                    value.day()
                ))
            })
            .unwrap_or(Value::Null),
        Float(value) => value
            .and_then(|value| serde_json::Number::from_f64(f64::from(value)))
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Integer(value) => json!(value),
        Currency(value) | Double(value) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        DateTime(value) => {
            let date = value.date();
            let time = value.time();
            Value::String(format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                date.year(),
                date.month(),
                date.day(),
                time.hours(),
                time.minutes(),
                time.seconds()
            ))
        }
        Memo(value) => Value::String(value),
    }
}

async fn run_blocking<T, F>(work: F) -> DriverResult<T>
where
    F: FnOnce() -> DriverResult<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| DriverError::Worker)?
}

mod dyn_xy {
    pub trait PointLike {
        fn x(&self) -> f64;
        fn y(&self) -> f64;
        fn coordinates(&self) -> Vec<f64> {
            vec![self.x(), self.y()]
        }
    }
    impl PointLike for shapefile::Point {
        fn x(&self) -> f64 {
            self.x
        }
        fn y(&self) -> f64 {
            self.y
        }
    }
    impl PointLike for shapefile::PointM {
        fn x(&self) -> f64 {
            self.x
        }
        fn y(&self) -> f64 {
            self.y
        }
    }
    impl PointLike for shapefile::PointZ {
        fn x(&self) -> f64 {
            self.x
        }
        fn y(&self) -> f64 {
            self.y
        }
        fn coordinates(&self) -> Vec<f64> {
            vec![self.x, self.y, self.z]
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum DriverError {
    #[error(transparent)]
    Archive(#[from] crate::ArchiveError),
    #[error("Shapefile data could not be read")]
    Read,
    #[error("Shapefile record count is invalid")]
    InvalidRecordCount,
    #[error("Shapefile components do not have matching physical record counts")]
    RecordCountMismatch,
    #[error("Shapefile DBF contains a deleted row and cannot preserve physical feature alignment")]
    DeletedRow,
    #[error("Shapefile DBF Memo fields are not supported without a validated memo companion")]
    MemoUnsupported,
    #[error("Shapefile DBF binary floating values must be finite")]
    BinaryNonFinite,
    #[error("Shapefile MultiPatch geometry is not supported")]
    MultipatchUnsupported,
    #[error("Shapefile polygon has an inner ring without a containing exterior ring")]
    OrphanPolygonRing,
    #[error("Shapefile polygon has an inner ring with ambiguous containing exterior rings")]
    AmbiguousPolygonRing,
    #[error("Shapefile CRS is missing, ambiguous, or unsupported for feature serving")]
    UnsupportedCrs,
    #[error("keyset token is not a non-negative physical record index")]
    InvalidToken,
    #[error("Shapefile driver does not support datetime filtering")]
    DatetimeUnsupported,
    #[error("Shapefile driver does not support the 'filter' parameter")]
    FilterUnsupported,
    #[error("Shapefile tile property has an unsupported value")]
    TileProperty,
    #[error(
        "Shapefile bbox scan limit exceeded (at most {max_records} records and {max_bytes} bytes)"
    )]
    ScanLimitExceeded { max_records: u64, max_bytes: u64 },
    #[error("Shapefile background read task failed")]
    Worker,
}

impl DriverError {
    fn read<T>(_error: T) -> Self {
        Self::Read
    }
}
type DriverResult<T> = std::result::Result<T, DriverError>;

impl From<DriverError> for CoreError {
    fn from(error: DriverError) -> Self {
        match error {
            DriverError::InvalidToken
            | DriverError::DatetimeUnsupported
            | DriverError::FilterUnsupported
            | DriverError::ScanLimitExceeded { .. } => Self::Invalid(error.to_string()),
            DriverError::Archive(_)
            | DriverError::Read
            | DriverError::InvalidRecordCount
            | DriverError::RecordCountMismatch
            | DriverError::DeletedRow
            | DriverError::MemoUnsupported
            | DriverError::BinaryNonFinite
            | DriverError::MultipatchUnsupported
            | DriverError::OrphanPolygonRing
            | DriverError::AmbiguousPolygonRing
            | DriverError::UnsupportedCrs
            | DriverError::TileProperty
            | DriverError::Worker => Self::Storage(Box::new(error)),
        }
    }
}
