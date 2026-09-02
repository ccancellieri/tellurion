use std::path::PathBuf;

use parquet::arrow::async_reader::AsyncFileReader;
use parquet::errors::Result;

#[cfg(feature = "remote")]
use std::collections::HashMap;
#[cfg(feature = "remote")]
use std::ops::Range;
#[cfg(feature = "remote")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "remote")]
use bytes::Bytes;
#[cfg(feature = "remote")]
use futures::future::BoxFuture;
#[cfg(feature = "remote")]
use futures::FutureExt;
#[cfg(feature = "remote")]
use parquet::arrow::arrow_reader::ArrowReaderOptions;
#[cfg(feature = "remote")]
use parquet::arrow::async_reader::MetadataFetch;
#[cfg(feature = "remote")]
use parquet::errors::ParquetError;
#[cfg(feature = "remote")]
use parquet::file::metadata::{
    PageIndexPolicy, ParquetMetaData, ParquetMetaDataOptions, ParquetMetaDataReader,
};
#[cfg(feature = "remote")]
use tellurion_http_source::{ContentIdentity, RangeObject, SourceError, SourceErrorKind};
#[cfg(feature = "remote")]
use tokio::sync::OnceCell;

#[cfg(feature = "remote")]
type MetadataCell = Arc<OnceCell<Arc<ParquetMetaData>>>;
#[cfg(feature = "remote")]
type MetadataCache = Mutex<HashMap<MetadataCacheKey, MetadataCell>>;

/// A GeoParquet input that can be read without buffering a remote object.
#[derive(Clone)]
pub enum GeoparquetInput {
    Local(PathBuf),
    #[cfg(feature = "remote")]
    Remote(Arc<dyn RangeObject>),
}

impl GeoparquetInput {
    /// A stable, display-only input name used to derive the collection name.
    pub fn display_name(&self) -> String {
        match self {
            Self::Local(path) => path.display().to_string(),
            #[cfg(feature = "remote")]
            Self::Remote(object) => object.display_name().to_string(),
        }
    }

    /// Opens this input for Apache Parquet's asynchronous reader.
    pub async fn into_async_reader(self) -> Result<Box<dyn AsyncFileReader>> {
        match self {
            Self::Local(path) => Ok(Box::new(tokio::fs::File::open(path).await?)),
            #[cfg(feature = "remote")]
            Self::Remote(object) => Ok(Box::new(RemoteParquetReader::new(object))),
        }
    }
}

/// Adapts a bounded remote object to Parquet's asynchronous random-access API.
#[cfg(feature = "remote")]
pub struct RemoteParquetReader {
    object: Arc<dyn RangeObject>,
    revision_key: [u8; 32],
    metadata: Arc<MetadataCache>,
}

#[cfg(feature = "remote")]
impl RemoteParquetReader {
    pub fn new(object: Arc<dyn RangeObject>) -> Self {
        Self {
            revision_key: revision_key(&object),
            object,
            metadata: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[cfg(feature = "remote")]
impl AsyncFileReader for RemoteParquetReader {
    fn get_bytes(&mut self, range: Range<u64>) -> BoxFuture<'_, Result<Bytes>> {
        let object = Arc::clone(&self.object);
        async move { object.get_range(range).await.map_err(map_source_error) }.boxed()
    }

    fn get_metadata<'a>(
        &'a mut self,
        options: Option<&'a ArrowReaderOptions>,
    ) -> BoxFuture<'a, Result<Arc<ParquetMetaData>>> {
        let object = Arc::clone(&self.object);
        let cached_revision = self.revision_key;
        let metadata = self
            .metadata
            .lock()
            .unwrap()
            .entry(MetadataCacheKey::from(options))
            .or_default()
            .clone();
        let options = options.cloned();

        async move {
            validate_revision(&object, cached_revision)?;

            let length = object.length();
            let object_for_init = Arc::clone(&object);
            let metadata = metadata
                .get_or_try_init(|| async move {
                    // Metadata parsing is canonical so callers with arbitrary per-column
                    // statistics policies safely share the same cache entry. Page-index
                    // policies remain part of the cache key below.
                    let mut reader = ParquetMetaDataReader::new()
                        .with_metadata_options(Some(ParquetMetaDataOptions::default()));
                    if let Some(options) = options {
                        reader = reader
                            .with_column_index_policy(options.column_index_policy())
                            .with_offset_index_policy(options.offset_index_policy());
                    }

                    let fetcher = RangeObjectMetadataFetcher {
                        object: Arc::clone(&object_for_init),
                        expected_revision: cached_revision,
                    };
                    let metadata = reader.load_and_finish(fetcher, length).await?;
                    validate_revision(&object_for_init, cached_revision)?;
                    Ok::<Arc<ParquetMetaData>, ParquetError>(Arc::new(metadata))
                })
                .await?;
            validate_revision(&object, cached_revision)?;
            Ok(Arc::clone(metadata))
        }
        .boxed()
    }
}

#[cfg(feature = "remote")]
struct RangeObjectMetadataFetcher {
    object: Arc<dyn RangeObject>,
    expected_revision: [u8; 32],
}

#[cfg(feature = "remote")]
impl MetadataFetch for RangeObjectMetadataFetcher {
    fn fetch(&mut self, range: Range<u64>) -> BoxFuture<'_, Result<Bytes>> {
        let object = Arc::clone(&self.object);
        let expected_revision = self.expected_revision;
        async move {
            validate_revision(&object, expected_revision)?;
            let bytes = object.get_range(range).await.map_err(map_source_error)?;
            validate_revision(&object, expected_revision)?;
            Ok(bytes)
        }
        .boxed()
    }
}

#[cfg(feature = "remote")]
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct MetadataCacheKey {
    column_index: PageIndexPolicyKey,
    offset_index: PageIndexPolicyKey,
}

#[cfg(feature = "remote")]
impl From<Option<&ArrowReaderOptions>> for MetadataCacheKey {
    fn from(options: Option<&ArrowReaderOptions>) -> Self {
        Self {
            column_index: options
                .map(ArrowReaderOptions::column_index_policy)
                .unwrap_or(PageIndexPolicy::Skip)
                .into(),
            offset_index: options
                .map(ArrowReaderOptions::offset_index_policy)
                .unwrap_or(PageIndexPolicy::Skip)
                .into(),
        }
    }
}

#[cfg(feature = "remote")]
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum PageIndexPolicyKey {
    Skip,
    Optional,
    Required,
}

#[cfg(feature = "remote")]
impl From<PageIndexPolicy> for PageIndexPolicyKey {
    fn from(policy: PageIndexPolicy) -> Self {
        match policy {
            PageIndexPolicy::Skip => Self::Skip,
            PageIndexPolicy::Optional => Self::Optional,
            PageIndexPolicy::Required => Self::Required,
        }
    }
}

#[cfg(feature = "remote")]
fn revision_key(object: &Arc<dyn RangeObject>) -> [u8; 32] {
    match object.identity() {
        ContentIdentity::StrongEtag { revision_key, .. } => *revision_key,
    }
}

#[cfg(feature = "remote")]
fn validate_revision(object: &Arc<dyn RangeObject>, expected_revision: [u8; 32]) -> Result<()> {
    if revision_key(object) == expected_revision {
        Ok(())
    } else {
        Err(map_source_error(SourceError::for_handle(
            SourceErrorKind::Invalidated,
            object.handle(),
        )))
    }
}

#[cfg(feature = "remote")]
fn map_source_error(error: SourceError) -> ParquetError {
    ParquetError::External(Box::new(error))
}
