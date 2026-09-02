//! Platform-admin inspection surface for webhook subscriptions (`#115`).
//! Subscription definitions are created and changed through the existing
//! compare-and-swap config endpoint; this module exposes the runtime-only
//! dead-letter ring, which cannot be reconstructed from the config document.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use tellurion_core::{AppContext, DeadLetterEntry};

use crate::app::problem_response;
use crate::webhook_consumer::WebhookRegistry;

#[derive(Debug, Default, Deserialize)]
pub struct DeadLetterQuery {
    since: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeadLetterPage {
    subscription: String,
    entries: Vec<DeadLetterEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next: Option<String>,
}

/// `GET /config/webhooks/{subscription}/dead-letters`: a bounded keyset
/// page over one running subscription's compact dead-letter envelopes.
pub async fn list_dead_letters(
    State(ctx): State<Arc<AppContext>>,
    Extension(registry): Extension<Arc<WebhookRegistry>>,
    Path(subscription): Path<String>,
    Query(query): Query<DeadLetterQuery>,
) -> Response {
    let Some(runtime) = registry.get(&subscription) else {
        return problem_response(
            StatusCode::NOT_FOUND,
            "WebhookSubscriptionNotRunning",
            format!("webhook subscription '{subscription}' is not running on this instance"),
        );
    };

    let conf = ctx.current().config.server.webhook_delivery;
    let limit = query.limit.unwrap_or(conf.dead_letter_default_page_size);
    if limit == 0 || limit > conf.dead_letter_max_page_size {
        return problem_response(
            StatusCode::BAD_REQUEST,
            "InvalidLimit",
            format!(
                "limit must be between 1 and {}",
                conf.dead_letter_max_page_size
            ),
        );
    }
    let (entries, next) = match runtime.dead_letters(query.since.as_deref(), limit) {
        Ok(page) => page,
        Err(error) => {
            return problem_response(StatusCode::BAD_REQUEST, "InvalidCursor", error.to_string())
        }
    };
    Json(DeadLetterPage {
        subscription,
        entries,
        next,
    })
    .into_response()
}
