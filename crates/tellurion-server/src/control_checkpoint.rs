//! The one administrative authorization checkpoint (`#215`).
//!
//! `tellurion_core::control_policy` decides; this module is the only thing
//! that asks it. Handlers contain no role checks, no scope arithmetic and no
//! policy lookups — the issue's "apply authorization in one
//! middleware/service, never inline in handlers" is enforced by there being
//! exactly one call to `ControlPolicySet::authorize` in this workspace, and
//! it is below.
//!
//! # What it does, in order
//!
//! 1. **Nothing at all** when the active snapshot declares no statements.
//!    Not "decode and then find no policy" — nothing: no decoding, no
//!    resolve, no subject derivation. That is the literal meaning of "a
//!    deployment that declared no path scopes authorises exactly what it
//!    authorises today," and it is a `return` rather than a comment.
//! 2. **Decode** the raw request-line path
//!    ([`decoded_segments`]) with axum's own single-pass rule, so the string
//!    this layer decides about is the string the handler will serve. See
//!    `control_path`'s own doc for why agreement, not rejection, is what
//!    closes the encoded-separator / dot-segment / alias class.
//! 3. **Classify** it against the closed administrative route table
//!    ([`AdminResource`]). Anything not in that table passes straight
//!    through — every data-plane path included.
//! 4. **Resolve** every external id to its internal one, each within its
//!    parent — `resolve_catalog(tenant, …)` and
//!    `resolve_collection(catalog, …)` *are* the ownership check, which is
//!    why there is no separate one to forget.
//! 5. **Authorize**, then attach the decision context for the audit trail.
//!
//! Every step from 2 to 4 that cannot produce an answer — a path that does
//! not decode, a shape that is not administrative, an id that does not
//! resolve under its parent — leaves the request exactly as it was and lets
//! whatever answered it before answer it again. This layer adds refusals; it
//! never adds a new kind of failure.
//!
//! # What a refusal discloses, and why
//!
//! Decided per case, following the precedents this repository already set.
//!
//! - **An unresolvable or wrongly-parented id** is not answered here at all:
//!   this layer passes the request through and the handler's own `404`
//!   stands, byte-identical to the one an unknown id already produced.
//!   `app::enforce_collection_kind` (`#192`) answers a bare `404` for the
//!   same reason — naming the resource ahead of the policy checkpoint would
//!   disclose it — and the cheapest way to be identical to that answer is to
//!   let the same code produce it.
//! - **A policy deny at platform or tenant scope** answers `403`. Both of
//!   those callers have already crossed the gate that owns the resource's
//!   existence: the platform lane's own `401`/`403` (`#110`) already
//!   distinguishes them, and a tenant-scoped caller passed
//!   `enforce_tenant_auth`, so the tenant is not news. A `404` here would
//!   additionally be a regression — `#110` answers `403` on this surface
//!   today, and a deployment's existing answers may not change.
//! - **A policy deny at catalog or collection scope** answers a bare `404`,
//!   indistinguishable from the one an unknown catalog or collection
//!   already produces. This is the case where `403`-versus-`404` is itself
//!   the leak: delegated administration exists precisely so a catalog
//!   administrator cannot see the tenant's other catalogs, and a `403` would
//!   let them enumerate every sibling by probing. The honesty cost is real
//!   and accepted: an authorised operator who mistypes a catalog id gets the
//!   same `404` as an unauthorised one who guesses right. It is bounded,
//!   though — the `404` is reachable only on a path an operator's own
//!   statement brought under policy.
//!
//! No refusal names the statement, the role or the scope that produced it.
//! Deriving the response from the caller's grants is exactly the grant
//! oracle `#208` refused when it kept the `Allow` header
//! subject-independent; the same trap is available here and is declined the
//! same way. [`ControlDecisionContext`] exists, carries all of that, and
//! reaches only the audit record — which the refused caller cannot read.

use std::sync::Arc;

use axum::extract::{OriginalUri, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use tellurion_core::control_path::decoded_segments;
use tellurion_core::control_policy::{
    AdminResource, ControlDecision, ControlDecisionContext, ControlRequestContext,
};
use tellurion_core::{AppContext, ControlScope, PrincipalIdentity};

use crate::app::{extract_credential, problem_response};

/// The decision this checkpoint reached for the current request, attached as
/// a request extension so an administrative mutation can record *why* it was
/// allowed without asking the policy engine a second question and possibly
/// getting a second answer.
///
/// Present only on a request the checkpoint actually decided: absent means
/// [`ControlDecision::NotEngaged`], which the audit trail records as such
/// rather than inventing a scope nobody declared.
#[derive(Debug, Clone)]
pub(crate) struct ControlAuthorization(pub ControlDecisionContext);

/// The checkpoint. Registered on every router that holds an administrative
/// route and on no other, so a request that reaches it has already matched
/// an administrative route template — which is what makes the `400` in step
/// 2 unambiguous and keeps it off every data-plane path.
///
/// Registered as the innermost of its router's layers, so the tenant and
/// platform-admin trust boundaries are crossed first. Running before them
/// would let an unauthenticated caller probe the policy document by reading
/// `403`-versus-`401`, and would let this layer pre-empt a `401` that a
/// deployment already answers — both of which are the disclosure this
/// module exists to avoid.
pub(crate) async fn enforce_control_policy(
    State(ctx): State<Arc<AppContext>>,
    OriginalUri(uri): OriginalUri,
    mut request: Request,
    next: Next,
) -> Response {
    let state = ctx.current();
    // Step 1. The whole of `#215` for a deployment that declared nothing.
    if state.control_policy.is_empty() {
        return next.run(request).await;
    }

    // Step 2. The raw request-line path, decoded exactly as axum decodes
    // it — see this module's own doc for why agreement matters more than
    // strictness here, and why a path that does not decode is passed through
    // rather than refused.
    let Some(segments) = decoded_segments(uri.path()) else {
        return next.run(request).await;
    };

    // Step 3.
    let Some(resource) = AdminResource::of(&segments) else {
        return next.run(request).await;
    };

    // Step 4. Each id resolved within its parent; an id that does not
    // resolve there is left to the handler's own `404`.
    let (tenant_ext, catalog_ext, collection_ext) = resource.external_ids();
    let mut tenant_id = None;
    let mut catalog_id = None;
    let mut collection_id = None;
    if let Some(external) = tenant_ext {
        let Ok(resolved) = state.resolver.resolve_tenant(external).await else {
            return next.run(request).await;
        };
        tenant_id = Some(resolved);
    }
    if let Some(external) = catalog_ext {
        let tenant = tenant_id.as_deref().unwrap_or_default();
        let Ok(resolved) = state.resolver.resolve_catalog(tenant, external).await else {
            return next.run(request).await;
        };
        catalog_id = Some(resolved);
    }
    if let Some(external) = collection_ext {
        let catalog = catalog_id.as_deref().unwrap_or_default();
        let Ok(resolved) = state.resolver.resolve_collection(catalog, external).await else {
            return next.run(request).await;
        };
        collection_id = Some(resolved);
    }

    let control_request = resource.resolve(
        request.method().as_str(),
        tenant_id.as_deref(),
        catalog_id.as_deref(),
        collection_id.as_deref(),
    );

    // Step 5. The authenticated identity, or none. A credential that
    // establishes no verified `(issuer, subject)` pair — an unnamed static
    // token, an unverifiable JWT, no header at all — holds no bindings and
    // therefore contributes no roles; the precedence rule handles it with no
    // special case, which is why there is none here either.
    let identity: Option<PrincipalIdentity> = match state.authorizer.as_ref() {
        Some(authorizer) => {
            let credential = extract_credential(request.headers());
            authorizer.subject(&credential).await.identity
        }
        None => None,
    };

    match state
        .control_policy
        .authorize(identity.as_ref(), &control_request)
    {
        ControlDecision::NotEngaged => next.run(request).await,
        ControlDecision::Allow(context) => {
            request
                .extensions_mut()
                .insert(ControlAuthorization(context));
            next.run(request).await
        }
        ControlDecision::Deny(context) => {
            // The one place the decision context is allowed to be seen by an
            // operator: a server-side log line, never the response.
            tracing::debug!(
                decision = %context.summary(),
                canonical_path = %control_request.canonical_path_string(),
                "administrative request refused by path policy"
            );
            refusal(&control_request)
        }
    }
}

/// The refusal shape for a denied request, chosen per scope — see this
/// module's own doc for the reasoning behind each one. Deliberately a
/// function of the target scope alone: it reads nothing from the caller's
/// grants, so no caller can shape it into an oracle.
fn refusal(request: &ControlRequestContext) -> Response {
    match request.scope {
        ControlScope::Platform => problem_response(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "the presented credential does not authorize this administrative resource",
        ),
        ControlScope::Tenant { .. } => problem_response(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "the presented credential does not authorize this administrative resource",
        ),
        // Bare, bodiless — byte-identical to the `404` an unknown catalog or
        // collection already produces from `config_view`.
        ControlScope::Catalog { .. } | ControlScope::Collection { .. } => {
            StatusCode::NOT_FOUND.into_response()
        }
    }
}
