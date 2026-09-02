//! Shared RFC 9457 ("Problem Details for HTTP APIs") response body, reused
//! by every protocol crate that serves `application/problem+json` errors.
//!
//! This module is deliberately framework-free (no axum/http dependency) so
//! it can live in `tellurion-core` alongside the rest of the driver-agnostic
//! types. Each protocol crate is responsible for turning a [`Problem`] into
//! an actual `Response` — `IntoResponse` is a foreign trait and `Problem` is
//! a foreign type from any of those crates' perspective, so Rust's orphan
//! rule wouldn't let a shared blanket impl live here even if this crate did
//! take an axum dependency.

use serde::Serialize;

use crate::auth::Credential;
use crate::error::Error;

/// Media type every problem-details response across the server uses.
pub const PROBLEM_JSON: &str = "application/problem+json";

/// The RFC 9457 default `type` for problems that don't define a more
/// specific URI. Always serialized — RFC 9457 treats an absent `type` as
/// equivalent to this value, but conforming clients that key off presence
/// rather than default should still see it on the wire.
const DEFAULT_TYPE: &str = "about:blank";

/// RFC 9457 "Problem Details" body, extended with `code`: a short,
/// machine-readable extension member kept for compatibility with clients
/// written against this server's pre-RFC-9457 `{code, description}` shape.
#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    #[serde(rename = "type")]
    pub type_: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    pub code: String,
    /// `#188`: whole seconds a refused client should wait before retrying —
    /// `Some` only for a rate-ceiling refusal
    /// ([`for_rate_limited`](Self::for_rate_limited)), and absent from the
    /// wire entirely for every other problem.
    ///
    /// An RFC 9457 extension member *and* the source of the `Retry-After`
    /// header each protocol crate sets from it. Carrying it on the body is
    /// what lets that header be attached in one place per crate (the
    /// `IntoResponse` shim) instead of being threaded through every error
    /// constructor: a header each call site must remember eventually goes
    /// missing on one path.
    ///
    /// Spelled `retryAfter` on the wire: an RFC 9457 extension member is
    /// free to choose, and every other JSON member this server emits
    /// (`numberReturned`, `searchIncapableCollections`, ...) is camelCase —
    /// the RFC's own members happen to be single words, so this is the first
    /// place the question comes up.
    ///
    /// `u32`, not `u64`, and not merely to keep this struct small: a
    /// `Retry-After` delta is an HTTP header value in seconds, and
    /// `RateLimitDecl::validate` already caps a window at
    /// `MAX_RATE_WINDOW_SECONDS` (7 days), so nothing this server can
    /// produce comes near the 136-year ceiling of a `u32`.
    #[serde(rename = "retryAfter", skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u32>,
}

impl Problem {
    /// Builds a problem with the default `type` (`about:blank`) and the
    /// standard HTTP reason phrase for `status` as `title`.
    pub fn new(status: u16, code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            type_: DEFAULT_TYPE.to_string(),
            title: reason_phrase(status).to_string(),
            status,
            detail: detail.into(),
            code: code.into(),
            retry_after: None,
        }
    }

    /// `#188`: the `429` a `policy::enforce_rate_limits` refusal maps to.
    /// The detail is [`RateRefusal::detail`]'s own wording — which names the
    /// declared ceiling and window (facts a client needs to back off
    /// intelligently) but never which bucket was exhausted, so a shared
    /// `role`- or `tenant`-scoped ceiling can't be used to observe another
    /// client's traffic.
    pub fn for_rate_limited(refusal: &crate::rate_limit::RateRefusal) -> Self {
        let mut problem = Problem::new(429, "RateLimited", refusal.detail());
        // Saturating rather than `expect`: `RateLimitDecl::validate` bounds
        // every window well inside `u32`, so this can only ever be the exact
        // value — but a response is never worth panicking over.
        problem.retry_after = Some(u32::try_from(refusal.retry_after_seconds).unwrap_or(u32::MAX));
        problem
    }

    /// Maps a `tellurion_core::Error` to the problem body and HTTP status it
    /// belongs on — the mapping `tellurion-features` and `tellurion-stac`
    /// each used to carry a private, byte-identical copy of in their own
    /// `From<Error> for ApiError` impl. `lane` names the caller for the one
    /// branch (`Error::Storage`) whose log line used to differ between the
    /// two ("storage error serving a features/STAC request") — the detail
    /// text returned to the client never includes it; storage/config error
    /// detail is logged here, with `lane`, and never echoed into the
    /// returned `Problem`, so a caller further up never has to remember to
    /// redact it itself. Framework-free (no axum `StatusCode`): the status
    /// travels as this struct's own `status` field, a plain `u16`; a caller
    /// builds whatever HTTP-framework status type it needs from that.
    pub fn from_core_error(err: &Error, lane: &str) -> Self {
        match err {
            Error::Invalid(msg) => Problem::new(400, "InvalidParameter", msg.clone()),
            Error::PayloadTooLarge { limit } => Problem::new(
                413,
                "PayloadTooLarge",
                format!("request body exceeds the {limit}-byte limit"),
            ),
            Error::Conflict(msg) => Problem::new(409, "Conflict", msg.clone()),
            Error::UnsupportedMediaType(media_type) => Problem::new(
                415,
                "UnsupportedMediaType",
                format!("media type '{media_type}' is not accepted here"),
            ),
            Error::UnprocessableEntity(msg) => {
                Problem::new(422, "UnprocessableEntity", msg.clone())
            }
            Error::ItemsVertexBudgetExceeded {
                collection,
                feature_id,
                cumulative_vertices,
                limit,
            } => Problem::new(
                422,
                "ItemsVertexBudgetExceeded",
                format!(
                    "exact geometry for collection '{collection}' was refused at feature \
                     '{feature_id}': {} cumulative vertices exceeds the configured {}-vertex \
                     page budget; narrow the request or raise items_vertex_budget intentionally",
                    format_count(*cumulative_vertices),
                    format_count(*limit),
                ),
            ),
            Error::NotFound => {
                Problem::new(404, "NotFound", "the requested resource was not found")
            }
            Error::CapabilityUnsupported {
                collection,
                capability,
            } => Problem::new(
                404,
                "NotFound",
                format!("collection '{collection}' does not support '{capability}'"),
            ),
            Error::Timeout => Problem::new(504, "Timeout", "the request timed out"),
            Error::Storage(source) => {
                tracing::error!(error = %source, "storage error serving a {lane} request");
                Problem::new(
                    500,
                    "InternalServerError",
                    "an internal storage error occurred",
                )
            }
            Error::Config(msg) => {
                tracing::error!(error = %msg, "configuration error surfaced at request time");
                Problem::new(
                    500,
                    "InternalServerError",
                    "an internal configuration error occurred",
                )
            }
            Error::VersionConflict { expected, current } => Problem::new(
                409,
                "ConfigVersionConflict",
                format!(
                    "expected config version '{expected}' but the current version is '{current}'"
                ),
            ),
            Error::ControlValidation(msg) => {
                Problem::new(400, "ControlValidation", msg.clone())
            }
            Error::ControlEventOrder { .. } => {
                tracing::error!(error = %err, "invalid control event ordering");
                Problem::new(
                    500,
                    "InternalServerError",
                    "the control event stream is invalid",
                )
            }
            Error::ControlUninitialized => Problem::new(
                503,
                "ControlStoreUninitialized",
                "the control store is not initialized",
            ),
            Error::ControlRevisionConflict { expected, current } => Problem::new(
                409,
                "ControlRevisionConflict",
                format!("expected control revision {expected}, current revision is {current}"),
            ),
            Error::ControlIdempotencyConflict { key } => Problem::new(
                409,
                "ControlIdempotencyConflict",
                format!("idempotency key '{key}' was already used for another mutation"),
            ),
            Error::ControlIdempotencyAuthorizationConflict { key } => Problem::new(
                409,
                "ControlIdempotencyAuthorizationConflict",
                format!(
                    "idempotency key '{key}' was replayed by a different authenticated request"
                ),
            ),
            Error::ControlEntityVersionConflict {
                resource,
                expected,
                current,
            } => Problem::new(
                409,
                "ControlEntityVersionConflict",
                format!(
                    "resource '{resource}' expected entity version '{expected}', current version is '{current}'"
                ),
            ),
        }
    }

    /// Maps a `#34` `policy::authorize_resource` `Deny` verdict's credential
    /// to the problem body and status it belongs on — the mapping
    /// `tellurion-features` and `tellurion-stac` each used to carry a
    /// private, byte-identical copy of as their own `policy_denied`
    /// function: 401 when nothing was presented to authenticate at all
    /// (`Credential::None`), 403 when one was presented but doesn't
    /// authorize this resource. Never echoes `credential` into the returned
    /// `Problem`.
    pub fn for_denied_credential(credential: &Credential) -> Self {
        let (status, code) = match credential {
            Credential::None => (401, "Unauthorized"),
            Credential::Bearer(_) => (403, "Forbidden"),
        };
        Problem::new(
            status,
            code,
            "the presented credential does not authorize this resource",
        )
    }
}

fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in digits.bytes().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(char::from(byte));
    }
    formatted
}

/// The standard HTTP reason phrase for the status codes this server's error
/// paths actually use. Kept as a small local table (rather than pulling in
/// `http::StatusCode::canonical_reason`) so this crate stays framework-free.
fn reason_phrase(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    }
}

#[cfg(test)]
mod items_vertex_budget_tests {
    use super::*;

    #[test]
    fn exact_items_budget_refusal_is_a_named_422() {
        let problem = Problem::from_core_error(
            &Error::ItemsVertexBudgetExceeded {
                collection: "places".to_string(),
                feature_id: "large".to_string(),
                cumulative_vertices: 60_000,
                limit: 50_000,
            },
            "features",
        );

        assert_eq!(problem.status, 422);
        assert_eq!(problem.code, "ItemsVertexBudgetExceeded");
        assert!(problem.detail.contains("exact geometry"));
        assert!(problem.detail.contains("60,000"));
        assert!(problem.detail.contains("50,000"));
    }
}
