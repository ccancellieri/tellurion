//! RFC 9457 problem responses for `tellurion-edr`.

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

impl ApiError {
    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::from(CoreError::Invalid(detail.into()))
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        let mut err = Self::from(CoreError::NotFound);
        err.problem.detail = detail.into();
        err
    }

    pub fn unsupported_media_type(detail: impl Into<String>) -> Self {
        let mut err = Self::from(CoreError::Invalid(detail.into()));
        err.status = StatusCode::UNSUPPORTED_MEDIA_TYPE;
        err.problem.status = StatusCode::UNSUPPORTED_MEDIA_TYPE.as_u16();
        err
    }
}

pub fn policy_denied(credential: &Credential) -> ApiError {
    let problem = Problem::for_denied_credential(credential);
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::FORBIDDEN);
    ApiError { status, problem }
}

pub fn policy_rate_limited(refusal: &RateRefusal) -> ApiError {
    let problem = Problem::for_rate_limited(refusal);
    let status = StatusCode::from_u16(problem.status).unwrap_or(StatusCode::TOO_MANY_REQUESTS);
    ApiError { status, problem }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
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
        let problem = Problem::from_core_error(&err, "edr");
        let status =
            StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        ApiError { status, problem }
    }
}
