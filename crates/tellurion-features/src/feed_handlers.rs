//! `GET /collections/{cid}/changes` (`#115`): the pull change feed over the
//! same outbox the write lane commits to and the search/tile-generation
//! consumers already drain (`tellurion_core::feed`). Read-only, cursor-paged
//! on the outbox sequence — never `OFFSET` — with a bounded per-request page
//! size (`ServerConfig.change_feed`).
//!
//! This module deliberately does not call `handlers.rs`'s own
//! `resolve_tenant_catalog`/`authorize_lane`/`extract_credential` helpers —
//! they are private to that module. Mirrors `write_handlers.rs`'s own
//! documented reasoning for the identical choice: the small handful this
//! file needs are reimplemented locally rather than exported from a file
//! this lane does not own.
//!
//! Policy: every request runs through `policy::authorize_resource` against
//! `PolicyLane::Feed` with `lane_supports_filter: false` always — the feed
//! serves compact envelopes (ids/sequences), never a payload a filter could
//! narrow, so a grant matches this lane outright or not at all (collection-
//! level allow/deny, the same treatment the write lane's own `PolicyLane::
//! Write` checkpoint gives `authorize_write_lane`). `config::validate_grant`
//! refuses a filter-carrying `feed` grant at config load, before any request
//! ever reaches here.

use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use tellurion_core::auth::Credential;
use tellurion_core::policy::{self, PolicyDecision, ResourceContext};
use tellurion_core::query_params::{parse_bounded_limit, percent_encode};
use tellurion_core::{
    feed, AppContext, Error as CoreError, PolicyLane, RateCharge, RateCounter, RateVerdict,
};

use crate::handlers::{DEFAULT_CATALOG, DEFAULT_TENANT};
use crate::model::Link;
use crate::problem::ApiError;

const JSON_MEDIA_TYPE: &str = "application/json";

/// Mirrors `handlers.rs`'s own private `extract_credential` — see the module
/// doc for why this is reimplemented here rather than imported.
fn extract_credential(headers: &HeaderMap) -> Credential {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Credential::None;
    };
    let Ok(value) = value.to_str() else {
        return Credential::None;
    };
    match value.strip_prefix("Bearer ") {
        Some(token) if !token.is_empty() => Credential::Bearer(token.to_string()),
        _ => Credential::None,
    }
}

fn tenant_of(params: &HashMap<String, String>) -> String {
    params
        .get("tenant")
        .cloned()
        .unwrap_or_else(|| DEFAULT_TENANT.to_string())
}

fn catalog_of(params: &HashMap<String, String>) -> String {
    params
        .get("catalog")
        .cloned()
        .unwrap_or_else(|| DEFAULT_CATALOG.to_string())
}

async fn resolve_tenant_catalog(
    ctx: &AppContext,
    params: &HashMap<String, String>,
) -> Result<(String, String), ApiError> {
    let state = ctx.current();
    let tenant_id = state.resolver.resolve_tenant(&tenant_of(params)).await?;
    let catalog_id = state
        .resolver
        .resolve_catalog(&tenant_id, &catalog_of(params))
        .await?;
    Ok((tenant_id, catalog_id))
}

fn require_param(params: &HashMap<String, String>, name: &str) -> Result<String, ApiError> {
    params
        .get(name)
        .cloned()
        .ok_or(CoreError::NotFound)
        .map_err(ApiError::from)
}

/// The `#34`/`#115` policy checkpoint — identical shape to
/// `write_handlers.rs`'s own `authorize_write_lane`, evaluated against
/// `PolicyLane::Feed` with `lane_supports_filter: false` always (see the
/// module doc).
async fn authorize_feed_lane(
    state: &tellurion_core::ContextState,
    rate_counter: &dyn RateCounter,
    headers: &HeaderMap,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
) -> Result<(), ApiError> {
    let Some(authorizer) = state.authorizer.as_ref() else {
        return Ok(());
    };
    let credential = extract_credential(headers);
    let subject = authorizer.subject(&credential).await;
    let visibility = state
        .router
        .effective_visibility(collection_id)
        .cloned()
        .unwrap_or_default();
    let resource = ResourceContext {
        tenant_id,
        catalog_id,
        collection_id,
        lane: PolicyLane::Feed,
        visibility: &visibility,
    };
    match policy::authorize_resource(&state.config, &resource, &subject, false)? {
        PolicyDecision::Allow { .. } => {}
        PolicyDecision::Deny => return Err(crate::problem::policy_denied(&credential)),
    }
    // `#188`: a feed poll is one served request, and a polling consumer is
    // exactly the traffic shape a ceiling exists to bound — so it charges.
    match policy::enforce_rate_limits(
        &state.config,
        &resource,
        &subject,
        Some(rate_counter),
        RateCharge::Charge,
    )
    .await
    {
        RateVerdict::Permitted => Ok(()),
        RateVerdict::Refused(refusal) => Err(crate::problem::policy_rate_limited(&refusal)),
    }
}

/// `GET /collections/{cid}/changes`'s own query parameters: `since` (the
/// previous page's `next` link's token, echoed back verbatim — this feed's
/// own name for the same keyset-cursor concept `ItemsQueryParams::token`/
/// `CollectionsQueryParams::token` already use elsewhere in this crate,
/// following `#115`'s own "an opaque `since` token" wording) and `limit`,
/// bounded by `ServerConfig.change_feed`.
#[derive(Debug, Deserialize, Default, Clone, PartialEq)]
pub struct ChangesQueryParams {
    pub limit: Option<u32>,
    pub since: Option<String>,
}

fn changes_href(path: &str, params: &ChangesQueryParams, override_token: Option<&str>) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if let Some(limit) = params.limit {
        pairs.push(("limit", limit.to_string()));
    }
    let token = override_token
        .map(str::to_string)
        .or_else(|| params.since.clone());
    if let Some(token) = token {
        pairs.push(("since", token));
    }
    if pairs.is_empty() {
        return path.to_string();
    }
    let query = pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(&v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{query}")
}

#[derive(Debug, Serialize)]
struct ChangesResponse {
    links: Vec<Link>,
    changes: Vec<feed::FeedEntry>,
}

/// `GET /collections/{cid}/changes`. Resolves the collection's write lane's
/// own `OutboxSource` (`Router::resolve_outbox` — the identical read side
/// the index applier and tile-generation consumer already drain, never a
/// second log), reads at most `limit` obligations strictly after `since`,
/// and reports them as compact, versioned envelopes. A collection with no
/// resolvable outbox (no `routing.write` at all, or a driver that never
/// provisioned one) refuses with the same named `CapabilityUnsupported` 404
/// every other capability-gated lane already gives.
pub async fn list_changes(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    Query(raw_query): Query<ChangesQueryParams>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;

    let (decl, outbox) = state
        .router
        .resolve_outbox(&tenant_id, &catalog_id, &collection_id)
        .await?;

    authorize_feed_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
    )
    .await?;

    let change_feed_config = state.config.server.change_feed;
    let limit = parse_bounded_limit(
        raw_query.limit,
        change_feed_config.default_page_size,
        change_feed_config.max_page_size,
    )
    .map_err(ApiError::from)?;
    let since = match &raw_query.since {
        Some(token) => feed::decode_cursor(token).map_err(ApiError::from)?,
        None => tellurion_core::Sequence(0),
    };

    let obligations = outbox
        .read_after(&decl, since, limit)
        .await
        .map_err(ApiError::from)?;
    // `#39`: the envelope's own `collection` field echoes the EXTERNAL id
    // (what the caller already sees in the URL), never the internal one —
    // see `handlers.rs`'s own module doc for why an internal id is never
    // serialized.
    let page = feed::build_page(decl.external_id(), &obligations, limit);

    let path = uri.path().to_string();
    let mut links = vec![Link::new(
        changes_href(&path, &raw_query, None),
        "self",
        JSON_MEDIA_TYPE,
    )];
    if let Some(next_token) = page.next.as_deref() {
        links.push(Link::new(
            changes_href(&path, &raw_query, Some(next_token)),
            "next",
            JSON_MEDIA_TYPE,
        ));
    }

    metrics::counter!(
        "change_feed_requests_total",
        "collection" => decl.id.clone()
    )
    .increment(1);
    metrics::counter!(
        "change_feed_entries_served_total",
        "collection" => decl.id.clone()
    )
    .increment(page.entries.len() as u64);

    let body = ChangesResponse {
        links,
        changes: page.entries,
    };
    let mut response = (StatusCode::OK, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(JSON_MEDIA_TYPE),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_href_with_no_params_is_the_bare_path() {
        let params = ChangesQueryParams::default();
        assert_eq!(
            changes_href("/collections/demo/changes", &params, None),
            "/collections/demo/changes"
        );
    }

    #[test]
    fn changes_href_carries_limit_and_since() {
        let params = ChangesQueryParams {
            limit: Some(50),
            since: Some("10".to_string()),
        };
        assert_eq!(
            changes_href("/collections/demo/changes", &params, None),
            "/collections/demo/changes?limit=50&since=10"
        );
    }

    #[test]
    fn changes_href_override_token_replaces_the_query_since() {
        let params = ChangesQueryParams {
            limit: None,
            since: Some("10".to_string()),
        };
        assert_eq!(
            changes_href("/collections/demo/changes", &params, Some("20")),
            "/collections/demo/changes?since=20"
        );
    }
}
