use std::{
    collections::VecDeque,
    net::IpAddr,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use bytes::Bytes;

use crate::{
    range::{
        ExecutorError, ExecutorResponse, HttpExecutor, OutboundRequest, PublicHttpsGateway,
        Resolver, ResolverError,
    },
    Budget, BudgetErrorKind, BudgetLimits, SourceErrorKind,
};

#[test]
fn request_limit_exhaustion_refuses_the_next_origin_request() {
    let budget = Budget::new(BudgetLimits {
        requests: 1,
        bytes: 4,
        deadline: Duration::from_secs(1),
        concurrent: 1,
    });
    budget.reserve(1).unwrap().finish(1).unwrap();
    assert_eq!(
        budget.reserve(1).unwrap_err().kind(),
        BudgetErrorKind::RequestLimit
    );
}

#[tokio::test]
async fn concurrent_fetch_is_refused_while_a_deterministic_request_is_in_flight() {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let gateway = Arc::new(PublicHttpsGateway::with_transport(
        Arc::new(StaticResolver),
        Arc::new(BlockingExecutor {
            started: started.clone(),
            release: release.clone(),
        }),
    ));
    let budget = Arc::new(Budget::new(BudgetLimits {
        requests: 2,
        bytes: 4,
        deadline: Duration::from_secs(1),
        concurrent: 1,
    }));
    let locator = crate::validate_public_url("https://public.example/data.tif").unwrap();
    let first_gateway = gateway.clone();
    let first_budget = budget.clone();
    let first_locator = locator.clone();
    let first = tokio::spawn(async move {
        first_gateway
            .fetch(
                &first_locator,
                0..1,
                None,
                &[first_budget.as_ref()],
                Duration::from_secs(1),
                None,
            )
            .await
    });
    started.notified().await;
    let error = gateway
        .fetch(
            &locator,
            0..1,
            None,
            &[budget.as_ref()],
            Duration::from_secs(1),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), SourceErrorKind::Budget);
    release.notify_one();
    assert!(first.await.unwrap().is_ok());
}

#[tokio::test]
async fn mixed_dns_answers_and_identity_encoding_are_rejected_independently() {
    let mixed_gateway = PublicHttpsGateway::with_transport(
        Arc::new(ScriptedResolver::new(vec![vec![
            "8.8.8.8".parse().unwrap(),
            "10.0.0.1".parse().unwrap(),
        ]])),
        Arc::new(ScriptedExecutor::new(vec![])),
    );
    let error = match mixed_gateway
        .register_range_object(
            &mixed_gateway.open_session(),
            "https://public.example/data.tif",
        )
        .await
    {
        Ok(_) => panic!("mixed answers accepted"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), SourceErrorKind::AddressDenied);

    let gateway = gateway(vec![response(
        206,
        vec![
            ("content-range", "bytes 0-0/1"),
            ("etag", "\"a\""),
            ("content-encoding", "gzip"),
        ],
        b"a",
    )]);
    let error = match gateway
        .register_range_object(&gateway.open_session(), "https://public.example/data.tif")
        .await
    {
        Ok(_) => panic!("encoded response accepted"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), SourceErrorKind::Protocol);
}

#[tokio::test]
async fn elapsed_deadline_interrupts_a_hanging_executor() {
    let gateway =
        PublicHttpsGateway::with_transport(Arc::new(StaticResolver), Arc::new(HangingExecutor));
    let locator = crate::validate_public_url("https://public.example/data.tif").unwrap();
    let budget = Budget::new(BudgetLimits {
        requests: 1,
        bytes: 1,
        deadline: Duration::from_secs(1),
        concurrent: 1,
    });
    let result = tokio::time::timeout(
        Duration::from_millis(75),
        gateway.fetch(
            &locator,
            0..1,
            None,
            &[&budget],
            Duration::from_millis(20),
            None,
        ),
    )
    .await;
    let result = result.expect("gateway must enforce its deadline");
    assert_eq!(result.unwrap_err().kind(), SourceErrorKind::Timeout);
}

#[tokio::test]
async fn elapsed_timeout_charges_partial_bytes_before_cancelling_executor() {
    let gateway = PublicHttpsGateway::with_transport(
        Arc::new(StaticResolver),
        Arc::new(PartialHangingExecutor),
    );
    let locator = crate::validate_public_url("https://public.example/data.tif").unwrap();
    let first = Budget::new(BudgetLimits {
        requests: 1,
        bytes: 1,
        deadline: Duration::from_secs(1),
        concurrent: 1,
    });
    let second = Budget::new(BudgetLimits {
        requests: 2,
        bytes: 2,
        deadline: Duration::from_secs(1),
        concurrent: 1,
    });
    let error = gateway
        .fetch(
            &locator,
            0..1,
            None,
            &[&first, &second],
            Duration::from_millis(20),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), SourceErrorKind::Budget);
    assert_eq!(
        second.reserve(1).unwrap_err().kind(),
        BudgetErrorKind::ByteLimit
    );
}

#[test]
fn url_length_and_budget_admission_limits_are_enforced() {
    assert!(
        crate::validate_public_url(&format!("https://public.example/{}", "a".repeat(2_048)))
            .is_err()
    );
    let budget = Budget::new(BudgetLimits {
        requests: 1,
        bytes: 2,
        deadline: Duration::ZERO,
        concurrent: 0,
    });
    assert_eq!(
        budget.reserve(1).unwrap_err().kind(),
        BudgetErrorKind::Deadline
    );
}

#[test]
fn consumed_error_bytes_and_poisoned_budget_fail_closed_without_double_release() {
    let budget = Budget::new(BudgetLimits {
        requests: 2,
        bytes: 3,
        deadline: Duration::from_secs(1),
        concurrent: 1,
    });
    super::range::charge_reservations(vec![budget.reserve(2).unwrap()], 3).unwrap();
    assert_eq!(
        budget.reserve(1).unwrap_err().kind(),
        BudgetErrorKind::ByteLimit
    );

    let poisoned = Budget::new(BudgetLimits {
        requests: 1,
        bytes: 1,
        deadline: Duration::from_secs(1),
        concurrent: 1,
    });
    poisoned.poison_for_test();
    assert_eq!(
        poisoned.reserve(1).unwrap_err().kind(),
        BudgetErrorKind::Poisoned
    );
    assert!(poisoned.is_invalidated());
}

#[test]
fn failed_first_settlement_does_not_skip_later_budget_charges() {
    let first = Budget::new(BudgetLimits {
        requests: 2,
        bytes: 1,
        deadline: Duration::from_secs(1),
        concurrent: 1,
    });
    let second = Budget::new(BudgetLimits {
        requests: 2,
        bytes: 2,
        deadline: Duration::from_secs(1),
        concurrent: 1,
    });
    assert!(super::range::charge_reservations(
        vec![first.reserve(1).unwrap(), second.reserve(1).unwrap()],
        2,
    )
    .is_err());
    assert_eq!(
        second.reserve(1).unwrap_err().kind(),
        BudgetErrorKind::ByteLimit
    );
}

#[tokio::test]
async fn exact_range_requires_singleton_headers_and_strict_etags() {
    for headers in [
        vec![
            ("content-range", "bytes 0-0/1"),
            ("content-range", "bytes 0-0/1"),
            ("etag", "\"a\""),
        ],
        vec![
            ("content-range", "bytes 0-0/1"),
            ("etag", "\"a\""),
            ("etag", "\"b\""),
        ],
        vec![("content-range", "bytes 0-0/1"), ("etag", "\"a\", \"b\"")],
        vec![
            ("content-range", "bytes 0-0/1"),
            ("etag", "\"a\""),
            ("content-encoding", "identity"),
            ("content-encoding", "identity"),
        ],
    ] {
        let gateway = gateway(vec![response(206, headers, b"a")]);
        let result = gateway
            .register_range_object(&gateway.open_session(), "https://public.example/data.tif")
            .await;
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn range_protocol_matrix_rejects_ignored_redirect_encoded_and_malformed_responses() {
    for bad_probe in [
        response(
            200,
            vec![("content-range", "bytes 0-0/1"), ("etag", "\"a\"")],
            b"a",
        ),
        response(302, vec![], b""),
        response(
            206,
            vec![("content-range", "bytes 0-0/*"), ("etag", "\"a\"")],
            b"a",
        ),
        response(
            206,
            vec![("content-range", "bytes 0-1/2"), ("etag", "\"a\"")],
            b"a",
        ),
        response(
            206,
            vec![("content-range", "bytes 0-0/1"), ("etag", "W/\"a\"")],
            b"a",
        ),
        response(
            206,
            vec![
                ("content-range", "bytes 0-0/1"),
                ("etag", "\"a"),
                ("content-encoding", "gzip"),
            ],
            b"a",
        ),
    ] {
        let gateway = gateway(vec![bad_probe]);
        let error = match gateway
            .register_range_object(
                &gateway.open_session(),
                "https://public.example/private.tif",
            )
            .await
        {
            Ok(_) => panic!("bad probe accepted"),
            Err(error) => error,
        };
        assert!(!error.to_string().contains("private.tif"));
    }

    for bad_read in [
        response(
            206,
            vec![("content-range", "bytes 1-1/3"), ("etag", "\"a\"")],
            b"",
        ),
        response(
            206,
            vec![("content-range", "bytes 1-1/3"), ("etag", "\"a\"")],
            b"bb",
        ),
        response(
            206,
            vec![("content-range", "bytes 1-1/4"), ("etag", "\"a\"")],
            b"b",
        ),
        response(
            206,
            vec![("content-range", "bytes 1-1/3"), ("etag", "\"b\"")],
            b"b",
        ),
        response(412, vec![], b""),
    ] {
        let gateway = gateway(vec![exact(0..1, 3, "\"a\"", b"a"), bad_read]);
        let source = gateway
            .register_range_object(
                &gateway.open_session(),
                "https://public.example/private.tif",
            )
            .await
            .unwrap();
        assert!(source.get_range(1..2).await.is_err());
        assert_eq!(
            source.get_range(1..2).await.unwrap_err().kind(),
            SourceErrorKind::Invalidated
        );
    }
}

#[tokio::test]
async fn mixed_dns_rebinding_timeout_and_if_match_are_enforced() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let executor = Arc::new(RecordingExecutor {
        responses: Mutex::new(
            vec![exact(0..1, 2, "\"a\"", b"a"), exact(1..2, 2, "\"a\"", b"b")].into(),
        ),
        requests: requests.clone(),
    });
    let resolver = Arc::new(ScriptedResolver::new(vec![
        vec!["8.8.8.8".parse().unwrap()],
        vec!["10.0.0.1".parse().unwrap()],
    ]));
    let gateway = PublicHttpsGateway::with_transport(resolver, executor);
    let source = gateway
        .register_range_object(
            &gateway.open_session(),
            "https://public.example/private.tif",
        )
        .await
        .unwrap();
    assert_eq!(
        source.get_range(1..2).await.unwrap_err().kind(),
        SourceErrorKind::AddressDenied
    );
    assert_eq!(requests.lock().unwrap().len(), 1);

    let requests = Arc::new(Mutex::new(Vec::new()));
    let executor = Arc::new(RecordingExecutor {
        responses: Mutex::new(
            vec![exact(0..1, 2, "\"a\"", b"a"), exact(1..2, 2, "\"a\"", b"b")].into(),
        ),
        requests: requests.clone(),
    });
    let gateway = PublicHttpsGateway::with_transport(Arc::new(StaticResolver), executor);
    let source = gateway
        .register_range_object(
            &gateway.open_session(),
            "https://public.example/private.tif",
        )
        .await
        .unwrap();
    source.get_range(1..2).await.unwrap();
    assert_eq!(
        requests.lock().unwrap()[1].if_match.as_deref(),
        Some("\"a\"")
    );

    let gateway =
        PublicHttpsGateway::with_transport(Arc::new(StaticResolver), Arc::new(FailingExecutor));
    let error = match gateway
        .register_range_object(
            &gateway.open_session(),
            "https://public.example/private.tif",
        )
        .await
    {
        Ok(_) => panic!("timeout accepted"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), SourceErrorKind::Timeout);
}

#[tokio::test]
async fn reads_are_serialized_after_identity_invalidation() {
    let executor = Arc::new(CountingExecutor::new(vec![
        exact(0..1, 2, "\"a\"", b"a"),
        response(412, vec![("etag", "\"a\"")], b""),
    ]));
    let gateway = PublicHttpsGateway::with_transport(Arc::new(StaticResolver), executor.clone());
    let source = gateway
        .register_range_object(&gateway.open_session(), "https://public.example/data.tif")
        .await
        .unwrap();
    let (first, second) = tokio::join!(source.get_range(1..2), source.get_range(1..2));
    assert!(first.is_err());
    assert_eq!(second.unwrap_err().kind(), SourceErrorKind::Invalidated);
    assert_eq!(executor.max_active.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_releases_pending_source_slot() {
    let executor = Arc::new(HangingExecutor);
    let gateway = PublicHttpsGateway::with_transport(Arc::new(StaticResolver), executor);
    let session = gateway.open_session();
    let task_gateway = gateway.clone();
    let task_session = session.clone();
    let task = tokio::spawn(async move {
        let _ = task_gateway
            .register_range_object(&task_session, "https://public.example/data.tif")
            .await;
    });
    tokio::time::sleep(Duration::from_millis(5)).await;
    task.abort();
    let _ = task.await;
    assert_eq!(session.inner.sources.load(Ordering::Acquire), 0);
}

fn gateway(responses: Vec<ExecutorResponse>) -> PublicHttpsGateway {
    PublicHttpsGateway::with_transport(
        Arc::new(StaticResolver),
        Arc::new(ScriptedExecutor::new(responses)),
    )
}

fn response(status: u16, headers: Vec<(&str, &str)>, body: &[u8]) -> ExecutorResponse {
    ExecutorResponse::new(
        status,
        headers
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect(),
        Bytes::copy_from_slice(body),
    )
}

fn exact(range: std::ops::Range<u64>, total: u64, etag: &str, body: &[u8]) -> ExecutorResponse {
    response(
        206,
        vec![
            (
                "content-range",
                &format!("bytes {}-{}/{}", range.start, range.end - 1, total),
            ),
            ("etag", etag),
        ],
        body,
    )
}

struct StaticResolver;

#[async_trait]
impl Resolver for StaticResolver {
    async fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, ResolverError> {
        Ok(vec!["8.8.8.8".parse().unwrap()])
    }
}

struct ScriptedExecutor {
    responses: Mutex<VecDeque<ExecutorResponse>>,
}
impl ScriptedExecutor {
    fn new(responses: Vec<ExecutorResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}
#[async_trait]
impl HttpExecutor for ScriptedExecutor {
    async fn execute(&self, _: OutboundRequest) -> Result<ExecutorResponse, ExecutorError> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ExecutorError::unavailable(0))
    }
}

struct CountingExecutor {
    responses: Mutex<VecDeque<ExecutorResponse>>,
    active: AtomicU32,
    max_active: AtomicU32,
}
impl CountingExecutor {
    fn new(responses: Vec<ExecutorResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            active: AtomicU32::new(0),
            max_active: AtomicU32::new(0),
        }
    }
}
#[async_trait]
impl HttpExecutor for CountingExecutor {
    async fn execute(&self, _: OutboundRequest) -> Result<ExecutorResponse, ExecutorError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(5)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ExecutorError::unavailable(0))
    }
}

struct HangingExecutor;
#[async_trait]
impl HttpExecutor for HangingExecutor {
    async fn execute(&self, _: OutboundRequest) -> Result<ExecutorResponse, ExecutorError> {
        std::future::pending().await
    }
}

struct BlockingExecutor {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

struct PartialHangingExecutor;
#[async_trait]
impl HttpExecutor for PartialHangingExecutor {
    async fn execute(&self, request: OutboundRequest) -> Result<ExecutorResponse, ExecutorError> {
        request.record_consumed(2);
        std::future::pending().await
    }
}
#[async_trait]
impl HttpExecutor for BlockingExecutor {
    async fn execute(&self, _: OutboundRequest) -> Result<ExecutorResponse, ExecutorError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(exact(0..1, 1, "\"a\"", b"a"))
    }
}

struct ScriptedResolver {
    answers: Mutex<VecDeque<Vec<IpAddr>>>,
}
impl ScriptedResolver {
    fn new(answers: Vec<Vec<IpAddr>>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
        }
    }
}
#[async_trait]
impl Resolver for ScriptedResolver {
    async fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, ResolverError> {
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(ResolverError::unavailable)
    }
}

struct RecordingExecutor {
    responses: Mutex<VecDeque<ExecutorResponse>>,
    requests: Arc<Mutex<Vec<OutboundRequest>>>,
}
#[async_trait]
impl HttpExecutor for RecordingExecutor {
    async fn execute(&self, request: OutboundRequest) -> Result<ExecutorResponse, ExecutorError> {
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ExecutorError::unavailable(0))
    }
}

struct FailingExecutor;
#[async_trait]
impl HttpExecutor for FailingExecutor {
    async fn execute(&self, _: OutboundRequest) -> Result<ExecutorResponse, ExecutorError> {
        Err(ExecutorError::timed_out(0))
    }
}
