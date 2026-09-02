use std::{
    net::{IpAddr, SocketAddr},
    ops::Range,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use hmac::{Hmac, KeyInit, Mac};
use rand::Rng;
use reqwest::{header, redirect::Policy};
use sha2::{Digest, Sha256};
use tokio::{sync::Mutex, time::timeout};

use crate::{
    address::is_public_address,
    budget::{Budget, BudgetLimits},
    error::{SourceError, SourceErrorKind},
    url::{process_secret, PublicUrl},
};

const MAX_OBJECT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_RANGE_BYTES: u64 = 2 * 1024 * 1024;
const REGISTRATION_LIMITS: BudgetLimits = BudgetLimits {
    requests: 16,
    bytes: 2 * 1024 * 1024,
    deadline: Duration::from_secs(5),
    concurrent: 1,
};
const SESSION_LIMITS: BudgetLimits = BudgetLimits {
    requests: 256,
    bytes: 64 * 1024 * 1024,
    deadline: Duration::from_secs(15 * 60),
    concurrent: 2,
};

type HmacSha256 = Hmac<Sha256>;

/// An opaque source identifier. It contains no locator material.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct SourceHandle(String);

impl SourceHandle {
    /// Creates an identifier for a custom range object. Callers must supply
    /// an application-owned opaque value, never a locator or credential.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SourceHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::fmt::Debug for SourceHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("SourceHandle")
            .field(&self.0)
            .finish()
    }
}

/// The immutable identity established by the registration probe.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ContentIdentity {
    StrongEtag {
        source_key: [u8; 32],
        revision_key: [u8; 32],
        length: u64,
    },
}

#[async_trait]
pub trait RangeObject: Send + Sync {
    fn handle(&self) -> &SourceHandle;
    fn identity(&self) -> &ContentIdentity;
    fn length(&self) -> u64;
    fn display_name(&self) -> &str;
    async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError>;
}

/// DNS injection seam. Production resolves anew for every origin request.
#[async_trait]
pub(crate) trait Resolver: Send + Sync {
    async fn resolve(&self, hostname: &str) -> Result<Vec<IpAddr>, ResolverError>;
}

/// HTTP injection seam. Production pins each supplied address into one request.
#[async_trait]
pub(crate) trait HttpExecutor: Send + Sync {
    async fn execute(&self, request: OutboundRequest) -> Result<ExecutorResponse, ExecutorError>;
}

/// A redacted resolver failure.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolverError;

impl ResolverError {
    pub(crate) fn unavailable() -> Self {
        Self
    }
}

/// A redacted executor failure.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ExecutorError {
    kind: ExecutorErrorKind,
    consumed_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
enum ExecutorErrorKind {
    Transport,
    TooLarge,
    Timeout,
}

impl ExecutorError {
    pub(crate) fn unavailable(consumed_bytes: u64) -> Self {
        Self {
            kind: ExecutorErrorKind::Transport,
            consumed_bytes,
        }
    }

    pub(crate) fn response_too_large(consumed_bytes: u64) -> Self {
        Self {
            kind: ExecutorErrorKind::TooLarge,
            consumed_bytes,
        }
    }

    pub(crate) fn timed_out(consumed_bytes: u64) -> Self {
        Self {
            kind: ExecutorErrorKind::Timeout,
            consumed_bytes,
        }
    }
}

/// The safe details an injected executor may observe.
#[derive(Clone)]
pub(crate) struct OutboundRequest {
    pub(crate) hostname: String,
    pub(crate) addresses: Vec<IpAddr>,
    pub(crate) range: Range<u64>,
    pub(crate) if_match: Option<String>,
    pub(crate) timeout: Duration,
    locator: url::Url,
    consumed: Arc<AtomicU64>,
}

impl OutboundRequest {
    pub(crate) fn record_consumed(&self, bytes: u64) {
        self.consumed.fetch_add(bytes, Ordering::AcqRel);
    }
}

impl std::fmt::Debug for OutboundRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OutboundRequest")
            .field("hostname", &self.hostname)
            .field("addresses", &self.addresses)
            .field("range", &self.range)
            .field("if_match", &self.if_match)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// An executor response supplied to the strict range verifier.
#[derive(Clone)]
pub(crate) struct ExecutorResponse {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Bytes,
}

impl std::fmt::Debug for ExecutorResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecutorResponse")
            .field("status", &self.status)
            .field("header_count", &self.headers.len())
            .field("body_len", &self.body.len())
            .finish()
    }
}

impl ExecutorResponse {
    pub(crate) fn new(status: u16, headers: Vec<(String, String)>, body: Bytes) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    fn singleton_header(&self, name: &str) -> Result<Option<&str>, SourceErrorKind> {
        let mut values = self.headers.iter().filter_map(|(header_name, value)| {
            header_name
                .eq_ignore_ascii_case(name)
                .then_some(value.as_str())
        });
        let value = values.next();
        if values.next().is_some() {
            return Err(SourceErrorKind::Protocol);
        }
        Ok(value)
    }
}

/// Session-bound state and its hard lifetime. The identifier remains server-owned.
#[derive(Clone)]
pub struct SourceSession {
    pub(crate) inner: Arc<SessionState>,
}

impl SourceSession {
    /// Returns whether two opaque handles refer to the same source session.
    /// This reveals no session identifier and does not widen either handle's
    /// authority.
    pub fn same_session(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

pub(crate) struct SessionState {
    created_at: Instant,
    budget: Budget,
    pub(crate) sources: AtomicU32,
}

/// Factory for narrow, range-only public HTTPS sources.
#[derive(Clone)]
pub struct PublicHttpsGateway {
    resolver: Arc<dyn Resolver>,
    executor: Arc<dyn HttpExecutor>,
}

impl Default for PublicHttpsGateway {
    fn default() -> Self {
        Self::new()
    }
}

impl PublicHttpsGateway {
    pub fn new() -> Self {
        Self::with_transport(Arc::new(SystemResolver), Arc::new(ReqwestExecutor))
    }

    /// Constructs a gateway with deterministic transport seams for crate tests.
    pub(crate) fn with_transport(
        resolver: Arc<dyn Resolver>,
        executor: Arc<dyn HttpExecutor>,
    ) -> Self {
        Self { resolver, executor }
    }

    pub fn open_session(&self) -> SourceSession {
        SourceSession {
            inner: Arc::new(SessionState {
                created_at: Instant::now(),
                budget: Budget::new(SESSION_LIMITS),
                sources: AtomicU32::new(0),
            }),
        }
    }

    /// Probes and registers a single exact HTTPS range object for a session.
    pub async fn register_range_object(
        &self,
        session: &SourceSession,
        raw_url: &str,
    ) -> Result<Arc<dyn RangeObject>, SourceError> {
        let locator = match crate::validate_public_url(raw_url) {
            Ok(locator) => locator,
            Err(_) => {
                let safe_locator = fallback_locator(raw_url)?;
                return Err(SourceError::for_url(SourceErrorKind::Url, &safe_locator));
            }
        };
        self.ensure_session_live(session, &locator)?;
        let mut pending_slot = reserve_source(&session.inner, &locator)?;

        let registration_budget = Budget::new(REGISTRATION_LIMITS);
        let probe = match self
            .fetch(
                &locator,
                0..1,
                None,
                &[&registration_budget, &session.inner.budget],
                REGISTRATION_LIMITS.deadline,
                None,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => return Err(error),
        };
        let (length, etag) = match verify_response(&probe, 0..1, None) {
            Ok(value) => value,
            Err(kind) => return Err(SourceError::for_url(kind, &locator)),
        };
        if length > MAX_OBJECT_BYTES {
            return Err(SourceError::for_url(SourceErrorKind::Protocol, &locator));
        }

        let handle = SourceHandle(random_handle());
        let identity = ContentIdentity::StrongEtag {
            source_key: source_key(locator.locator().as_str()),
            revision_key: revision_key(&etag, length),
            length,
        };
        let source = Arc::new(HttpsRangeObject {
            handle,
            identity,
            locator,
            etag,
            session: session.inner.clone(),
            gateway: self.clone(),
            invalidated: AtomicBool::new(false),
            read_lock: Mutex::new(()),
        });
        pending_slot.commit();
        Ok(source)
    }

    fn ensure_session_live(
        &self,
        session: &SourceSession,
        locator: &PublicUrl,
    ) -> Result<(), SourceError> {
        if session.inner.created_at.elapsed() >= SESSION_LIMITS.deadline {
            return Err(SourceError::for_url(
                SourceErrorKind::SessionExpired,
                locator,
            ));
        }
        Ok(())
    }

    pub(crate) async fn fetch(
        &self,
        locator: &PublicUrl,
        range: Range<u64>,
        if_match: Option<&str>,
        budgets: &[&Budget],
        request_timeout: Duration,
        handle: Option<&SourceHandle>,
    ) -> Result<ExecutorResponse, SourceError> {
        let mut reservations = Vec::with_capacity(budgets.len());
        for budget in budgets {
            reservations.push(
                budget
                    .reserve(range.end - range.start)
                    .map_err(|_| self.error_for(handle, locator, SourceErrorKind::Budget))?,
            );
        }

        let deadline = Instant::now()
            + remaining_timeout(budgets, request_timeout)
                .map_err(|_| self.error_for(handle, locator, SourceErrorKind::Budget))?;
        let addresses = timeout(
            remaining_until(deadline)?,
            self.resolver.resolve(locator.host()),
        )
        .await
        .map_err(|_| self.error_for(handle, locator, SourceErrorKind::Timeout))?
        .map_err(|_| self.error_for(handle, locator, SourceErrorKind::Transport))?;
        if addresses.is_empty()
            || addresses
                .iter()
                .copied()
                .any(|address| !is_public_address(address))
        {
            return Err(self.error_for(handle, locator, SourceErrorKind::AddressDenied));
        }

        let consumed = Arc::new(AtomicU64::new(0));
        let request = OutboundRequest {
            hostname: locator.host().to_owned(),
            addresses,
            range: range.clone(),
            if_match: if_match.map(ToOwned::to_owned),
            timeout: remaining_until(deadline)?,
            locator: locator.locator().clone(),
            consumed: consumed.clone(),
        };
        let response =
            match timeout(remaining_until(deadline)?, self.executor.execute(request)).await {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => {
                    let charge_error = charge_reservations(
                        reservations,
                        consumed.load(Ordering::Acquire).max(error.consumed_bytes),
                    );
                    let kind = match error.kind {
                        ExecutorErrorKind::TooLarge => SourceErrorKind::Protocol,
                        ExecutorErrorKind::Timeout => SourceErrorKind::Timeout,
                        ExecutorErrorKind::Transport => SourceErrorKind::Transport,
                    };
                    return Err(if charge_error.is_err() {
                        self.error_for(handle, locator, SourceErrorKind::Budget)
                    } else {
                        self.error_for(handle, locator, kind)
                    });
                }
                Err(_) => {
                    let charge_error =
                        charge_reservations(reservations, consumed.load(Ordering::Acquire));
                    return Err(if charge_error.is_err() {
                        self.error_for(handle, locator, SourceErrorKind::Budget)
                    } else {
                        self.error_for(handle, locator, SourceErrorKind::Timeout)
                    });
                }
            };
        charge_reservations(reservations, response.body.len() as u64)
            .map_err(|_| self.error_for(handle, locator, SourceErrorKind::Budget))?;
        Ok(response)
    }

    fn error_for(
        &self,
        handle: Option<&SourceHandle>,
        locator: &PublicUrl,
        kind: SourceErrorKind,
    ) -> SourceError {
        handle.map_or_else(
            || SourceError::for_url(kind, locator),
            |handle| SourceError::for_handle(kind, handle),
        )
    }
}

fn remaining_timeout(budgets: &[&Budget], cap: Duration) -> Result<Duration, ()> {
    budgets.iter().try_fold(cap, |remaining, budget| {
        budget
            .remaining()
            .map(|value| remaining.min(value))
            .map_err(|_| ())
    })
}

fn remaining_until(deadline: Instant) -> Result<Duration, SourceError> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| SourceError::for_url(SourceErrorKind::Timeout, &safe_invalid_locator()))
}

pub(crate) fn charge_reservations(
    reservations: Vec<crate::BudgetReservation<'_>>,
    actual_bytes: u64,
) -> Result<(), crate::BudgetError> {
    let mut first_error = None;
    for reservation in reservations {
        if let Err(error) = reservation.finish(actual_bytes) {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

struct HttpsRangeObject {
    handle: SourceHandle,
    identity: ContentIdentity,
    locator: PublicUrl,
    etag: String,
    session: Arc<SessionState>,
    gateway: PublicHttpsGateway,
    invalidated: AtomicBool,
    read_lock: Mutex<()>,
}

impl Drop for HttpsRangeObject {
    fn drop(&mut self) {
        self.session.sources.fetch_sub(1, Ordering::AcqRel);
    }
}

#[async_trait]
impl RangeObject for HttpsRangeObject {
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
        self.locator.display_name()
    }

    async fn get_range(&self, range: Range<u64>) -> Result<Bytes, SourceError> {
        let _read_guard = self.read_lock.lock().await;
        if self.invalidated.load(Ordering::Acquire) {
            return Err(SourceError::for_handle(
                SourceErrorKind::Invalidated,
                &self.handle,
            ));
        }
        if range.start >= range.end
            || range.end > self.length()
            || range.end - range.start > MAX_RANGE_BYTES
        {
            return Err(SourceError::for_handle(
                SourceErrorKind::Range,
                &self.handle,
            ));
        }
        if self.session.created_at.elapsed() >= SESSION_LIMITS.deadline {
            return Err(SourceError::for_handle(
                SourceErrorKind::SessionExpired,
                &self.handle,
            ));
        }

        let response = self
            .gateway
            .fetch(
                &self.locator,
                range.clone(),
                Some(&self.etag),
                &[&self.session.budget],
                Duration::from_secs(10),
                Some(&self.handle),
            )
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                self.invalidated.store(true, Ordering::Release);
                return Err(error);
            }
        };
        match verify_response(&response, range, Some(&self.etag)) {
            Ok((observed_length, _)) if observed_length == self.length() => Ok(response.body),
            Ok(_) => {
                self.invalidated.store(true, Ordering::Release);
                Err(SourceError::for_handle(
                    SourceErrorKind::Identity,
                    &self.handle,
                ))
            }
            Err(kind) => {
                self.invalidated.store(true, Ordering::Release);
                Err(SourceError::for_handle(kind, &self.handle))
            }
        }
    }
}

struct PendingSourceSlot {
    session: Arc<SessionState>,
    committed: bool,
}

impl PendingSourceSlot {
    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingSourceSlot {
    fn drop(&mut self) {
        if !self.committed {
            self.session.sources.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

fn reserve_source(
    session: &Arc<SessionState>,
    locator: &PublicUrl,
) -> Result<PendingSourceSlot, SourceError> {
    loop {
        let current = session.sources.load(Ordering::Acquire);
        if current >= 3 {
            return Err(SourceError::for_url(SourceErrorKind::SourceLimit, locator));
        }
        if session
            .sources
            .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Ok(PendingSourceSlot {
                session: session.clone(),
                committed: false,
            });
        }
    }
}

fn verify_response(
    response: &ExecutorResponse,
    requested: Range<u64>,
    expected_etag: Option<&str>,
) -> Result<(u64, String), SourceErrorKind> {
    if response.status == 412 {
        return Err(SourceErrorKind::Identity);
    }
    if (300..400).contains(&response.status) {
        return Err(SourceErrorKind::Redirect);
    }
    if response.status != 206 {
        return Err(SourceErrorKind::Protocol);
    }
    if response
        .singleton_header("content-type")?
        .is_some_and(|value| {
            value
                .to_ascii_lowercase()
                .starts_with("multipart/byteranges")
        })
    {
        return Err(SourceErrorKind::Protocol);
    }
    if response
        .singleton_header("content-encoding")?
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(SourceErrorKind::Protocol);
    }
    let content_range = response
        .singleton_header("content-range")?
        .ok_or(SourceErrorKind::Protocol)
        .and_then(parse_content_range)?;
    if content_range.0 != requested {
        return Err(SourceErrorKind::Protocol);
    }
    if response.body.len() as u64 != requested.end - requested.start {
        return Err(SourceErrorKind::Protocol);
    }
    let etag = response
        .singleton_header("etag")?
        .filter(|value| is_strong_etag(value))
        .ok_or(SourceErrorKind::Identity)?
        .to_owned();
    if expected_etag.is_some_and(|expected| expected != etag) {
        return Err(SourceErrorKind::Identity);
    }
    Ok((content_range.1, etag))
}

fn parse_content_range(value: &str) -> Result<(Range<u64>, u64), SourceErrorKind> {
    let value = value
        .strip_prefix("bytes ")
        .ok_or(SourceErrorKind::Protocol)?;
    let (interval, total) = value.split_once('/').ok_or(SourceErrorKind::Protocol)?;
    let (start, end) = interval.split_once('-').ok_or(SourceErrorKind::Protocol)?;
    let start = start
        .parse::<u64>()
        .map_err(|_| SourceErrorKind::Protocol)?;
    let end = end.parse::<u64>().map_err(|_| SourceErrorKind::Protocol)?;
    let total = total
        .parse::<u64>()
        .map_err(|_| SourceErrorKind::Protocol)?;
    if total == 0 || end < start {
        return Err(SourceErrorKind::Protocol);
    }
    let end = end.checked_add(1).ok_or(SourceErrorKind::Protocol)?;
    if end > total {
        return Err(SourceErrorKind::Protocol);
    }
    Ok((start..end, total))
}

fn is_strong_etag(value: &str) -> bool {
    !value.starts_with("W/")
        && value.len() >= 2
        && value.starts_with('"')
        && value.ends_with('"')
        && value[1..value.len() - 1]
            .bytes()
            .all(|byte| byte == b'!' || (b'#'..=b'~').contains(&byte) || byte >= 0x80)
}

fn source_key(locator: &str) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(process_secret()).expect("HMAC accepts fixed keys");
    mac.update(locator.as_bytes());
    mac.finalize().into_bytes().into()
}

fn revision_key(etag: &str, length: u64) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(etag.as_bytes());
    digest.update([0]);
    digest.update(length.to_be_bytes());
    digest.finalize().into()
}

fn random_handle() -> String {
    let mut random = [0_u8; 16];
    rand::rng().fill(&mut random);
    random.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, hostname: &str) -> Result<Vec<IpAddr>, ResolverError> {
        tokio::net::lookup_host((hostname, 443))
            .await
            .map(|answers| answers.map(|address| address.ip()).collect())
            .map_err(|_| ResolverError::unavailable())
    }
}

struct ReqwestExecutor;

#[async_trait]
impl HttpExecutor for ReqwestExecutor {
    async fn execute(&self, request: OutboundRequest) -> Result<ExecutorResponse, ExecutorError> {
        let consumption = request.clone();
        let addrs: Vec<_> = request
            .addresses
            .iter()
            .map(|address| SocketAddr::new(*address, 443))
            .collect();
        let builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .connect_timeout(request.timeout)
            .timeout(request.timeout)
            .resolve_to_addrs(&request.hostname, &addrs);
        let client = builder.build().map_err(|_| ExecutorError::unavailable(0))?;
        let mut request_builder = client
            .get(request.locator)
            .header(header::ACCEPT_ENCODING, "identity")
            .header(
                header::RANGE,
                format!("bytes={}-{}", request.range.start, request.range.end - 1),
            );
        if let Some(etag) = request.if_match {
            request_builder = request_builder.header(header::IF_MATCH, etag);
        }
        let mut response = request_builder.send().await.map_err(|error| {
            if error.is_timeout() {
                ExecutorError::timed_out(0)
            } else {
                ExecutorError::unavailable(0)
            }
        })?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
            })
            .collect::<Vec<_>>();
        let maximum = request.range.end - request.range.start;
        if response
            .content_length()
            .is_some_and(|length| length > maximum)
        {
            return Err(ExecutorError::response_too_large(0));
        }
        let mut body = BytesMut::new();
        while let Some(chunk) = match response.chunk().await {
            Ok(chunk) => chunk,
            Err(error) => {
                return Err(if error.is_timeout() {
                    ExecutorError::timed_out(body.len() as u64)
                } else {
                    ExecutorError::unavailable(body.len() as u64)
                });
            }
        } {
            if body.len().saturating_add(chunk.len()) > maximum as usize {
                consumption.record_consumed(chunk.len() as u64);
                return Err(ExecutorError::response_too_large(
                    consumption.consumed.load(Ordering::Acquire),
                ));
            }
            consumption.record_consumed(chunk.len() as u64);
            body.extend_from_slice(&chunk);
        }
        Ok(ExecutorResponse::new(status, headers, body.freeze()))
    }
}

fn fallback_locator(raw_url: &str) -> Result<PublicUrl, SourceError> {
    let host = raw_url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(rest))
        .unwrap_or("invalid-host");
    crate::validate_public_url(&format!("https://{host}/"))
        .map_err(|_| SourceError::for_url(SourceErrorKind::Url, &safe_invalid_locator()))
}

fn safe_invalid_locator() -> PublicUrl {
    crate::validate_public_url("https://invalid.example/").expect("constant URL is valid")
}
