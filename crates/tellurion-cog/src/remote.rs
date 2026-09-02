//! Bounded ranged reads over remote GeoTIFF objects.
//!
//! The COG decoder owns only a `RangeObject` and never a locator or HTTP
//! client. Public sources are therefore mediated by `tellurion-http-source`.
//! The small administrative compatibility path is also implemented there and
//! is reachable only from trusted storage configuration.

use std::fmt;
use std::io;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tellurion_http_source::{
    AdministrativeRangeObject, AdministrativeSourceError, RangeObject, SourceErrorKind,
};
use tokio::runtime::Handle;

use crate::error::CogError;

/// Bytes fetched for each ordinary cache miss. Adjacent TIFF header and IFD
/// reads share one bounded window.
const WINDOW_BYTES: u64 = 64 * 1024;
const MAX_RANGE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_OPERATION_REQUESTS: u32 = 32;
const MAX_OPERATION_BYTES: u64 = 8 * 1024 * 1024;
const MAX_OPERATION_DURATION: Duration = Duration::from_secs(10);

/// One remote COG source. The brokered variant is the public contract; the
/// compatibility variant has no path from `PublicHttpsGateway`.
#[derive(Clone)]
pub enum RemoteCogSource {
    RangeObject(Arc<dyn RangeObject>),
    Administrative(AdministrativeRangeObject),
}

impl RemoteCogSource {
    pub fn from_range_object(object: Arc<dyn RangeObject>) -> Self {
        Self::RangeObject(object)
    }

    pub fn administrative_from_env(variable: &str) -> Result<Self, AdministrativeSourceError> {
        AdministrativeRangeObject::from_env(variable).map(Self::Administrative)
    }

    pub fn display_name(&self) -> &str {
        match self {
            Self::RangeObject(object) => object.display_name(),
            Self::Administrative(object) => object.display_name(),
        }
    }
}

impl fmt::Debug for RemoteCogSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteCogSource")
            .field("display_name", &self.display_name())
            .finish()
    }
}

/// Shared request envelope for all remote readers opened by one metadata or
/// tile operation.
#[derive(Clone, Debug)]
pub(crate) struct OperationContext(Arc<Mutex<OperationBudget>>);

impl OperationContext {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(OperationBudget::new())))
    }

    fn reserve(&self, bytes: u64) -> Result<Duration, CogError> {
        self.0
            .lock()
            .expect("operation budget mutex is not poisoned")
            .reserve(bytes)
    }

    #[cfg(test)]
    fn with_duration(duration: Duration) -> Self {
        Self(Arc::new(Mutex::new(OperationBudget {
            requests: 0,
            bytes: 0,
            deadline: Instant::now() + duration,
        })))
    }
}

/// A synchronous `Read + Seek` view over one remote COG. At most one window
/// is retained, and its reads charge the operation context shared by every
/// reader opened for the same metadata or tile operation.
#[derive(Debug)]
pub struct HttpRangeReader {
    source: RemoteCogSource,
    handle: Handle,
    len: u64,
    pos: u64,
    window: Option<(u64, Vec<u8>)>,
    operation: OperationContext,
}

impl HttpRangeReader {
    pub(crate) fn open_with_operation(
        source: RemoteCogSource,
        operation: OperationContext,
    ) -> Result<Self, CogError> {
        let handle = Handle::current();
        let (len, window) = match &source {
            RemoteCogSource::RangeObject(object) => {
                let len = object.length();
                (len, None)
            }
            RemoteCogSource::Administrative(object) => {
                let timeout = operation.reserve(1)?;
                let (len, bytes) = handle.block_on(async {
                    tokio::time::timeout(timeout, object.get_range(0..1))
                        .await
                        .map_err(|_| CogError::RemoteOperationBudget)?
                        .map_err(administrative_error)
                })?;
                (len, Some((0, bytes.to_vec())))
            }
        };
        if len == 0 {
            return Err(CogError::RemoteRead {
                kind: SourceErrorKind::Range,
            });
        }
        Ok(Self {
            source,
            handle,
            len,
            pos: 0,
            window,
            operation,
        })
    }

    fn ensure_window(&mut self, want: usize) -> io::Result<()> {
        if self.pos >= self.len {
            return Ok(());
        }
        if let Some((start, data)) = &self.window {
            let end = start + data.len() as u64;
            if self.pos >= *start && self.pos + want as u64 <= end {
                return Ok(());
            }
        }
        let remaining = self.len - self.pos;
        let fetch_len = (want as u64)
            .clamp(WINDOW_BYTES, MAX_RANGE_BYTES)
            .min(remaining);
        let start = self.pos;
        let end = start + fetch_len;
        let timeout = self
            .operation
            .reserve(fetch_len)
            .map_err(io::Error::other)?;
        let source = self.source.clone();
        let expected_len = self.len;
        let bytes = self
            .handle
            .block_on(async move {
                tokio::time::timeout(timeout, async move {
                    match source {
                        RemoteCogSource::RangeObject(object) => object
                            .get_range(start..end)
                            .await
                            .map_err(range_object_error),
                        RemoteCogSource::Administrative(object) => {
                            let (total, bytes) = object
                                .get_range(start..end)
                                .await
                                .map_err(administrative_error)?;
                            if total != expected_len {
                                return Err(CogError::RemoteRead {
                                    kind: SourceErrorKind::Identity,
                                });
                            }
                            Ok(bytes)
                        }
                    }
                })
                .await
                .map_err(|_| CogError::RemoteOperationBudget)?
            })
            .map_err(io::Error::other)?;
        if bytes.len() as u64 != fetch_len {
            return Err(io::Error::other(CogError::RemoteRead {
                kind: SourceErrorKind::Protocol,
            }));
        }
        self.window = Some((start, bytes.to_vec()));
        Ok(())
    }
}

impl Read for HttpRangeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.pos >= self.len {
            return Ok(0);
        }
        self.ensure_window(buf.len())?;
        let (start, data) = self
            .window
            .as_ref()
            .expect("a non-empty source always has a filled window after ensure_window");
        let offset = (self.pos - start) as usize;
        let available = data.len().saturating_sub(offset);
        let count = available.min(buf.len());
        buf[..count].copy_from_slice(&data[offset..offset + count]);
        self.pos += count as u64;
        Ok(count)
    }
}

impl Seek for HttpRangeReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::End(offset) => self.len as i64 + offset,
            SeekFrom::Current(offset) => self.pos as i64 + offset,
        };
        if new_pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek to a negative position",
            ));
        }
        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}

#[derive(Debug)]
struct OperationBudget {
    requests: u32,
    bytes: u64,
    deadline: Instant,
}

impl OperationBudget {
    fn new() -> Self {
        Self {
            requests: 0,
            bytes: 0,
            deadline: Instant::now() + MAX_OPERATION_DURATION,
        }
    }

    fn reserve(&mut self, bytes: u64) -> Result<Duration, CogError> {
        if Instant::now() >= self.deadline
            || self.requests >= MAX_OPERATION_REQUESTS
            || self.bytes.saturating_add(bytes) > MAX_OPERATION_BYTES
        {
            return Err(CogError::RemoteOperationBudget);
        }
        self.requests += 1;
        self.bytes += bytes;
        Ok(self.deadline.saturating_duration_since(Instant::now()))
    }
}

fn range_object_error(error: tellurion_http_source::SourceError) -> CogError {
    CogError::RemoteRead { kind: error.kind() }
}

fn administrative_error(error: AdministrativeSourceError) -> CogError {
    CogError::RemoteRead {
        kind: match error {
            AdministrativeSourceError::Invalid => SourceErrorKind::Url,
            AdministrativeSourceError::Transport => SourceErrorKind::Transport,
            AdministrativeSourceError::Range => SourceErrorKind::Protocol,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use bytes::Bytes;
    use tellurion_http_source::{ContentIdentity, RangeObject, SourceError, SourceHandle};

    use super::*;

    async fn in_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        tokio::task::spawn_blocking(f).await.unwrap()
    }

    struct RangeFixture {
        handle: SourceHandle,
        identity: ContentIdentity,
        bytes: Bytes,
        requests: AtomicUsize,
        invalidated: bool,
        delay: Option<Duration>,
    }

    impl RangeFixture {
        fn bytes(bytes: Vec<u8>) -> Self {
            let length = bytes.len() as u64;
            Self {
                handle: SourceHandle::new("opaque-fixture"),
                identity: ContentIdentity::StrongEtag {
                    source_key: [3; 32],
                    revision_key: [4; 32],
                    length,
                },
                bytes: Bytes::from(bytes),
                requests: AtomicUsize::new(0),
                invalidated: false,
                delay: None,
            }
        }

        fn invalidated() -> Self {
            let mut source = Self::bytes(vec![0; 1]);
            source.invalidated = true;
            source
        }

        fn delayed(bytes: Vec<u8>, delay: Duration) -> Self {
            let mut source = Self::bytes(bytes);
            source.delay = Some(delay);
            source
        }
    }

    #[async_trait]
    impl RangeObject for RangeFixture {
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
            "opaque-fixture"
        }

        async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            if self.invalidated {
                return Err(SourceError::for_handle(
                    SourceErrorKind::Invalidated,
                    &self.handle,
                ));
            }
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            Ok(self.bytes.slice(range.start as usize..range.end as usize))
        }
    }

    #[tokio::test]
    async fn invalidated_range_object_is_an_opaque_remote_error() {
        let source = RemoteCogSource::from_range_object(Arc::new(RangeFixture::invalidated()));
        let error = in_blocking(move || {
            let mut reader =
                HttpRangeReader::open_with_operation(source, OperationContext::new()).unwrap();
            let mut byte = [0; 1];
            reader.read_exact(&mut byte).unwrap_err()
        })
        .await;
        assert!(error.to_string().contains("invalidated"));
        assert!(!error.to_string().contains("https://"));
    }

    #[tokio::test]
    async fn range_object_reader_refuses_the_thirty_third_window_request() {
        let source = Arc::new(RangeFixture::bytes(vec![0; (WINDOW_BYTES * 34) as usize]));
        let reader_source = RemoteCogSource::from_range_object(source.clone());
        let error = in_blocking(move || {
            let mut reader =
                HttpRangeReader::open_with_operation(reader_source, OperationContext::new())
                    .unwrap();
            let mut byte = [0; 1];
            for window in 0..32 {
                reader.seek(SeekFrom::Start(window * WINDOW_BYTES)).unwrap();
                reader.read_exact(&mut byte).unwrap();
            }
            reader.seek(SeekFrom::Start(32 * WINDOW_BYTES)).unwrap();
            reader.read_exact(&mut byte).unwrap_err()
        })
        .await;
        assert!(error.to_string().contains("operation budget"));
        assert_eq!(source.requests.load(Ordering::SeqCst), 32);
    }

    #[tokio::test]
    async fn readers_in_one_operation_share_the_range_budget() {
        let object = Arc::new(RangeFixture::bytes(vec![0; (WINDOW_BYTES * 34) as usize]));
        let source = RemoteCogSource::from_range_object(object.clone());
        let error = in_blocking(move || {
            let operation = OperationContext::new();
            let mut first =
                HttpRangeReader::open_with_operation(source.clone(), operation.clone()).unwrap();
            let mut byte = [0; 1];
            for window in 0..32 {
                first.seek(SeekFrom::Start(window * WINDOW_BYTES)).unwrap();
                first.read_exact(&mut byte).unwrap();
            }

            let mut second = HttpRangeReader::open_with_operation(source, operation).unwrap();
            second.read_exact(&mut byte).unwrap_err()
        })
        .await;
        assert!(error.to_string().contains("operation budget"));
        assert_eq!(object.requests.load(Ordering::SeqCst), 32);
    }

    #[tokio::test]
    async fn reader_coalesces_scattered_reads_inside_one_window() {
        let object = Arc::new(RangeFixture::bytes(vec![9; (WINDOW_BYTES * 2) as usize]));
        let source = RemoteCogSource::from_range_object(object.clone());
        in_blocking(move || {
            let mut reader =
                HttpRangeReader::open_with_operation(source, OperationContext::new()).unwrap();
            let mut byte = [0; 1];
            for offset in [0, 500, 1_000] {
                reader.seek(SeekFrom::Start(offset)).unwrap();
                reader.read_exact(&mut byte).unwrap();
                assert_eq!(byte, [9]);
            }
        })
        .await;
        assert_eq!(object.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reader_opens_one_new_window_for_a_distant_read() {
        let object = Arc::new(RangeFixture::bytes(vec![7; (WINDOW_BYTES * 3) as usize]));
        let source = RemoteCogSource::from_range_object(object.clone());
        in_blocking(move || {
            let mut reader =
                HttpRangeReader::open_with_operation(source, OperationContext::new()).unwrap();
            let mut byte = [0; 1];
            reader.read_exact(&mut byte).unwrap();
            reader.seek(SeekFrom::Start(WINDOW_BYTES + 10)).unwrap();
            reader.read_exact(&mut byte).unwrap();
        })
        .await;
        assert_eq!(object.requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn reader_returns_the_exact_bytes_for_a_large_read() {
        let object = Arc::new(RangeFixture::bytes(vec![3; (WINDOW_BYTES * 4) as usize]));
        let source = RemoteCogSource::from_range_object(object.clone());
        let bytes = in_blocking(move || {
            let mut reader =
                HttpRangeReader::open_with_operation(source, OperationContext::new()).unwrap();
            let mut bytes = vec![0; (WINDOW_BYTES * 2) as usize];
            reader.read_exact(&mut bytes).unwrap();
            bytes
        })
        .await;
        assert!(bytes.iter().all(|byte| *byte == 3));
        assert_eq!(object.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn range_request_cannot_run_past_the_operation_deadline() {
        let object = Arc::new(RangeFixture::delayed(
            vec![1; WINDOW_BYTES as usize],
            Duration::from_millis(20),
        ));
        let source = RemoteCogSource::from_range_object(object);
        let error = in_blocking(move || {
            let operation = OperationContext::with_duration(Duration::from_millis(1));
            let mut reader = HttpRangeReader::open_with_operation(source, operation).unwrap();
            let mut byte = [0; 1];
            reader.read_exact(&mut byte).unwrap_err()
        })
        .await;
        assert!(error.to_string().contains("operation budget"));
    }
}
