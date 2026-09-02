//! Authenticated configuration-mutation control lane (`#110`): reads and
//! compare-and-swap-writes the RAW whole `AppConfig` document this
//! instance's `AppContext::config_store` manages — distinct from the
//! read-only, provenance-tagged *merged settings* view `config_view.rs`
//! serves (that one answers "what value applies here, and why"; this one
//! answers "let me see, and change, the actual document"). Gated entirely
//! by `enforce_platform_admin_auth` (`app.rs`) — see that middleware's own
//! doc for why an absent/disabled `auth:` renders this indistinguishable
//! from an unregistered route.
//!
//! ```text
//! GET  /config     -> the current raw document, with its ConfigVersion in
//!                     the `X-Config-Version` response header
//! PUT  /config     -> apply (or, with `?dry_run=true`, only validate) a
//!                     replacement document
//! ```
//!
//! **Mutation contract.** `PUT` always carries the FULL replacement
//! document (never a partial patch) plus an `X-Config-Expected-Version`
//! header naming the version the caller last read — the exact same
//! compare-and-swap contract `ConfigStore::write` implements. The whole
//! candidate document is validated (`AppConfig::validate` — uniqueness
//! scopes, reserved segments, referential integrity, profile/`final`-key
//! shape; the identical function `ConfigStore::load`/`write`, boot, and
//! `#47`'s reload all call) before it is ever persisted: a bad edit is
//! refused, named, and the old document keeps serving. A version mismatch
//! is a named `409` (`ConfigVersionConflict`), never a silently-applied
//! lost update.
//!
//! This endpoint deliberately stops at `AppConfig::validate` and does not
//! additionally rebuild a `Router` to repeat `main`'s/`reload.rs`'s own
//! live driver-connectivity sweep (`Router::validate_catalog`, run under
//! `registry.validation: eager`) — doing so here would mean this rarely-
//! used control-lane request making its own storage-backend network calls,
//! a materially different risk/latency shape than a shape-and-reference
//! check. A candidate that passes here but would fail that sweep (e.g. a
//! collection naming a table that doesn't exist) is still caught safely:
//! the `#47` reload pipeline this write feeds (see "Propagation" below)
//! re-validates fully before ever swapping, logs the failure, and leaves
//! the previous config serving — the operator learns this from the reload
//! log / the config-version gauge failing to advance, rather than from
//! this endpoint's own response. Closing that gap (having this endpoint
//! attempt the same live sweep) is a reasonable follow-up, not done here.
//!
//! **Propagation, not application.** This module never touches the live
//! `AppContext` state directly — it only asks `ConfigStore::write` to
//! persist the new document to whatever backing store this instance uses.
//! Applying the change to this (and every other) instance's live routing
//! is the existing `#47` reload pipeline's job: for the file backend, the
//! write above is a real file write, which the already-running
//! `reload::run` file-watch trigger picks up within its own debounce
//! window (see that module's own documented staleness bound). A mutation
//! and a hand edit are, from the reload pipeline's point of view, the same
//! kind of event.
//!
//! That sameness now includes `#260`'s unchanged-document guard, and
//! including it is deliberate: a write whose persisted bytes are
//! byte-for-byte what is already serving leaves the pipeline nothing to
//! activate, so it activates nothing. The `200` below stays truthful
//! because it never claimed an activation — it reports the version now
//! persisted, which in that case is the version already running, and this
//! module's contract has always been "persisted", with convergence
//! observed through the config-version gauge rather than asserted in this
//! response. Skipping an activation therefore needs no new response shape
//! here; inventing one (a "nothing to do" status) would report on a step
//! this handler does not perform and cannot wait for. The case is narrow
//! in practice: `ConfigStore::write` persists a re-serialization of the
//! candidate, so a document differing from the file only in comments or
//! key order still produces different bytes and still activates. Only a
//! candidate that re-serializes to exactly the current file's bytes is
//! declined, and declining it is the right answer.
//!
//! **Dry run.** `?dry_run=true` runs the identical validation and reports
//! the verdict — `{ "valid": true }` or `{ "valid": false, "detail":
//! "..." }` — without ever calling `ConfigStore::write`, and without
//! requiring `X-Config-Expected-Version` at all (a dry run doesn't touch
//! the version). A dry run intentionally answers with `200`, not an error
//! status, regardless of the verdict: the request to "tell me whether this
//! would apply" itself always succeeds; only the (non-dry-run) attempt to
//! actually apply an invalid document is a `422`.
//!
//! **Audit.** Every successfully applied (non-dry-run) write appends one
//! record to `AppContext::audit_log` — principal (from
//! `enforce_platform_admin_auth`'s own `PlatformAdminPrincipal` request
//! extension), timestamp, expected/new version, and a shallow, bounded
//! summary of which top-level config sections changed (`summarize_change`)
//! — never the whole before/after document.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use tellurion_core::{
    AppConfig, AppContext, ConfigVersion, ControlScope, Error, WebhookSubscriptionDecl,
};

use crate::app::{problem_response, PlatformAdminPrincipal};

/// Carries the compare-and-swap token from a prior `GET`/`PUT` response
/// into a subsequent `PUT` — a plain header rather than a wrapper JSON
/// body field, so the request body stays exactly the candidate `AppConfig`
/// document with nothing else mixed in.
const EXPECTED_VERSION_HEADER: &str = "x-config-expected-version";
/// Mirrors `EXPECTED_VERSION_HEADER` back on a successful `GET`'s response,
/// so a client's read-then-write round trip never has to parse the body
/// just to learn the version it read.
const CURRENT_VERSION_HEADER: &str = "x-config-version";

#[derive(Debug, Deserialize)]
pub struct MutationQuery {
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Serialize)]
struct DryRunVerdict {
    valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Debug, Serialize)]
struct WriteResult {
    version: String,
}

#[derive(Debug, Serialize)]
pub struct WebhookList {
    subscriptions: Vec<WebhookSubscriptionDecl>,
}

/// `GET /config/webhooks` (`#115`): the subscriptions in the currently
/// applied config generation. Creation and edits continue to use the same
/// compare-and-swap `PUT /config` contract as every other config resource;
/// this bounded projection makes subscriptions directly enumerable without
/// exposing unrelated configuration sections.
pub async fn list_webhooks(State(ctx): State<Arc<AppContext>>) -> Json<WebhookList> {
    Json(WebhookList {
        subscriptions: ctx.current().config.webhooks.clone(),
    })
}

/// `GET /config` (`#110`): the current raw document plus its version.
/// `404` when this instance has no writable `ConfigStore` attached — see
/// `AppContext::config_store`'s own doc for why that, not an empty
/// document, is the honest answer.
pub async fn get_raw_config(State(ctx): State<Arc<AppContext>>) -> Response {
    let Some(store) = ctx.config_store.as_ref() else {
        return no_config_store_response();
    };
    match store.load_versioned() {
        Ok(versioned) => {
            let mut response = Json(versioned.config).into_response();
            if let Ok(value) = HeaderValue::from_str(&versioned.version.to_string()) {
                response.headers_mut().insert(CURRENT_VERSION_HEADER, value);
            }
            response
        }
        Err(error) => {
            tracing::error!(%error, "config mutation: failed to read the current config document");
            problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                "failed to read the current configuration document",
            )
        }
    }
}

/// `PUT /config` (`#110`): validate (dry run) or validate-then-apply a
/// replacement document — see this module's own doc for the full
/// contract.
pub async fn put_config(
    State(ctx): State<Arc<AppContext>>,
    Query(query): Query<MutationQuery>,
    Extension(principal): Extension<PlatformAdminPrincipal>,
    // `#215`: the administrative checkpoint's own decision for this request,
    // absent when no declared statement mentions this path. Read here rather
    // than re-derived, so the audit record names the decision that actually
    // let the write through and not a second one computed later.
    authorization: Option<Extension<crate::control_checkpoint::ControlAuthorization>>,
    headers: HeaderMap,
    Json(candidate): Json<AppConfig>,
) -> Response {
    if query.dry_run {
        return match candidate.validate() {
            Ok(()) => (
                StatusCode::OK,
                Json(DryRunVerdict {
                    valid: true,
                    detail: None,
                }),
            )
                .into_response(),
            Err(error) => (
                StatusCode::OK,
                Json(DryRunVerdict {
                    valid: false,
                    detail: Some(error.to_string()),
                }),
            )
                .into_response(),
        };
    }

    let Some(store) = ctx.config_store.as_ref() else {
        return no_config_store_response();
    };

    let Some(expected_header) = headers.get(EXPECTED_VERSION_HEADER) else {
        return problem_response(
            StatusCode::BAD_REQUEST,
            "MissingExpectedVersion",
            format!(
                "a non-dry-run PUT must carry the '{EXPECTED_VERSION_HEADER}' header naming the version this write is conditioned on"
            ),
        );
    };
    let Ok(expected_str) = expected_header.to_str() else {
        return problem_response(
            StatusCode::BAD_REQUEST,
            "MalformedExpectedVersion",
            format!("'{EXPECTED_VERSION_HEADER}' must be a UTF-8 header value"),
        );
    };
    let expected = ConfigVersion::from_wire(expected_str);

    // Best-effort "before" snapshot for the audit summary — never blocks
    // the write itself: `ConfigStore::write` performs its own independent
    // read-and-compare regardless, so a failure here only means a less
    // specific summary, not a less correct write.
    let before = store
        .load_versioned()
        .ok()
        .map(|versioned| versioned.config);

    match store.write(&expected, &candidate) {
        Ok(new_version) => {
            let summary = before
                .as_ref()
                .map(|before| summarize_change(before, &candidate))
                .unwrap_or_else(|| {
                    "unable to compute a change summary (the prior document could not be read)"
                        .to_string()
                });
            // `#215` acceptance criterion: principal, effective scope,
            // decision context and revision, on every administrative
            // mutation. `not_engaged` is the honest answer for a deployment
            // that declared no statements — the platform-admin gate alone
            // authorised this write, and the record says so.
            let decision = authorization
                .as_ref()
                .map(|Extension(authorization)| authorization.0.summary())
                .unwrap_or_else(|| "not_engaged".to_string());
            let effective_scope = authorization
                .as_ref()
                .map(|Extension(authorization)| authorization.0.scope.clone())
                .unwrap_or_else(|| ControlScope::Platform.resource_key());
            ctx.audit_log.record(
                principal.0,
                expected.to_string(),
                new_version.to_string(),
                summary,
                effective_scope,
                decision,
            );
            (
                StatusCode::OK,
                Json(WriteResult {
                    version: new_version.to_string(),
                }),
            )
                .into_response()
        }
        Err(Error::VersionConflict { expected, current }) => problem_response(
            StatusCode::CONFLICT,
            "ConfigVersionConflict",
            format!("expected config version '{expected}' but the current version is '{current}'"),
        ),
        Err(Error::Config(message)) => problem_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "InvalidConfiguration",
            message,
        ),
        Err(error) => {
            tracing::error!(%error, "config mutation: failed to write the new config document");
            problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                "failed to write the new configuration document",
            )
        }
    }
}

fn no_config_store_response() -> Response {
    problem_response(
        StatusCode::NOT_FOUND,
        "NotFound",
        "this instance has no writable configuration store attached",
    )
}

/// A shallow, bounded summary of which top-level `AppConfig` sections
/// differ between `before` and `after` — never a field-by-field or
/// byte-level diff (unbounded, and largely redundant with the version
/// tokens the audit record already carries). Every section named here is a
/// fixed field of `AppConfig` itself, so this can never grow unboundedly
/// regardless of how large a candidate document is.
fn summarize_change(before: &AppConfig, after: &AppConfig) -> String {
    let mut changed = Vec::new();
    if before.server != after.server {
        changed.push("server");
    }
    if before.cache != after.cache {
        changed.push("cache");
    }
    if before.storages != after.storages {
        changed.push("storages");
    }
    if before.object_stores != after.object_stores {
        changed.push("object_stores");
    }
    if before.tenants != after.tenants {
        changed.push("tenants");
    }
    if before.catalogs != after.catalogs {
        changed.push("catalogs");
    }
    if before.collections != after.collections {
        changed.push("collections");
    }
    if before.styles != after.styles {
        changed.push("styles");
    }
    if before.profiles != after.profiles {
        changed.push("profiles");
    }
    if before.settings != after.settings {
        changed.push("settings");
    }
    if before.auth != after.auth {
        changed.push("auth");
    }
    if before.registry != after.registry {
        changed.push("registry");
    }
    if before.policy != after.policy {
        changed.push("policy");
    }
    if before.webhooks != after.webhooks {
        changed.push("webhooks");
    }
    if changed.is_empty() {
        "no top-level section differs".to_string()
    } else {
        changed.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config() -> AppConfig {
        serde_yaml::from_str("storages: []").unwrap()
    }

    #[test]
    fn summarize_change_reports_no_change_for_identical_documents() {
        let config = base_config();
        assert_eq!(
            summarize_change(&config, &config),
            "no top-level section differs"
        );
    }

    #[test]
    fn summarize_change_names_every_changed_top_level_section() {
        let before = base_config();
        let after: AppConfig = serde_yaml::from_str(
            r#"
storages: []
tenants: [ { id: public } ]
settings: { cache_ttl_s: 99 }
"#,
        )
        .unwrap();

        let summary = summarize_change(&before, &after);
        assert!(summary.contains("settings"), "summary was: {summary}");
        assert!(summary.contains("tenants"), "summary was: {summary}");
        assert!(!summary.contains("storages"), "summary was: {summary}");
    }

    /// `#115`: a webhook-subscription edit is named in the audit summary
    /// like every other top-level section — before this, `webhooks` was
    /// missing from `summarize_change`'s own field list entirely, so a
    /// mutation that only added/removed a subscription would have audited
    /// as "no top-level section differs", an honest-looking but wrong
    /// record of what the write actually changed.
    #[test]
    fn summarize_change_names_a_changed_webhooks_section() {
        let before = base_config();
        let after: AppConfig = serde_yaml::from_str(
            r#"
storages: []
webhooks:
  - id: alerts
    url: https://example.test/hook
    secret_env: ALERTS_WEBHOOK_SECRET
"#,
        )
        .unwrap();

        let summary = summarize_change(&before, &after);
        assert!(summary.contains("webhooks"), "summary was: {summary}");
    }
}
