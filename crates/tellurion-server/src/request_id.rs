//! Request correlation (`#189`): accept or mint an `X-Request-ID`, echo it on
//! the response, and stamp it on the request's trace span so every log line a
//! request produces — including the `event=slow_request` diagnostic — carries
//! one id a client or proxy can quote back.
//!
//! The middleware sits outermost, outside even `observe_request`: a minted id
//! must already be on the request when the observation layer captures it for
//! slow-request events, and the echo must survive on responses synthesized by
//! load-shed and timeout, which never reach an inner layer.
//!
//! An inbound id is honored only when it is short, visible ASCII. Anything
//! else — absent, oversized, control bytes, non-ASCII — is replaced by a
//! freshly minted UUID rather than rejected: correlation is a diagnostic aid,
//! never a reason to fail a request, and echoing arbitrary client bytes into
//! response headers and structured logs is exactly the injection surface the
//! validation exists to close.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Span;

pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Longest inbound id honored verbatim. Generous enough for every tracing
/// scheme observed in the wild (UUIDs, W3C traceparent, load-balancer ids),
/// small enough that a hostile header can't bloat every downstream log line.
const MAX_INBOUND_LEN: usize = 128;

fn is_honored(value: &HeaderValue) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_INBOUND_LEN
        && bytes.iter().all(|byte| byte.is_ascii_graphic())
}

pub async fn propagate_request_id(mut request: Request, next: Next) -> Response {
    let id = match request.headers().get(&REQUEST_ID_HEADER) {
        Some(value) if is_honored(value) => value.clone(),
        _ => {
            let minted = uuid::Uuid::new_v4().to_string();
            // A UUID's hyphenated form is always a valid header value.
            let minted = HeaderValue::from_str(&minted).expect("uuid is valid header value");
            request
                .headers_mut()
                .insert(&REQUEST_ID_HEADER, minted.clone());
            minted
        }
    };
    let mut response = next.run(request).await;
    response.headers_mut().insert(&REQUEST_ID_HEADER, id);
    response
}

/// `TraceLayer` span factory: the default `DefaultMakeSpan` fields plus
/// `request_id`, which [`propagate_request_id`] has already guaranteed is
/// present and clean by the time this layer runs.
pub fn trace_span(request: &axum::http::Request<Body>) -> Span {
    let request_id = request
        .headers()
        .get(&REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown");
    let uri = trace_uri(request.uri());
    tracing::info_span!(
        "request",
        method = %request.method(),
        uri,
        version = ?request.version(),
        request_id,
    )
}

fn trace_uri(uri: &axum::http::Uri) -> String {
    let has_sensitive_oidc_query = uri.query().is_some_and(|query| {
        url::form_urlencoded::parse(query.as_bytes()).any(|(key, _)| {
            [
                "code",
                "state",
                "id_token",
                "access_token",
                "client_secret",
                "nonce",
                "code_verifier",
                "code_challenge",
                "session_id",
                "tellurion_control_session",
                "tellurion_control_login",
                "error_description",
            ]
            .iter()
            .any(|sensitive| key.eq_ignore_ascii_case(sensitive))
        })
    });
    if uri.path().starts_with("/_auth/control/") || has_sensitive_oidc_query {
        uri.path().to_string()
    } else {
        uri.to_string()
    }
}

/// The id `observe_request` folds into `event=slow_request`, captured before
/// the inner layers consume the request.
pub fn current_id(headers: &axum::http::HeaderMap) -> String {
    headers
        .get(&REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honors_a_clean_inbound_id() {
        assert!(is_honored(&HeaderValue::from_static("req-abc.123_DEF")));
    }

    #[test]
    fn rejects_empty_oversized_and_non_graphic_ids() {
        assert!(!is_honored(&HeaderValue::from_static("")));
        let oversized = "a".repeat(MAX_INBOUND_LEN + 1);
        assert!(!is_honored(&HeaderValue::from_str(&oversized).unwrap()));
        assert!(!is_honored(&HeaderValue::from_static("has space")));
        assert!(!is_honored(&HeaderValue::from_bytes(b"tab\there").unwrap()));
    }

    #[test]
    fn control_auth_trace_uri_never_contains_callback_code_or_state() {
        let auth: axum::http::Uri =
            "/_auth/control/callback?code=supplied-code&state=supplied-state"
                .parse()
                .unwrap();
        let ordinary: axum::http::Uri = "/acme/features?limit=10".parse().unwrap();

        assert_eq!(trace_uri(&auth), "/_auth/control/callback");
        assert_eq!(trace_uri(&ordinary), "/acme/features?limit=10");
    }

    #[test]
    fn sensitive_oidc_query_keys_are_redacted_independently_of_the_request_path() {
        for raw in [
            "/_auth%2Fcontrol/callback?code=supplied-code",
            "/_auth/control%2Fcallback?state=supplied-state",
            "/_auth%ZZcontrol/callback?id_token=supplied-id-token",
            "/acme/features?access_token=supplied-access-token",
            "/acme/features?client_secret=supplied-client-secret",
            "/acme/features?%63ode=percent-encoded-key",
            "/acme/features?NONCE=supplied-nonce",
            "/acme/features?code_verifier=supplied-verifier",
            "/acme/features?code_challenge=supplied-challenge",
            "/acme/features?session_id=supplied-session-id",
            "/acme/features?tellurion_control_session=supplied-cookie",
            "/acme/features?error_description=supplied-description",
            "/unrelated%2Fpath?%6Eonce=encoded-nonce-key",
            "/unrelated%2Fpath?%54ELLURION_CONTROL_LOGIN=supplied-login-cookie",
        ] {
            let uri: axum::http::Uri = raw.parse().unwrap();
            assert_eq!(trace_uri(&uri), uri.path(), "{raw}");
        }

        for raw in [
            "/acme/features?limit=10&cursor=next",
            "/acme/features?postcode=00100&decode=true",
        ] {
            let uri: axum::http::Uri = raw.parse().unwrap();
            assert_eq!(trace_uri(&uri), raw, "{raw}");
        }
    }
}
