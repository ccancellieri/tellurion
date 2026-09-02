//! Turns a `tellurion_core::Error`/denied-credential into an axum `Response`.
//! The actual status/code/detail mapping lives in `tellurion_core::Problem`
//! (`from_core_error`/`for_denied_credential`) — this module is only the
//! thin axum-specific shim around it: `Problem`/`Error` are both foreign
//! types from this crate's perspective, so implementing `IntoResponse` for
//! either directly would violate the orphan rule; `ApiError` is the local
//! type that makes the impl legal.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use tellurion_core::problem::{Problem, PROBLEM_JSON};
use tellurion_core::{Credential, Error as CoreError, RateRefusal};

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub problem: Problem,
}

/// `#34`: the response `policy::authorize_resource`'s `Deny` verdict maps to.
/// See `Problem::for_denied_credential`'s own doc for the status/code rule.
pub fn policy_denied(credential: &Credential) -> ApiError {
    let problem = Problem::for_denied_credential(credential);
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::FORBIDDEN);
    ApiError { status, problem }
}

/// `#188`: the response a `policy::enforce_rate_limits` refusal maps to — a
/// `429` whose `Retry-After` header is derived from the problem body itself
/// in [`IntoResponse`] below, not attached here. See
/// `Problem::for_rate_limited`'s own doc for the body's wording and for what
/// it deliberately does not say.
pub fn policy_rate_limited(refusal: &RateRefusal) -> ApiError {
    let problem = Problem::for_rate_limited(refusal);
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::TOO_MANY_REQUESTS);
    ApiError { status, problem }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // `#188`: one place per crate attaches `Retry-After`, reading it back
        // off the body — see `Problem::retry_after`'s own doc for why it
        // travels there rather than through every error constructor. A
        // seconds count is always plain ASCII digits, so `from_str` cannot
        // actually fail; omitting the header rather than failing the whole
        // response is the same defensive rule `set_etag` follows.
        let retry_after = self.problem.retry_after;
        let mut response = (self.status, Json(self.problem)).into_response();
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
        if let Some(seconds) = retry_after {
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
        response
    }
}

impl From<CoreError> for ApiError {
    fn from(err: CoreError) -> Self {
        let problem = Problem::from_core_error(&err, "features");
        let status =
            StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        ApiError { status, problem }
    }
}
