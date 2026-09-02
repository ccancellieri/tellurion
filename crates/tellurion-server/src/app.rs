//! Wires the route tree, the metrics endpoint, and the request-shaping
//! middleware into one axum service.
//!
//! Route tree (`#39`):
//!
//! ```text
//! /                                                   minimal service descriptor (no tenants listed)
//! /metrics                                            top-level, unaffected by tenancy
//! /healthz                                            process liveness
//! /readyz                                             bounded dependency readiness
//! /config/effective                                   platform effective-config view (`#110`)
//! /config/profiles                                    named settings profiles (`#111`)
//! /config                                             GET/PUT raw config document, platform-admin gated (`#110`)
//! /{tenant}/                                          tenant directory doc
//! /{tenant}/config/effective                          tenant effective-config view (`#110`)
//! /{tenant}/config/catalogs/{catalog}/effective        catalog effective-config view (`#110`)
//! /{tenant}/config/catalogs/{catalog}/collections/{cid}/effective  collection effective-config view (`#110`)
//! /{tenant}/features/catalogs/{catalog}/...           full OGC API Features root
//! /{tenant}/tiles/catalogs/{catalog}/...              full OGC API Tiles root
//! /{tenant}/styles/catalogs/{catalog}/...             full OGC API Styles root
//! /{tenant}/3dtiles/catalogs/{catalog}/...             full 3D Tiles root
//! /{tenant}/stac/catalogs/{catalog}/...                STAC API root (`#36`, core + collections)
//! ```
//!
//! The effective-config view (`#110`, read-only slice — see
//! `config_view.rs`'s own module doc for the full contract) is not a
//! protocol root: no `/`, `/conformance`, `/api` scaffolding, just the one
//! `GET .../effective` resource per node depth.
//!
//! Every `/{tenant}/{protocol}/catalogs/{catalog}` prefix is a complete API
//! root — its own `/`, `/conformance`, `/api`, plus that protocol crate's
//! own resource router, all layered with `Extension(Protocol)` so
//! `landing.rs`/`openapi.rs` know which root a request landed in. The STAC
//! root's `/` is the one exception: `stac_root` (not `protocol_root`) wires
//! it to `landing::stac_landing` instead of the generic OGC API Common
//! landing page every other protocol shares, since STAC API - Core mandates
//! a different document shape (a STAC Catalog object).
//! `tenant`/`catalog` path segments are EXTERNAL ids; the protocol crates
//! resolve them to internal ones through `AppContext::current().resolver`
//! before ever touching `Router` — see each crate's own handler docs. A
//! tenant external id that collides with `/metrics` (or any other top-level
//! literal segment) can never actually route there — axum matches a literal
//! segment ahead of a `{tenant}` capture at the same level — which is
//! exactly why `AppConfig::validate` refuses that external id at boot
//! instead: the alternative is a tenant whose entire route tree is silently
//! unreachable.
//!
//! Which of those roots a given `(tenant, catalog)` actually serves is not
//! fixed at build time (`#185`): `settings.protocols` rides the ordinary
//! platform -> tenant -> catalog settings chain, and every root is wrapped
//! in [`enforce_protocol_exposure`] so a disabled protocol answers `404`
//! (the prefix ceases to exist) and a disabled Features write lane answers
//! `405` with a truthful `Allow` (the reads on those same URIs keep
//! serving). The route TOPOLOGY still stays static across a reload — only
//! `ContextState` is swapped — exactly like `enforce_tenant_auth` and
//! `enforce_platform_admin_auth` above; the matrix is read per request from
//! the current snapshot's `Router`, where it was materialized per catalog at
//! load time.
//!
//! Middleware order, outermost first: bounded request observation, trace,
//! plain-OPTIONS `Allow` responder, CORS, compression, error mapping,
//! load-shed, concurrency limit, request timeout, panic isolation.
//!
//! The plain-OPTIONS responder sits just outside `cors` deliberately:
//! tower_http's `CorsLayer` answers every `OPTIONS` request itself —
//! including one with no `Origin` header at all — and never forwards it to
//! the router underneath, so a route-level `.options(...)` handler would be
//! unreachable dead code as long as `cors` wraps the router. See
//! [`respond_to_plain_options_on_write_resources`] for the OGC API Features
//! Part 4 requirement this exists to satisfy and why it only claims the
//! narrow case `cors` doesn't already own.
//!
//! The concurrency-limit/timeout pair is the operational rule "queue, then
//! shed ahead of the storage pool": `load_shed` rejects immediately (503)
//! rather than queuing once `concurrency_limit`'s bounded slots are
//! exhausted, and `timeout` is the 60s hard ceiling from
//! `server.request_timeout_s`. The concurrency ceiling is derived from the
//! backends' reported capacity (else cgroup-aware CPU parallelism, see
//! `tellurion_core::resources::effective_cpu_count`) unless
//! `server.max_concurrency` pins it explicitly.
//!
//! `CatchPanicLayer` sits innermost, wrapping the protocol router directly:
//! the release profile keeps `panic = "unwind"` (the workspace default)
//! rather than `"abort"` precisely so a handler panic can be caught here and
//! turned into one honest 500 instead of killing every other in-flight
//! request on this multi-tenant server. Placing it inside `concurrency_limit`
//! means a caught panic completes that request's future normally, so the
//! limiter's slot is released the same way it would be for any other
//! response — no special-casing needed there.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::error_handling::HandleErrorLayer;
use axum::extract::{OriginalUri, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use metrics_exporter_prometheus::PrometheusHandle;
use tower::{BoxError, ServiceBuilder};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use tellurion_core::auth::{AuthDecision, Credential, DenyReason, PlatformAdminDecision};
use tellurion_core::problem::{Problem, PROBLEM_JSON};
use tellurion_core::{effective_cpu_count, AppContext, CollectionKind};

use crate::protocol::{Protocol, RootAvailability};
use crate::readiness::Readiness;
use crate::webhook_consumer::WebhookRegistry;
use crate::{
    config_mutation, config_view, landing, metrics as srv_metrics, openapi, webhook_admin,
};

/// How much headroom the concurrency limiter gives itself above a backend's
/// reported capacity: most tile requests are cache hits that never touch a
/// pool, so admitting more than the pool's connection count is correct, but
/// staying within a small multiple of it keeps the limiter in the same order
/// of magnitude as what the backend can actually sustain — the gap that let
/// DB-bound traffic queue at the pool instead of shedding at the edge.
const BACKEND_ADMISSION_MULTIPLIER: usize = 8;

/// Bounded concurrency ahead of the storage pool. When every registered
/// storage reports a capacity hint (e.g. the PostGIS pool's connection
/// count), the limit is derived from that combined capacity so the two knobs
/// stay coherent; otherwise it falls back to the cores-only heuristic the
/// PostGIS pool itself used to follow alone.
fn derive_max_concurrency(backend_capacity_hint: Option<usize>) -> usize {
    match backend_capacity_hint {
        Some(capacity) => capacity
            .saturating_mul(BACKEND_ADMISSION_MULTIPLIER)
            .clamp(64, 4096),
        // No pooled backend to size against (e.g. a file-backed-only
        // deployment): fall back to cgroup-aware CPU parallelism, the same
        // "derived" tier `tellurion-postgis`'s own pool sizing uses, so a
        // throttled container doesn't derive a ceiling sized for the whole
        // host.
        None => (effective_cpu_count() * 64).clamp(64, 4096),
    }
}

async fn service_root() -> Response {
    landing::service_descriptor().await.into_response()
}

#[cfg(all(feature = "public-demo", feature = "ui"))]
async fn public_demo_root() -> Response {
    axum::response::Redirect::temporary("/ui/").into_response()
}

/// Panics unconditionally; exists only so tests below can exercise
/// `CatchPanicLayer`. The explicit return type keeps the never-type coercion
/// out of type inference (edition 2024 disallows relying on `!`-to-`()`
/// fallback here).
#[cfg(test)]
async fn always_panics() -> StatusCode {
    panic!("deliberate panic for the catch-panic test")
}

#[cfg(test)]
async fn slow_for_observation_test() -> StatusCode {
    tokio::time::sleep(Duration::from_millis(25)).await;
    StatusCode::OK
}

#[cfg(test)]
async fn timeout_for_observation_test() -> StatusCode {
    tokio::time::sleep(Duration::from_secs(2)).await;
    StatusCode::OK
}

/// Builds one full OGC API root — `/`, `/conformance`, `/api`, plus
/// `resource_router` (that protocol crate's own paths, e.g.
/// `/collections/...`) — and layers `Extension(protocol)` so `landing.rs`/
/// `openapi.rs` answer for the right protocol. The caller `.nest()`s the
/// result under `/{protocol_segment}/catalogs/{catalog}`. Not used for the
/// STAC root — see [`stac_root`], which needs a different `/` handler.
fn protocol_root(
    ctx: &Arc<AppContext>,
    protocol: Protocol,
    resource_router: Router<Arc<AppContext>>,
) -> Router<Arc<AppContext>> {
    let root = resource_router
        .route("/", get(landing::protocol_landing))
        .route("/conformance", get(landing::protocol_conformance))
        .route("/api", get(openapi::api_doc))
        .layer(Extension(protocol));
    let root = gate_on_collection_kind(ctx, protocol, root);
    gate_on_protocol_exposure(ctx, protocol, root)
}

/// Wraps one assembled protocol root in the `#185` exposure gate, with
/// `protocol` captured by closure — see [`enforce_protocol_exposure`]'s own
/// doc for why it cannot be extracted from the request instead. Must stay
/// the LAST thing applied to the root, so nothing added afterwards escapes
/// the gate.
///
/// `layer`, deliberately, not `route_layer`: axum's `route_layer` wraps only
/// the method handlers a route actually declares, so a request the
/// `MethodRouter` answers on its own — an `OPTIONS` with no handler, or a
/// `405` on an unregistered method — would slip past it and reveal a root
/// this catalog does not serve. A plain `layer` covers those too, and the
/// gate is written to pass any request it cannot resolve a catalog for
/// straight through, so nothing that used to 404 now answers differently.
fn gate_on_protocol_exposure(
    ctx: &Arc<AppContext>,
    protocol: Protocol,
    root: Router<Arc<AppContext>>,
) -> Router<Arc<AppContext>> {
    root.layer(axum::middleware::from_fn_with_state(
        Arc::clone(ctx),
        move |State(ctx): State<Arc<AppContext>>,
              OriginalUri(uri): OriginalUri,
              request: Request,
              next: Next| async move {
            enforce_protocol_exposure(ctx, protocol, uri.path().to_string(), request, next).await
        },
    ))
}

/// Tenant trust-boundary enforcement (`#17`): layered onto `tenant_scope` —
/// every route nested under `/{tenant}` — so a request never reaches a
/// protocol handler's own `Resolver::resolve_tenant` call without first
/// passing whatever `TenantAuthorizer` the current config selects. Reserved
/// top-level segments (`/`, `/metrics`, `/ui`) live outside `tenant_scope`
/// and never see this layer at all — see `config::RESERVED_TENANT_SEGMENTS`.
///
/// Absent `auth:` config (`ctx.current().authorizer` is `None`) skips
/// straight to `next.run` — no `resolve_tenant` call, no extra work at all —
/// so a deployment with no `auth:` section is byte-for-byte the pre-`#17`
/// behavior, not merely an equivalent one.
///
/// An unresolvable tenant external id passes through unauthenticated too:
/// there is nothing to authorize for a tenant that doesn't exist, and the
/// eventual handler (`landing::tenant_directory` or a protocol resolve)
/// answers 404, exactly as it did before this layer existed — auth
/// enforcement never changes the shape of a not-found response.
///
/// A deny is rendered with the same shared RFC 9457 problem+json body every
/// other error path on this server uses ([`problem_response`]): 401 when no
/// credential was presented at all, 403 when one was presented but doesn't
/// authorize this tenant. Neither that body, an error, nor any log line
/// ever includes the credential's raw value.
async fn enforce_tenant_auth(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    request: Request,
    next: Next,
) -> Response {
    let state = ctx.current();
    let Some(authorizer) = state.authorizer.as_ref() else {
        return next.run(request).await;
    };

    // Every route this layer wraps is nested under `/{tenant}`, so the
    // capture always exists in practice — mirrors
    // `landing::tenant_directory`'s own `Path<HashMap<String, String>>`
    // extraction one level up.
    let Some(tenant_ext) = params.get("tenant") else {
        return next.run(request).await;
    };

    let Ok(tenant_id) = state.resolver.resolve_tenant(tenant_ext).await else {
        return next.run(request).await;
    };

    let credential = extract_credential(request.headers());
    match authorizer.authorize(&credential, &tenant_id).await {
        AuthDecision::Allow => next.run(request).await,
        AuthDecision::Deny(DenyReason::NoCredential) => problem_response(
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            "this tenant requires a credential",
        ),
        AuthDecision::Deny(DenyReason::NotAuthorized) => problem_response(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "the presented credential does not authorize this tenant",
        ),
    }
}

/// Platform-admin trust-boundary enforcement (`#110`): layered onto the
/// config-mutation control lane (`/config` GET/PUT — see
/// `config_mutation.rs`'s own module doc), never onto anything else. Absent
/// `auth:` config skips straight to a `404` — see this function's own doc
/// for why a `404`, not a `401`, is deliberate here.
///
/// Unlike [`enforce_tenant_auth`], a `PlatformAdminDecision::Allow`
/// carries a `principal` the mutation handler needs for its audit record
/// (`tellurion_core::audit`) — inserted as a request [`Extension`] so the
/// handler reads it back out rather than re-deriving it a second time
/// against the same credential.
async fn enforce_platform_admin_auth(
    State(ctx): State<Arc<AppContext>>,
    mut request: Request,
    next: Next,
) -> Response {
    let state = ctx.current();
    // `#110`: absent/disabled auth means the config-mutation surface does
    // not exist at all for this deployment — the issue's own framing. The
    // route topology stays static across a reload here, same as every
    // other route this server mounts (only `ContextState` is swapped), so
    // this is enforced dynamically, per request, against the CURRENT
    // config — the identical "an auth: edit takes effect on the next
    // reload, no restart" pattern `enforce_tenant_auth` already
    // establishes. `404`, not `401`: a deployment that never configured
    // auth should look, from the outside, exactly like one where this
    // route was never registered at all — a `401` would instead advertise
    // "there is a protected resource here," which is precisely the
    // exposure this rule exists to avoid.
    let Some(authorizer) = state.authorizer.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let credential = extract_credential(request.headers());
    match authorizer.authorize_platform_admin(&credential).await {
        PlatformAdminDecision::Allow { principal } => {
            request
                .extensions_mut()
                .insert(PlatformAdminPrincipal(principal));
            next.run(request).await
        }
        PlatformAdminDecision::Deny(DenyReason::NoCredential) => problem_response(
            StatusCode::UNAUTHORIZED,
            "Unauthorized",
            "the configuration mutation surface requires a credential",
        ),
        PlatformAdminDecision::Deny(DenyReason::NotAuthorized) => problem_response(
            StatusCode::FORBIDDEN,
            "Forbidden",
            "the presented credential does not authorize configuration mutations",
        ),
    }
}

/// The platform-admin principal [`enforce_platform_admin_auth`] resolved
/// for the current request (`#110`) — a request `Extension` so
/// `config_mutation`'s handlers can read it back for the audit trail
/// without a second authorization lookup against the same credential.
#[derive(Debug, Clone)]
pub(crate) struct PlatformAdminPrincipal(pub String);

/// Extracts a [`Credential`] from `Authorization: Bearer <token>` — the only
/// scheme any authorizer in this crate understands today. Any other or
/// malformed `Authorization` header (missing, non-UTF-8, wrong scheme, empty
/// token) is treated as `Credential::None`, the same "nothing was presented"
/// case as no header at all.
pub(crate) fn extract_credential(headers: &HeaderMap) -> Credential {
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

/// One of the write-capable resource shapes this server exposes under a
/// `features` protocol root (`tellurion_features::router`'s own write
/// routes). Recognized by path shape alone, and only under `features`: the
/// same shapes under a different root are read-only and never match (STAC's
/// own `/collections/{cid}/items` has the segment `stac`, not `features`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteResource {
    /// `/collections/{cid}/items` — `GET` plus Part 4's `POST` create.
    ItemCollection,
    /// `/collections/{cid}/items/batch` (`#114`) — `POST` only. Unlike the
    /// other two shapes this resource has no read representation at all, so
    /// with the write lane off it supports nothing but `OPTIONS`. Matched
    /// ahead of [`WriteResource::Item`] below for the same reason axum
    /// itself routes it there: a literal segment beats a `{fid}` capture.
    BatchIngest,
    /// `/collections/{cid}/items/{fid}` — `GET` plus Part 4's `PUT`/
    /// `PATCH`/`DELETE`.
    Item,
}

impl WriteResource {
    /// Which shape `path` is, or `None` for a path that is not a write
    /// resource of this server at all.
    fn of(path: &str) -> Option<Self> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        match segments.as_slice() {
            [_, "features", "catalogs", _, "collections", _, "items"] => {
                Some(WriteResource::ItemCollection)
            }
            [_, "features", "catalogs", _, "collections", _, "items", "batch"] => {
                Some(WriteResource::BatchIngest)
            }
            [_, "features", "catalogs", _, "collections", _, "items", _] => {
                Some(WriteResource::Item)
            }
            _ => None,
        }
    }

    /// The `Allow` value for this resource, given whether a write to it
    /// would actually be accepted here and now.
    ///
    /// OGC API — Features — Part 4 (OGC 20-002r1) Requirement 16 clause C
    /// (`/req/create-replace-delete/options-response`): "The value of the
    /// `Allow` header SHALL be the list of methods that are allowed for the
    /// resource at the time and within the context of the request." That is
    /// why `writes_allowed` is a parameter rather than this being the
    /// static table it originally was — and, since `#208`, why the caller
    /// computes it from two live facts rather than one:
    ///
    /// - the `#185` exposure matrix for this catalog
    ///   (`settings.protocols.features_write`), and
    /// - whether this collection's write lane resolves to a `WriteSink`
    ///   at all (`Router::write_lane_resolves`, the same predicate
    ///   `Router::resolve_write` enforces when the write actually arrives).
    ///
    /// Either one being false means every write method on this resource
    /// will be refused, so naming them would be the overclaim Requirement 16
    /// forbids. Section 6.5.1 of the same document is explicit that this is
    /// allowed to vary per resource: "A server is not required to implement
    /// every method described in this Standard (i.e. POST, PUT, PATCH or
    /// DELETE) for every mutable resource that it offers."
    ///
    /// What `writes_allowed` deliberately does NOT depend on is the
    /// authenticated subject — see
    /// [`respond_to_plain_options_on_write_resources`] for why.
    fn allow(self, writes_allowed: bool) -> HeaderValue {
        match (self, writes_allowed) {
            (WriteResource::ItemCollection, true) => HeaderValue::from_static("GET, POST, OPTIONS"),
            (WriteResource::ItemCollection, false) => HeaderValue::from_static("GET, OPTIONS"),
            (WriteResource::BatchIngest, true) => HeaderValue::from_static("POST, OPTIONS"),
            (WriteResource::BatchIngest, false) => HeaderValue::from_static("OPTIONS"),
            (WriteResource::Item, true) => {
                HeaderValue::from_static("GET, PUT, PATCH, DELETE, OPTIONS")
            }
            (WriteResource::Item, false) => HeaderValue::from_static("GET, OPTIONS"),
        }
    }
}

/// The exposure matrix (`#185`) governing `path`, resolved through the
/// tenant/catalog segments the path itself carries — `None` when the path
/// names no resolvable `(tenant, catalog)` pair, in which case there is
/// names a catalog the current routing snapshot never indexed. Both callers
/// treat `None` as "nothing to gate" and let whatever answer the route would
/// otherwise give stand: there is no matrix to enforce for a catalog that
/// isn't served, and the route underneath already answers its own 404.
///
/// Read off the path rather than off axum's captured `Path` parameters
/// because one of the two callers — the plain-`OPTIONS` responder — sits
/// outside the router entirely and has no captures to read. The
/// `/{tenant}/{protocol}/catalogs/{catalog}/...` mount shape
/// [`WriteResource::of`] already matches on is then the single source of
/// truth for both.
async fn protocols_for_path(ctx: &AppContext, path: &str) -> Option<tellurion_core::ProtocolsConf> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let [tenant_ext, _protocol, "catalogs", catalog_ext, ..] = segments.as_slice() else {
        return None;
    };
    let state = ctx.current();
    let tenant_id = state.resolver.resolve_tenant(tenant_ext).await.ok()?;
    let catalog_id = state
        .resolver
        .resolve_catalog(&tenant_id, catalog_ext)
        .await
        .ok()?;
    state.router.catalog_protocols(&catalog_id)
}

/// Whether a write to the collection `path` names would reach a `WriteSink`
/// (`#208`) — `None` whenever there is nothing to answer for: the path names
/// no collection, or some segment of `(tenant, catalog, collection)` does not
/// resolve.
///
/// The same shape as [`collection_kind_of_path`], off the same
/// [`collection_of_path`] segments and through the same resolver hops,
/// because it answers the same kind of question one layer along: what does
/// this deployment's current routing snapshot say about the collection this
/// URI names? Split out of the caller for the same reason too — so the
/// borrowed `ContextState` snapshot is dropped before the request is handed
/// onward.
///
/// `None` is not "read-only": an unresolvable collection has no write
/// capability to describe, and the caller restores the pre-`#208` "describe
/// the URI shape" answer for it, which is what keeps a not-found resource
/// answering exactly as it did before (the issue's own third scope bullet)
/// instead of acquiring a narrower `Allow` that would make `OPTIONS` a
/// collection-existence oracle in the negative direction.
async fn write_lane_resolves_for_path(ctx: &AppContext, path: &str) -> Option<bool> {
    let state = ctx.current();
    let (tenant_ext, catalog_ext, collection_ext) = collection_of_path(path)?;
    let tenant_id = state.resolver.resolve_tenant(tenant_ext).await.ok()?;
    let catalog_id = state
        .resolver
        .resolve_catalog(&tenant_id, catalog_ext)
        .await
        .ok()?;
    let collection_id = state
        .resolver
        .resolve_collection(&catalog_id, collection_ext)
        .await
        .ok()?;
    Some(state.router.write_lane_resolves(&collection_id))
}

/// OGC API Features Part 4 (`/req/core/methods` clause B, `/req/create-
/// replace-delete/options-op`, `/req/create-replace-delete/options-
/// response`): a plain `OPTIONS` request to a mutable resource SHALL get a
/// `200` response with an `Allow` header naming the methods that resource
/// supports.
///
/// `cors` (below) answers every `OPTIONS` request itself, including one
/// with no `Origin` header at all, and never forwards it to the router — see
/// this module's own doc for why a route-level `.options(...)` handler
/// would never run. This layer, positioned just outside `cors`, claims only
/// the narrow slice `cors` doesn't already own: an `OPTIONS` request that
/// carries no `Access-Control-Request-Method` header (i.e. not a browser
/// CORS preflight) to one of the two write resource shapes
/// [`WriteResource::of`] recognizes. A genuine preflight, or an `OPTIONS`
/// to any other path, passes through unchanged to `cors`/the router exactly
/// as before this layer existed.
///
/// The `Allow` this answers with is narrowed by two live facts, so a write
/// method is never advertised that the very next request would be refused
/// for:
///
/// - `#185`, the resolved exposure matrix: a catalog with
///   `protocols.features_write: disabled` refuses every write method with a
///   `405`. A `protocols.features: disabled` catalog is not answered for at
///   all — its `features` root does not exist, so this layer has no resource
///   to describe and falls through, exactly as it does for any other
///   unrecognized path.
/// - `#208`, the collection's own write lane: a collection with no
///   `routing.write`, or one routed at a storage that advertises no
///   `WriteSink`, refuses every write method with `resolve_write`'s named
///   `CapabilityUnsupported`. `Router::write_lane_resolves` is the *same*
///   predicate `resolve_write` enforces, read off the same routing
///   snapshot, so — as in `#220`'s `root_serves` — the advertisement and
///   the request are structurally incapable of disagreeing.
///
/// ## What `Allow` depends on, and what it deliberately does not
///
/// Tenant and catalog (through the matrix) and collection (through the
/// lane). **Not the authenticated subject**, and that is a decision rather
/// than an omission.
///
/// Part 4 section 6.5.1 does contemplate subject-dependence — it names
/// "user access control requirements (e.g. user "X" is only allowed to
/// create resources but not update or delete resources)" among the controls
/// a server advertises "within the control context in place". But RFC 9110
/// section 15.5.6 draws the line this server already draws everywhere else:
/// a `405` (with its mandatory `Allow`) says the *resource* does not support
/// the method, while `401`/`403` say this *caller* may not use a method the
/// resource does support. A write grant this caller lacks is the second
/// kind, and `write_handlers::authorize_write_lane` already answers it by
/// name at the point of use.
///
/// Deriving `Allow` from the subject would also make this layer — which
/// sits outside the router, ahead of every policy checkpoint — into a grant
/// oracle: the same URI would answer differently per credential, letting a
/// caller enumerate which subjects hold write grants on which collections
/// without ever attempting a write. `#192` refused a comparable
/// pre-authorization disclosure by answering a bare `404`; the answer here
/// is the same reasoning reaching a different shape — keep the value
/// subject-independent, so there is nothing about any subject to disclose.
///
/// The one thing this *does* expose ahead of authorization is that a
/// resolvable collection is read-only (a narrowed `Allow`, where a writable
/// or unresolvable one keeps the full list). That is deployment routing
/// configuration, not data; and collection existence itself is already
/// disclosed pre-authorization on every lane in this server, since
/// `resolve_collection` runs before `authorize_lane`/`authorize_write_lane`
/// in every handler, so a nonexistent id already answers `404` where an
/// existing one answers `401`.
///
/// ## What a refused write still answers
///
/// A write method this header withholds is refused by
/// `Router::resolve_write`'s named `CapabilityUnsupported` — a `404` whose
/// detail names the collection and the `write` capability. `#208` left that
/// status code alone deliberately, and the choice is worth recording rather
/// than rediscovering: `enforce_protocol_exposure` above answers `405` for
/// the *catalog-wide* write lane being off, and the same argument (a `404`
/// claims the resource does not exist while a `GET` on that exact URI
/// returns it) applies to a read-only collection too. Making the two agree
/// on the status code as well as on the method list is a change to write
/// semantics rather than to an advertisement, though: it would have to
/// distinguish the lane-absent refusal from every other
/// `CapabilityUnsupported` a driver raises, and leave the changes feed —
/// which resolves the same write lane for a `GET` (`feed_handlers`) — still
/// answering `404`. It belongs in its own slice.
async fn respond_to_plain_options_on_write_resources(
    State(ctx): State<Arc<AppContext>>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() == Method::OPTIONS
        && !request
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
    {
        if let Some(resource) = WriteResource::of(request.uri().path()) {
            let protocols = protocols_for_path(&ctx, request.uri().path()).await;
            let writes_allowed = match protocols {
                Some(protocols) if !protocols.features.is_enabled() => {
                    return next.run(request).await;
                }
                Some(protocols) => {
                    protocols.features_write.is_enabled()
                        && write_lane_resolves_for_path(&ctx, request.uri().path())
                            .await
                            // An unresolvable collection: see
                            // `write_lane_resolves_for_path`'s own doc — the
                            // same "describe the shape, the 404 underneath
                            // still stands" answer the arm below gives.
                            .unwrap_or(true)
                }
                // No resolvable `(tenant, catalog)` behind this path: the
                // request is bound for a 404 anyway, and describing the URI
                // shape is what this layer did before `#185` existed.
                None => true,
            };
            let mut response = StatusCode::OK.into_response();
            response
                .headers_mut()
                .insert(header::ALLOW, resource.allow(writes_allowed));
            return response;
        }
    }
    next.run(request).await
}

/// Per-tenant protocol exposure enforcement (`#185`): layered onto every
/// protocol root ([`protocol_root`]/[`stac_root`]), so a request never
/// reaches a protocol handler for a surface this deployment's
/// `settings.protocols` matrix turns off.
///
/// `protocol` is passed in by the caller's closure rather than extracted
/// from the request: `Extension(Protocol)` is applied *inside* those two
/// functions, so a layer that runs ahead of the routes — which this one
/// must, to refuse before any handler work happens — would find nothing to
/// extract.
///
/// Two different refusals, because the two toggles are two different
/// statements:
///
/// - **A whole protocol off is `404`.** The root and everything under it
///   ceases to exist for this catalog; nothing answers at that prefix, and
///   a bare `404` (no problem body) is indistinguishable from a prefix that
///   was never mounted, which is the point — existence is not leaked, the
///   same rule the visibility model already follows.
/// - **`features_write` off is `405` with a truthful `Allow`.** The write
///   methods live on the *same URIs* as the reads (`POST /collections/
///   {cid}/items`, `PUT|PATCH|DELETE /collections/{cid}/items/{fid}`), and
///   the reads keep serving. A `404` there would claim the resource does
///   not exist while a `GET` on that exact URI returns it. OGC API -
///   Features - Part 4 defines `405` for precisely this ("the resource only
///   supports GET requests") and explicitly permits a server to implement
///   only a subset of the write methods, so the honest answer names what
///   remains instead of denying the resource.
///
/// A `(tenant, catalog)` pair that does not resolve, or a catalog absent
/// from the current routing snapshot, passes straight through: there is no
/// matrix to enforce for a catalog that isn't served, and the handler
/// underneath answers its own `404` exactly as it did before — the same
/// "enforcement never changes the shape of a not-found response" rule
/// [`enforce_tenant_auth`] follows.
async fn enforce_protocol_exposure(
    ctx: Arc<AppContext>,
    protocol: Protocol,
    original_path: String,
    request: Request,
    next: Next,
) -> Response {
    let Some(protocols) = protocols_for_path(&ctx, &original_path).await else {
        return next.run(request).await;
    };

    if !protocol.exposure(&protocols).is_enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }

    // The write lane narrows methods rather than removing paths, and it
    // only exists under the Features root — every non-read method
    // `tellurion_features::router` registers is one of Part 4's write
    // operations or the batch-ingest `POST`, so the method itself is the
    // whole test; no second list of write paths to drift from the router.
    if protocol == Protocol::Features
        && !protocols.features_write.is_enabled()
        && !matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::OPTIONS
        )
    {
        let mut response = problem_response(
            StatusCode::METHOD_NOT_ALLOWED,
            "MethodNotAllowed",
            "the write lane is not exposed for this catalog",
        );
        // `Allow` is mandatory on a 405 (RFC 9110 15.5.6) and must name what
        // this resource really still supports. The path here is the
        // `OriginalUri` one: `nest` strips the mount prefix off
        // `request.uri()`, while `WriteResource::of` matches on the full
        // `/{tenant}/features/catalogs/{catalog}/...` shape.
        if let Some(resource) = WriteResource::of(&original_path) {
            response
                .headers_mut()
                .insert(header::ALLOW, resource.allow(false));
        }
        return response;
    }

    next.run(request).await
}

/// STAC's own root (`#36`, slice A): the same `/conformance` + `/api`
/// scaffolding [`protocol_root`] gives every other protocol, but `/`
/// answers with a STAC Catalog document (`landing::stac_landing`) instead of
/// the generic OGC API Common landing page every other protocol shares —
/// STAC API - Core mandates a specific document shape (`type`,
/// `stac_version`, `id`, an embedded `conformsTo`) that `landing::
/// protocol_landing` doesn't produce, so this can't reuse `protocol_root`
/// as-is.
fn stac_root(
    ctx: &Arc<AppContext>,
    resource_router: Router<Arc<AppContext>>,
) -> Router<Arc<AppContext>> {
    let root = resource_router
        .route("/", get(landing::stac_landing))
        .route("/conformance", get(landing::protocol_conformance))
        .route("/api", get(openapi::api_doc))
        .layer(Extension(Protocol::Stac));
    let root = gate_on_collection_kind(ctx, Protocol::Stac, root);
    gate_on_protocol_exposure(ctx, Protocol::Stac, root)
}

/// The OGC API — Processes root (`#182`), whose availability is a *capability*
/// question the exposure matrix cannot answer.
///
/// `lane` is `Some` only where this deployment has both a durable job ledger
/// and at least one registered runner (`process_lane::build`). With `None`,
/// the root is still mounted — route topology stays static across a reload,
/// like every other root on this server — but every path under it answers the
/// bare `404` an unmounted prefix answers. A Processes root that accepted a
/// job it could not record, or advertised processes nothing could execute,
/// is the half-working surface `#182` exists to prevent.
///
/// The availability gate is applied OUTSIDE the `#185` exposure gate, so it is
/// the outermost thing on this root: a capability the deployment does not have
/// cannot be re-enabled by any per-catalog setting, and layering it outermost
/// makes that structurally true rather than a matter of reading two closures
/// in the right order. `Extension(lane)` is layered on the resource router
/// *before* `protocol_root` adds `/`, `/conformance` and `/api`, since only
/// the resource handlers read it — and, being an extension rather than a
/// responder, it can never answer a request that the gates above would have
/// refused.
fn processes_root(
    ctx: &Arc<AppContext>,
    lane: Option<Arc<tellurion_core::ProcessLane>>,
    availability: RootAvailability,
) -> Router<Arc<AppContext>> {
    let resources = tellurion_processes::router();
    let Some(lane) = lane.filter(|_| availability.serves(Protocol::Processes)) else {
        let root = protocol_root(ctx, Protocol::Processes, resources);
        return root.layer(axum::middleware::from_fn(
            |_request: Request, _next: Next| async move { StatusCode::NOT_FOUND.into_response() },
        ));
    };
    let resources = resources.layer(Extension(lane));
    protocol_root(ctx, Protocol::Processes, resources)
}

/// Wraps one assembled protocol root in the `#192` collection-kind gate,
/// with `protocol` captured by closure for the same reason
/// [`gate_on_protocol_exposure`] captures it. Applied *inside* that gate (so
/// the exposure decision, which can remove a whole root, is made first) and
/// with the same plain `layer` rather than `route_layer`, for the same
/// reason: a request the `MethodRouter` answers on its own must not slip
/// past.
fn gate_on_collection_kind(
    ctx: &Arc<AppContext>,
    protocol: Protocol,
    root: Router<Arc<AppContext>>,
) -> Router<Arc<AppContext>> {
    root.layer(axum::middleware::from_fn_with_state(
        Arc::clone(ctx),
        move |State(ctx): State<Arc<AppContext>>,
              OriginalUri(uri): OriginalUri,
              request: Request,
              next: Next| async move {
            enforce_collection_kind(ctx, protocol, uri.path().to_string(), request, next).await
        },
    ))
}

/// The collection id in a `/{tenant}/{protocol}/catalogs/{catalog}/collections/{cid}/...`
/// path, alongside the tenant and catalog segments that scope it — `None`
/// for any path that does not name a collection at all (a landing page,
/// `/conformance`, `/api`, `/tileMatrixSets`, `/styles`, `/search`).
///
/// Read off the path rather than axum's captured `Path` parameters for the
/// same reason [`protocols_for_path`] reads its own segments there: this
/// runs ahead of the routes, so there are no captures yet.
fn collection_of_path(path: &str) -> Option<(&str, &str, &str)> {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        [tenant, _protocol, "catalogs", catalog, "collections", collection, ..] => {
            Some((tenant, catalog, collection))
        }
        _ => None,
    }
}

/// Per-protocol collection-kind enforcement (`#192`): a request naming a
/// collection this root does not serve gets the same bare `404` a request to
/// a protocol root this catalog turns off gets, and for the same reason —
/// that collection does not exist *here*, and saying so without a body
/// leaves it indistinguishable from a collection that does not exist at all.
///
/// This is the single seam that keeps every root's `/collections` listing
/// and its per-collection resources telling the same story. Each root's
/// listing filters by `Protocol::serves_kind` (see
/// `tellurion_features::handlers::list_collections`,
/// `tellurion_records::handlers::list_catalogs`); without this layer, a
/// record collection would be absent from the Features root's listing while
/// `GET /features/.../collections/{cid}/items` kept serving it, and the
/// listing would be a lie.
///
/// **Cost, and why an unconfigured deployment pays none of it.** The first
/// thing this does is ask `Router::has_record_collections()`, a `bool` over
/// the already-built routing index. Every kind other than `record` is served
/// by exactly the roots that served it before `#192`, so with no record
/// collection declared anywhere there is nothing this layer could refuse —
/// it returns immediately, without parsing the path, resolving a tenant, or
/// touching the resolver. That is what keeps the tiles hot path unchanged
/// and what makes "a deployment that never asked for the records lane is
/// byte-for-byte what it was" true rather than merely intended.
///
/// A path that names no collection, a tenant/catalog/collection that does
/// not resolve, or a collection the router never indexed all pass straight
/// through: there is no kind to enforce, and the handler underneath answers
/// its own `404` exactly as before — the same rule
/// [`enforce_protocol_exposure`] follows for an unresolvable catalog.
///
/// **Why a bare `404` rather than a named problem body.** This workspace
/// refuses by name wherever it can (`CapabilityUnsupported` and friends), and
/// `tellurion_records::handlers::require_record_collection` does exactly that
/// for the same condition inside the Records crate. This layer deliberately
/// does not: it runs *ahead of every handler*, and therefore ahead of the
/// `#34` policy checkpoint, so a body naming the collection would disclose
/// that a collection by that id exists to a subject the policy layer has not
/// yet been asked about — the precise disclosure the visibility model and the
/// `#185` exposure gate both answer with a bodiless `404` to avoid. The
/// refusal is still named where naming it costs nothing: in the `debug` log
/// line below, which carries the collection, its kind and the root that
/// declined it, and in each root's `/collections` listing, which is the
/// client-facing statement of the same fact.
async fn enforce_collection_kind(
    ctx: Arc<AppContext>,
    protocol: Protocol,
    original_path: String,
    request: Request,
    next: Next,
) -> Response {
    let Some(kind) = collection_kind_of_path(&ctx, &original_path).await else {
        return next.run(request).await;
    };
    if protocol.serves_kind(kind) {
        return next.run(request).await;
    }
    tracing::debug!(
        path = %original_path,
        protocol = protocol.segment(),
        kind = ?kind,
        "refused: this protocol root does not serve collections of this kind"
    );
    StatusCode::NOT_FOUND.into_response()
}

/// The declared [`CollectionKind`] of the collection `path` names — `None`
/// whenever there is nothing to enforce: no record collection exists in this
/// deployment at all, the path names no collection, or some segment of
/// `(tenant, catalog, collection)` does not resolve. Split out of
/// [`enforce_collection_kind`] so the borrowed `ContextState` snapshot is
/// dropped before the request is handed onward.
async fn collection_kind_of_path(ctx: &AppContext, path: &str) -> Option<CollectionKind> {
    let state = ctx.current();
    if !state.router.has_record_collections() {
        return None;
    }
    let (tenant_ext, catalog_ext, collection_ext) = collection_of_path(path)?;
    let tenant_id = state.resolver.resolve_tenant(tenant_ext).await.ok()?;
    let catalog_id = state
        .resolver
        .resolve_catalog(&tenant_id, catalog_ext)
        .await
        .ok()?;
    let collection_id = state
        .resolver
        .resolve_collection(&catalog_id, collection_ext)
        .await
        .ok()?;
    state.router.collection_kind(&collection_id)
}

#[cfg(test)]
pub fn build(
    ctx: Arc<AppContext>,
    prometheus_handle: PrometheusHandle,
    request_timeout_s: u64,
) -> Router {
    build_with_readiness(ctx, prometheus_handle, request_timeout_s, Readiness::new())
}

#[cfg(test)]
pub(crate) fn build_with_readiness(
    ctx: Arc<AppContext>,
    prometheus_handle: PrometheusHandle,
    request_timeout_s: u64,
    readiness: Readiness,
) -> Router {
    // `#182`: resolved from the very same `AppContext` the production path
    // resolves it from, rather than passed in as a test-only `None`. A test
    // config with no `server.processes` block therefore exercises the real
    // capability gate — the Processes root is genuinely absent, not absent
    // because the fixture said so.
    let process_lane = crate::process_lane::build(&ctx);
    build_with_webhook_registry(
        ctx,
        prometheus_handle,
        request_timeout_s,
        readiness,
        Arc::new(WebhookRegistry::new()),
        process_lane,
    )
}

/// Builds the deliberately narrow public evaluation appliance. Unlike the
/// ordinary application router, this surface has no tenant protocol roots,
/// control/configuration APIs, or metrics endpoint. It still reuses the same
/// request bounds, panic isolation, observation, readiness state, embedded UI,
/// and expiring HTTPS-source sandbox as the full server.
#[cfg(all(feature = "public-demo", feature = "ui"))]
pub(crate) fn build_public_demo(
    ctx: Arc<AppContext>,
    request_timeout_s: u64,
    readiness: Readiness,
) -> Router {
    let state = ctx.current();
    let backend_capacity_hint = state.router.total_capacity_hint();
    let max_concurrency = state
        .config
        .server
        .max_concurrency
        .unwrap_or_else(|| derive_max_concurrency(backend_capacity_hint));
    drop(state);

    let router: Router<Arc<AppContext>> = Router::new()
        .route("/", get(public_demo_root))
        .route("/healthz", get(crate::readiness::healthz))
        .route("/readyz", get(crate::readiness::readyz))
        .merge(tellurion::public_demo::router())
        .merge(crate::ui_assets::router());

    let observation_ctx = Arc::clone(&ctx);
    router
        .with_state(ctx)
        .layer(Extension(readiness))
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http().make_span_with(crate::request_id::trace_span))
                .layer(CompressionLayer::new())
                .layer(HandleErrorLayer::new(handle_middleware_error))
                .load_shed()
                .concurrency_limit(max_concurrency)
                .timeout(Duration::from_secs(request_timeout_s))
                .layer(CatchPanicLayer::custom(handle_panic)),
        )
        .layer(axum::middleware::from_fn_with_state(
            observation_ctx,
            srv_metrics::observe_request,
        ))
        .layer(axum::middleware::from_fn(
            crate::request_id::propagate_request_id,
        ))
        .layer(axum::middleware::from_fn(
            tellurion::public_demo::private_demo_responses,
        ))
}

#[cfg(test)]
pub(crate) fn build_with_webhook_registry(
    ctx: Arc<AppContext>,
    prometheus_handle: PrometheusHandle,
    request_timeout_s: u64,
    readiness: Readiness,
    webhook_registry: Arc<WebhookRegistry>,
    process_lane: Option<Arc<tellurion_core::ProcessLane>>,
) -> Router {
    build_with_webhook_registry_and_control_browser(
        ctx,
        prometheus_handle,
        request_timeout_s,
        readiness,
        webhook_registry,
        process_lane,
        None,
    )
}

pub(crate) fn build_with_webhook_registry_and_control_browser(
    ctx: Arc<AppContext>,
    prometheus_handle: PrometheusHandle,
    request_timeout_s: u64,
    readiness: Readiness,
    webhook_registry: Arc<WebhookRegistry>,
    process_lane: Option<Arc<tellurion_core::ProcessLane>>,
    control_browser: Option<Arc<crate::control_browser_auth::ControlBrowserAuth>>,
) -> Router {
    let state = ctx.current();
    let backend_capacity_hint = state.router.total_capacity_hint();
    let max_concurrency = state
        .config
        .server
        .max_concurrency
        .unwrap_or_else(|| derive_max_concurrency(backend_capacity_hint));
    tracing::info!(
        max_concurrency,
        backend_capacity_hint = ?backend_capacity_hint,
        explicit_override = state.config.server.max_concurrency.is_some(),
        "admission control: concurrency ceiling (explicit config wins over derived)"
    );
    // `#66`: every tenant's fair share is a slice of this same process-wide
    // ceiling. The ceiling remains fixed at startup; the per-tenant gates
    // are rebuilt lazily from each atomically published context generation
    // so relational tenant/settings reloads take effect without splitting
    // one generation across multiple registries.
    let admission_registry = Arc::new(crate::admission::ReloadableAdmissionRegistry::new(
        max_concurrency,
    ));
    drop(state);

    // `#182`: computed once, here, and used by both the root's own gate and
    // the tenant directory's link filter — see `RootAvailability`'s own doc
    // for why a capability precondition is not folded into the `#185`
    // exposure matrix.
    let root_availability = RootAvailability {
        processes: process_lane.is_some(),
    };

    // `#215`: the tenant/catalog/collection administrative resources, held
    // in a router of their own so the policy checkpoint can be registered on
    // exactly them. Registering it on the whole of `tenant_scope` would put
    // a canonicalization refusal in front of every data-plane path too,
    // which is reach `#215` never asked for; a router holding only
    // administrative routes means every request the checkpoint sees has
    // already matched an administrative route template.
    //
    // `route_layer` here is innermost by construction: `tenant_scope`'s own
    // `enforce_tenant_auth`/admission wraps are registered further down and
    // therefore run first, which is the ordering the checkpoint's own doc
    // requires.
    let tenant_admin_scope: Router<Arc<AppContext>> = Router::new()
        .route("/config/effective", get(config_view::effective_config_view))
        .route(
            "/config/catalogs/{catalog}/effective",
            get(config_view::effective_config_view),
        )
        .route(
            "/config/catalogs/{catalog}/collections/{collection}/effective",
            get(config_view::effective_config_view),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            Arc::clone(&ctx),
            crate::control_checkpoint::enforce_control_policy,
        ));

    let tenant_scope: Router<Arc<AppContext>> = Router::new()
        .route("/", get(landing::tenant_directory))
        .merge(tenant_admin_scope)
        .nest(
            "/features/catalogs/{catalog}",
            protocol_root(&ctx, Protocol::Features, tellurion_features::router()),
        )
        .nest(
            "/tiles/catalogs/{catalog}",
            protocol_root(&ctx, Protocol::Tiles, tellurion_tiles::router()),
        )
        .nest(
            "/styles/catalogs/{catalog}",
            protocol_root(&ctx, Protocol::Styles, tellurion_styles::router()),
        )
        .nest(
            "/3dtiles/catalogs/{catalog}",
            protocol_root(&ctx, Protocol::ThreeDTiles, tellurion_places::router()),
        )
        .nest(
            "/stac/catalogs/{catalog}",
            stac_root(&ctx, tellurion_stac::router()),
        )
        // `#192`: the OGC API — Records root. Mounted unconditionally, like
        // its five siblings, and gated by `protocols.records` — which,
        // unlike its siblings, defaults to `disabled` (see
        // `ProtocolsConf::records`), so this prefix answers exactly the
        // `404` an unmounted one answers until an operator asks for it.
        .nest(
            "/records/catalogs/{catalog}",
            protocol_root(&ctx, Protocol::Records, tellurion_records::router()),
        )
        // `#182`: the OGC API — Processes root. Mounted unconditionally, like
        // its six siblings, and gated twice — by `protocols.processes` (which,
        // like `records`, defaults to `disabled`) and by whether this
        // deployment actually has a job ledger and a runner at all. See
        // `processes_root`'s own doc for why the second gate is the outermost
        // one.
        .nest(
            "/processes/catalogs/{catalog}",
            processes_root(&ctx, process_lane, root_availability),
        )
        // `#182`: the tenant directory needs the same availability verdict the
        // Processes root was gated on, so it never links a prefix it already
        // knows answers `404`. Layered here rather than passed as an argument
        // so `landing::tenant_directory` reads it the same way it reads every
        // other per-request value, and so the one place the verdict is
        // computed is the one place it comes from.
        .layer(Extension(root_availability))
        // `#66`: per-tenant admission control, scoped to exactly this
        // router the same way `#17`'s auth check below is. Registered
        // first so it ends up innermost of the two `route_layer` wraps —
        // an unauthenticated request is turned away by auth before it ever
        // spends one of its tenant's admission slots.
        .route_layer(axum::middleware::from_fn_with_state(
            Arc::clone(&ctx),
            crate::admission::enforce_tenant_admission,
        ))
        // `#17`: the tenant trust-boundary check, scoped to exactly this
        // router (everything nested under `/{tenant}`) via `route_layer` so
        // it never touches the top-level service, metrics, or probe routes
        // below. It must stay the LAST registration here: `route_layer`
        // only wraps routes already added, so a nest after it would escape
        // the check.
        .route_layer(axum::middleware::from_fn_with_state(
            Arc::clone(&ctx),
            enforce_tenant_auth,
        ));

    // `#110`: the authenticated config-mutation control lane, gated by
    // `enforce_platform_admin_auth` alone — never nested under
    // `tenant_scope` (a platform mutation is not tenant-scoped) and never
    // wrapped by `enforce_tenant_auth` either. See `config_mutation.rs`'s
    // own module doc for the resource shape and `enforce_platform_admin_auth`'s
    // own doc for why an absent/disabled `auth:` renders this indistinguishable
    // from an unregistered route.
    let config_mutation_scope: Router<Arc<AppContext>> = Router::new()
        .route(
            "/config",
            get(config_mutation::get_raw_config).put(config_mutation::put_config),
        )
        .route("/config/webhooks", get(config_mutation::list_webhooks))
        .route(
            "/config/webhooks/{subscription}/dead-letters",
            get(webhook_admin::list_dead_letters),
        )
        .layer(Extension(webhook_registry))
        // `#215`: registered BEFORE the platform-admin gate below, so it
        // ends up inside it — the checkpoint composes with that trust
        // boundary and never pre-empts its `401`.
        .route_layer(axum::middleware::from_fn_with_state(
            Arc::clone(&ctx),
            crate::control_checkpoint::enforce_control_policy,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            Arc::clone(&ctx),
            enforce_platform_admin_auth,
        ));

    // `#215`: the platform-scope administrative reads. Ungated before this
    // change and still ungated when no statement mentions them — the
    // checkpoint's first step returns without doing anything at all for a
    // deployment that declared none, so `/config/effective` stays exactly as
    // open (or as closed) as this deployment already made it.
    let platform_admin_read_scope: Router<Arc<AppContext>> = Router::new()
        .route("/config/effective", get(config_view::effective_config_view))
        .route("/config/profiles", get(config_view::profiles_view))
        .route_layer(axum::middleware::from_fn_with_state(
            Arc::clone(&ctx),
            crate::control_checkpoint::enforce_control_policy,
        ));

    let router: Router<Arc<AppContext>> = Router::new()
        .route("/", get(service_root))
        .route("/metrics", get(srv_metrics::metrics_handler))
        .route("/healthz", get(crate::readiness::healthz))
        .route("/readyz", get(crate::readiness::readyz))
        .merge(platform_admin_read_scope)
        .merge(config_mutation_scope)
        .merge(crate::control_api::router_with_browser(
            &ctx,
            control_browser.clone(),
        ))
        .nest("/{tenant}", tenant_scope);

    let router = if let Some(control_browser) = control_browser {
        router.merge(crate::control_browser_auth::router(control_browser))
    } else {
        router
    };

    // The public demonstration route is a separate, expiring sandbox. It is
    // feature-gated so ordinary deployments expose no anonymous remote-source
    // registration surface, and it intentionally does not use `AppContext`'s
    // configured control store or router.
    #[cfg(feature = "public-demo")]
    let router = router.merge(tellurion::public_demo::router());

    // The embedded demo UI (issue #35) — default-off, see `ui_assets`'s own
    // doc comment for why this is a separate merge rather than another
    // `.route(...)` in the chain above (it needs its own `Router<...>`
    // built by a `#[cfg(feature = "ui")]`-gated module). Stays top-level,
    // same as `/metrics`.
    #[cfg(feature = "ui")]
    let router = router.merge(crate::ui_assets::router());

    // Test-only route that panics unconditionally, mounted ahead of the
    // middleware stack so the `#[cfg(test)]` tests below can exercise
    // `CatchPanicLayer` through the exact same layering production traffic
    // sees. Never compiled into a release binary.
    #[cfg(test)]
    let router = router
        .route("/__test_panic", get(always_panics))
        .route("/__test_slow", get(slow_for_observation_test))
        .route("/__test_timeout", get(timeout_for_observation_test));

    // `allow_methods` now names the write verbs (`POST`/`PUT`/`PATCH`/`DELETE`)
    // alongside the read ones — a browser client could reach `/collections`
    // for read traffic before this, but every cross-origin write attempt
    // failed CORS preflight before ever reaching the write handlers.
    // `allow_headers(Any)` is required too: a write body is always
    // `application/json`-shaped, which is not a CORS-safelisted content
    // type, so a preflight without an allowed-headers answer still blocks
    // the real request even once the method itself is allowed. `expose_headers`
    // makes `Location` (the header a `POST` create response's whole point
    // is to carry) readable by a cross-origin browser client's own script;
    // without it the create response arrives but the client can't read
    // where the new resource landed.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([
            Method::GET,
            Method::HEAD,
            Method::OPTIONS,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers(Any)
        .expose_headers([header::LOCATION]);

    let observation_ctx = Arc::clone(&ctx);
    let options_ctx = Arc::clone(&ctx);
    let router = router
        .with_state(ctx)
        .layer(Extension(prometheus_handle))
        .layer(Extension(readiness))
        .layer(Extension(admission_registry))
        .layer(
            ServiceBuilder::new()
                // `#189`: the default `TraceLayer` span plus `request_id`,
                // which the outermost request-id layer has already ensured.
                .layer(TraceLayer::new_for_http().make_span_with(crate::request_id::trace_span))
                .layer(axum::middleware::from_fn_with_state(
                    Arc::clone(&options_ctx),
                    respond_to_plain_options_on_write_resources,
                ))
                .layer(cors)
                .layer(CompressionLayer::new())
                .layer(HandleErrorLayer::new(handle_middleware_error))
                .load_shed()
                .concurrency_limit(max_concurrency)
                .timeout(Duration::from_secs(request_timeout_s))
                .layer(CatchPanicLayer::custom(handle_panic)),
        )
        // Deliberately outermost: records unmatched/UI traffic and responses
        // synthesized by load-shed and timeout, after TraceLayer's span has
        // closed so slow-request diagnostics can never inherit a raw URI.
        .layer(axum::middleware::from_fn_with_state(
            observation_ctx,
            srv_metrics::observe_request,
        ))
        // Outside even the observation layer (`#189`): a minted id must be on
        // the request before `observe_request` captures it for slow-request
        // events, and the echo must survive on load-shed/timeout responses,
        // which never reach an inner layer.
        .layer(axum::middleware::from_fn(
            crate::request_id::propagate_request_id,
        ));

    // This must stay outside timeout, load-shed, panic isolation, and the
    // router itself: a demo response synthesized by any of those layers is
    // still browser-visible and must never be shared by an intermediary.
    #[cfg(feature = "public-demo")]
    let router = router.layer(axum::middleware::from_fn(
        tellurion::public_demo::private_demo_responses,
    ));

    // Auth responses can be synthesized by CORS, timeout, load shedding, or
    // panic isolation before the auth subrouter sees the request. Keep this
    // path-aware guard outside every global response-producing layer while
    // leaving unrelated routes' caching semantics unchanged.
    router.layer(axum::middleware::from_fn(
        crate::control_browser_auth::auth_paths_no_store,
    ))
}

/// Shared RFC 9457 problem-details body — same shape `tellurion-features`,
/// `tellurion-places`, and `tellurion-styles` each build their own copy of.
/// Not used on the load-shed path ([`LOAD_SHED_BODY`]): that one must avoid
/// this function's per-request `Problem`/`Json` allocation entirely.
pub(crate) fn problem_response(
    status: StatusCode,
    code: &str,
    detail: impl Into<String>,
) -> Response {
    let problem = Problem::new(status.as_u16(), code, detail);
    let mut response = (status, Json(problem)).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
    response
}

/// Renders a caught handler panic as the same shared problem+json body
/// every other error path on this server answers with: generic title/detail,
/// no panic message or backtrace in the body. The panic payload is logged
/// here, in full, before it's discarded from the response.
fn handle_panic(err: Box<dyn std::any::Any + Send + 'static>) -> Response {
    let message = if let Some(s) = err.downcast_ref::<String>() {
        s.as_str()
    } else if let Some(s) = err.downcast_ref::<&str>() {
        s
    } else {
        "non-string panic payload"
    };
    tracing::error!(panic = %message, "request handler panicked");

    problem_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "InternalServerError",
        "an internal server error occurred",
    )
}

/// Precomputed RFC 9457 body for the load-shed 503
/// (`tower::load_shed::error::Overloaded`). Serialized once, here, at
/// compile time — not through [`problem_response`]'s `Problem` + `Json` path
/// on every rejected request. Load-shedding exists precisely because the
/// server is already saturated, so this response must cost nothing beyond
/// writing these fixed bytes onto the wire. Field values match what
/// `problem_response(StatusCode::SERVICE_UNAVAILABLE, "ServiceUnavailable",
/// "server is at capacity; retry shortly")` would produce; the test below
/// parses this constant and checks it field-by-field against that so the two
/// can never silently drift apart.
const LOAD_SHED_BODY: &str = r#"{"type":"about:blank","title":"Service Unavailable","status":503,"detail":"server is at capacity; retry shortly","code":"ServiceUnavailable"}"#;

/// Converts the `BoxError` that `load_shed`/`concurrency_limit`/`timeout`
/// can produce into an honest problem+json response instead of a bare
/// connection error: 503 (with `Retry-After`) when shedding load, 504 when
/// the request exceeded the timeout, 500 for anything else.
async fn handle_middleware_error(err: BoxError) -> Response {
    if err.is::<tower::load_shed::error::Overloaded>() {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON)),
                (header::RETRY_AFTER, HeaderValue::from_static("1")),
            ],
            LOAD_SHED_BODY,
        )
            .into_response()
    } else if err.is::<tower::timeout::error::Elapsed>() {
        problem_response(
            StatusCode::GATEWAY_TIMEOUT,
            "Timeout",
            "the request exceeded the time limit",
        )
    } else {
        tracing::error!(error = %err, "unhandled middleware error");
        problem_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalServerError",
            "an internal server error occurred",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{self, Write};
    use std::sync::Mutex;
    use std::time::SystemTime;

    use axum::body::{to_bytes, Body};
    use axum::extract::Query;
    use axum::http::Request;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    use tellurion_core::{
        AppConfig, CatalogSource, CollectionDecl, ControlBootstrapMode, ControlBrowserAuthConfig,
        ControlChangeSet, ControlOperation, ControlScope, ControlSnapshot, ControlStore,
        DriverFactory, FeaturePage, FeatureSource, FileStyleStore, Filter, InMemoryControlStore,
        ItemsQuery, MokaTileCache, MutationKind, Obligation, OutboxSource, PhysicalCollection,
        PrincipalIdentity, RasterSource, RasterWindow, Registry, Resolver, Result as CoreResult,
        RoleBinding, Router as CoreRouter, Sequence, StaticResolver, StorageDecl, StorageDriver,
        StyleStore, TileCache, TileCoord, TileSource, VersionedControlOperation,
        WebhookConsumerSettings, WebhookDeliverer, WebhookRetryPolicy, WebhookSubscriptionRuntime,
    };

    struct DeadLetterOutbox {
        obligations: Vec<Obligation>,
    }

    #[async_trait::async_trait]
    impl OutboxSource for DeadLetterOutbox {
        async fn read_after(
            &self,
            _collection: &CollectionDecl,
            after: Sequence,
            limit: u32,
        ) -> CoreResult<Vec<Obligation>> {
            Ok(self
                .obligations
                .iter()
                .filter(|obligation| obligation.sequence > after)
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn primary_high_water(&self, _collection: &CollectionDecl) -> CoreResult<Sequence> {
            Ok(self
                .obligations
                .last()
                .map(|obligation| obligation.sequence)
                .unwrap_or(Sequence(0)))
        }
    }

    struct AlwaysFailWebhook;

    #[async_trait::async_trait]
    impl WebhookDeliverer for AlwaysFailWebhook {
        async fn deliver(&self, _url: &str, _body: &[u8], _signature: &str) -> bool {
            false
        }
    }

    #[derive(Clone, Default)]
    struct TraceCapture(Arc<Mutex<Vec<u8>>>);

    struct TraceWriter(Arc<Mutex<Vec<u8>>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TraceCapture {
        type Writer = TraceWriter;

        fn make_writer(&'a self) -> Self::Writer {
            TraceWriter(Arc::clone(&self.0))
        }
    }

    impl Write for TraceWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A `CatalogSource` that reports no collections — this module's tests
    /// exercise the built app directly, not `Router::validate_catalog`, so
    /// this is present only to satisfy the trait.
    struct EmptyCatalog;

    #[async_trait::async_trait]
    impl CatalogSource for EmptyCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![])
        }
    }

    struct FakeBackend;

    #[async_trait::async_trait]
    impl FeatureSource for FakeBackend {
        async fn items(
            &self,
            _collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> CoreResult<FeaturePage> {
            Ok(FeaturePage {
                features_geojson: vec![],
                number_matched: Some(0),
                next_token: None,
            })
        }

        async fn item(
            &self,
            _collection: &CollectionDecl,
            _id: &str,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<serde_json::Value>> {
            Ok(None)
        }
    }

    #[async_trait::async_trait]
    impl TileSource for FakeBackend {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<bytes::Bytes>> {
            Ok(None)
        }
    }

    struct FakeDriver;

    impl StorageDriver for FakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(FakeBackend) as Arc<dyn FeatureSource>)
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            Some(Arc::new(FakeBackend) as Arc<dyn TileSource>)
        }
    }

    struct FakeFactory;

    impl DriverFactory for FakeFactory {
        fn name(&self) -> &str {
            "fake"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FakeDriver))
        }
    }

    /// A `WriteSink` that accepts every mutation (`#208`): the point of the
    /// fixture is that the write genuinely lands, so the `Allow` header that
    /// promised it can be checked against a real outcome rather than against
    /// another assertion about the same header.
    struct AcceptingSink;

    #[async_trait::async_trait]
    impl tellurion_core::WriteSink for AcceptingSink {
        async fn apply(
            &self,
            _collection: &CollectionDecl,
            _mutation: tellurion_core::Mutation,
        ) -> CoreResult<Sequence> {
            Ok(Sequence(1))
        }

        /// Overridden so the items-collection half of the pair can execute
        /// its advertised `POST` too, rather than stopping at the trait's
        /// default `CapabilityUnsupported("create")` refusal and leaving
        /// that direction asserted only on the header.
        async fn create(
            &self,
            _collection: &CollectionDecl,
            _feature: serde_json::Value,
        ) -> CoreResult<(String, Sequence)> {
            Ok(("minted-1".to_string(), Sequence(1)))
        }
    }

    /// [`FakeDriver`] plus a live `write_sink` — the "this deployment really
    /// can write here" half of `#208`'s fixture. `FakeDriver` itself
    /// deliberately keeps returning `None` for `write_sink`, so the two
    /// drivers in one config give a genuinely writable collection and a
    /// genuinely read-only one side by side.
    struct WritableFakeDriver;

    impl StorageDriver for WritableFakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(FakeBackend) as Arc<dyn FeatureSource>)
        }

        fn write_sink(&self) -> Option<Arc<dyn tellurion_core::WriteSink>> {
            Some(Arc::new(AcceptingSink) as Arc<dyn tellurion_core::WriteSink>)
        }
    }

    struct WritableFakeFactory;

    impl DriverFactory for WritableFakeFactory {
        fn name(&self) -> &str {
            "fake-write"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(WritableFakeDriver))
        }
    }

    /// Holds every `items` call at a deterministic gate so a low
    /// `server.max_concurrency` can be proven to bind workload traffic.
    struct SlowBackend {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl FeatureSource for SlowBackend {
        async fn items(
            &self,
            _collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> CoreResult<FeaturePage> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(FeaturePage {
                features_geojson: vec![],
                number_matched: Some(0),
                next_token: None,
            })
        }

        async fn item(
            &self,
            _collection: &CollectionDecl,
            _id: &str,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<serde_json::Value>> {
            Ok(None)
        }
    }

    struct SlowDriver {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    impl StorageDriver for SlowDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(SlowBackend {
                entered: Arc::clone(&self.entered),
                release: Arc::clone(&self.release),
            }) as Arc<dyn FeatureSource>)
        }
    }

    struct SlowFactory {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    impl DriverFactory for SlowFactory {
        fn name(&self) -> &str {
            "slow"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(SlowDriver {
                entered: Arc::clone(&self.entered),
                release: Arc::clone(&self.release),
            }))
        }
    }

    /// A bare recorder/handle, never installed globally and with no HTTP
    /// listener of its own, so many tests can each build one without
    /// tripping metrics' "recorder already installed" panic or fighting
    /// over a bound port.
    fn test_metrics_handle() -> PrometheusHandle {
        PrometheusBuilder::new().build_recorder().handle()
    }

    /// Every acceptance/route test in this module reaches the "demo"
    /// collection through this same tenant/catalog pair: external ids
    /// `public`/`default`, matching their internal ids exactly (this fixture
    /// is not testing renames — the dedicated `#39` acceptance tests below
    /// use their own configs with genuinely distinct internal/external ids).
    const TENANT_EXT: &str = "public";
    const CATALOG_EXT: &str = "default";

    fn test_app() -> Router {
        build(test_ctx(), test_metrics_handle(), 60)
    }

    fn test_app_with_control_browser() -> Router {
        let ctx = test_ctx();
        let browser = crate::control_browser_auth::ControlBrowserAuth::new(
            ControlBrowserAuthConfig {
                issuer: "https://id.example.com".to_string(),
                client_id: "control-ui".to_string(),
                client_secret_env: None,
                public_origin: "https://console.example.com".to_string(),
                scopes: vec!["openid".to_string()],
                session_ttl_s: 3_600,
                login_ttl_s: 300,
                max_sessions: 16,
            },
            None,
            std::iter::empty(),
            &ctx,
        )
        .unwrap();
        build_with_webhook_registry_and_control_browser(
            ctx,
            test_metrics_handle(),
            60,
            Readiness::new(),
            Arc::new(WebhookRegistry::new()),
            None,
            Some(browser),
        )
    }

    fn test_ctx() -> Arc<AppContext> {
        test_ctx_with_catalog_settings("{}")
    }

    #[cfg(not(feature = "public-demo"))]
    #[tokio::test]
    async fn public_demo_routes_are_absent_without_the_feature() {
        let response = get(&test_app(), "/demo/sources/not-a-source").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn full_app_cors_preflight_for_control_auth_is_no_store_without_affecting_other_paths() {
        let app = test_app_with_control_browser();
        let auth_preflight = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/_auth/control/session")
                    .header(header::ORIGIN, "https://console.example.com")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(auth_preflight.status(), StatusCode::OK);
        assert_eq!(
            auth_preflight.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );

        let unrelated = get(&app, "/").await;
        assert!(unrelated.headers().get(header::CACHE_CONTROL).is_none());
    }

    #[cfg(feature = "public-demo")]
    #[tokio::test]
    async fn outer_demo_cache_guard_covers_router_generated_responses() {
        for method in [Method::PUT, Method::OPTIONS] {
            let response = test_app()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/demo/sources")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.headers().get(header::CACHE_CONTROL).unwrap(),
                "private, no-store"
            );
        }
    }

    #[cfg(all(feature = "public-demo", feature = "ui"))]
    #[tokio::test]
    async fn dedicated_public_demo_exposes_only_the_evaluation_surface() {
        let app = build_public_demo(test_ctx(), 60, Readiness::new());
        let response = get(&app, "/").await;
        assert!(response.status().is_redirection());
        assert_eq!(response.headers().get(header::LOCATION).unwrap(), "/ui/");

        for private_path in [
            "/service",
            "/metrics",
            "/config/effective",
            "/config/profiles",
            "/public",
        ] {
            assert_eq!(
                get(&app, private_path).await.status(),
                StatusCode::NOT_FOUND
            );
        }

        let ordinary_root = get(&test_app(), "/").await;
        assert_eq!(ordinary_root.status(), StatusCode::OK);
        assert_eq!(
            ordinary_root.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }

    /// The same fixture as [`test_ctx`], with an inline `settings:` block on
    /// the `default` catalog — how every `#185` exposure test below turns one
    /// protocol (or the write lane) off without restating the whole config.
    fn test_ctx_with_catalog_settings(settings: &str) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
server:
  metrics_collection_allowlist:
    - {{ tenant: public, catalog: default, collection: demo }}
storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public, settings: {settings} }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    places3d: {{ height_property: height }}
"#
        ))
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ))
    }

    /// `/{TENANT_EXT}/{protocol}/catalogs/{CATALOG_EXT}` — the catalog root
    /// prefix every protocol-scoped test below builds its path from.
    fn catalog_root(protocol: &str) -> String {
        format!("/{TENANT_EXT}/{protocol}/catalogs/{CATALOG_EXT}")
    }

    async fn get(app: &Router, path: &str) -> Response {
        app.clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn json_body(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// `#208`'s fixture: one catalog holding two collections that differ in
    /// exactly one respect — `writable` routes its write lane at a storage
    /// whose driver advertises a `WriteSink`, `demo` declares no
    /// `routing.write` at all (there is no "defaults to the single storage"
    /// fallback for write; see `Router::resolve_write`). Everything else —
    /// tenant, catalog, exposure matrix, pinned physical fields — is
    /// identical, so any difference the tests below observe is the write
    /// lane and nothing else.
    ///
    /// No `auth:` block, so `state.authorizer` stays `None` and the write
    /// lane's policy checkpoint allows through: this fixture is testing
    /// capability, which is the thing `Allow` reports, not authorization,
    /// which is the thing it deliberately does not.
    fn write_capability_ctx() -> Arc<AppContext> {
        write_capability_ctx_with_catalog_settings("{}")
    }

    /// The same fixture with an inline `settings:` block on the catalog, so
    /// `#185`'s exposure matrix can be turned off over a collection that
    /// genuinely CAN write. Without one, a `features_write: disabled` test
    /// aimed at a collection with no write lane would pass on `#208`'s
    /// narrowing alone and quietly stop testing the matrix at all.
    fn write_capability_ctx_with_catalog_settings(settings: &str) -> Arc<AppContext> {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages:
  - {{ id: main, driver: fake, url_env: DATABASE_URL }}
  - {{ id: writer, driver: fake-write, url_env: DATABASE_URL }}
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public, settings: {settings} }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
  - id: writable
    catalog: default
    storage: writer
    table: writable
    geometry: geom
    pk: id
    routing: {{ write: writer }}
"#
        ))
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        registry.register(Arc::new(WritableFakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ))
    }

    fn write_capability_app() -> Router {
        build(write_capability_ctx(), test_metrics_handle(), 60)
    }

    /// The `Allow` value a plain `OPTIONS` reports for `path`.
    async fn allow_of(app: &Router, path: &str) -> String {
        let response = options(app, path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        response
            .headers()
            .get(header::ALLOW)
            .unwrap_or_else(|| {
                panic!("a plain OPTIONS response must carry an Allow header: {path}")
            })
            .to_str()
            .unwrap()
            .to_string()
    }

    async fn send(app: &Router, method: &str, path: &str) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::CONTENT_TYPE, "application/geo+json")
                    .body(Body::from(
                        r#"{"type":"Feature","geometry":null,"properties":{}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn test_tile_content(Query(query): Query<HashMap<String, String>>) -> Response {
        let content_type = match query.get("format").map(String::as_str) {
            Some("png") => "image/png",
            _ => "application/vnd.mapbox-vector-tile",
        };
        ([(header::CONTENT_TYPE, content_type)], Body::empty()).into_response()
    }

    fn lane_observation_app(ctx: Arc<AppContext>) -> Router {
        Router::new()
            .route("/", axum::routing::get(|| async { StatusCode::OK }))
            .route(
                "/{tenant}/features/catalogs/{catalog}/collections/{cid}/items",
                axum::routing::get(|| async { StatusCode::OK }),
            )
            .route(
                "/{tenant}/features/catalogs/{catalog}/collections/{cid}/__test_slow",
                axum::routing::get(slow_for_observation_test),
            )
            .route(
                "/{tenant}/tiles/catalogs/{catalog}/collections/{cid}/tiles",
                axum::routing::get(|| async { StatusCode::OK }),
            )
            .route(
                "/{tenant}/tiles/catalogs/{catalog}/collections/{cid}/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}",
                axum::routing::get(test_tile_content),
            )
            .route(
                "/{tenant}/tiles/catalogs/{catalog}/collections/{cid}/styles/{styleId}/map/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}",
                axum::routing::get(|| async {
                    ([(header::CONTENT_TYPE, "image/png")], Body::empty())
                }),
            )
            .route(
                "/{tenant}/3dtiles/catalogs/{catalog}/collections/{cid}/3dtiles",
                axum::routing::get(|| async { StatusCode::OK }),
            )
            .route(
                "/{tenant}/styles/catalogs/{catalog}/styles",
                axum::routing::get(|| async { StatusCode::OK }),
            )
            .route(
                "/{tenant}/stac/catalogs/{catalog}/collections",
                axum::routing::get(|| async { StatusCode::OK }),
            )
            .fallback(|| async { StatusCode::NOT_FOUND })
            .with_state(Arc::clone(&ctx))
            .layer(axum::middleware::from_fn_with_state(
                ctx,
                srv_metrics::observe_request,
            ))
    }

    /// `#39` acceptance test 1 (top-level slice): `/` is a minimal service
    /// descriptor — self link only, no `data`/`tiles`/`styles`/`conformance`/
    /// `service-desc` the way a protocol root's landing page has, and
    /// critically no tenant id anywhere in the body.
    #[tokio::test]
    async fn top_level_landing_page_is_minimal_and_never_names_a_tenant() {
        let app = test_app();
        let response = get(&app, "/").await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !text.contains(TENANT_EXT),
            "top-level landing page body must never mention a tenant id: {text}"
        );
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let rels: Vec<&str> = json["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|link| link["rel"].as_str().unwrap())
            .collect();
        assert_eq!(rels, vec!["self"]);
    }

    /// `#39` acceptance test 1: every protocol root has its own landing page
    /// with the classes/links belonging to it alone.
    #[tokio::test]
    async fn protocol_landing_pages_carry_their_own_links() {
        let app = test_app();

        let features = json_body(get(&app, &catalog_root("features")).await).await;
        let features_rels: Vec<&str> = features["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|link| link["rel"].as_str().unwrap())
            .collect();
        assert!(features_rels.contains(&"self"));
        assert!(features_rels.contains(&"conformance"));
        assert!(features_rels.contains(&"service-desc"));
        assert!(features_rels.contains(&"data"));
        assert!(!features_rels.contains(&"tiles"));
        assert!(!features_rels.contains(&"styles"));

        let tiles = json_body(get(&app, &catalog_root("tiles")).await).await;
        let tiles_rels: Vec<&str> = tiles["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|link| link["rel"].as_str().unwrap())
            .collect();
        assert!(tiles_rels.contains(&"tiles"));
        assert!(!tiles_rels.contains(&"data"));
    }

    /// `#39` acceptance test 1: conformance is per protocol root, not one
    /// shared aggregate — a features root cites features classes and not
    /// tiles classes, and vice versa.
    #[tokio::test]
    async fn protocol_conformance_is_scoped_to_its_own_protocol() {
        let app = test_app();

        let features_response =
            get(&app, &format!("{}/conformance", catalog_root("features"))).await;
        assert_eq!(features_response.status(), StatusCode::OK);
        let features = json_body(features_response).await;
        let features_classes: Vec<&str> = features["conformsTo"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert!(features_classes
            .iter()
            .any(|c| c.contains("ogcapi-common-1")));
        assert!(features_classes
            .iter()
            .any(|c| c.contains("ogcapi-features-1")));
        assert!(
            !features_classes
                .iter()
                .any(|c| c.contains("ogcapi-tiles-1")),
            "the features root must not cite tiles conformance classes"
        );

        let tiles_response = get(&app, &format!("{}/conformance", catalog_root("tiles"))).await;
        assert_eq!(tiles_response.status(), StatusCode::OK);
        let tiles = json_body(tiles_response).await;
        let tiles_classes: Vec<&str> = tiles["conformsTo"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert!(tiles_classes.iter().any(|c| c.contains("ogcapi-tiles-1")));
        assert!(
            !tiles_classes
                .iter()
                .any(|c| c.contains("ogcapi-features-1")),
            "the tiles root must not cite features conformance classes"
        );
    }

    /// `#86`: the tiles root's `/conformance` also cites OGC API — Maps
    /// Part 1's own Core and PNG conformance classes, since
    /// `/collections/{cid}/map` is mounted on this same protocol root.
    /// `#229` adds CRS (the `crs` parameter was always implemented, never
    /// declared) and pins the two classes that must stay UNDECLARED while
    /// their own parameters (`subset`/`center`, `scale-denominator`) are
    /// unimplemented — an advertised class this lane does not honour is
    /// exactly the bug `#229` set out to remove.
    #[tokio::test]
    async fn tiles_conformance_declares_the_maps_part_1_classes_it_honours() {
        let app = test_app();
        let response = get(&app, &format!("{}/conformance", catalog_root("tiles"))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let classes: Vec<String> = json_body(response).await["conformsTo"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        assert!(classes.contains(&tellurion_tiles::CONFORMANCE_MAPS_CORE.to_string()));
        assert!(classes.contains(&tellurion_tiles::CONFORMANCE_MAPS_CRS.to_string()));
        assert!(classes.contains(&tellurion_tiles::CONFORMANCE_MAPS_PNG.to_string()));
        for undeclared in [
            "ogcapi-maps-1/1.0/conf/spatial-subsetting",
            "ogcapi-maps-1/1.0/conf/scaling",
        ] {
            assert!(
                !classes.iter().any(|class| class.contains(undeclared)),
                "{undeclared} is not implemented and must not be advertised"
            );
        }
    }

    /// `#39` acceptance test 1: each protocol root serves its OWN `/api`
    /// document, not one shared aggregate.
    #[tokio::test]
    async fn protocol_api_doc_serves_the_matching_embedded_document() {
        let app = test_app();

        for protocol in ["features", "tiles", "styles", "3dtiles", "stac"] {
            let response = get(&app, &format!("{}/api", catalog_root(protocol))).await;
            assert_eq!(response.status(), StatusCode::OK, "protocol: {protocol}");
            let json = json_body(response).await;
            assert_eq!(json["openapi"], "3.0.3", "protocol: {protocol}");
        }
    }

    #[tokio::test]
    async fn features_api_doc_advertises_json_merge_patch() {
        let app = test_app();
        let response = get(&app, &format!("{}/api", catalog_root("features"))).await;
        let json = json_body(response).await;
        assert_eq!(
            json["paths"]["/collections/{collectionId}/items/{featureId}"]["patch"]["requestBody"]
                ["content"]["application/merge-patch+json"]["schema"],
            serde_json::json!({})
        );
    }

    #[tokio::test]
    async fn item_api_docs_advertise_exact_geometry_budget_refusals() {
        let app = test_app();
        for (protocol, paths) in [
            (
                "features",
                vec![
                    "/collections/{collectionId}/items",
                    "/collections/{collectionId}/items/{featureId}",
                ],
            ),
            (
                "stac",
                vec![
                    "/collections/{collectionId}/items",
                    "/collections/{collectionId}/items/{itemId}",
                    "/search",
                ],
            ),
        ] {
            let response = get(&app, &format!("{}/api", catalog_root(protocol))).await;
            let json = json_body(response).await;
            for path in paths {
                assert_eq!(
                    json["paths"][path]["get"]["responses"]["422"]["$ref"],
                    "#/components/responses/itemsVertexBudgetExceeded",
                    "{protocol} {path}"
                );
            }
            assert!(
                json["components"]["responses"]["itemsVertexBudgetExceeded"]["description"]
                    .as_str()
                    .unwrap()
                    .contains("Exact geometry")
            );
        }
    }

    #[tokio::test]
    async fn metrics_endpoint_serves_prometheus_text() {
        let app = test_app();
        let response = get(&app, "/metrics").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; version=0.0.4"
        );
    }

    #[tokio::test]
    async fn healthz_is_always_live_even_while_draining() {
        let readiness = crate::readiness::Readiness::new();
        let app = build_with_readiness(test_ctx(), test_metrics_handle(), 60, readiness.clone());

        assert_eq!(get(&app, "/healthz").await.status(), StatusCode::OK);
        readiness.begin_draining();
        assert_eq!(get(&app, "/healthz").await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn readyz_is_a_generic_problem_until_a_probe_succeeds() {
        let ctx = test_ctx();
        let readiness = crate::readiness::Readiness::new();
        let app = build_with_readiness(
            Arc::clone(&ctx),
            test_metrics_handle(),
            60,
            readiness.clone(),
        );

        let response = get(&app, "/readyz").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let body = json_body(response).await;
        assert_eq!(body["status"], 503);
        assert_eq!(body["code"], "ServiceUnavailable");
        assert_eq!(body["detail"], "server is not ready");
        assert_eq!(body.as_object().unwrap().len(), 5);

        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        assert_eq!(get(&app, "/readyz").await.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn draining_makes_readyz_false_and_a_late_probe_cannot_restore_it() {
        let ctx = test_ctx();
        let readiness = crate::readiness::Readiness::new();
        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        let app = build_with_readiness(
            Arc::clone(&ctx),
            test_metrics_handle(),
            60,
            readiness.clone(),
        );
        assert_eq!(get(&app, "/readyz").await.status(), StatusCode::OK);

        readiness.begin_draining();
        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;

        let response = get(&app, "/readyz").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let text = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(!text.contains("main"));
        assert!(!text.contains("fake"));
    }

    #[tokio::test]
    async fn features_router_is_mounted_and_reachable() {
        let app = test_app();
        let response = get(&app, &format!("{}/collections", catalog_root("features"))).await;
        assert_eq!(response.status(), StatusCode::OK);

        let json = json_body(response).await;
        assert_eq!(json["collections"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn tiles_router_is_mounted_and_reachable() {
        let app = test_app();
        let response = get(&app, &format!("{}/tileMatrixSets", catalog_root("tiles"))).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// `#86`: `/collections/{cid}/map` is reachable through the real server
    /// mount, on the tiles protocol root — full param/render coverage lives
    /// in `tellurion-tiles`' own `maps.rs` tests.
    ///
    /// `#270`: the `bbox` here is `WebMercatorQuad` metres (its latitudes,
    /// ±100, are not even expressible in CRS84), so it now declares
    /// `bbox-crs`. Without the declaration Maps Part 1 Requirement 18
    /// clause C makes those numbers degrees, and this lane refuses them by
    /// name rather than reading them as a window nobody asked for. The
    /// window and the `Content-Bbox` this asserts are unchanged — only the
    /// request now says which CRS its four numbers are in.
    #[tokio::test]
    async fn maps_endpoint_is_mounted_and_reachable() {
        let app = test_app();
        let response = get(
            &app,
            &format!(
                "{}/collections/demo/map?bbox=-100,-100,100,100\
                 &bbox-crs=http://www.opengis.net/def/crs/EPSG/0/3857\
                 &width=32&height=32",
                catalog_root("tiles")
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/png"
        );
        // `#229`: Maps Part 1 `/req/core/map-response` C/D/E — every map
        // response georeferences itself, through the real mount too.
        assert!(response.headers().get("content-crs").is_some());
        assert_eq!(
            response.headers().get("content-bbox").unwrap(),
            "-100,-100,100,100"
        );
    }

    #[tokio::test]
    async fn places_router_is_mounted_and_reachable() {
        let app = test_app();
        let response = get(
            &app,
            &format!("{}/collections/demo/3dtiles", catalog_root("3dtiles")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn styles_router_is_mounted_and_reachable() {
        let app = test_app();
        let response = get(&app, &format!("{}/styles", catalog_root("styles"))).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn stac_router_is_mounted_and_reachable() {
        let app = test_app();
        let response = get(&app, &format!("{}/collections", catalog_root("stac"))).await;
        assert_eq!(response.status(), StatusCode::OK);

        let json = json_body(response).await;
        assert_eq!(json["collections"].as_array().unwrap().len(), 1);
        assert_eq!(json["collections"][0]["type"], "Collection");
    }

    /// `#36` slice C: `/search` is reachable through the real server mount
    /// (both methods), same "mounted and reachable" style as
    /// `stac_router_is_mounted_and_reachable` above — the exhaustive
    /// parameter/paging/filter/capability tests live in
    /// `tellurion-stac`'s own `tests/handlers.rs`.
    #[tokio::test]
    async fn stac_search_is_mounted_and_reachable_over_get_and_post() {
        let app = test_app();

        let get_response = get(&app, &format!("{}/search", catalog_root("stac"))).await;
        assert_eq!(get_response.status(), StatusCode::OK);
        let get_json = json_body(get_response).await;
        assert_eq!(get_json["type"], "FeatureCollection");

        let post_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{}/search", catalog_root("stac")))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(post_response.status(), StatusCode::OK);
        let post_json = json_body(post_response).await;
        assert_eq!(post_json["type"], "FeatureCollection");
    }

    /// `#36` slice C: the STAC landing page is a genuine STAC Catalog
    /// object (`type: Catalog`, `stac_version`, an embedded `conformsTo`),
    /// not the generic OGC API Common landing page every other protocol
    /// root shares, and now links `search` twice — once per method, per the
    /// item-search spec's own landing-page example (`landing::stac_landing`'s
    /// doc).
    #[tokio::test]
    async fn stac_landing_page_is_a_stac_catalog() {
        let app = test_app();
        let response = get(&app, &catalog_root("stac")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;

        assert_eq!(json["type"], "Catalog");
        assert_eq!(json["stac_version"], "1.1.0");
        assert_eq!(json["id"], CATALOG_EXT);
        assert!(json["description"].is_string());
        assert!(json["conformsTo"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "https://api.stacspec.org/v1.0.0/core"));
        assert!(json["conformsTo"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "https://api.stacspec.org/v1.0.0/collections"));

        let links = json["links"].as_array().unwrap();
        let rels: Vec<&str> = links
            .iter()
            .map(|link| link["rel"].as_str().unwrap())
            .collect();
        assert!(rels.contains(&"self"));
        assert!(rels.contains(&"root"));
        assert!(rels.contains(&"conformance"));
        assert!(rels.contains(&"service-desc"));
        assert!(rels.contains(&"data"));

        let search_links: Vec<&serde_json::Value> = links
            .iter()
            .filter(|link| link["rel"] == "search")
            .collect();
        assert_eq!(
            search_links.len(),
            2,
            "expected one 'search' link per method (GET and POST): {links:?}"
        );
        let methods: Vec<&str> = search_links
            .iter()
            .map(|link| link["method"].as_str().unwrap())
            .collect();
        assert!(methods.contains(&"GET"));
        assert!(methods.contains(&"POST"));
        for link in &search_links {
            assert_eq!(link["type"], "application/geo+json");
            assert_eq!(link["href"], catalog_root("stac") + "/search");
        }
    }

    /// `#36` slice C: the STAC root's `/conformance` cites STAC API - Core,
    /// Collections, Features (`#36` slice B), and now Item Search plus its
    /// Filter Extension class (`#36` slice C, `/search` exists).
    ///
    /// No CQL2 class here any more (`#105`): `FakeBackend` (this module's
    /// fixture driver) never overrides `FeatureSource::filter_capable` or
    /// `cql2_conformance_classes` (both stay at the trait default), so the
    /// honest, per-deployment intersection `Router::cql2_conformance_classes`
    /// computes is empty for this fixture — this test asserted `basic-cql2`
    /// before `#105` only because the old workspace-wide list declared it
    /// unconditionally, regardless of what this fixture driver could
    /// actually compile.
    #[tokio::test]
    async fn stac_conformance_declares_core_collections_features_and_item_search() {
        let app = test_app();
        let response = get(&app, &format!("{}/conformance", catalog_root("stac"))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        let classes: Vec<&str> = json["conformsTo"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert!(classes.contains(&"https://api.stacspec.org/v1.0.0/core"));
        assert!(classes.contains(&"https://api.stacspec.org/v1.0.0/collections"));
        assert!(classes.contains(&"https://api.stacspec.org/v1.0.0/ogcapi-features"));
        assert!(classes.contains(&"http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core"));
        assert!(classes.contains(&"https://api.stacspec.org/v1.0.0/item-search"));
        assert!(
            !classes.contains(&"http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2"),
            "this fixture's driver never declares any CQL2 class, so the honest \
             per-deployment intersection must not claim one either"
        );
        // `#248`: and for exactly the same reason it must not claim the Item
        // Search Filter class either. That class *binds* Filter and Basic
        // CQL2 to `/search` (the extension's own words), so declaring it in a
        // document that withholds every CQL2 class — as this one correctly
        // does, and did before `#248` — asserted a binding to something the
        // very same response denied. This fixture's driver leaves
        // `FeatureSource::filter_capable` at its `false` default, so
        // `Router::item_search_filter_conformance_classes` folds the class
        // away.
        assert!(
            !classes.contains(&tellurion_core::filter::ITEM_SEARCH_FILTER_CLASS),
            "a deployment whose driver answers 400 to every filter must not claim the STAC \
             Item Search Filter class"
        );
    }

    /// `#39` acceptance test 1: the `/{tenant}/` directory doc links every
    /// catalog it owns, crossed with every protocol.
    #[tokio::test]
    async fn tenant_directory_lists_every_protocol_for_the_catalog() {
        let app = test_app();
        let response = get(&app, &format!("/{TENANT_EXT}")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let json = json_body(response).await;
        assert_eq!(json["tenant"], TENANT_EXT);
        let hrefs: Vec<&str> = json["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|link| link["href"].as_str().unwrap())
            .collect();
        for protocol in ["features", "tiles", "styles", "3dtiles", "stac"] {
            assert!(
                hrefs.contains(&catalog_root(protocol).as_str()),
                "missing {protocol} href in {hrefs:?}"
            );
        }
    }

    #[tokio::test]
    async fn unknown_tenant_directory_is_not_found() {
        let app = test_app();
        let response = get(&app, "/nonexistent-tenant").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // -- tenant directory catalog paging (`#42`, `#59`) ----------------------

    /// How many protocol roots a catalog that declares no `protocols:`
    /// block at all advertises in the tenant directory — every variant of
    /// `Protocol::ALL` whose default exposure is `enabled`. Derived from the
    /// default matrix rather than hardcoded, so a future root that defaults
    /// on is counted automatically and one that defaults off (as `records`
    /// does, `#192`) is not.
    fn exposed_by_default() -> usize {
        let matrix = tellurion_core::ProtocolsConf::default();
        Protocol::ALL
            .iter()
            .filter(|protocol| protocol.exposure(&matrix).is_enabled())
            .count()
    }

    fn build_multi_catalog_app() -> Router {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs:
  - { id: alpha, tenant: public }
  - { id: bravo, tenant: public }
  - { id: charlie, tenant: public }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        build(ctx, test_metrics_handle(), 60)
    }

    /// A tenant with fewer catalogs than the default page size still gets
    /// every catalog back on the one, only page — no `next` link, same
    /// no-regression guard `/collections` pagination already has.
    #[tokio::test]
    async fn tenant_directory_default_limit_returns_every_catalog_on_one_page() {
        let app = build_multi_catalog_app();
        let response = get(&app, "/public").await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        let links = json["links"].as_array().unwrap();
        assert!(
            links.iter().all(|link| link["rel"] != "next"),
            "a tenant smaller than the default page size must not paginate: {links:?}"
        );
        // self + 3 catalogs, each crossed with every EXPOSED protocol. The
        // Records root (`#192`) defaults to `disabled` and so is absent
        // here, which is the point: this count is the same one it was
        // before that root existed.
        assert_eq!(links.len(), 1 + 3 * exposed_by_default());
    }

    /// The paging round trip: `limit=1` over three catalogs returns the
    /// first (in stable, external-id order) plus a `next` link; walking
    /// that link returns the second catalog, same mechanism `/collections`
    /// pagination exercises.
    #[tokio::test]
    async fn tenant_directory_paginates_catalogs_with_a_limit_and_a_next_link() {
        let app = build_multi_catalog_app();

        let first = get(&app, "/public?limit=1").await;
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = json_body(first).await;
        let first_links = first_body["links"].as_array().unwrap();
        // self + one catalog's worth of protocol links + next.
        assert_eq!(first_links.len(), 1 + exposed_by_default() + 1);
        assert!(
            first_links
                .iter()
                .any(|link| link["href"].as_str().unwrap().contains("/catalogs/alpha")),
            "the first page must be the alpha catalog: {first_links:?}"
        );
        let next_href = first_links
            .iter()
            .find(|link| link["rel"] == "next")
            .expect("a next link when more catalogs remain")["href"]
            .as_str()
            .unwrap()
            .to_string();

        let second = get(&app, &next_href).await;
        assert_eq!(second.status(), StatusCode::OK);
        let second_body = json_body(second).await;
        let second_links = second_body["links"].as_array().unwrap();
        assert!(
            second_links
                .iter()
                .any(|link| link["href"].as_str().unwrap().contains("/catalogs/bravo")),
            "the second page must be the bravo catalog: {second_links:?}"
        );
        assert!(
            second_links.iter().any(|link| link["rel"] == "next"),
            "not the last page yet — charlie still remains: {second_links:?}"
        );
    }

    #[tokio::test]
    async fn tenant_directory_rejects_a_zero_limit() {
        let app = build_multi_catalog_app();
        let response = get(&app, "/public?limit=0").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_route_is_not_found() {
        let app = test_app();
        let response = get(&app, "/nope").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn saml_protocol_routes_are_not_exposed() {
        let app = test_app();

        for path in ["/saml/metadata", "/saml/login", "/.well-known/saml"] {
            let response = get(&app, path).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_metrics_keep_hostile_identifiers_out_of_labels() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _recorder_guard = metrics::set_default_local_recorder(&recorder);
        let app = build(test_ctx(), handle.clone(), 60);

        let response = get(
            &app,
            "/public/features/catalogs/default/collections/demo/items",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let hostile = "tenant-secret-6741";
        let feature_secret = "feature-secret-9137";
        let response = get(
            &app,
            &format!(
                "/{hostile}/features/catalogs/default/collections/demo/items/{feature_secret}?token=credential-secret"
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let text = handle.render();
        assert!(text.contains("lane=\"features\""), "{text}");
        assert!(text.contains("tenant=\"other\""), "{text}");
        assert!(!text.contains("tenant=\"public\""), "{text}");
        assert!(
            text.contains("collection=\"public/default/demo\""),
            "{text}"
        );
        assert!(text.contains("collection=\"other\""), "{text}");
        assert!(text.contains("tenant=\"unknown\""), "{text}");
        assert!(
            !text.contains(hostile),
            "raw tenant leaked into metrics: {text}"
        );
        assert!(
            !text.contains(feature_secret),
            "feature id leaked into metrics: {text}"
        );
        assert!(
            !text.contains("credential-secret"),
            "query leaked into metrics: {text}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn real_observation_middleware_records_every_bounded_lane() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _recorder_guard = metrics::set_default_local_recorder(&recorder);
        let app = lane_observation_app(test_ctx());
        let paths = [
            "/",
            "/public/features/catalogs/default/collections/demo/items",
            "/public/tiles/catalogs/default/collections/demo/tiles",
            "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0?format=mvt",
            "/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0?format=png",
            "/public/tiles/catalogs/default/collections/demo/styles/basic/map/tiles/WebMercatorQuad/0/0/0",
            "/public/3dtiles/catalogs/default/collections/demo/3dtiles",
            "/public/styles/catalogs/default/styles",
            "/public/stac/catalogs/default/collections",
            "/hostile-unmatched-secret",
        ];
        for path in paths {
            let _ = get(&app, path).await;
        }

        let text = handle.render();
        for lane in [
            "features",
            "tiles",
            "mvt",
            "png",
            "styled_png",
            "places3d",
            "styles",
            "stac",
            "control",
            "unmatched",
        ] {
            assert!(
                text.contains(&format!("lane=\"{lane}\"")),
                "missing {lane}: {text}"
            );
        }
        assert!(text.contains("path=\"unmatched\""), "{text}");
        assert!(!text.contains("hostile-unmatched-secret"), "{text}");
    }

    #[test]
    fn request_observation_reads_a_reloaded_slow_threshold() {
        let ctx = test_ctx();
        assert_eq!(
            srv_metrics::current_control_slow_threshold(&ctx),
            Duration::from_millis(1000)
        );

        let mut config = ctx.current().config.clone();
        config.settings.slow_request_ms = Some(77);
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        ctx.reload(config, router, resolver, None);

        assert_eq!(
            srv_metrics::current_control_slow_threshold(&ctx),
            Duration::from_millis(77)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unresolved_collection_ignores_catalog_slow_threshold() {
        let ctx = test_ctx();
        let mut config = ctx.current().config.clone();
        config.settings.slow_request_ms = Some(200);
        config.tenants[0].settings.slow_request_ms = Some(100);
        config.catalogs[0].settings.slow_request_ms = Some(1);
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        ctx.reload(config, router, resolver, None);
        let app = lane_observation_app(ctx);

        let capture = TraceCapture::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .without_time()
            .with_writer(capture.clone())
            .finish();
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);

        assert_eq!(
            get(
                &app,
                "/public/features/catalogs/default/collections/missing/__test_slow",
            )
            .await
            .status(),
            StatusCode::OK
        );
        let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert!(!output.contains("slow_request"), "{output}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn middleware_emits_one_private_slow_event_only_above_threshold() {
        let ctx = test_ctx();
        let app = build(Arc::clone(&ctx), test_metrics_handle(), 60);

        let capture = TraceCapture::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .without_time()
            .with_writer(capture.clone())
            .finish();
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);

        assert_eq!(get(&app, "/").await.status(), StatusCode::OK);

        let mut config = ctx.current().config.clone();
        config.settings.slow_request_ms = Some(10);
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        ctx.reload(config, router, resolver, None);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/__test_slow?token=query-secret&feature=feature-secret&style=style-secret&tile=tile-secret&internal=internal-secret&table=physical-secret&error=error-secret")
                    .header(header::AUTHORIZATION, "Bearer header-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        let slow: Vec<serde_json::Value> = output
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|line: &serde_json::Value| line["fields"]["event"] == "slow_request")
            .collect();
        assert_eq!(slow.len(), 1, "{output}");
        let fields = slow[0]["fields"].as_object().unwrap();
        for required in [
            "method",
            "route",
            "lane",
            "status",
            "tenant",
            "catalog",
            "collection",
            "elapsed_ms",
            "routing_ms",
            "query_ms",
            "cache_ms",
            "encode_ms",
        ] {
            assert!(
                fields.contains_key(required),
                "missing {required}: {output}"
            );
        }
        assert!(slow[0].get("span").is_none(), "{output}");
        assert!(slow[0].get("spans").is_none(), "{output}");
        for forbidden_field in ["uri", "headers", "credential", "error"] {
            assert!(!fields.contains_key(forbidden_field), "{output}");
        }
        let line = serde_json::to_string(&slow[0]).unwrap();
        for forbidden in [
            "query-secret",
            "header-secret",
            "feature-secret",
            "style-secret",
            "tile-secret",
            "internal-secret",
            "physical-secret",
            "error-secret",
            "token=",
            "Authorization",
        ] {
            assert!(!line.contains(forbidden), "leaked {forbidden}: {line}");
        }
        assert_eq!(fields["route"], "/__test_slow");
    }

    #[test]
    fn max_concurrency_falls_back_to_the_cores_heuristic_without_a_backend_hint() {
        let n = derive_max_concurrency(None);
        assert!((64..=4096).contains(&n));
    }

    #[test]
    fn max_concurrency_is_coherent_with_a_reported_backend_capacity() {
        // A mid-sized pool lands at capacity * BACKEND_ADMISSION_MULTIPLIER —
        // nowhere near the old cores * 64 ceiling a box that size used to get.
        let n = derive_max_concurrency(Some(16));
        assert_eq!(n, 16 * BACKEND_ADMISSION_MULTIPLIER);

        // A pool below the floor still gets the same absolute minimum every
        // deployment gets, never less.
        let n = derive_max_concurrency(Some(4));
        assert_eq!(n, 64);

        // A large combined capacity still clamps at the outer ceiling.
        let n = derive_max_concurrency(Some(10_000));
        assert_eq!(n, 4096);
    }

    #[tokio::test]
    async fn a_handler_panic_is_caught_as_a_problem_json_500_instead_of_killing_the_connection() {
        let app = test_app();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/__test_panic")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );

        let json = json_body(response).await;
        assert_eq!(json["type"], "about:blank");
        assert_eq!(json["title"], "Internal Server Error");
        assert_eq!(json["status"], 500);
        assert_eq!(json["detail"], "an internal server error occurred");
        assert_eq!(json["code"], "InternalServerError");
        // No panic message, file/line location, or backtrace ever reaches the body.
        assert_eq!(
            json.as_object().unwrap().len(),
            5,
            "panic response body should carry only type, title, status, detail, and code"
        );

        // The service must still be usable afterwards — a caught panic must
        // not have taken anything down with it.
        let follow_up = get(&app, "/").await;
        assert_eq!(follow_up.status(), StatusCode::OK);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn max_concurrency_sheds_a_second_workload_request_but_not_probes() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let metrics_handle = recorder.handle();
        let _recorder_guard = metrics::set_default_local_recorder(&recorder);
        let config: AppConfig = serde_yaml::from_str(
            r#"
server: { max_concurrency: 1 }
storages: [ { id: main, driver: slow, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let mut registry = Registry::new();
        registry.register(Arc::new(SlowFactory {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        let readiness = crate::readiness::Readiness::new();
        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        let app = build_with_readiness(ctx, metrics_handle.clone(), 60, readiness);
        let items_path = format!("{}/collections/demo/items", catalog_root("features"));

        let first = app.clone();
        let first_items_path = items_path.clone();
        let first_task = tokio::spawn(async move {
            first
                .oneshot(
                    Request::builder()
                        .uri(first_items_path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        });
        entered.notified().await;

        assert_eq!(get(&app, "/healthz").await.status(), StatusCode::OK);
        assert_eq!(get(&app, "/readyz").await.status(), StatusCode::OK);

        let second = app.clone();
        let second_response = second
            .oneshot(
                Request::builder()
                    .uri(items_path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        release.notify_one();
        let first_response = first_task.await.unwrap();

        assert_eq!(first_response.status(), StatusCode::OK);
        assert_eq!(second_response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            second_response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json",
            "a shed request must carry the same problem+json content type as every other error"
        );
        assert_eq!(
            second_response.headers().get(header::RETRY_AFTER).unwrap(),
            "1"
        );
        let body = to_bytes(second_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            body.as_ref(),
            LOAD_SHED_BODY.as_bytes(),
            "the real load_shed layer must serve the exact precomputed body, not a freshly \
             serialized one"
        );
        let metrics = metrics_handle.render();
        assert!(metrics.contains("status=\"503\""), "{metrics}");
        assert!(metrics.contains("lane=\"features\""), "{metrics}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn request_observation_records_a_timeout_synthesized_by_the_real_stack() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let _recorder_guard = metrics::set_default_local_recorder(&recorder);
        let app = build(test_ctx(), handle.clone(), 1);

        let response = get(&app, "/__test_timeout").await;
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        let metrics = handle.render();
        assert!(metrics.contains("status=\"504\""), "{metrics}");
        assert!(metrics.contains("lane=\"control\""), "{metrics}");
    }

    /// Exercises `handle_middleware_error` directly rather than racing the
    /// real `timeout`/`load_shed` layers against wall-clock sleeps — this
    /// covers the same body-construction logic deterministically. The
    /// load-shed (503) case is additionally proven end to end through the
    /// real stack by
    /// `max_concurrency_config_override_sheds_the_second_concurrent_request`
    /// above.
    #[tokio::test]
    async fn middleware_load_shed_error_returns_the_precomputed_503_problem_json_body() {
        let response =
            handle_middleware_error(Box::new(tower::load_shed::error::Overloaded::new())).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "1");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body.as_ref(), LOAD_SHED_BODY.as_bytes());

        // `LOAD_SHED_BODY`'s field values must never silently drift from
        // what `problem_response` would build for the same status/code/detail.
        let precomputed: serde_json::Value = serde_json::from_str(LOAD_SHED_BODY).unwrap();
        let equivalent = problem_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "ServiceUnavailable",
            "server is at capacity; retry shortly",
        );
        let equivalent_body = to_bytes(equivalent.into_body(), usize::MAX).await.unwrap();
        let equivalent_json: serde_json::Value = serde_json::from_slice(&equivalent_body).unwrap();
        assert_eq!(precomputed, equivalent_json);
    }

    #[tokio::test]
    async fn middleware_timeout_error_returns_a_504_problem_json() {
        let response =
            handle_middleware_error(Box::new(tower::timeout::error::Elapsed::new())).await;
        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["type"], "about:blank");
        assert_eq!(json["title"], "Gateway Timeout");
        assert_eq!(json["status"], 504);
        assert_eq!(json["detail"], "the request exceeded the time limit");
        assert_eq!(json["code"], "Timeout");
    }

    #[derive(Debug)]
    struct SomeOtherMiddlewareError;

    impl std::fmt::Display for SomeOtherMiddlewareError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "some other middleware error")
        }
    }

    impl std::error::Error for SomeOtherMiddlewareError {}

    #[tokio::test]
    async fn middleware_unrecognized_error_returns_a_500_problem_json() {
        let response = handle_middleware_error(Box::new(SomeOtherMiddlewareError)).await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["type"], "about:blank");
        assert_eq!(json["title"], "Internal Server Error");
        assert_eq!(json["status"], 500);
        assert_eq!(json["detail"], "an internal server error occurred");
        assert_eq!(json["code"], "InternalServerError");
    }

    // ------------------------------------------------------------------
    // `#39` acceptance tests 2, 3, 4, plus the internal-id-never-serializes
    // guard. Acceptance test 6 (reserved segment fails boot) lives in
    // `tellurion-core`'s `config::tests` — that's where `AppConfig::
    // validate` actually runs, and every path in this crate that loads a
    // config would inherit that same failure via `?`, so there is nothing
    // route-tree-specific left to prove here. Acceptance test 5 (settings
    // nearest-wins at all four levels) is likewise covered directly in
    // `tellurion-core` (`settings::tests::*` walks the whole four-level
    // chain; `router::tests::resolve_tiles_carries_the_catalogs_inherited_
    // tile_caps_onto_the_served_decl` proves it reaches the served decl) —
    // re-deriving it at the HTTP layer would only re-test the same logic
    // through more machinery.
    // ------------------------------------------------------------------

    /// A `TileSource` that always answers the same fixed, non-empty MVT
    /// bytes and counts every call — the seam the rename tests use to prove
    /// a cache HIT (call count unchanged) rather than merely a byte-for-byte
    /// identical response (which a cache MISS that happens to re-derive the
    /// same answer would also produce).
    struct CountingTileSource {
        calls: std::sync::atomic::AtomicUsize,
        payload: bytes::Bytes,
    }

    impl CountingTileSource {
        fn new(payload: &'static [u8]) -> Arc<Self> {
            Arc::new(Self {
                calls: std::sync::atomic::AtomicUsize::new(0),
                payload: bytes::Bytes::from_static(payload),
            })
        }

        fn call_count(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl TileSource for CountingTileSource {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<bytes::Bytes>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(self.payload.clone()))
        }
    }

    struct CountingDriver {
        tiles: Arc<CountingTileSource>,
    }

    impl StorageDriver for CountingDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            Some(Arc::clone(&self.tiles) as Arc<dyn TileSource>)
        }
    }

    struct CountingFactory {
        name: &'static str,
        tiles: Arc<CountingTileSource>,
    }

    impl DriverFactory for CountingFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(CountingDriver {
                tiles: Arc::clone(&self.tiles),
            }))
        }
    }

    /// Builds a fresh `(Router, Arc<dyn Resolver>)` pair from `config`
    /// against `registry` — the same two-step every config reload takes.
    /// Used directly here (rather than through a live file-watch trigger,
    /// which `main.rs` does not wire up yet) to exercise `AppContext::
    /// reload` itself.
    fn rebuild(config: &AppConfig, registry: &Registry) -> (CoreRouter, Arc<dyn Resolver>) {
        let router = CoreRouter::build(config, registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(config));
        (router, resolver)
    }

    fn tile_path(tenant: &str, catalog: &str, collection: &str) -> String {
        format!(
            "/{tenant}/tiles/catalogs/{catalog}/collections/{collection}/tiles/WebMercatorQuad/0/0/0"
        )
    }

    fn tileset_path(tenant: &str, catalog: &str, collection: &str) -> String {
        format!("/{tenant}/tiles/catalogs/{catalog}/collections/{collection}/tiles/WebMercatorQuad")
    }

    /// `#39` acceptance test 2 (collection level): renaming a collection's
    /// `external_id` via a config reload serves the exact same cached tile
    /// under the new name — the driver is never called a second time — and
    /// no response along the way ever names the collection's internal id.
    #[tokio::test]
    async fn renaming_a_collections_external_id_is_a_cache_hit_under_the_new_name() {
        const INTERNAL_ID: &str = "collection-internal-marker";
        let make_config = |external_id: &str| -> AppConfig {
            serde_yaml::from_str(&format!(
                r#"
storages: [ {{ id: main, driver: counting-collection-rename, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: {INTERNAL_ID}
    external_id: {external_id}
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
"#
            ))
            .unwrap()
        };

        let tiles = CountingTileSource::new(b"mvt-bytes-for-collection-rename-test");
        let mut registry = Registry::new();
        registry.register(Arc::new(CountingFactory {
            name: "counting-collection-rename",
            tiles: Arc::clone(&tiles),
        }));

        let config_a = make_config("demo-old");
        config_a.validate().unwrap();
        let (router_a, resolver_a) = rebuild(&config_a, &registry);
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config_a,
            router_a,
            resolver_a,
            None,
            Arc::clone(&cache),
            style_store,
        ));
        let app = build(Arc::clone(&ctx), test_metrics_handle(), 60);

        let before = get(&app, &tile_path("public", "default", "demo-old")).await;
        assert_eq!(before.status(), StatusCode::OK);
        let before_body = to_bytes(before.into_body(), usize::MAX).await.unwrap();
        assert_eq!(tiles.call_count(), 1);

        let config_b = make_config("demo-new");
        config_b.validate().unwrap();
        let (router_b, resolver_b) = rebuild(&config_b, &registry);
        ctx.reload(config_b, router_b, resolver_b, None);

        let after = get(&app, &tile_path("public", "default", "demo-new")).await;
        assert_eq!(after.status(), StatusCode::OK);
        let after_body = to_bytes(after.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            before_body, after_body,
            "the renamed collection must serve byte-identical cached content"
        );
        assert_eq!(
            tiles.call_count(),
            1,
            "the driver must not be called again — this must be a cache HIT under the new name"
        );

        let old_name = get(&app, &tile_path("public", "default", "demo-old")).await;
        assert_eq!(old_name.status(), StatusCode::NOT_FOUND);

        let tileset = get(&app, &tileset_path("public", "default", "demo-new")).await;
        let tileset_body = to_bytes(tileset.into_body(), usize::MAX).await.unwrap();
        let tileset_text = String::from_utf8_lossy(&tileset_body);
        assert!(
            !tileset_text.contains(INTERNAL_ID),
            "response must never contain the internal id: {tileset_text}"
        );
    }

    /// `#39` acceptance test 2 (catalog level): same rename-survives-a-
    /// cache-hit proof, this time renaming the catalog's own `external_id`.
    #[tokio::test]
    async fn renaming_a_catalogs_external_id_is_a_cache_hit_under_the_new_name() {
        const INTERNAL_ID: &str = "catalog-internal-marker";
        let make_config = |external_id: &str| -> AppConfig {
            serde_yaml::from_str(&format!(
                r#"
storages: [ {{ id: main, driver: counting-catalog-rename, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: {INTERNAL_ID}, external_id: {external_id}, tenant: public }} ]
collections:
  - id: demo
    catalog: {INTERNAL_ID}
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
"#
            ))
            .unwrap()
        };

        let tiles = CountingTileSource::new(b"mvt-bytes-for-catalog-rename-test");
        let mut registry = Registry::new();
        registry.register(Arc::new(CountingFactory {
            name: "counting-catalog-rename",
            tiles: Arc::clone(&tiles),
        }));

        let config_a = make_config("catalog-old");
        config_a.validate().unwrap();
        let (router_a, resolver_a) = rebuild(&config_a, &registry);
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config_a,
            router_a,
            resolver_a,
            None,
            Arc::clone(&cache),
            style_store,
        ));
        let app = build(Arc::clone(&ctx), test_metrics_handle(), 60);

        let before = get(&app, &tile_path("public", "catalog-old", "demo")).await;
        assert_eq!(before.status(), StatusCode::OK);
        let before_body = to_bytes(before.into_body(), usize::MAX).await.unwrap();
        assert_eq!(tiles.call_count(), 1);

        let config_b = make_config("catalog-new");
        config_b.validate().unwrap();
        let (router_b, resolver_b) = rebuild(&config_b, &registry);
        ctx.reload(config_b, router_b, resolver_b, None);

        let after = get(&app, &tile_path("public", "catalog-new", "demo")).await;
        assert_eq!(after.status(), StatusCode::OK);
        let after_body = to_bytes(after.into_body(), usize::MAX).await.unwrap();
        assert_eq!(before_body, after_body);
        assert_eq!(
            tiles.call_count(),
            1,
            "the driver must not be called again after the catalog rename"
        );

        let old_name = get(&app, &tile_path("public", "catalog-old", "demo")).await;
        assert_eq!(old_name.status(), StatusCode::NOT_FOUND);

        let tileset = get(&app, &tileset_path("public", "catalog-new", "demo")).await;
        let tileset_body = to_bytes(tileset.into_body(), usize::MAX).await.unwrap();
        let tileset_text = String::from_utf8_lossy(&tileset_body);
        assert!(!tileset_text.contains(INTERNAL_ID));
    }

    /// `#39` acceptance test 2 (tenant level): same rename-survives-a-
    /// cache-hit proof, this time renaming the tenant's own `external_id`.
    #[tokio::test]
    async fn renaming_a_tenants_external_id_is_a_cache_hit_under_the_new_name() {
        const INTERNAL_ID: &str = "tenant-internal-marker";
        let make_config = |external_id: &str| -> AppConfig {
            serde_yaml::from_str(&format!(
                r#"
storages: [ {{ id: main, driver: counting-tenant-rename, url_env: DATABASE_URL }} ]
tenants: [ {{ id: {INTERNAL_ID}, external_id: {external_id} }} ]
catalogs: [ {{ id: default, tenant: {INTERNAL_ID} }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
"#
            ))
            .unwrap()
        };

        let tiles = CountingTileSource::new(b"mvt-bytes-for-tenant-rename-test");
        let mut registry = Registry::new();
        registry.register(Arc::new(CountingFactory {
            name: "counting-tenant-rename",
            tiles: Arc::clone(&tiles),
        }));

        let config_a = make_config("tenant-old");
        config_a.validate().unwrap();
        let (router_a, resolver_a) = rebuild(&config_a, &registry);
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config_a,
            router_a,
            resolver_a,
            None,
            Arc::clone(&cache),
            style_store,
        ));
        let app = build(Arc::clone(&ctx), test_metrics_handle(), 60);

        let before = get(&app, &tile_path("tenant-old", "default", "demo")).await;
        assert_eq!(before.status(), StatusCode::OK);
        let before_body = to_bytes(before.into_body(), usize::MAX).await.unwrap();
        assert_eq!(tiles.call_count(), 1);

        let config_b = make_config("tenant-new");
        config_b.validate().unwrap();
        let (router_b, resolver_b) = rebuild(&config_b, &registry);
        ctx.reload(config_b, router_b, resolver_b, None);

        let after = get(&app, &tile_path("tenant-new", "default", "demo")).await;
        assert_eq!(after.status(), StatusCode::OK);
        let after_body = to_bytes(after.into_body(), usize::MAX).await.unwrap();
        assert_eq!(before_body, after_body);
        assert_eq!(
            tiles.call_count(),
            1,
            "the driver must not be called again after the tenant rename"
        );

        let old_name = get(&app, &tile_path("tenant-old", "default", "demo")).await;
        assert_eq!(old_name.status(), StatusCode::NOT_FOUND);

        let tileset = get(&app, &tileset_path("tenant-new", "default", "demo")).await;
        let tileset_body = to_bytes(tileset.into_body(), usize::MAX).await.unwrap();
        let tileset_text = String::from_utf8_lossy(&tileset_body);
        assert!(!tileset_text.contains(INTERNAL_ID));
    }

    /// `#39` acceptance test 3: two tenants that both declare a catalog
    /// external id `default` and a collection external id `demo` resolve to
    /// their own distinct storages — no collision, no cross-tenant leak.
    #[tokio::test]
    async fn two_tenants_with_identical_catalog_and_collection_names_never_collide() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: storage-a, driver: counting-tenant-a, url_env: DATABASE_URL }
  - { id: storage-b, driver: counting-tenant-b, url_env: DATABASE_URL2 }
tenants:
  - { id: tenant-a-internal, external_id: acme }
  - { id: tenant-b-internal, external_id: globex }
catalogs:
  - { id: catalog-a-internal, external_id: default, tenant: tenant-a-internal }
  - { id: catalog-b-internal, external_id: default, tenant: tenant-b-internal }
collections:
  - id: collection-a-internal
    external_id: demo
    catalog: catalog-a-internal
    storage: storage-a
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
  - id: collection-b-internal
    external_id: demo
    catalog: catalog-b-internal
    storage: storage-b
    table: demo
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 5, caps: {} }
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let tiles_a = CountingTileSource::new(b"tenant-a-payload");
        let tiles_b = CountingTileSource::new(b"tenant-b-payload");
        let mut registry = Registry::new();
        registry.register(Arc::new(CountingFactory {
            name: "counting-tenant-a",
            tiles: Arc::clone(&tiles_a),
        }));
        registry.register(Arc::new(CountingFactory {
            name: "counting-tenant-b",
            tiles: Arc::clone(&tiles_b),
        }));

        let (router, resolver) = rebuild(&config, &registry);
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        let app = build(ctx, test_metrics_handle(), 60);

        let a_response = get(&app, &tile_path("acme", "default", "demo")).await;
        assert_eq!(a_response.status(), StatusCode::OK);
        let a_body = to_bytes(a_response.into_body(), usize::MAX).await.unwrap();

        let b_response = get(&app, &tile_path("globex", "default", "demo")).await;
        assert_eq!(b_response.status(), StatusCode::OK);
        let b_body = to_bytes(b_response.into_body(), usize::MAX).await.unwrap();

        assert_ne!(
            a_body, b_body,
            "identical catalog/collection external ids under different tenants must not collide"
        );
        assert_eq!(a_body.as_ref(), b"tenant-a-payload");
        assert_eq!(b_body.as_ref(), b"tenant-b-payload");
        assert_eq!(tiles_a.call_count(), 1);
        assert_eq!(tiles_b.call_count(), 1);
    }

    /// `#39` acceptance test 4: starting from the tenant directory doc,
    /// every protocol root's href for the same catalog resolves — a client
    /// can navigate features -> tiles -> styles -> 3dtiles -> stac within
    /// one tenant purely by following links, and lands on the SAME
    /// underlying "demo" collection through each of them.
    #[tokio::test]
    async fn cross_root_links_resolve_within_a_tenant() {
        let app = test_app();

        let directory = json_body(get(&app, &format!("/{TENANT_EXT}")).await).await;
        let links = directory["links"].as_array().unwrap().clone();
        let href_for = |rel: &str| -> String {
            links
                .iter()
                .find(|l| l["rel"] == rel)
                .unwrap_or_else(|| panic!("missing '{rel}' link in {links:?}"))["href"]
                .as_str()
                .unwrap()
                .to_string()
        };

        let features_root = href_for("features");
        assert_eq!(features_root, catalog_root("features"));
        let features_landing = json_body(get(&app, &features_root).await).await;
        let data_href = features_landing["links"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["rel"] == "data")
            .unwrap()["href"]
            .as_str()
            .unwrap()
            .to_string();
        let collections = json_body(get(&app, &data_href).await).await;
        assert_eq!(collections["collections"].as_array().unwrap().len(), 1);

        let tiles_root = href_for("tiles");
        let tiles_landing = json_body(get(&app, &tiles_root).await).await;
        let tiles_href = tiles_landing["links"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["rel"] == "tiles")
            .unwrap()["href"]
            .as_str()
            .unwrap()
            .to_string();
        let tile_matrix_sets = get(&app, &tiles_href).await;
        assert_eq!(tile_matrix_sets.status(), StatusCode::OK);

        // The SAME "demo" collection is reachable, in the SAME tenant,
        // through the tiles root too — proof this is one coherent catalog
        // seen through two protocol prefixes, not two unrelated ones.
        let tileset = get(
            &app,
            &format!("{tiles_root}/collections/demo/tiles/WebMercatorQuad"),
        )
        .await;
        assert_eq!(tileset.status(), StatusCode::OK);

        let styles_root = href_for("styles");
        let styles_landing = json_body(get(&app, &styles_root).await).await;
        let styles_href = styles_landing["links"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["rel"] == "styles")
            .unwrap()["href"]
            .as_str()
            .unwrap()
            .to_string();
        let styles_list = get(&app, &styles_href).await;
        assert_eq!(styles_list.status(), StatusCode::OK);

        let threedtiles_root = href_for("3dtiles");
        let threedtiles_landing = get(&app, &threedtiles_root).await;
        assert_eq!(threedtiles_landing.status(), StatusCode::OK);

        let stac_root = href_for("stac");
        assert_eq!(stac_root, catalog_root("stac"));
        let stac_landing = json_body(get(&app, &stac_root).await).await;
        let stac_data_href = stac_landing["links"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["rel"] == "data")
            .unwrap()["href"]
            .as_str()
            .unwrap()
            .to_string();
        let stac_collections = json_body(get(&app, &stac_data_href).await).await;
        assert_eq!(stac_collections["collections"].as_array().unwrap().len(), 1);
        assert_eq!(stac_collections["collections"][0]["id"], "demo");
    }

    /// A `FeatureSource`-only backend — no `TileSource`, no places3d — for
    /// `#49`'s feature-only-collection acceptance test.
    struct FeaturesOnlyBackend;

    #[async_trait::async_trait]
    impl FeatureSource for FeaturesOnlyBackend {
        async fn items(
            &self,
            _collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> CoreResult<FeaturePage> {
            Ok(FeaturePage {
                features_geojson: vec![],
                number_matched: Some(0),
                next_token: None,
            })
        }

        async fn item(
            &self,
            _collection: &CollectionDecl,
            _id: &str,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<serde_json::Value>> {
            Ok(None)
        }
    }

    struct FeaturesOnlyDriver;

    impl StorageDriver for FeaturesOnlyDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(FeaturesOnlyBackend) as Arc<dyn FeatureSource>)
        }
    }

    struct FeaturesOnlyFactory;

    impl DriverFactory for FeaturesOnlyFactory {
        fn name(&self) -> &str {
            "fake-features-only"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FeaturesOnlyDriver))
        }
    }

    /// A `TileSource`-only backend — no `FeatureSource` — for `#49`'s
    /// tiles-only-collection acceptance test (the PMTiles shape, `#20`).
    struct TilesOnlyBackend;

    #[async_trait::async_trait]
    impl TileSource for TilesOnlyBackend {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<bytes::Bytes>> {
            Ok(None)
        }
    }

    /// Unlike `EmptyCatalog`, reports the one physical collection the
    /// tiles-only test config below declares — a tiles-only collection has
    /// no `table`/`geometry`/`pk` override, so `Router` must derive its
    /// descriptor from this driver's own catalog, same as any other
    /// no-override collection (`#19`/`#20`).
    struct TilesOnlyCatalog;

    #[async_trait::async_trait]
    impl CatalogSource for TilesOnlyCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![PhysicalCollection {
                name: "tiles-demo".to_string(),
                geometry_column: None,
                primary_key: None,
                srid: None,
                geometry_type: None,
            }])
        }
    }

    struct TilesOnlyDriver;

    impl StorageDriver for TilesOnlyDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(TilesOnlyCatalog)
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            Some(Arc::new(TilesOnlyBackend) as Arc<dyn TileSource>)
        }
    }

    struct TilesOnlyFactory;

    impl DriverFactory for TilesOnlyFactory {
        fn name(&self) -> &str {
            "fake-tiles-only"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(TilesOnlyDriver))
        }
    }

    /// `#49` acceptance: a collection with only a `FeatureSource` advertises
    /// its `items` link but neither of the tile-capability links nor the
    /// places3d extension link — there is nothing at the other end of any of
    /// those to serve.
    #[tokio::test]
    async fn feature_only_collection_advertises_items_but_no_tile_or_3d_links() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake-features-only, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: feat-demo
    catalog: default
    storage: main
    table: feat_demo
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FeaturesOnlyFactory));
        let (router, resolver) = rebuild(&config, &registry);
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get(
            &app,
            &format!("{}/collections/feat-demo", catalog_root("features")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        let rels: Vec<&str> = json["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|link| link["rel"].as_str().unwrap())
            .collect();
        assert!(rels.contains(&"items"));
        assert!(!rels.iter().any(|rel| rel.contains("tilesets-vector")));
        assert!(!rels.iter().any(|rel| rel.contains("tilesets-map")));
        assert!(!rels.iter().any(|rel| rel.contains("tileset-3d")));
    }

    /// `#49` acceptance: a collection with only a `TileSource` (the PMTiles
    /// shape, `#20`) advertises both tile-capability links but no `items`
    /// link — mirrors the existing tiles-only coverage in
    /// `tellurion-features`'s own test suite, at the real server mount so
    /// the cross-protocol hrefs actually engage.
    #[tokio::test]
    async fn tiles_only_collection_advertises_tile_links_but_no_items_link() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake-tiles-only, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: tiles-demo
    catalog: default
    storage: main
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(TilesOnlyFactory));
        let (router, resolver) = rebuild(&config, &registry);
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get(
            &app,
            &format!("{}/collections/tiles-demo", catalog_root("features")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        let rels: Vec<&str> = json["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|link| link["rel"].as_str().unwrap())
            .collect();
        assert!(!rels.contains(&"items"));
        let tiles_href = json["links"]
            .as_array()
            .unwrap()
            .iter()
            .find(|link| link["rel"].as_str().unwrap().contains("tilesets-vector"))
            .expect("tilesets-vector link present")["href"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            tiles_href,
            format!("{}/collections/tiles-demo/tiles", catalog_root("tiles"))
        );

        // The link is genuinely followable, not just a well-formed string.
        let tiles_list = get(&app, &tiles_href).await;
        assert_eq!(tiles_list.status(), StatusCode::OK);

        // `#287`: the LISTING carries the same vector-tiles advertisement.
        // Asserted separately because the listing derives its capabilities
        // through `Router::canonical_descriptor` (the `#50` merge and its
        // `tiles_vector` field) while `GET /collections/{cid}` above probes
        // its own lanes live — an over-subtraction in either path (treating
        // a vector-tiles collection as raster) must fail here by name.
        let listing =
            json_body(get(&app, &format!("{}/collections", catalog_root("features"))).await).await;
        let entry = &listing["collections"][0];
        assert_eq!(entry["id"], "tiles-demo");
        assert!(
            entry["links"]
                .as_array()
                .unwrap()
                .iter()
                .any(|link| link["rel"].as_str().unwrap().contains("tilesets-vector")),
            "a vector-tiles collection must keep its tilesets-vector link in the listing"
        );
    }

    /// `#49` acceptance, end to end: a collection whose `external_id`
    /// differs from its internal `id` advertises capability links built from
    /// the external id, and following the `tilesets-vector` link all the way
    /// to the `TileSet` resource shows the external id as the vector layer
    /// name too — never the internal one, at any hop.
    #[tokio::test]
    async fn collection_capability_links_use_the_external_id_end_to_end() {
        const COLLECTION_INTERNAL: &str = "zzz-internal-alias-marker";

        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: {COLLECTION_INTERNAL}
    external_id: public-demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    places3d: {{ height_property: height }}
"#
        ))
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let (router, resolver) = rebuild(&config, &registry);
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get(
            &app,
            &format!("{}/collections/public-demo", catalog_root("features")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        assert_eq!(json["id"], "public-demo");
        let links = json["links"].as_array().unwrap().clone();
        assert!(
            !links
                .iter()
                .any(|link| link["href"].as_str().unwrap().contains(COLLECTION_INTERNAL)),
            "no link href may leak the internal id: {links:?}"
        );

        let tiles_href = links
            .iter()
            .find(|link| link["rel"].as_str().unwrap().contains("tilesets-vector"))
            .expect("tilesets-vector link present")["href"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            tiles_href,
            format!("{}/collections/public-demo/tiles", catalog_root("tiles"))
        );
        let places_href = links
            .iter()
            .find(|link| link["rel"].as_str().unwrap().contains("tileset-3d"))
            .expect("places3d link present")["href"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            places_href,
            format!(
                "{}/collections/public-demo/3dtiles",
                catalog_root("3dtiles")
            )
        );

        // Follow the tilesets-vector link all the way to the real TileSet
        // resource and check its `layers[].id` — the actual acceptance
        // criterion: a client can use this exact name to style the tile.
        let tileset =
            json_body(get(&app, &tileset_path("public", "default", "public-demo")).await).await;
        assert_eq!(
            tileset["layers"],
            serde_json::json!([{ "id": "public-demo", "dataType": "vector" }]),
            "the internal id must never appear as the advertised layer name: {tileset}"
        );

        // Both cross-protocol links resolve for real, not just as strings.
        assert_eq!(get(&app, &tiles_href).await.status(), StatusCode::OK);
        assert_eq!(get(&app, &places_href).await.status(), StatusCode::OK);
    }

    // -- capability-derived advertisement (`#287`) ---------------------------

    /// A raster-only backend (the COG/Zarr shape, `#37`): a `RasterSource`
    /// and no `FeatureSource`/`TileSource` at all.
    struct RasterOnlyBackend;

    #[async_trait::async_trait]
    impl RasterSource for RasterOnlyBackend {
        async fn raster_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
        ) -> CoreResult<Option<RasterWindow>> {
            Ok(None)
        }
    }

    struct RasterOnlyCatalog;

    #[async_trait::async_trait]
    impl CatalogSource for RasterOnlyCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![PhysicalCollection {
                name: "raster-demo".to_string(),
                geometry_column: None,
                primary_key: None,
                srid: None,
                geometry_type: None,
            }])
        }
    }

    struct RasterOnlyDriver;

    impl StorageDriver for RasterOnlyDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(RasterOnlyCatalog)
        }

        fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
            Some(Arc::new(RasterOnlyBackend) as Arc<dyn RasterSource>)
        }
    }

    struct RasterOnlyFactory;

    impl DriverFactory for RasterOnlyFactory {
        fn name(&self) -> &str {
            "fake-raster-only"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(RasterOnlyDriver))
        }
    }

    fn raster_only_app() -> Router {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake-raster-only, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: raster-demo
    catalog: default
    storage: main
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(RasterOnlyFactory));
        let (router, resolver) = rebuild(&config, &registry);
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        build(ctx, test_metrics_handle(), 60)
    }

    /// `#287` acceptance, at the real server mount: a raster-only
    /// collection's `/collections` entry carries the map lane its driver
    /// honours (`tilesets-map`) and NOT the `tilesets-vector` link whose
    /// `.mvt` target answers 400 on this very collection — the same
    /// independent vector-vs-raster resolution `TilesLinkContributor`
    /// already performs, now applied to the collection document's own `#49`
    /// sibling links. Mutating that gate back to the coarse `has_tiles`
    /// fails this test by name; so does re-deriving `itemType`/`crs`/
    /// `storageCrs` unconditionally.
    #[tokio::test]
    async fn a_raster_only_collections_document_advertises_map_but_not_vector_tiles() {
        let app = raster_only_app();
        let response = get(&app, &format!("{}/collections", catalog_root("features"))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        let entry = &json["collections"][0];
        assert_eq!(entry["id"], "raster-demo");
        let rels: Vec<&str> = entry["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|link| link["rel"].as_str().unwrap())
            .collect();
        assert!(
            !rels.iter().any(|rel| rel.contains("tilesets-vector")),
            "a raster-only collection cannot serve MVT and must not link a vector tileset: {rels:?}"
        );
        assert!(
            rels.iter().any(|rel| rel.contains("tilesets-map")),
            "the PNG lane genuinely serves, so the map tileset link must survive: {rels:?}"
        );
        for absent in ["itemType", "crs", "storageCrs"] {
            assert!(
                entry.as_object().unwrap().get(absent).is_none(),
                "a raster-only collection must not carry `{absent}`"
            );
        }
    }

    /// `#297`: no compiled driver in a raster-only deployment can evaluate a
    /// CQL2 expression or honour an optimistic-locking precondition, so the
    /// Features root must not advertise either capability family.
    #[tokio::test]
    async fn a_raster_only_deployment_omits_driver_honoured_conformance_classes() {
        let classes = features_conformance_classes(&raster_only_app()).await;
        assert!(
            classes
                .iter()
                .any(|class| class == "http://www.opengis.net/spec/ogcapi-common-1/1.0/conf/core"),
            "a raster-only deployment must retain the server-honoured Common core class"
        );
        for class in tellurion_core::filter::CQL2_CONFORMANCE_CLASSES {
            assert!(
                !classes.contains(&class.to_string()),
                "a raster-only deployment must not advertise {class}"
            );
        }
        for class in tellurion_core::locking::LOCKING_CONFORMANCE_CLASSES {
            assert!(
                !classes.contains(&class.to_string()),
                "a raster-only deployment must not advertise {class}"
            );
        }
    }

    /// A features backend that genuinely honours the capability-graded
    /// classes — CRS reprojection (Part 2), `filter` with `filter-crs`
    /// (Part 3), and a CQL2 set (`#105`) — so the conformance folds below
    /// have something real to lose if a raster entry were ever allowed to
    /// participate in them.
    struct StrongFeaturesBackend;

    #[async_trait::async_trait]
    impl FeatureSource for StrongFeaturesBackend {
        async fn items(
            &self,
            _collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> CoreResult<FeaturePage> {
            Ok(FeaturePage {
                features_geojson: vec![],
                number_matched: Some(0),
                next_token: None,
            })
        }

        async fn item(
            &self,
            _collection: &CollectionDecl,
            _id: &str,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<serde_json::Value>> {
            Ok(None)
        }

        fn crs_capable(&self) -> bool {
            true
        }

        fn filter_capable(&self) -> bool {
            true
        }

        fn filter_crs_capable(&self) -> bool {
            true
        }

        fn cql2_conformance_classes(&self) -> Vec<&'static str> {
            vec![
                "http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2",
                "http://www.opengis.net/spec/cql2/1.0/conf/cql2-text",
            ]
        }
    }

    struct StrongFeaturesDriver;

    impl StorageDriver for StrongFeaturesDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(StrongFeaturesBackend) as Arc<dyn FeatureSource>)
        }
    }

    struct StrongFeaturesFactory;

    impl DriverFactory for StrongFeaturesFactory {
        fn name(&self) -> &str {
            "fake-strong-features"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(StrongFeaturesDriver))
        }
    }

    fn conformance_app(with_raster: bool) -> Router {
        let mut yaml = String::from(
            "storages:\n  - { id: vec, driver: fake-strong-features, url_env: DATABASE_URL }\n",
        );
        if with_raster {
            yaml.push_str("  - { id: ras, driver: fake-raster-only, url_env: DATABASE_URL2 }\n");
        }
        yaml.push_str(
            r#"tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: vec-demo
    catalog: default
    storage: vec
    table: vec_demo
    geometry: geom
    pk: id
"#,
        );
        if with_raster {
            yaml.push_str("  - id: raster-demo\n    catalog: default\n    storage: ras\n");
        }
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(StrongFeaturesFactory));
        registry.register(Arc::new(RasterOnlyFactory));
        let (router, resolver) = rebuild(&config, &registry);
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        build(ctx, test_metrics_handle(), 60)
    }

    /// `#287`'s conformance-fold half: adding a raster-only driver AND a
    /// raster-only collection to a deployment moves the features root's
    /// `/conformance` by nothing, because a raster entry does not
    /// participate in any feature-capability fold
    /// (`fold_conformance_classes`' `None` case — see
    /// `tellurion_core::router`) — it neither claims a class nor narrows
    /// one away from the vector driver beside it. The vector-only list is
    /// asserted non-vacuous first (it really carries the Part 2 and CQL2
    /// classes the strong driver earns), so a mutation that lets the raster
    /// driver participate-and-honour-nothing visibly strips those classes
    /// from the mixed fold and fails the equality by name.
    #[tokio::test]
    async fn a_raster_only_collection_leaves_the_features_conformance_fold_untouched() {
        let vector_only = json_body(
            get(
                &conformance_app(false),
                &format!("{}/conformance", catalog_root("features")),
            )
            .await,
        )
        .await;
        let classes: Vec<&str> = vector_only["conformsTo"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert!(
            classes.contains(&"http://www.opengis.net/spec/ogcapi-features-2/1.0/conf/crs"),
            "the strong vector driver must earn Part 2 CRS, or this equality proves nothing"
        );
        assert!(
            classes.contains(&"http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2"),
            "the strong vector driver must earn its CQL2 classes, or this equality proves nothing"
        );

        let mixed = json_body(
            get(
                &conformance_app(true),
                &format!("{}/conformance", catalog_root("features")),
            )
            .await,
        )
        .await;
        assert_eq!(
            vector_only, mixed,
            "a raster-only entry participates in no feature-capability fold: \
             it must neither add nor remove a single conformance class"
        );
    }

    /// `#39` design guard: internal ids never serialize, anywhere. Builds a
    /// config where every internal id is deliberately distinct from — and
    /// textually unrelated to — its external id, then sweeps every route
    /// shape this crate exposes for a leaked internal id substring.
    #[tokio::test]
    async fn internal_ids_never_appear_in_any_response_body() {
        const TENANT_INTERNAL: &str = "zzz-tenant-internal-marker";
        const CATALOG_INTERNAL: &str = "zzz-catalog-internal-marker";
        const COLLECTION_INTERNAL: &str = "zzz-collection-internal-marker";

        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: {TENANT_INTERNAL}, external_id: acme }} ]
catalogs: [ {{ id: {CATALOG_INTERNAL}, external_id: maps, tenant: {TENANT_INTERNAL} }} ]
collections:
  - id: {COLLECTION_INTERNAL}
    external_id: demo
    catalog: {CATALOG_INTERNAL}
    storage: main
    table: demo
    geometry: geom
    pk: id
    places3d: {{ height_property: height }}
"#
        ))
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let (router, resolver) = rebuild(&config, &registry);
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        let app = build(ctx, test_metrics_handle(), 60);

        let paths = [
            "/".to_string(),
            "/acme".to_string(),
            "/acme/features/catalogs/maps".to_string(),
            "/acme/features/catalogs/maps/collections".to_string(),
            "/acme/features/catalogs/maps/collections/demo".to_string(),
            "/acme/tiles/catalogs/maps".to_string(),
            "/acme/tiles/catalogs/maps/collections/demo/tiles/WebMercatorQuad".to_string(),
            "/acme/styles/catalogs/maps".to_string(),
            "/acme/styles/catalogs/maps/styles".to_string(),
            "/acme/3dtiles/catalogs/maps".to_string(),
            "/acme/3dtiles/catalogs/maps/collections/demo/3dtiles".to_string(),
            "/acme/stac/catalogs/maps".to_string(),
            "/acme/stac/catalogs/maps/collections".to_string(),
            "/acme/stac/catalogs/maps/collections/demo".to_string(),
        ];

        for path in paths {
            let response = get(&app, &path).await;
            let status = response.status();
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let text = String::from_utf8_lossy(&body);
            assert_eq!(status, StatusCode::OK, "path {path} was not 200: {text}");
            assert!(
                !text.contains(TENANT_INTERNAL),
                "{path} leaked the tenant internal id: {text}"
            );
            assert!(
                !text.contains(CATALOG_INTERNAL),
                "{path} leaked the catalog internal id: {text}"
            );
            assert!(
                !text.contains(COLLECTION_INTERNAL),
                "{path} leaked the collection internal id: {text}"
            );
        }
    }

    // ------------------------------------------------------------------
    // `#17`: tenant trust-boundary enforcement.
    // ------------------------------------------------------------------

    /// Two tenants, each with their own token, so the cross-tenant tests
    /// below can prove tenant B's token cannot read tenant A and vice versa.
    const AUTH_TENANT_A: &str = "tenant-a";
    const AUTH_TENANT_B: &str = "tenant-b";
    const AUTH_TOKEN_A: &str = "token-for-tenant-a";
    const AUTH_TOKEN_B: &str = "token-for-tenant-b";

    fn auth_test_config() -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]
tenants:
  - {{ id: {AUTH_TENANT_A} }}
  - {{ id: {AUTH_TENANT_B} }}
catalogs:
  - {{ id: catalog-a, tenant: {AUTH_TENANT_A} }}
  - {{ id: catalog-b, tenant: {AUTH_TENANT_B} }}
collections:
  - id: demo-a
    catalog: catalog-a
    storage: main
    table: demo
    geometry: geom
    pk: id
  - id: demo-b
    catalog: catalog-b
    storage: main
    table: demo
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - {{ token: {AUTH_TOKEN_A}, tenants: [{AUTH_TENANT_A}] }}
    - {{ token: {AUTH_TOKEN_B}, tenants: [{AUTH_TENANT_B}] }}
"#
        ))
        .unwrap();
        config.validate().unwrap();
        config
    }

    fn auth_test_ctx() -> Arc<AppContext> {
        let config = auth_test_config();
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer = tellurion_core::build_authorizer(&config.auth)
            .expect("no bearer principal in this fixture reads a token_env");
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        Arc::new(AppContext::new(
            config,
            router,
            resolver,
            authorizer,
            cache,
            style_store,
        ))
    }

    /// `path`'s `Authorization` header carries `Bearer <token>` when `bearer`
    /// is `Some`, else no `Authorization` header at all.
    async fn get_with_bearer(app: &Router, path: &str, bearer: Option<&str>) -> Response {
        let mut builder = Request::builder().uri(path);
        if let Some(token) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        app.clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn collections_path(tenant: &str, catalog: &str) -> String {
        format!("/{tenant}/features/catalogs/{catalog}/collections")
    }

    /// `#17`'s explicit requirement: no `auth:` section at all leaves every
    /// tenant route open, with no `Authorization` header presented — the
    /// module's own `test_app()`/`test_ctx()` fixtures never configure
    /// `auth:`, so this is the same fixture every other test in this module
    /// already exercises unauthenticated.
    #[tokio::test]
    async fn no_auth_config_leaves_every_tenant_route_open() {
        let app = test_app();
        let response =
            get_with_bearer(&app, &collections_path(TENANT_EXT, CATALOG_EXT), None).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// `#17`: `auth:` configured, no `Authorization` header at all -> 401,
    /// with the shared RFC 9457 problem+json body.
    #[tokio::test]
    async fn missing_credential_on_an_auth_configured_tenant_is_401() {
        let ctx = auth_test_ctx();
        let app = build(ctx, test_metrics_handle(), 60);
        let response =
            get_with_bearer(&app, &collections_path(AUTH_TENANT_A, "catalog-a"), None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let json = json_body(response).await;
        assert_eq!(json["status"], 401);
        assert_eq!(json["type"], "about:blank");
    }

    /// `#17`: a credential was presented, but it doesn't authorize the
    /// target tenant -> 403, not 401 — the two cases are distinguished.
    #[tokio::test]
    async fn wrong_tenant_token_on_an_auth_configured_tenant_is_403() {
        let ctx = auth_test_ctx();
        let app = build(ctx, test_metrics_handle(), 60);
        let response = get_with_bearer(
            &app,
            &collections_path(AUTH_TENANT_A, "catalog-a"),
            Some(AUTH_TOKEN_B),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let json = json_body(response).await;
        assert_eq!(json["status"], 403);
    }

    /// `#17`: the right token for the target tenant is allowed through to
    /// the real handler, exactly as an unauthenticated request would be
    /// with no `auth:` section configured at all.
    #[tokio::test]
    async fn right_token_on_an_auth_configured_tenant_is_200() {
        let ctx = auth_test_ctx();
        let app = build(ctx, test_metrics_handle(), 60);
        let response = get_with_bearer(
            &app,
            &collections_path(AUTH_TENANT_A, "catalog-a"),
            Some(AUTH_TOKEN_A),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        assert_eq!(json["collections"].as_array().unwrap().len(), 1);
    }

    /// `#17`'s acceptance case: tenant B's token cannot read tenant A's
    /// collections, and tenant A's token cannot read tenant B's — each
    /// token only ever authorizes its own tenant, checked both directions.
    #[tokio::test]
    async fn a_tenants_token_cannot_read_a_different_tenant() {
        let ctx = auth_test_ctx();
        let app = build(ctx, test_metrics_handle(), 60);

        let a_with_bs_token = get_with_bearer(
            &app,
            &collections_path(AUTH_TENANT_A, "catalog-a"),
            Some(AUTH_TOKEN_B),
        )
        .await;
        assert_eq!(a_with_bs_token.status(), StatusCode::FORBIDDEN);

        let b_with_as_token = get_with_bearer(
            &app,
            &collections_path(AUTH_TENANT_B, "catalog-b"),
            Some(AUTH_TOKEN_A),
        )
        .await;
        assert_eq!(b_with_as_token.status(), StatusCode::FORBIDDEN);

        // Each token still works against its OWN tenant — proof this is
        // real per-tenant authorization, not a blanket deny.
        let b_with_bs_token = get_with_bearer(
            &app,
            &collections_path(AUTH_TENANT_B, "catalog-b"),
            Some(AUTH_TOKEN_B),
        )
        .await;
        assert_eq!(b_with_bs_token.status(), StatusCode::OK);
    }

    /// `#17`: reserved top-level segments never pass through the tenant
    /// authorizer at all, even when `auth:` is configured — `/metrics` is
    /// reachable with no credential the same as it always was.
    #[tokio::test]
    async fn auth_never_applies_to_the_top_level_metrics_route() {
        let ctx = auth_test_ctx();
        let app = build(ctx, test_metrics_handle(), 60);
        let response = get_with_bearer(&app, "/metrics", None).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_never_applies_to_top_level_probe_routes() {
        let ctx = auth_test_ctx();
        let readiness = crate::readiness::Readiness::new();
        let app = build_with_readiness(ctx, test_metrics_handle(), 60, readiness);

        assert_eq!(
            get_with_bearer(&app, "/healthz", None).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            get_with_bearer(&app, "/readyz", None).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    // -- effective-config view (`#110`, read-only slice) --------------------

    /// The platform node sits at the top level, unauthenticated, the same
    /// as `/metrics` — settings are behavior, not secrets.
    #[tokio::test]
    async fn auth_never_applies_to_the_platform_effective_config_route() {
        let ctx = auth_test_ctx();
        let app = build(ctx, test_metrics_handle(), 60);
        let response = get_with_bearer(&app, "/config/effective", None).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// The tenant/catalog/collection mounts nest under `/{tenant}`, so they
    /// inherit `enforce_tenant_auth` (`#17`) exactly like every other
    /// tenant-scoped resource — no bespoke gating invented for this
    /// endpoint.
    #[tokio::test]
    async fn tenant_effective_config_requires_the_same_credential_every_other_tenant_route_does() {
        let ctx = auth_test_ctx();
        let app = build(ctx, test_metrics_handle(), 60);

        let denied =
            get_with_bearer(&app, &format!("/{AUTH_TENANT_A}/config/effective"), None).await;
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

        let allowed = get_with_bearer(
            &app,
            &format!("/{AUTH_TENANT_A}/config/effective"),
            Some(AUTH_TOKEN_A),
        )
        .await;
        assert_eq!(allowed.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn platform_effective_config_reports_the_platform_node_and_only_built_in_defaults() {
        let app = test_app();
        let response = get(&app, "/config/effective").await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        assert_eq!(json["node"]["level"], "platform");
        assert!(json["node"].get("tenant").is_none());
        assert_eq!(
            json["settings"]["cache_ttl_s"]["provenance"]["kind"],
            "built_in_default"
        );
    }

    #[tokio::test]
    async fn tenant_effective_config_names_the_tenant_and_404s_for_an_unknown_one() {
        let app = test_app();

        let response = get(&app, &format!("/{TENANT_EXT}/config/effective")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        assert_eq!(json["node"]["level"], "tenant");
        assert_eq!(json["node"]["tenant"], TENANT_EXT);

        let missing = get(&app, "/no-such-tenant/config/effective").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn catalog_effective_config_names_the_catalog_and_404s_for_an_unknown_one() {
        let app = test_app();
        let path = format!("/{TENANT_EXT}/config/catalogs/{CATALOG_EXT}/effective");

        let response = get(&app, &path).await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        assert_eq!(json["node"]["level"], "catalog");
        assert_eq!(json["node"]["tenant"], TENANT_EXT);
        assert_eq!(json["node"]["catalog"], CATALOG_EXT);

        let missing = get(
            &app,
            &format!("/{TENANT_EXT}/config/catalogs/no-such-catalog/effective"),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn collection_effective_config_names_the_collection_and_404s_for_an_unknown_one() {
        let app = test_app();
        let path =
            format!("/{TENANT_EXT}/config/catalogs/{CATALOG_EXT}/collections/demo/effective");

        let response = get(&app, &path).await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;
        assert_eq!(json["node"]["level"], "collection");
        assert_eq!(json["node"]["collection"], "demo");
        // Every settings key is present, each carrying a `value` and a
        // `provenance.kind` — the full contract, not just a couple of
        // fields.
        for key in [
            "tile_caps",
            "cache_ttl_s",
            "slow_request_ms",
            "stac",
            "tile_properties",
            "colormap",
            "max_request_body_bytes",
            "tile_vertex_budget",
            "items_vertex_budget",
            "page_max_bytes",
            "max_asset_bytes",
            "asset_media_types",
            "batch",
        ] {
            assert!(
                json["settings"][key]["provenance"]["kind"].is_string(),
                "missing provenance for '{key}': {json}"
            );
        }

        let missing = get(
            &app,
            &format!("/{TENANT_EXT}/config/catalogs/{CATALOG_EXT}/collections/no-such-collection/effective"),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    /// `#110` anti-drift, at the HTTP boundary: a platform-level value
    /// inherited by a collection, a collection's own local override, and a
    /// derived `tile_caps` (the collection's physical `tiles.caps` block)
    /// must all be tagged exactly as the issue's own vocabulary describes,
    /// on the same document the request lanes serve from.
    #[tokio::test]
    async fn collection_effective_config_reports_every_provenance_shape_the_issue_names() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs:
  - id: default
    tenant: public
    settings: { tile_caps: { z0: 500 } }
settings:
  slow_request_ms: 9000
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    settings: { tile_vertex_budget: 4242 }
    tiles: { caps: { z0: 42 } }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get(
            &app,
            "/public/config/catalogs/default/collections/demo/effective",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;

        // Derived: the collection's own physical `tiles.caps`.
        assert_eq!(
            json["settings"]["tile_caps"]["provenance"]["kind"],
            "derived"
        );
        assert_eq!(json["settings"]["tile_caps"]["value"]["z0"], 42);

        // Local override: the collection's own `settings.tile_vertex_budget`.
        assert_eq!(
            json["settings"]["tile_vertex_budget"]["provenance"]["kind"],
            "local_override"
        );
        assert_eq!(json["settings"]["tile_vertex_budget"]["value"], 4242);

        // Inherited, naming the platform level.
        assert_eq!(
            json["settings"]["slow_request_ms"]["provenance"]["kind"],
            "inherited"
        );
        assert_eq!(
            json["settings"]["slow_request_ms"]["provenance"]["level"],
            "platform"
        );
        assert_eq!(json["settings"]["slow_request_ms"]["value"], 9000);

        // Built-in default: nothing in the chain ever declares
        // `cache_ttl_s`.
        assert_eq!(
            json["settings"]["cache_ttl_s"]["provenance"]["kind"],
            "built_in_default"
        );
    }

    /// `#111`, at the HTTP boundary: a value a named profile supplies
    /// reports the issue's own `profile:<id>` vocabulary — the one-line
    /// answer to "why does this collection have this budget."
    #[tokio::test]
    async fn collection_effective_config_reports_a_profile_sourced_value_as_profile_colon_id() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
profiles:
  - id: heavy-raster
    cache_ttl_s: 7200
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    settings: { profile: heavy-raster }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get(
            &app,
            "/public/config/catalogs/default/collections/demo/effective",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;

        assert_eq!(json["settings"]["cache_ttl_s"]["value"], 7200);
        assert_eq!(
            json["settings"]["cache_ttl_s"]["provenance"]["kind"],
            "profile"
        );
        assert_eq!(
            json["settings"]["cache_ttl_s"]["provenance"]["profile_id"],
            "heavy-raster"
        );
        assert_eq!(
            json["settings"]["cache_ttl_s"]["provenance"]["level"],
            "collection"
        );
    }

    /// `#111` read-only enumeration: every declared profile, id and
    /// contents, at the platform-level, unauthenticated mount — the same
    /// "settings values are behavior, not secrets" posture `/config/
    /// effective`'s platform node already documents.
    #[tokio::test]
    async fn config_profiles_lists_every_declared_profile_and_its_contents() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
profiles:
  - id: heavy-raster
    cache_ttl_s: 7200
    tile_vertex_budget: 250000
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let router = CoreRouter::build(&config, &Registry::new()).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new(
            config,
            router,
            resolver,
            None,
            cache,
            style_store,
        ));
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get(&app, "/config/profiles").await;
        assert_eq!(response.status(), StatusCode::OK);
        let json = json_body(response).await;

        assert_eq!(json["profiles"].as_array().unwrap().len(), 1);
        assert_eq!(json["profiles"][0]["id"], "heavy-raster");
        assert_eq!(json["profiles"][0]["settings"]["cache_ttl_s"], 7200);
        assert_eq!(
            json["profiles"][0]["settings"]["tile_vertex_budget"],
            250000
        );
    }

    /// Defense in depth for the "never echo a token value" rule: a denied
    /// request's problem+json body never contains the raw token that was
    /// rejected, whether it was simply unknown or valid-for-a-different-tenant.
    #[tokio::test]
    async fn a_denied_responses_body_never_contains_the_presented_token() {
        let ctx = auth_test_ctx();
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get_with_bearer(
            &app,
            &collections_path(AUTH_TENANT_A, "catalog-a"),
            Some(AUTH_TOKEN_B),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains(AUTH_TOKEN_B),
            "response leaked the token: {text}"
        );
    }

    // -- OGC API Features Part 4: plain OPTIONS `Allow`, CORS write verbs ---

    fn allow_for(path: &str, writes_allowed: bool) -> Option<HeaderValue> {
        WriteResource::of(path).map(|resource| resource.allow(writes_allowed))
    }

    #[test]
    fn write_resource_allow_header_covers_every_write_resource_shape() {
        assert_eq!(
            allow_for(
                "/public/features/catalogs/default/collections/demo/items",
                true
            ),
            Some(HeaderValue::from_static("GET, POST, OPTIONS"))
        );
        assert_eq!(
            allow_for(
                "/public/features/catalogs/default/collections/demo/items/x",
                true
            ),
            Some(HeaderValue::from_static("GET, PUT, PATCH, DELETE, OPTIONS"))
        );
        // `#114`'s batch-ingest resource is `POST`-only: it has no read
        // representation, so its `Allow` must never name `GET`.
        assert_eq!(
            allow_for(
                "/public/features/catalogs/default/collections/demo/items/batch",
                true
            ),
            Some(HeaderValue::from_static("POST, OPTIONS"))
        );
    }

    /// `#185`: with `protocols.features_write` disabled, every write
    /// resource still exists (the reads keep serving) but supports only what
    /// remains — the truthful `Allow` OGC API - Features Part 4
    /// `/req/create-replace-delete/options-response` demands.
    #[test]
    fn write_resource_allow_header_drops_the_write_verbs_when_the_write_lane_is_not_exposed() {
        assert_eq!(
            allow_for(
                "/public/features/catalogs/default/collections/demo/items",
                false
            ),
            Some(HeaderValue::from_static("GET, OPTIONS"))
        );
        assert_eq!(
            allow_for(
                "/public/features/catalogs/default/collections/demo/items/x",
                false
            ),
            Some(HeaderValue::from_static("GET, OPTIONS"))
        );
        assert_eq!(
            allow_for(
                "/public/features/catalogs/default/collections/demo/items/batch",
                false
            ),
            Some(HeaderValue::from_static("OPTIONS"))
        );
    }

    #[test]
    fn write_resource_allow_header_ignores_the_same_shape_under_a_different_protocol_root() {
        // STAC has its own read-only `/collections/{cid}/items` — this must
        // never claim write verbs are allowed there.
        assert_eq!(
            allow_for("/public/stac/catalogs/default/collections/demo/items", true),
            None
        );
        assert_eq!(
            allow_for("/public/features/catalogs/default/collections", true),
            None
        );
    }

    async fn options(app: &Router, path: &str) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// `/req/core/methods` clause B, `/req/create-replace-delete/options-op`/
    /// `options-response`: a plain `OPTIONS` (no CORS preflight headers) to
    /// the items-collection resource gets `200` with an `Allow` header
    /// naming the methods that resource supports.
    #[tokio::test]
    async fn plain_options_on_the_items_collection_reports_the_allowed_methods() {
        let app = write_capability_app();
        let response = options(
            &app,
            &format!("{}/collections/writable/items", catalog_root("features")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let allow = response
            .headers()
            .get(header::ALLOW)
            .expect("a plain OPTIONS response must carry an Allow header")
            .to_str()
            .unwrap();
        assert!(allow.contains("GET"), "{allow}");
        assert!(allow.contains("POST"), "{allow}");
        // `OPTIONS` itself: axum's own built-in fallback Allow (present for
        // any matched route regardless of this layer) never lists it, since
        // this crate never registers an explicit `.options(...)` handler —
        // only `WriteResource::allow` names it, so its presence is
        // proof this response came from this layer, not axum's default.
        assert!(allow.contains("OPTIONS"), "{allow}");
    }

    /// Same requirement, for the single-item resource (`PUT`/`DELETE`
    /// instead of `POST`).
    #[tokio::test]
    async fn plain_options_on_a_single_item_reports_the_allowed_methods() {
        let app = write_capability_app();
        let response = options(
            &app,
            &format!("{}/collections/writable/items/x", catalog_root("features")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let allow = response
            .headers()
            .get(header::ALLOW)
            .expect("a plain OPTIONS response must carry an Allow header")
            .to_str()
            .unwrap();
        assert!(allow.contains("GET"), "{allow}");
        assert!(allow.contains("PUT"), "{allow}");
        assert!(allow.contains("PATCH"), "{allow}");
        assert!(allow.contains("DELETE"), "{allow}");
        // Same "proves this layer, not axum's own fallback" reasoning as
        // the items-collection test above.
        assert!(allow.contains("OPTIONS"), "{allow}");
    }

    /// `#208`, the decisive pair: `Allow` and the actual method outcome are
    /// asserted against each other on two collections that differ only in
    /// their write lane, so a header that is confidently wrong cannot pass.
    ///
    /// OGC API — Features — Part 4 (OGC 20-002r1) Requirement 16 clause C
    /// (`/req/create-replace-delete/options-response`): "The value of the
    /// `Allow` header SHALL be the list of methods that are allowed for the
    /// resource at the time and within the context of the request."
    ///
    /// Before `#208` both halves answered the SAME `Allow` — the URI shape
    /// was the whole input — so the read-only half advertised `PUT` and then
    /// refused it. Asserting the header alone would have passed on that;
    /// issuing the advertised method is what does not.
    #[tokio::test]
    async fn allow_and_the_method_it_advertises_agree_on_the_single_item_resource() {
        let app = write_capability_app();
        let writable = format!("{}/collections/writable/items/x", catalog_root("features"));
        let read_only = format!("{}/collections/demo/items/x", catalog_root("features"));

        // Advertised, and accepted: `PUT` is named, and a `PUT` lands.
        let allow = allow_of(&app, &writable).await;
        assert!(allow.contains("PUT"), "{allow}");
        assert_eq!(
            send(&app, "PUT", &writable).await.status(),
            StatusCode::NO_CONTENT,
            "Allow named PUT on {writable}, so a PUT must be honored there"
        );

        // Not advertised, and refused: `PUT` is absent, and a `PUT` is
        // refused by name. `GET` stays in the list on both — the resource
        // itself has not gone anywhere, only its write methods.
        let allow = allow_of(&app, &read_only).await;
        assert_eq!(allow, "GET, OPTIONS", "{read_only}");
        let refused = send(&app, "PUT", &read_only).await;
        assert_ne!(
            refused.status(),
            StatusCode::NO_CONTENT,
            "Allow withheld PUT on {read_only}, so a PUT must not be honored there"
        );
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        let problem = json_body(refused).await;
        assert_eq!(problem["code"], "NotFound");
        assert_eq!(
            problem["detail"], "collection 'demo' does not support 'write'",
            "the refusal must stay the named `CapabilityUnsupported` one \
             `resolve_write` already gives, naming the collection and the \
             capability"
        );
    }

    /// Same pair for the items-collection resource, whose write method is
    /// `POST` and whose read representation keeps serving either way.
    #[tokio::test]
    async fn allow_and_the_method_it_advertises_agree_on_the_items_collection() {
        let app = write_capability_app();
        let writable = format!("{}/collections/writable/items", catalog_root("features"));
        let read_only = format!("{}/collections/demo/items", catalog_root("features"));

        // Advertised, and accepted.
        let allow = allow_of(&app, &writable).await;
        assert!(allow.contains("POST"), "{allow}");
        assert_eq!(
            send(&app, "POST", &writable).await.status(),
            StatusCode::CREATED,
            "Allow named POST on {writable}, so a POST must be honored there"
        );

        // Not advertised, and refused.
        let allow = allow_of(&app, &read_only).await;
        assert_eq!(allow, "GET, OPTIONS", "{read_only}");
        let refused = send(&app, "POST", &read_only).await;
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(refused).await["detail"],
            "collection 'demo' does not support 'write'"
        );
        // The read half of the very same URI is untouched: this is a
        // narrowing of methods, not a resource that stopped existing — the
        // reason `Allow` still names `GET` above.
        assert_eq!(get(&app, &read_only).await.status(), StatusCode::OK);
    }

    /// The batch-ingest resource (`#114`) has no read representation, so a
    /// read-only collection's `Allow` there collapses to `OPTIONS` alone.
    #[tokio::test]
    async fn allow_on_batch_ingest_names_post_only_where_the_write_lane_resolves() {
        let app = write_capability_app();
        assert_eq!(
            allow_of(
                &app,
                &format!(
                    "{}/collections/writable/items/batch",
                    catalog_root("features")
                )
            )
            .await,
            "POST, OPTIONS"
        );
        assert_eq!(
            allow_of(
                &app,
                &format!("{}/collections/demo/items/batch", catalog_root("features"))
            )
            .await,
            "OPTIONS"
        );
    }

    /// The features root's `conformsTo` list, as a real HTTP response.
    async fn features_conformance_classes(app: &Router) -> Vec<String> {
        let response = get(app, &format!("{}/conformance", catalog_root("features"))).await;
        assert_eq!(response.status(), StatusCode::OK);
        json_body(response).await["conformsTo"]
            .as_array()
            .expect("conformsTo is an array")
            .iter()
            .map(|class| class.as_str().unwrap().to_string())
            .collect()
    }

    /// `#263`, the decisive pair for the conformance list — built the same
    /// way `#208`'s `Allow` pairs above are, and for the same reason: a test
    /// that only asserted the class string was absent would pass on a list
    /// that is confidently wrong. The expected declaration is *derived from*
    /// the behaviour, by running the class's own three methods.
    ///
    /// OGC 20-002r1 Requirement 1 clause A: "A server SHALL implement one or
    /// more of the methods HTTP POST, PUT and/or DELETE for each mutable
    /// resource." So the class is declared exactly when at least one of
    /// those three actually lands somewhere on this deployment, and withheld
    /// when all three are refused everywhere.
    ///
    /// The writable fixture is also `#263`'s "whole deployment or per
    /// collection" answer in executable form: `write_capability_ctx` holds a
    /// writable collection and a read-only one in ONE catalog, and the class
    /// survives — a collection never offered as mutable is not a resource
    /// Requirement 1 clause A quantifies over, so it must not narrow the
    /// claim. The read-only fixture is the Italy demo's shape: nothing
    /// mutable anywhere, so nothing to declare.
    #[tokio::test]
    async fn conformance_declares_create_replace_delete_exactly_when_a_write_method_lands() {
        for (label, app, collection) in [
            (
                "a catalog with one writable collection beside a read-only one",
                write_capability_app(),
                "writable",
            ),
            ("a deployment with nothing mutable", test_app(), "demo"),
        ] {
            let root = catalog_root("features");
            let items = format!("{root}/collections/{collection}/items");
            let item = format!("{items}/x");

            // What the class's own three methods do here, right now. All
            // three are issued, and any one of them landing satisfies clause
            // A's "one or more of the methods HTTP POST, PUT and/or DELETE"
            // — a deployment that implemented only `DELETE` would count too,
            // which is why this is an OR rather than three separate
            // assertions.
            let mut a_method_lands = false;
            for (method, path, success) in [
                ("POST", items.as_str(), StatusCode::CREATED),
                ("PUT", item.as_str(), StatusCode::NO_CONTENT),
                ("DELETE", item.as_str(), StatusCode::NO_CONTENT),
            ] {
                if send(&app, method, path).await.status() == success {
                    a_method_lands = true;
                }
            }

            let declared = features_conformance_classes(&app).await.contains(
                &tellurion_core::outbox::CREATE_REPLACE_DELETE_CONFORMANCE_CLASS.to_string(),
            );
            assert_eq!(
                declared, a_method_lands,
                "the /conformance declaration disagreed with what POST/PUT/DELETE \
                 actually do on {label}"
            );
        }
    }

    /// And the withheld half is withheld by a *named* refusal, never a
    /// silent degradation: the deployment that declares nothing refuses each
    /// of the class's three methods with the same `CapabilityUnsupported`
    /// answer `resolve_write` already gives, naming the collection and the
    /// capability. The test above pins the declaration against the
    /// behaviour; this one pins what the behaviour tells the client.
    #[tokio::test]
    async fn a_deployment_that_withholds_create_replace_delete_refuses_its_methods_by_name() {
        let app = test_app();
        let root = catalog_root("features");
        let items = format!("{root}/collections/demo/items");
        let item = format!("{items}/x");
        for (method, path) in [
            ("POST", items.as_str()),
            ("PUT", item.as_str()),
            ("DELETE", item.as_str()),
        ] {
            let refused = send(&app, method, path).await;
            assert_eq!(refused.status(), StatusCode::NOT_FOUND, "{method} {path}");
            assert_eq!(
                json_body(refused).await["detail"],
                "collection 'demo' does not support 'write'",
                "{method} {path}"
            );
        }
    }

    /// Part 4 clause 9.1 gives the Features requirements class a Dependency
    /// on Requirements Class "Create/Replace/Delete", and clause 5.4 makes a
    /// direct dependency one that "Every server implementing the
    /// requirements class has to conform to". Pinned on the assembled
    /// response, not only at the fold, so a wiring change that extends the
    /// two in independently cannot publish `conf/features` alone.
    #[tokio::test]
    async fn the_features_root_never_cites_part_4_features_without_create_replace_delete() {
        for app in [write_capability_app(), test_app()] {
            let classes = features_conformance_classes(&app).await;
            let cites = |class: &str| classes.iter().any(|declared| declared == class);
            assert!(
                !cites(tellurion_core::outbox::FEATURES_PART4_FEATURES_CLASS)
                    || cites(tellurion_core::outbox::CREATE_REPLACE_DELETE_CONFORMANCE_CLASS),
                "cited conf/features without its conf/create-replace-delete dependency: \
                 {classes:?}"
            );
        }
    }

    /// The issue's third scope bullet: an unresolvable collection id keeps
    /// answering exactly as it did before `#208`. It has no write capability
    /// to describe, so narrowing its `Allow` would say something this server
    /// does not know — and would make `OPTIONS` a collection-existence
    /// oracle in the negative direction, since a narrowed `Allow` would then
    /// mean "does not exist" as well as "is read-only".
    #[tokio::test]
    async fn allow_for_an_unresolvable_collection_still_describes_the_uri_shape() {
        let app = write_capability_app();
        assert_eq!(
            allow_of(
                &app,
                &format!("{}/collections/nope/items/x", catalog_root("features"))
            )
            .await,
            "GET, PUT, PATCH, DELETE, OPTIONS"
        );
    }

    /// A genuine CORS preflight (carrying `Access-Control-Request-Method`)
    /// to a write resource is still answered by `cors`, not this layer —
    /// and now names the write verb it asked about, proving the CORS
    /// `allow_methods` fix actually reaches a preflight for these routes.
    #[tokio::test]
    async fn cors_preflight_on_the_items_collection_allows_post() {
        let app = test_app();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri(format!(
                        "{}/collections/demo/items",
                        catalog_root("features")
                    ))
                    .header(header::ORIGIN, "https://example.test")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let allow_methods = response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .expect("a CORS preflight must carry Access-Control-Allow-Methods")
            .to_str()
            .unwrap();
        assert!(
            allow_methods.contains("POST"),
            "CORS must now permit the write verb the preflight asked about: {allow_methods}"
        );
    }

    /// Same preflight proof for `PUT`/`DELETE` against the single-item
    /// resource.
    #[tokio::test]
    async fn cors_preflight_on_a_single_item_allows_put_and_delete() {
        let app = test_app();
        for method in ["PUT", "PATCH", "DELETE"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("OPTIONS")
                        .uri(format!(
                            "{}/collections/demo/items/x",
                            catalog_root("features")
                        ))
                        .header(header::ORIGIN, "https://example.test")
                        .header(header::ACCESS_CONTROL_REQUEST_METHOD, method)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let allow_methods = response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_METHODS)
                .unwrap()
                .to_str()
                .unwrap();
            assert!(
                allow_methods.contains(method),
                "CORS must permit {method}: {allow_methods}"
            );
        }
    }

    // -- `#185`: the per-tenant protocol exposure matrix ------------------

    async fn request(app: &Router, method: &str, path: &str) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    fn exposure_app(settings: &str) -> Router {
        build(
            test_ctx_with_catalog_settings(settings),
            test_metrics_handle(),
            60,
        )
    }

    /// Every whole-protocol toggle takes its own root down — and only its
    /// own. A `404` (not a bespoke status, not a problem body naming the
    /// setting) is the point: a disabled root must be indistinguishable from
    /// a prefix this server never mounted.
    #[tokio::test]
    async fn a_disabled_protocol_root_answers_404_and_leaves_its_siblings_serving() {
        for (segment, key) in [
            ("features", "features"),
            ("tiles", "tiles"),
            ("styles", "styles"),
            ("3dtiles", "3dtiles"),
            ("stac", "stac"),
        ] {
            let app = exposure_app(&format!("{{ protocols: {{ {key}: disabled }} }}"));
            let response = get(&app, &catalog_root(segment)).await;
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{segment} root must stop answering when '{key}' is disabled"
            );
            assert!(
                to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .is_empty(),
                "{segment}'s refusal must not describe itself"
            );

            for other in ["features", "tiles", "styles", "3dtiles", "stac"] {
                if other == segment {
                    continue;
                }
                assert_eq!(
                    get(&app, &catalog_root(other)).await.status(),
                    StatusCode::OK,
                    "disabling '{key}' must not touch the {other} root"
                );
            }
        }
    }

    /// The gate covers the whole root, not just its landing page.
    #[tokio::test]
    async fn a_disabled_protocol_root_answers_404_on_every_resource_beneath_it() {
        let app = exposure_app("{ protocols: { features: disabled } }");
        for suffix in ["/conformance", "/api", "/collections", "/collections/demo"] {
            assert_eq!(
                get(&app, &format!("{}{suffix}", catalog_root("features")))
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "{suffix} must be gone with the root"
            );
        }
    }

    /// `features_write: disabled` is deliberately NOT a 404: the write
    /// methods share their URIs with reads that keep serving, so the honest
    /// answer is Part 4's `405` naming what remains.
    #[tokio::test]
    async fn a_disabled_write_lane_answers_405_with_a_truthful_allow() {
        let app = exposure_app("{ protocols: { features_write: disabled } }");
        let items = format!("{}/collections/demo/items", catalog_root("features"));
        let item = format!("{}/collections/demo/items/x", catalog_root("features"));
        let batch = format!("{}/collections/demo/items/batch", catalog_root("features"));

        for (method, path, expected_allow) in [
            ("POST", items.as_str(), "GET, OPTIONS"),
            ("PUT", item.as_str(), "GET, OPTIONS"),
            ("PATCH", item.as_str(), "GET, OPTIONS"),
            ("DELETE", item.as_str(), "GET, OPTIONS"),
            ("POST", batch.as_str(), "OPTIONS"),
        ] {
            let response = request(&app, method, path).await;
            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path}"
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::ALLOW)
                    .expect("a 405 must carry an Allow header")
                    .to_str()
                    .unwrap(),
                expected_allow,
                "{method} {path}"
            );
        }

        // The reads on those very same URIs are untouched — the whole reason
        // this is a 405 and not a 404.
        assert_eq!(
            get(&app, &catalog_root("features")).await.status(),
            StatusCode::OK
        );
        assert_ne!(
            get(&app, &items).await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    /// A disabled write lane is scoped to the Features root: STAC's own
    /// read-only surface never had write methods to lose, and the other
    /// roots' non-`GET` traffic is not the Features write lane.
    #[tokio::test]
    async fn a_disabled_write_lane_leaves_the_features_reads_and_other_roots_alone() {
        let app = exposure_app("{ protocols: { features_write: disabled } }");
        for segment in ["features", "tiles", "styles", "3dtiles", "stac"] {
            assert_eq!(
                get(&app, &catalog_root(segment)).await.status(),
                StatusCode::OK,
                "{segment} root must still serve"
            );
        }
    }

    /// The write lane is exposed by default: nothing about the pre-`#185`
    /// behavior changes for a deployment that declares no matrix at all.
    #[tokio::test]
    async fn writes_are_exposed_when_no_level_declares_a_matrix() {
        let app = test_app();
        let response = request(
            &app,
            "POST",
            &format!("{}/collections/demo/items", catalog_root("features")),
        )
        .await;
        assert_ne!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "an undeclared matrix must never gate the write lane"
        );
    }

    /// `/req/create-replace-delete/options-response`: `Allow` names the
    /// methods allowed "at the time and within the context of the request",
    /// so a plain `OPTIONS` must not advertise a verb the very next request
    /// would be refused for.
    ///
    /// Aimed at `write_capability_ctx`'s `writable` collection rather than a
    /// collection with no write lane, so this stays a test of `#185`'s
    /// matrix: the collection here genuinely resolves a `WriteSink`, and the
    /// ONLY reason its write verbs disappear is `features_write: disabled`.
    /// Against a read-only collection it would pass on `#208`'s narrowing
    /// alone and prove nothing about the matrix.
    #[tokio::test]
    async fn plain_options_drops_the_write_verbs_when_the_write_lane_is_not_exposed() {
        let app = build(
            write_capability_ctx_with_catalog_settings(
                "{ protocols: { features_write: disabled } }",
            ),
            test_metrics_handle(),
            60,
        );
        for path in [
            format!("{}/collections/writable/items", catalog_root("features")),
            format!("{}/collections/writable/items/x", catalog_root("features")),
        ] {
            let response = options(&app, &path).await;
            assert_eq!(response.status(), StatusCode::OK);
            let allow = response
                .headers()
                .get(header::ALLOW)
                .expect("a plain OPTIONS response must carry an Allow header")
                .to_str()
                .unwrap();
            assert_eq!(allow, "GET, OPTIONS", "{path}");
        }
    }

    /// With the whole `features` root gone there is no write resource left
    /// for this layer to describe, so it stops answering for one.
    ///
    /// What remains on the response is axum's own route-shape `Allow`,
    /// stamped onto anything its `MethodRouter` fallback produces — and
    /// `cors` answers every `OPTIONS` through exactly that fallback (see
    /// this module's own doc), so no layer in this stack is positioned to
    /// remove it. Narrowing *that* header is `#208`'s standalone
    /// capability-derived-`Allow` work; all `#185` owes here is that this
    /// layer stop making a write claim of its own.
    #[tokio::test]
    async fn plain_options_stops_describing_write_resources_under_a_disabled_features_root() {
        let app = exposure_app("{ protocols: { features: disabled } }");
        let response = options(
            &app,
            &format!("{}/collections/demo/items", catalog_root("features")),
        )
        .await;
        assert_ne!(
            response
                .headers()
                .get(header::ALLOW)
                .map(|value| value.to_str().unwrap().to_string()),
            Some("GET, POST, OPTIONS".to_string()),
            "this layer must not describe a write resource it no longer serves"
        );
    }

    // -- the Processes capability gate (`#182`) ------------------------------

    /// The whole `#182` gating rule, executed against the real assembled app:
    /// a deployment with no job ledger gets **no** Processes root, and turning
    /// the exposure key on does not conjure one. Every path under the prefix
    /// answers the bare `404` an unmounted prefix answers — landing page,
    /// `/conformance` and `/api` included, which is what the availability
    /// gate being outermost buys.
    #[tokio::test]
    async fn a_deployment_with_no_job_ledger_serves_no_processes_root() {
        for settings in ["{}", "{ protocols: { processes: enabled } }"] {
            let app = exposure_app(settings);
            for suffix in ["", "/conformance", "/api", "/processes", "/jobs/anything"] {
                let path = format!("{}{suffix}", catalog_root("processes"));
                let response = get(&app, &path).await;
                assert_eq!(
                    response.status(),
                    StatusCode::NOT_FOUND,
                    "settings {settings}: {path} must not answer without a ledger"
                );
            }
        }
    }

    /// And the tenant directory does not advertise a root that does not
    /// answer — under BOTH reasons it can fail to answer. With the key at its
    /// `disabled` default the directory is byte-for-byte what it was before
    /// this root existed; with the key explicitly `enabled` but no ledger
    /// capability, the directory must still stay silent rather than publish a
    /// link it already knows resolves to `404`.
    #[tokio::test]
    async fn the_tenant_directory_never_advertises_a_processes_root_that_cannot_answer() {
        for settings in ["{}", "{ protocols: { processes: enabled } }"] {
            let app = exposure_app(settings);
            let body = json_body(get(&app, &format!("/{TENANT_EXT}")).await).await;
            let hrefs: Vec<String> = body["links"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|link| link["href"].as_str().map(str::to_string))
                .collect();
            assert!(
                !hrefs.iter().any(|href| href.contains("/processes/")),
                "settings {settings}: a dead processes link must not be published: {hrefs:?}"
            );
            // The other roots' links are untouched by any of this.
            assert!(hrefs.iter().any(|href| href.contains("/features/")));
        }
    }

    // -- collection-kind gate (`#192`) ---------------------------------------

    /// The path shapes the kind gate recognizes, and — more importantly —
    /// the ones it must leave alone. A landing page, `/conformance`, `/api`,
    /// `/tileMatrixSets`, `/styles` and STAC's `/search` all name no
    /// collection, so the gate must never try to resolve one out of them.
    #[test]
    fn collection_of_path_matches_only_paths_that_name_a_collection() {
        assert_eq!(
            collection_of_path("/public/features/catalogs/default/collections/demo"),
            Some(("public", "default", "demo"))
        );
        assert_eq!(
            collection_of_path("/public/features/catalogs/default/collections/demo/items/7"),
            Some(("public", "default", "demo"))
        );
        assert_eq!(
            collection_of_path("/public/records/catalogs/default/collections/thesaurus/items"),
            Some(("public", "default", "thesaurus"))
        );

        for path in [
            "/",
            "/public/",
            "/public/features/catalogs/default/",
            "/public/features/catalogs/default/conformance",
            "/public/features/catalogs/default/api",
            "/public/features/catalogs/default/collections",
            "/public/tiles/catalogs/default/tileMatrixSets",
            "/public/styles/catalogs/default/styles",
            "/public/stac/catalogs/default/search",
            "/metrics",
        ] {
            assert_eq!(collection_of_path(path), None, "path: {path}");
        }
    }

    /// `landing::tenant_directory` walks `Protocol::ALL`; a disabled
    /// protocol must drop out of it, or the directory publishes links it
    /// already knows answer 404.
    #[tokio::test]
    async fn the_tenant_directory_omits_disabled_protocol_roots() {
        let app = exposure_app("{ protocols: { tiles: disabled, stac: disabled } }");
        let body = json_body(get(&app, &format!("/{TENANT_EXT}")).await).await;
        let rels: Vec<String> = body["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|link| link["rel"].as_str().unwrap().to_string())
            .collect();
        assert!(rels.contains(&"features".to_string()), "{rels:?}");
        assert!(rels.contains(&"styles".to_string()), "{rels:?}");
        assert!(rels.contains(&"3dtiles".to_string()), "{rels:?}");
        assert!(!rels.contains(&"tiles".to_string()), "{rels:?}");
        assert!(!rels.contains(&"stac".to_string()), "{rels:?}");

        // Every advertised root really answers.
        for rel in ["features", "styles", "3dtiles"] {
            assert_eq!(get(&app, &catalog_root(rel)).await.status(), StatusCode::OK);
        }
    }

    /// The effective-config view is the issue's own answer to "why is this
    /// protocol off here" — value plus the level that supplied it.
    #[tokio::test]
    async fn the_effective_config_view_reports_the_exposure_matrix_and_its_level() {
        let app = exposure_app("{ protocols: { tiles: disabled } }");

        let catalog = json_body(
            get(
                &app,
                &format!("/{TENANT_EXT}/config/catalogs/{CATALOG_EXT}/effective"),
            )
            .await,
        )
        .await;
        assert_eq!(
            catalog["settings"]["protocols"]["value"]["tiles"],
            "disabled"
        );
        assert_eq!(
            catalog["settings"]["protocols"]["provenance"]["kind"],
            "local_override"
        );

        // Nothing declared above the catalog, so the tenant node still
        // reports `null` — "nobody expressed an opinion," not a fabricated
        // all-enabled matrix.
        let tenant = json_body(get(&app, &format!("/{TENANT_EXT}/config/effective")).await).await;
        assert!(tenant["settings"]["protocols"]["value"].is_null());
        assert_eq!(
            tenant["settings"]["protocols"]["provenance"]["kind"],
            "built_in_default"
        );
    }

    // -- config-mutation control lane (`#110`) -------------------------
    //
    // Distinct from `test_ctx`/`test_app` above: the mutation surface needs
    // a REAL file-backed `ConfigStore` (compare-and-swap writes read and
    // rewrite an actual file) and an authenticated platform-admin token, so
    // every test in this section builds its own context rather than reusing
    // the in-memory fixture the rest of this module shares.

    const MUTATION_ADMIN_TOKEN: &str = "mutation-admin-token";
    const MUTATION_ADMIN_PRINCIPAL: &str = "mutation-test-admin";
    const MUTATION_NON_ADMIN_TOKEN: &str = "mutation-non-admin-token";
    const DURABLE_CONTROL_TOKEN_ENV: &str = "TELLURION_DURABLE_CONTROL_TEST_TOKEN";
    const DURABLE_CONTROL_TOKEN: &str = "durable-control-test-token";
    static TEST_ENV_OVERRIDE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct AsyncScopedEnvOverride {
        _lock: Option<tokio::sync::MutexGuard<'static, ()>>,
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl AsyncScopedEnvOverride {
        async fn set(name: &'static str, value: &str) -> Self {
            let lock = TEST_ENV_OVERRIDE_LOCK.lock().await;
            Self::set_while_locked(name, value, Some(lock))
        }

        fn set_while_locked(
            name: &'static str,
            value: &str,
            lock: Option<tokio::sync::MutexGuard<'static, ()>>,
        ) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self {
                _lock: lock,
                name,
                previous,
            }
        }

        async fn restored_value_after_override(
            name: &'static str,
            previous: &std::ffi::OsStr,
            value: &str,
        ) -> Option<std::ffi::OsString> {
            let lock = TEST_ENV_OVERRIDE_LOCK.lock().await;
            let original = std::env::var_os(name);
            std::env::set_var(name, previous);
            drop(Self::set_while_locked(name, value, None));
            let restored = std::env::var_os(name);
            match original {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
            drop(lock);
            restored
        }
    }

    impl Drop for AsyncScopedEnvOverride {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

    /// A fresh, private temp file for one test's config document — private
    /// (its own directory) and uniquely named via a per-process atomic
    /// counter, the same "never a wall-clock timestamp" fix
    /// `config_store.rs`'s own tests already apply to their temp-file
    /// naming, so parallel `cargo test` runs never collide.
    fn mutation_test_config_path(contents: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "tellurion-app-mutation-test-{}-{unique}.yaml",
            std::process::id()
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// A config document with the mutation control lane's own auth fixture:
    /// one platform-admin token (`MUTATION_ADMIN_TOKEN`) and one ordinary,
    /// non-admin bearer token (`MUTATION_NON_ADMIN_TOKEN`) authorizing the
    /// same tenant — enough to exercise every `enforce_platform_admin_auth`
    /// branch (no credential / wrong credential / right credential) without
    /// a second config fixture.
    const MUTATION_TEST_CONFIG: &str = r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
webhooks:
  - id: alerts
    url: https://example.test/hook
    secret_env: ALERTS_WEBHOOK_SECRET
auth:
  bearer_tokens:
    - token: mutation-admin-token
      tenants: [public]
      platform_admin: true
      principal: mutation-test-admin
    - token: mutation-non-admin-token
      tenants: [public]
"#;

    /// A config document with no `auth:` section at all — the fixture
    /// `config_mutation_routes_do_not_exist_without_auth_configured` needs
    /// to prove the mutation surface behaves as if unregistered.
    const MUTATION_TEST_CONFIG_NO_AUTH: &str = r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#;

    const DURABLE_CONTROL_TEST_CONFIG: &str = r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - token_env: TELLURION_DURABLE_CONTROL_TEST_TOKEN
      tenants: [public]
      platform_admin: true
      principal: mutation-test-admin
"#;

    /// Builds a real `AppContext` backed by a real `FileConfigStore` over
    /// `path` (already containing `contents`) plus whatever `auth:` section
    /// that document declares — the mutation control lane's own test
    /// fixture, distinct from `test_ctx` (which never attaches a
    /// `ConfigStore` at all).
    fn mutation_test_ctx(path: &std::path::Path) -> Arc<AppContext> {
        let store = tellurion_core::FileConfigStore::new(path);
        let config = tellurion_core::ConfigStore::load(&store).unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer = tellurion_core::build_authorizer(&config.auth)
            .expect("no bearer principal in this fixture reads a token_env");
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        Arc::new(
            AppContext::new(config, router, resolver, authorizer, cache, style_store)
                .with_config_store(Arc::new(tellurion_core::FileConfigStore::new(path))
                    as Arc<dyn tellurion_core::ConfigStore>),
        )
    }

    fn durable_control_test_ctx_with_store(
        store: Option<Arc<dyn ControlStore>>,
    ) -> Arc<AppContext> {
        let path = mutation_test_config_path(DURABLE_CONTROL_TEST_CONFIG);
        let config_store = tellurion_core::FileConfigStore::new(&path);
        let config = tellurion_core::ConfigStore::load(&config_store).unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer = tellurion_core::build_authorizer(&config.auth)
            .expect("no bearer principal in this fixture reads a token_env");
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let context = AppContext::new(
            config.clone(),
            router,
            resolver,
            authorizer,
            cache,
            style_store,
        )
        .with_config_store(Arc::new(tellurion_core::FileConfigStore::new(&path))
            as Arc<dyn tellurion_core::ConfigStore>);
        Arc::new(match store {
            Some(store) => context.with_control_store(store),
            None => context,
        })
    }

    fn durable_control_principal() -> PrincipalIdentity {
        PrincipalIdentity {
            issuer: "urn:tellurion:static".to_string(),
            subject: MUTATION_ADMIN_PRINCIPAL.to_string(),
        }
    }

    fn durable_control_snapshot() -> ControlSnapshot {
        ControlSnapshot {
            config: serde_yaml::from_str(DURABLE_CONTROL_TEST_CONFIG).unwrap(),
            role_bindings: vec![RoleBinding {
                principal: durable_control_principal(),
                role: "sysadmin".to_string(),
                scope: ControlScope::Platform,
            }],
            path_policies: Vec::new(),
            tombstoned_resources: Vec::new(),
        }
    }

    async fn durable_control_test_ctx(
        with_store: bool,
    ) -> (Arc<AppContext>, Option<Arc<InMemoryControlStore>>) {
        if !with_store {
            return (durable_control_test_ctx_with_store(None), None);
        }

        let principal = durable_control_principal();
        let store = Arc::new(InMemoryControlStore::new());
        store
            .bootstrap_if_empty(
                &durable_control_snapshot(),
                &principal,
                ControlBootstrapMode::RequireInitialSysadmin,
            )
            .await
            .unwrap();
        (
            durable_control_test_ctx_with_store(Some(Arc::clone(&store) as Arc<dyn ControlStore>)),
            Some(store),
        )
    }

    fn replace_durable_platform_settings() -> ControlChangeSet {
        let mut changed_config: AppConfig =
            serde_yaml::from_str(DURABLE_CONTROL_TEST_CONFIG).unwrap();
        changed_config.settings.cache_ttl_s = Some(8);
        ControlChangeSet {
            idempotency_key: None,
            operations: vec![VersionedControlOperation {
                expected_entity_version: None,
                operation: ControlOperation::ReplacePlatformSettings(changed_config),
            }],
        }
    }

    async fn durable_control_request(app: &Router) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/_control/v1/platform/settings")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {DURABLE_CONTROL_TOKEN}"),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-request-id", "durable-control-full-app-1")
                    .body(Body::from(
                        serde_json::to_vec(&replace_durable_platform_settings()).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn durable_control_token_env_restores_a_preexisting_value() {
        const RESTORE_TEST_ENV: &str = "TELLURION_DURABLE_CONTROL_RESTORE_TEST_TOKEN";
        let previous = std::ffi::OsString::from("preexisting-durable-control-token");
        let restored = AsyncScopedEnvOverride::restored_value_after_override(
            RESTORE_TEST_ENV,
            &previous,
            DURABLE_CONTROL_TOKEN,
        )
        .await;
        assert_eq!(restored, Some(previous));
    }

    #[tokio::test]
    async fn full_app_mounts_durable_control_only_for_a_durable_context() {
        let _token_env =
            AsyncScopedEnvOverride::set(DURABLE_CONTROL_TOKEN_ENV, DURABLE_CONTROL_TOKEN).await;
        let (without_store, _) = durable_control_test_ctx(false).await;
        let app_without_store = build(without_store, test_metrics_handle(), 60);
        assert_eq!(
            durable_control_request(&app_without_store).await.status(),
            StatusCode::NOT_FOUND
        );

        let (with_store, store) = durable_control_test_ctx(true).await;
        let app_with_store = build(with_store, test_metrics_handle(), 60);
        let response = durable_control_request(&app_with_store).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-request-id").unwrap(),
            "durable-control-full-app-1"
        );
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(json_body(response).await["revision"], 2);
        drop(store);
    }

    #[cfg(feature = "control-sqlite")]
    #[tokio::test]
    async fn sqlite_http_commit_survives_reopen() {
        let _token_env =
            AsyncScopedEnvOverride::set(DURABLE_CONTROL_TOKEN_ENV, DURABLE_CONTROL_TOKEN).await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("control.sqlite");
        let store = Arc::new(
            tellurion_control_sqlite::SqliteControlStore::open(&path)
                .await
                .unwrap(),
        );
        store
            .bootstrap_if_empty(
                &durable_control_snapshot(),
                &durable_control_principal(),
                ControlBootstrapMode::RequireInitialSysadmin,
            )
            .await
            .unwrap();
        let store_for_context: Arc<dyn ControlStore> = Arc::clone(&store) as Arc<dyn ControlStore>;
        let ctx = durable_control_test_ctx_with_store(Some(Arc::clone(&store_for_context)));
        let app = build(Arc::clone(&ctx), test_metrics_handle(), 60);

        let response = durable_control_request(&app).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json_body(response).await["revision"], 2);

        drop(app);
        drop(ctx);
        drop(store_for_context);
        drop(store);

        let reopened = tellurion_control_sqlite::SqliteControlStore::open(&path)
            .await
            .unwrap();
        let snapshot = reopened.load_snapshot().await.unwrap();
        assert_eq!(snapshot.revision, 2);
        assert_eq!(snapshot.snapshot.config.settings.cache_ttl_s, Some(8));
        drop(reopened);
    }

    /// A `PUT /config` request, optionally bearing an expected-version
    /// header and a JSON body — the mutation endpoint's own request shape.
    async fn put_config_request(
        app: &Router,
        path: &str,
        bearer: Option<&str>,
        expected_version: Option<&str>,
        body: &serde_json::Value,
    ) -> Response {
        let mut builder = Request::builder().method("PUT").uri(path);
        if let Some(token) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(version) = expected_version {
            builder = builder.header("x-config-expected-version", version);
        }
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        app.clone()
            .oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    fn current_version_header(response: &Response) -> String {
        response
            .headers()
            .get("x-config-version")
            .expect("a successful GET /config must carry the current version header")
            .to_str()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn config_mutation_routes_do_not_exist_without_auth_configured() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG_NO_AUTH);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        assert_eq!(get(&app, "/config").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            put_config_request(&app, "/config", None, None, &serde_json::json!({}))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn config_mutation_requires_a_credential() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get_with_bearer(&app, "/config", None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn config_mutation_denies_a_non_platform_admin_token() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get_with_bearer(&app, "/config", Some(MUTATION_NON_ADMIN_TOKEN)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn get_raw_config_returns_the_document_and_a_version_header() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get_with_bearer(&app, "/config", Some(MUTATION_ADMIN_TOKEN)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let version = current_version_header(&response);
        assert!(!version.is_empty());
        let body = json_body(response).await;
        assert_eq!(body["tenants"][0]["id"], "public");
        let _ = std::fs::remove_file(path);
    }

    /// `#144`, the decisive negative. A platform admin whose token lives in
    /// an environment variable authorizes exactly as an inline one does —
    /// and the three surfaces that serialize configuration back out do not
    /// contain the credential anywhere, because the document never held it:
    ///
    /// - `GET /config`, which returns the RAW whole `AppConfig` (it echoes
    ///   whatever the document says, by contract — a read-then-`PUT` round
    ///   trip would be corrupted by masking a value in place, so the fix is
    ///   for the document not to carry one);
    /// - `GET /config/effective`, the unauthenticated platform node;
    /// - `GET /config/profiles`, mounted beside it.
    ///
    /// Asserted against the raw response bytes rather than a parsed field,
    /// so a token surfacing under any key at all fails this.
    #[tokio::test]
    async fn a_token_env_credential_authorizes_and_reaches_no_config_response_body() {
        const VAR: &str = "TELLURION_TEST_ADMIN_TOKEN_144";
        const SECRET: &str = "s3cret-admin-token-from-the-environment";
        std::env::set_var(VAR, SECRET);

        let path = mutation_test_config_path(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - token_env: TELLURION_TEST_ADMIN_TOKEN_144
      tenants: [public]
      platform_admin: true
      principal: env-sourced-admin
"#,
        );
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        // The environment's value IS the credential: the platform-admin
        // lane, which nothing else in this deployment can open, opens.
        let response = get_with_bearer(&app, "/config", Some(SECRET)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let raw = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let raw = String::from_utf8_lossy(&raw).to_string();
        assert!(
            !raw.contains(SECRET),
            "GET /config echoed the credential: {raw}"
        );
        // And the document it did return still names where the value lives,
        // so this is a document carrying the reference, not one that lost
        // the principal.
        assert!(raw.contains(VAR), "GET /config lost the token_env: {raw}");

        for route in ["/config/effective", "/config/profiles"] {
            let body = to_bytes(get(&app, route).await.into_body(), usize::MAX)
                .await
                .unwrap();
            let body = String::from_utf8_lossy(&body).to_string();
            assert!(!body.contains(SECRET), "{route} echoed the credential");
        }

        std::env::remove_var(VAR);
        let _ = std::fs::remove_file(path);
    }

    /// The same negative for a deployment that has NOT moved its
    /// credentials yet: `/config/effective` is unauthenticated (settings are
    /// behavior, not secrets), so the one thing it must never grow is a
    /// credential — including from an `auth:` section that still declares
    /// one inline.
    #[tokio::test]
    async fn the_unauthenticated_effective_config_view_never_carries_an_inline_token() {
        let ctx = auth_test_ctx();
        let app = build(ctx, test_metrics_handle(), 60);
        for route in [
            "/config/effective".to_string(),
            "/config/profiles".to_string(),
            format!("/{AUTH_TENANT_A}/config/effective"),
        ] {
            let response = get_with_bearer(&app, &route, Some(AUTH_TOKEN_A)).await;
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body = String::from_utf8_lossy(&body).to_string();
            assert!(
                !body.contains(AUTH_TOKEN_A) && !body.contains(AUTH_TOKEN_B),
                "{route} echoed a bearer token: {body}"
            );
        }
    }

    #[tokio::test]
    async fn webhook_subscriptions_are_enumerable_on_the_control_lane() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get_with_bearer(&app, "/config/webhooks", Some(MUTATION_ADMIN_TOKEN)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["subscriptions"][0]["id"], "alerts");
        assert_eq!(body["subscriptions"][0]["enabled"], true);
        assert_eq!(
            body["subscriptions"][0]["secret_env"],
            "ALERTS_WEBHOOK_SECRET"
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn dead_letter_inspection_names_a_subscription_that_is_not_running() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        let response = get_with_bearer(
            &app,
            "/config/webhooks/alerts/dead-letters",
            Some(MUTATION_ADMIN_TOKEN),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = json_body(response).await;
        assert_eq!(body["code"], "WebhookSubscriptionNotRunning");

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn dead_letter_inspection_fills_and_pages_a_running_subscription() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let webhook_registry = Arc::new(WebhookRegistry::new());
        let runtime = Arc::new(WebhookSubscriptionRuntime::new(
            "alerts".to_string(),
            "https://example.test/hook".to_string(),
            b"test-secret".to_vec(),
            Vec::new(),
            ["demo".to_string()],
            10,
        ));
        let outbox = DeadLetterOutbox {
            obligations: (1..=2)
                .map(|sequence| Obligation {
                    sequence: Sequence(sequence),
                    feature_id: format!("f{sequence}"),
                    kind: MutationKind::Upsert(serde_json::json!({
                        "type": "Feature",
                        "properties": {"private": "payload"},
                        "geometry": null
                    })),
                    version: Sequence(sequence),
                    committed_at: SystemTime::UNIX_EPOCH,
                    extent: tellurion_core::ObligationExtent::Unrecorded,
                })
                .collect(),
        };
        let (delivery_shutdown_tx, delivery_shutdown_rx) = tokio::sync::watch::channel(false);
        let delivery = tokio::spawn(tellurion_core::run_webhook_consumer(
            Arc::new(outbox),
            Arc::clone(&runtime),
            ctx.current().config.collections[0].clone(),
            Arc::new(AlwaysFailWebhook),
            WebhookConsumerSettings {
                batch_size: 10,
                retry: WebhookRetryPolicy {
                    max_attempts: 1,
                    base_backoff_ms: 0,
                    max_backoff_ms: 0,
                },
                poll_interval: Duration::from_secs(60),
            },
            delivery_shutdown_rx,
        ));
        tokio::time::timeout(Duration::from_secs(1), async {
            while runtime.dead_letter_count() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the failing delivery should fill the dead-letter ring");
        delivery_shutdown_tx.send(true).unwrap();
        delivery.await.unwrap();
        webhook_registry.replace(HashMap::from([("alerts".to_string(), runtime)]));
        let app = build_with_webhook_registry(
            ctx,
            test_metrics_handle(),
            60,
            Readiness::new(),
            webhook_registry,
            None,
        );

        let response = get_with_bearer(
            &app,
            "/config/webhooks/alerts/dead-letters?limit=1",
            Some(MUTATION_ADMIN_TOKEN),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["subscription"], "alerts");
        assert_eq!(body["entries"][0]["entry"]["itemId"], "f1");
        assert_eq!(body["entries"][0]["entry"]["collection"], "demo");
        assert!(body["entries"][0]["entry"].get("payload").is_none());
        assert!(body["entries"][0]["entry"].get("properties").is_none());
        let next = body["next"].as_str().unwrap();

        let response = get_with_bearer(
            &app,
            &format!("/config/webhooks/alerts/dead-letters?limit=1&since={next}"),
            Some(MUTATION_ADMIN_TOKEN),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["entries"][0]["entry"]["itemId"], "f2");
        assert!(body.get("next").is_none());

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn put_config_applies_a_valid_change_and_records_an_audit_entry() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(Arc::clone(&ctx), test_metrics_handle(), 60);

        let before = get_with_bearer(&app, "/config", Some(MUTATION_ADMIN_TOKEN)).await;
        let version = current_version_header(&before);
        let mut candidate = json_body(before).await;
        candidate["tenants"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "id": "second" }));

        let response = put_config_request(
            &app,
            "/config",
            Some(MUTATION_ADMIN_TOKEN),
            Some(&version),
            &candidate,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let result = json_body(response).await;
        let new_version = result["version"].as_str().unwrap().to_string();
        assert_ne!(new_version, version);

        // Persisted: a fresh GET sees the added tenant and the new version.
        let after = get_with_bearer(&app, "/config", Some(MUTATION_ADMIN_TOKEN)).await;
        assert_eq!(current_version_header(&after), new_version);
        let after_body = json_body(after).await;
        assert_eq!(after_body["tenants"].as_array().unwrap().len(), 2);

        // Audit: exactly one record, naming the real principal and both
        // versions — never the raw token value.
        let recent = ctx.audit_log.recent();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].principal, MUTATION_ADMIN_PRINCIPAL);
        assert_eq!(recent[0].expected_version, version);
        assert_eq!(recent[0].new_version, new_version);
        assert!(
            recent[0].summary.contains("tenants"),
            "summary was: {}",
            recent[0].summary
        );

        let _ = std::fs::remove_file(path.with_extension("yaml.bak"));
        let _ = std::fs::remove_file(path);
    }

    /// The issue's named test: a concurrent-write conflict is a named
    /// `409`, never a silently applied lost update.
    #[tokio::test]
    async fn put_config_with_a_stale_version_is_a_named_409_conflict() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        let before = get_with_bearer(&app, "/config", Some(MUTATION_ADMIN_TOKEN)).await;
        let stale_version = current_version_header(&before);
        let candidate = json_body(before).await;

        // First writer, using the still-current version, succeeds.
        let first = put_config_request(
            &app,
            "/config",
            Some(MUTATION_ADMIN_TOKEN),
            Some(&stale_version),
            &candidate,
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);

        // Retrying with the now-stale version must be refused.
        let second = put_config_request(
            &app,
            "/config",
            Some(MUTATION_ADMIN_TOKEN),
            Some(&stale_version),
            &candidate,
        )
        .await;
        assert_eq!(second.status(), StatusCode::CONFLICT);
        let problem = json_body(second).await;
        assert_eq!(problem["code"], "ConfigVersionConflict");

        let _ = std::fs::remove_file(path.with_extension("yaml.bak"));
        let _ = std::fs::remove_file(path);
    }

    /// The issue's named test, the other half: an invalid edit is refused
    /// (`422`) and the previously-persisted document keeps serving — proved
    /// here by re-reading the store directly and asserting it is
    /// byte-for-byte the same version as before the rejected attempt, not
    /// merely "an error was returned."
    #[tokio::test]
    async fn put_config_with_an_invalid_document_is_refused_and_the_old_document_keeps_serving() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        let before = get_with_bearer(&app, "/config", Some(MUTATION_ADMIN_TOKEN)).await;
        let version = current_version_header(&before);

        // Referentially broken: a collection naming a catalog nothing
        // declares.
        let invalid = serde_yaml::from_str::<serde_json::Value>(
            r#"
collections:
  - id: broken
    catalog: nonexistent
    storage: nonexistent
    table: demo
    geometry: geom
    pk: id
"#,
        )
        .unwrap();

        let response = put_config_request(
            &app,
            "/config",
            Some(MUTATION_ADMIN_TOKEN),
            Some(&version),
            &invalid,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let problem = json_body(response).await;
        assert_eq!(problem["code"], "InvalidConfiguration");

        // The store itself proves continuity: same version, same tenants.
        let after = get_with_bearer(&app, "/config", Some(MUTATION_ADMIN_TOKEN)).await;
        assert_eq!(
            current_version_header(&after),
            version,
            "a refused write must never change the store's version"
        );
        let after_body = json_body(after).await;
        assert_eq!(after_body["tenants"][0]["id"], "public");

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn put_config_dry_run_reports_a_valid_verdict_without_applying() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        let before = get_with_bearer(&app, "/config", Some(MUTATION_ADMIN_TOKEN)).await;
        let version = current_version_header(&before);
        let mut candidate = json_body(before).await;
        candidate["tenants"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "id": "second" }));

        let response = put_config_request(
            &app,
            "/config?dry_run=true",
            Some(MUTATION_ADMIN_TOKEN),
            None,
            &candidate,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let verdict = json_body(response).await;
        assert_eq!(verdict["valid"], true);

        // Never applied: the version and content are unchanged.
        let after = get_with_bearer(&app, "/config", Some(MUTATION_ADMIN_TOKEN)).await;
        assert_eq!(current_version_header(&after), version);
        let after_body = json_body(after).await;
        assert_eq!(after_body["tenants"].as_array().unwrap().len(), 1);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn put_config_dry_run_reports_an_invalid_verdict_as_a_200_not_an_error() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        let invalid = serde_json::json!({
            "collections": [
                { "id": "broken", "catalog": "nonexistent", "storage": "nonexistent",
                  "table": "demo", "geometry": "geom", "pk": "id" }
            ]
        });

        let response = put_config_request(
            &app,
            "/config?dry_run=true",
            Some(MUTATION_ADMIN_TOKEN),
            None,
            &invalid,
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "a dry run itself always succeeds regardless of the verdict"
        );
        let verdict = json_body(response).await;
        assert_eq!(verdict["valid"], false);
        assert!(verdict["detail"].as_str().unwrap().contains("nonexistent"));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn put_config_without_the_expected_version_header_is_a_400() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        let response = put_config_request(
            &app,
            "/config",
            Some(MUTATION_ADMIN_TOKEN),
            None,
            &serde_json::json!({ "storages": [] }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let _ = std::fs::remove_file(path);
    }

    /// `#110`'s `final` settings keys, proved through the write path (load
    /// time is covered exhaustively in `tellurion_core::config`'s own
    /// tests): a candidate document that has a catalog override a
    /// platform-declared final key is refused the same way any other
    /// invalid document is — `422`, naming the key.
    #[tokio::test]
    async fn put_config_refuses_a_final_key_override_by_name() {
        let path = mutation_test_config_path(MUTATION_TEST_CONFIG);
        let ctx = mutation_test_ctx(&path);
        let app = build(ctx, test_metrics_handle(), 60);

        let before = get_with_bearer(&app, "/config", Some(MUTATION_ADMIN_TOKEN)).await;
        let version = current_version_header(&before);
        let mut candidate = json_body(before).await;
        candidate["settings"] = serde_json::json!({
            "tile_vertex_budget": 500000,
            "final": ["tile_vertex_budget"],
        });
        candidate["catalogs"] = serde_json::json!([
            { "id": "default", "tenant": "public", "settings": { "tile_vertex_budget": 1 } }
        ]);

        let response = put_config_request(
            &app,
            "/config",
            Some(MUTATION_ADMIN_TOKEN),
            Some(&version),
            &candidate,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let problem = json_body(response).await;
        assert!(
            problem["detail"]
                .as_str()
                .unwrap()
                .contains("tile_vertex_budget"),
            "detail was: {problem}"
        );

        let _ = std::fs::remove_file(path);
    }

    // -- advanced CQL2 and CRS by Reference, end to end over real HTTP ------
    //
    // Everything above drives the built app against an in-memory fake
    // driver; these instead register the real `tellurion-postgis` driver
    // (feature-gated the same way `tests/binary.rs` gates its own postgis-
    // only tests) against a live database, so a request genuinely crosses
    // axum routing -> `tellurion-features` handlers -> real SQL -> a real
    // PostGIS instance and back. Skipped gracefully unless
    // `TELLURION_TEST_DATABASE_URL` is set, matching every other
    // database-backed test in this workspace. `tellurion-postgis` itself
    // already proves `ST_Transform`/`ST_FlipCoordinates` reprojection
    // directly against the driver (`tellurion-postgis/tests/live.rs`); what
    // these tests add is the piece only reachable over HTTP: `bbox-crs`
    // axis-order handling, which is `tellurion-features`' handler's own job
    // (`params::resolve_items_crs`), not the driver's.
    #[cfg(feature = "postgis")]
    mod live_crs_and_advanced_cql2 {
        use super::*;
        use tellurion_postgis::PostgisDriverFactory;
        use tokio_postgres::NoTls;

        const LIVE_URL_ENV_VAR: &str = "TELLURION_SERVER_LIVE_TEST_URL";

        async fn seed_typed_attributes(database_url: &str, table: &str) {
            let (client, connection) = tokio_postgres::connect(database_url, NoTls)
                .await
                .expect("connects to the test database");
            tokio::spawn(async move {
                let _ = connection.await;
            });
            client
                .batch_execute(&format!(
                    "DROP TABLE IF EXISTS {table};
                     CREATE TABLE {table} (
                         id bigserial PRIMARY KEY,
                         geom geometry(Point, 4326) NOT NULL,
                         name text
                     );
                     INSERT INTO {table} (geom, name) VALUES
                         (ST_SetSRID(ST_MakePoint(10, 45), 4326), 'alpha'),
                         (ST_SetSRID(ST_MakePoint(11, 46), 4326), 'beta'),
                         (ST_SetSRID(ST_MakePoint(12, 47), 4326), 'gamma');
                     ANALYZE {table};"
                ))
                .await
                .expect("seeds the typed-attribute test table");
        }

        /// A single config with `table`/`geometry`/`pk` all omitted, so
        /// `Router::effective_decl` actually derives the descriptor (and
        /// carries its `srid` onto the served decl) — the same reason
        /// `tellurion-features`' own in-memory `CrsCatalog` fixture omits
        /// them; here the physical shape comes from a real PostGIS table
        /// instead of a fake `CatalogSource`.
        fn live_postgis_ctx(table: &str, database_url: &str) -> Arc<AppContext> {
            let config: AppConfig = serde_yaml::from_str(&format!(
                r#"
storages: [ {{ id: main, driver: postgis, url_env: {LIVE_URL_ENV_VAR} }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: {table}
    catalog: default
    storage: main
"#
            ))
            .unwrap();
            config.validate().unwrap();

            // Safety: every test in this module sets this var to the same
            // `database_url` value before building its own `Router` (which
            // resolves it once, synchronously, before any connection pool
            // spawns worker tasks) — same reasoning `tellurion-postgis`'s own
            // live tests already document for their equivalent env var.
            unsafe {
                std::env::set_var(LIVE_URL_ENV_VAR, database_url);
            }

            let mut registry = Registry::new();
            registry.register(Arc::new(PostgisDriverFactory::new(60)));
            let router = CoreRouter::build(&config, &registry).unwrap();
            let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
            let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
            let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
            Arc::new(AppContext::new(
                config,
                router,
                resolver,
                None,
                cache,
                style_store,
            ))
        }

        /// `#33` follow-up, advanced comparison operators and CASEI, over
        /// real HTTP: `LIKE`, `BETWEEN`-shaped `IN`, and `CASEI` each narrow
        /// the seeded three-row table exactly the way a plain `=` filter
        /// already does end to end in `tellurion-features`' own (in-memory)
        /// `filter_narrows_the_result_set_end_to_end`.
        #[tokio::test]
        async fn like_in_and_casei_narrow_the_result_set_through_real_http() {
            let Ok(database_url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
                eprintln!("skipping like_in_and_casei_narrow_the_result_set_through_real_http: TELLURION_TEST_DATABASE_URL not set");
                return;
            };
            let table = "tellurion_server_live_test_advanced_cql2";
            seed_typed_attributes(&database_url, table).await;

            let ctx = live_postgis_ctx(table, &database_url);
            let app = build(ctx, test_metrics_handle(), 60);
            let items_path = format!("{}/collections/{table}/items", catalog_root("features"));

            let like = get(&app, &format!("{items_path}?filter=name%20LIKE%20'al%25'")).await;
            assert_eq!(like.status(), StatusCode::OK);
            let like_body = json_body(like).await;
            assert_eq!(like_body["numberReturned"], 1);
            assert_eq!(like_body["features"][0]["properties"]["name"], "alpha");

            let in_filter = get(
                &app,
                &format!("{items_path}?filter=name%20IN%20('alpha'%2C'gamma')"),
            )
            .await;
            assert_eq!(in_filter.status(), StatusCode::OK);
            let in_body = json_body(in_filter).await;
            assert_eq!(in_body["numberReturned"], 2);

            let casei = get(
                &app,
                &format!("{items_path}?filter=CASEI(name)%20%3D%20CASEI('ALPHA')"),
            )
            .await;
            assert_eq!(casei.status(), StatusCode::OK);
            let casei_body = json_body(casei).await;
            assert_eq!(casei_body["numberReturned"], 1);
            assert_eq!(casei_body["features"][0]["properties"]["name"], "alpha");
        }

        /// The classic OGC API Features Part 2 axis-order trap, over real
        /// HTTP: a `bbox-crs` naming this collection's own storage CRS
        /// (EPSG:4326 by authority — latitude before longitude) must select
        /// the same row a `bbox-crs=CRS84` (longitude before latitude) query
        /// covering the identical geographic box does. This is the one
        /// piece of CRS handling that lives in `tellurion-features`' handler
        /// (`params::resolve_items_crs`'s axis swap), not in
        /// `tellurion-postgis`, so it needs a real HTTP round trip — a
        /// direct driver call (`tellurion-postgis/tests/live.rs`) never goes
        /// through that swap at all.
        #[tokio::test]
        async fn bbox_crs_axis_order_both_ways_select_the_same_row_through_real_http() {
            let Ok(database_url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
                eprintln!("skipping bbox_crs_axis_order_both_ways_select_the_same_row_through_real_http: TELLURION_TEST_DATABASE_URL not set");
                return;
            };
            let table = "tellurion_server_live_test_bbox_crs_axis";
            seed_typed_attributes(&database_url, table).await;

            let ctx = live_postgis_ctx(table, &database_url);
            let app = build(ctx, test_metrics_handle(), 60);
            let items_path = format!("{}/collections/{table}/items", catalog_root("features"));

            // CRS84 order (longitude first): covers only the 'alpha' point
            // (lon 10, lat 45).
            let crs84 = get(&app, &format!("{items_path}?bbox=9,44,10.5,45.5")).await;
            assert_eq!(crs84.status(), StatusCode::OK);
            let crs84_body = json_body(crs84).await;
            assert_eq!(crs84_body["numberReturned"], 1);
            assert_eq!(crs84_body["features"][0]["properties"]["name"], "alpha");

            // The identical geographic box, but in EPSG:4326-by-authority
            // order (latitude first) via `bbox-crs=<this collection's own
            // storage CRS>` — the handler must swap it back before the SQL
            // envelope is built, selecting the same row.
            let storage_crs_encoded = "http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F4326";
            let flipped = get(
                &app,
                &format!("{items_path}?bbox=44,9,45.5,10.5&bbox-crs={storage_crs_encoded}"),
            )
            .await;
            assert_eq!(flipped.status(), StatusCode::OK);
            let flipped_body = json_body(flipped).await;
            assert_eq!(
                flipped_body["numberReturned"], 1,
                "a bbox-crs in the storage CRS's own (latitude-first) order must select the \
                 same row bbox-crs=CRS84 did, once the handler axis-swaps it back"
            );
            assert_eq!(flipped_body["features"][0]["properties"]["name"], "alpha");
        }

        /// **`filter-crs` end to end** (`#217`, OGC API — Features Part 3
        /// Requirement 8, `/req/filter/filter-crs-param`): a real HTTP query
        /// string, through `params::resolve_items_crs`, into real SQL,
        /// against a real PostGIS instance.
        ///
        /// The same seeded 4326 table and the same trick the `bbox-crs`
        /// test above uses — EPSG:4326 referenced by authority is
        /// latitude-before-longitude, CRS84 is longitude-first — but the
        /// opposite expectation, and that difference is the point.
        /// `bbox-crs` is axis-corrected in the handler, so both orders must
        /// select the *same* row. A filter's spatial literals are not
        /// rewritten in the handler at all; they are corrected in SQL by the
        /// driver, and only when `filter-crs` says so. So the *identical*
        /// CQL2 text must select a row under one `filter-crs` and no row
        /// under the other. Before `#217` the parameter was accepted and
        /// dropped on the floor, and both of these returned 'alpha' — the
        /// wrong features under a 200.
        #[tokio::test]
        async fn filter_crs_changes_which_rows_match_through_real_http() {
            let Ok(database_url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
                eprintln!("skipping filter_crs_changes_which_rows_match_through_real_http: TELLURION_TEST_DATABASE_URL not set");
                return;
            };
            let table = "tellurion_server_live_test_filter_crs";
            seed_typed_attributes(&database_url, table).await;

            let ctx = live_postgis_ctx(table, &database_url);
            let app = build(ctx, test_metrics_handle(), 60);
            let items_path = format!("{}/collections/{table}/items", catalog_root("features"));
            // S_INTERSECTS(geom, BBOX(9,44,10.5,45.5)) — longitude-first, so
            // it covers the seeded 'alpha' point at (lon 10, lat 45).
            let lon_lat_filter = "filter=S_INTERSECTS%28geom%2CBBOX%289%2C44%2C10.5%2C45.5%29%29";
            let crs84_encoded = "http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FOGC%2F1.3%2FCRS84";
            let storage_crs_encoded = "http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F4326";

            // No `filter-crs` at all: Requirement 7's CRS84 default, and
            // byte-for-byte the behaviour this deployment had before `#217`.
            let default = get(&app, &format!("{items_path}?{lon_lat_filter}")).await;
            assert_eq!(default.status(), StatusCode::OK);
            let default_body = json_body(default).await;
            assert_eq!(default_body["numberReturned"], 1);
            assert_eq!(default_body["features"][0]["properties"]["name"], "alpha");

            // `filter-crs=CRS84` spelled out: the same answer.
            let crs84 = get(
                &app,
                &format!("{items_path}?{lon_lat_filter}&filter-crs={crs84_encoded}"),
            )
            .await;
            assert_eq!(crs84.status(), StatusCode::OK);
            let crs84_body = json_body(crs84).await;
            assert_eq!(crs84_body["numberReturned"], 1);
            assert_eq!(crs84_body["features"][0]["properties"]["name"], "alpha");

            // The identical filter text under `filter-crs=<this collection's
            // own storage CRS>`: read latitude-first, that box covers
            // longitudes 44-45.5, where nothing is seeded.
            let flipped = get(
                &app,
                &format!("{items_path}?{lon_lat_filter}&filter-crs={storage_crs_encoded}"),
            )
            .await;
            assert_eq!(flipped.status(), StatusCode::OK);
            let flipped_body = json_body(flipped).await;
            assert_eq!(
                flipped_body["numberReturned"], 0,
                "the same filter geometry read in EPSG:4326-by-authority order must select \
                 nothing; if it still returns 'alpha', filter-crs was ignored"
            );

            // And the mirror image: the axis-swapped numbers select 'alpha'
            // again under `filter-crs=<storage>`, and nothing under CRS84.
            let lat_lon_filter = "filter=S_INTERSECTS%28geom%2CBBOX%2844%2C9%2C45.5%2C10.5%29%29";
            let swapped = get(
                &app,
                &format!("{items_path}?{lat_lon_filter}&filter-crs={storage_crs_encoded}"),
            )
            .await;
            assert_eq!(swapped.status(), StatusCode::OK);
            let swapped_body = json_body(swapped).await;
            assert_eq!(swapped_body["numberReturned"], 1);
            assert_eq!(swapped_body["features"][0]["properties"]["name"], "alpha");

            let swapped_as_crs84 = get(
                &app,
                &format!("{items_path}?{lat_lon_filter}&filter-crs={crs84_encoded}"),
            )
            .await;
            assert_eq!(swapped_as_crs84.status(), StatusCode::OK);
            assert_eq!(json_body(swapped_as_crs84).await["numberReturned"], 0);

            // The `self` link must carry `filter-crs` through, or a client
            // paging from it would evaluate page two in a different CRS.
            let linked = get(
                &app,
                &format!("{items_path}?{lat_lon_filter}&filter-crs={storage_crs_encoded}"),
            )
            .await;
            let links = json_body(linked).await;
            let self_href = links["links"]
                .as_array()
                .unwrap()
                .iter()
                .find(|l| l["rel"] == "self")
                .expect("a self link")["href"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(
                self_href.contains("filter-crs="),
                "self link dropped filter-crs: {self_href}"
            );
        }

        /// `Content-Crs` and a rejected non-default `crs`, over real HTTP —
        /// `tellurion-features`' own in-memory tests already cover this
        /// against a driver that can't reproject; this proves the header is
        /// also set correctly (and a supported non-default `crs` accepted)
        /// against a driver that genuinely can.
        #[tokio::test]
        async fn crs_header_and_storage_crs_output_through_real_http() {
            let Ok(database_url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
                eprintln!("skipping crs_header_and_storage_crs_output_through_real_http: TELLURION_TEST_DATABASE_URL not set");
                return;
            };
            let table = "tellurion_server_live_test_crs_header";
            seed_typed_attributes(&database_url, table).await;

            let ctx = live_postgis_ctx(table, &database_url);
            let app = build(ctx, test_metrics_handle(), 60);
            let items_path = format!(
                "{}/collections/{table}/items?limit=1",
                catalog_root("features")
            );

            let default_response = get(&app, &items_path).await;
            assert_eq!(default_response.status(), StatusCode::OK);
            assert_eq!(
                default_response.headers().get("content-crs").unwrap(),
                "<http://www.opengis.net/def/crs/OGC/1.3/CRS84>"
            );

            let storage_crs_encoded = "http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F4326";
            let storage_response =
                get(&app, &format!("{items_path}&crs={storage_crs_encoded}")).await;
            assert_eq!(storage_response.status(), StatusCode::OK);
            assert_eq!(
                storage_response.headers().get("content-crs").unwrap(),
                "<http://www.opengis.net/def/crs/EPSG/0/4326>"
            );
            let storage_body = json_body(storage_response).await;
            let coords = storage_body["features"][0]["geometry"]["coordinates"]
                .as_array()
                .unwrap();
            assert!(
                (coords[0].as_f64().unwrap() - 45.0).abs() < f64::EPSILON,
                "expected latitude first for the storage CRS, coords were {coords:?}"
            );

            let unsupported = get(&app, &format!("{items_path}&crs=bogus")).await;
            assert_eq!(unsupported.status(), StatusCode::BAD_REQUEST);
        }

        // -- STAC /search filter-crs, end to end over real HTTP (`#248`) ----
        //
        // The `/items` tests above cover OGC API — Features Part 3's own
        // `filter-crs`, whose value space includes a collection's storage CRS.
        // The STAC API Filter Extension pins the same parameter to CRS84 —
        // "server must only accept `http://www.opengis.net/def/crs/OGC/1.3/
        // CRS84` as a valid value, may reject any others" — so the `/search`
        // lane honours exactly that one value and refuses the rest by name.
        // Both halves are proved here against a real database, because both
        // are things only real SQL can demonstrate: a CRS84 literal genuinely
        // reprojected into a projected storage CRS, and a refusal that
        // replaces rows that used to come back wrong.

        /// A points table in a *projected* storage CRS (Web Mercator), seeded
        /// with the same three points `seed_typed_attributes` uses, expressed
        /// in metres. A CRS84 filter literal cannot match anything in this
        /// table without a real `ST_Transform` — degrees and metres do not
        /// overlap — which is exactly what makes it able to tell "`filter-crs`
        /// was honoured" from "`filter-crs` was accepted and dropped".
        async fn seed_projected_points(database_url: &str, table: &str) {
            let (client, connection) = tokio_postgres::connect(database_url, NoTls)
                .await
                .expect("connects to the test database");
            tokio::spawn(async move {
                let _ = connection.await;
            });
            client
                .batch_execute(&format!(
                    "DROP TABLE IF EXISTS {table};
                     CREATE TABLE {table} (
                         id bigserial PRIMARY KEY,
                         geom geometry(Point, 3857) NOT NULL,
                         name text
                     );
                     INSERT INTO {table} (geom, name) VALUES
                         (ST_Transform(ST_SetSRID(ST_MakePoint(10, 45), 4326), 3857), 'alpha'),
                         (ST_Transform(ST_SetSRID(ST_MakePoint(11, 46), 4326), 3857), 'beta'),
                         (ST_Transform(ST_SetSRID(ST_MakePoint(12, 47), 4326), 3857), 'gamma');
                     ANALYZE {table};"
                ))
                .await
                .expect("seeds the projected-storage test table");
        }

        async fn post_search(app: &Router, path: &str, body: serde_json::Value) -> Response {
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header(axum::http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap()
        }

        /// **The `#248` truth table that can only pass for the right reason.**
        ///
        /// One 3857 collection, one CQL2 filter shape, four cells over
        /// `(box, filter-crs)` — and every cell answers differently from its
        /// neighbours, with the declared CRS as the only explanation:
        ///
        /// |                    | `filter-crs=CRS84` | `filter-crs=EPSG:0:3857` |
        /// |--------------------|--------------------|--------------------------|
        /// | box over the point | 200, 1 feature     | 400 naming `filter-crs`  |
        /// | box beside it      | 200, 0 features    | 400 naming `filter-crs`  |
        ///
        /// The left column is the honoured half: the box's four numbers are
        /// degrees, the geometry column is metres, and the only thing that can
        /// make a degree box select a metre point is the `ST_Transform`
        /// `sql::geometry_literal_expr`'s `RequestedCrs::Crs84` arm emits —
        /// which it only emits because the parameter reached the driver. Two
        /// different boxes rather than one, so the column cannot pass by
        /// matching everything.
        ///
        /// The right column is the refused half, and it is the one that would
        /// look most reasonable to implement: `EPSG:0:3857` is this
        /// collection's own storage CRS, the value the `/items` lane genuinely
        /// honours. On `/search` it is refused by name, because a
        /// cross-collection endpoint has no single storage CRS a URI could
        /// name and the extension pins the value space to CRS84 regardless.
        #[tokio::test]
        async fn stac_search_filter_crs_truth_table_over_a_projected_collection_against_a_real_database(
        ) {
            let Ok(database_url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
                eprintln!("skipping stac_search_filter_crs_truth_table_over_a_projected_collection_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
                return;
            };
            let table = "tellurion_server_live_test_stac_filter_crs_3857";
            seed_projected_points(&database_url, table).await;

            let ctx = live_postgis_ctx(table, &database_url);
            let app = build(ctx, test_metrics_handle(), 60);
            let search_path = format!("{}/search", catalog_root("stac"));
            let crs84_encoded = "http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FOGC%2F1.3%2FCRS84";
            let storage_crs_encoded = "http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F3857";
            // CRS84 degrees. The first covers (lon 10, lat 45) — 'alpha'; the
            // second is a degree box over open water off Portugal, covering
            // none of the three seeded points.
            let over_the_point = "S_INTERSECTS%28geom%2CBBOX%289%2C44%2C10.5%2C45.5%29%29";
            let beside_the_point = "S_INTERSECTS%28geom%2CBBOX%28-20%2C30%2C-19%2C31%29%29";

            for (label, filter, expected) in [
                ("box over the point", over_the_point, 1),
                ("box beside the point", beside_the_point, 0),
            ] {
                let response = get(
                    &app,
                    &format!(
                        "{search_path}?collections={table}&filter={filter}&filter-crs={crs84_encoded}"
                    ),
                )
                .await;
                assert_eq!(
                    response.status(),
                    StatusCode::OK,
                    "{label}: an explicit CRS84 filter-crs must be honoured, not refused"
                );
                let body = json_body(response).await;
                assert_eq!(
                    body["numberReturned"], expected,
                    "{label}: a CRS84 degree box can only select rows in a 3857 column once \
                     the literal is genuinely transformed; got {body}"
                );
                if expected == 1 {
                    assert_eq!(body["features"][0]["properties"]["name"], "alpha");
                }

                // The same filter text under this collection's own storage
                // CRS: refused by name on GET...
                let refused = get(
                    &app,
                    &format!(
                        "{search_path}?collections={table}&filter={filter}&filter-crs={storage_crs_encoded}"
                    ),
                )
                .await;
                assert_eq!(
                    refused.status(),
                    StatusCode::BAD_REQUEST,
                    "{label}: /search accepts CRS84 only"
                );
                let detail = json_body(refused).await["detail"]
                    .as_str()
                    .unwrap()
                    .to_string();
                assert!(
                    detail.contains("filter-crs"),
                    "{label}: the refusal must name the parameter, got: {detail}"
                );
            }
        }

        /// The defect `#248` was opened for, closed over real HTTP against a
        /// 4326 collection — the shape every deployment in this workspace
        /// actually runs.
        ///
        /// EPSG:4326 referenced by authority is datum-identical to CRS84 and
        /// latitude-first, so before `#248` a client that declared it had its
        /// longitude-first box read longitude-first anyway and got 'alpha'
        /// back under a `200`: rows selected in a CRS it did not ask for. All
        /// three requests below returned exactly that. Now the third is a 400
        /// naming the parameter, on GET *and* on POST — where the parameter is
        /// the body field `filter-crs`, spelled exactly as on the query string.
        #[tokio::test]
        async fn stac_search_refuses_a_non_crs84_filter_crs_by_name_against_a_real_database() {
            let Ok(database_url) = std::env::var("TELLURION_TEST_DATABASE_URL") else {
                eprintln!("skipping stac_search_refuses_a_non_crs84_filter_crs_by_name_against_a_real_database: TELLURION_TEST_DATABASE_URL not set");
                return;
            };
            let table = "tellurion_server_live_test_stac_filter_crs_4326";
            seed_typed_attributes(&database_url, table).await;

            let ctx = live_postgis_ctx(table, &database_url);
            let app = build(ctx, test_metrics_handle(), 60);
            let search_path = format!("{}/search", catalog_root("stac"));
            let cql2 = "S_INTERSECTS(geom,BBOX(9,44,10.5,45.5))";
            let encoded_filter = "S_INTERSECTS%28geom%2CBBOX%289%2C44%2C10.5%2C45.5%29%29";
            let crs84_encoded = "http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FOGC%2F1.3%2FCRS84";
            let crs84 = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";
            let authority_4326_encoded =
                "http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F4326";
            let authority_4326 = "http://www.opengis.net/def/crs/EPSG/0/4326";

            // No `filter-crs`: campaign rule 1 — byte-for-byte what this lane
            // served before `#248`.
            let default = get(
                &app,
                &format!("{search_path}?collections={table}&filter={encoded_filter}"),
            )
            .await;
            assert_eq!(default.status(), StatusCode::OK);
            let default_body = json_body(default).await;
            assert_eq!(default_body["numberReturned"], 1);
            assert_eq!(default_body["features"][0]["properties"]["name"], "alpha");

            // `filter-crs=CRS84` spelled out: the extension's own default, and
            // a no-op against a CRS84 storage — the same answer, and the
            // `self` link carries the parameter so page two reads it the same
            // way page one did.
            let explicit = get(
                &app,
                &format!(
                    "{search_path}?collections={table}&filter={encoded_filter}&filter-crs={crs84_encoded}"
                ),
            )
            .await;
            assert_eq!(explicit.status(), StatusCode::OK);
            let explicit_body = json_body(explicit).await;
            assert_eq!(explicit_body["numberReturned"], 1);
            assert_eq!(explicit_body["features"][0]["properties"]["name"], "alpha");
            let self_href = explicit_body["links"]
                .as_array()
                .unwrap()
                .iter()
                .find(|l| l["rel"] == "self")
                .expect("a self link")["href"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(
                self_href.contains("filter-crs="),
                "self link dropped filter-crs: {self_href}"
            );

            // The same filter text, declared in EPSG:4326 by authority. Before
            // `#248`: 200, 'alpha'. Now: a 400 naming the parameter.
            let refused = get(
                &app,
                &format!(
                    "{search_path}?collections={table}&filter={encoded_filter}&filter-crs={authority_4326_encoded}"
                ),
            )
            .await;
            assert_eq!(
                refused.status(),
                StatusCode::BAD_REQUEST,
                "a filter-crs /search cannot honour must be refused, never answered with rows \
                 read in a different CRS"
            );
            assert!(json_body(refused).await["detail"]
                .as_str()
                .unwrap()
                .contains("filter-crs"));

            // ...and the POST body field behaves identically, under the same
            // hyphenated name.
            let post_ok = post_search(
                &app,
                &search_path,
                serde_json::json!({
                    "collections": [table],
                    "filter": cql2,
                    "filter-lang": "cql2-text",
                    "filter-crs": crs84,
                }),
            )
            .await;
            assert_eq!(post_ok.status(), StatusCode::OK);
            assert_eq!(json_body(post_ok).await["numberReturned"], 1);

            let post_refused = post_search(
                &app,
                &search_path,
                serde_json::json!({
                    "collections": [table],
                    "filter": cql2,
                    "filter-lang": "cql2-text",
                    "filter-crs": authority_4326,
                }),
            )
            .await;
            assert_eq!(post_refused.status(), StatusCode::BAD_REQUEST);
            assert!(json_body(post_refused).await["detail"]
                .as_str()
                .unwrap()
                .contains("filter-crs"));
        }
    }

    // ---------------------------------------------------------------------
    // `#215`: hierarchical path-scoped administration policy.
    //
    // Every test below issues a real HTTP request against the real router.
    // A test that asked `ControlPolicySet::authorize` directly would prove
    // the evaluator; these prove the server, which is the only thing an
    // unauthorised caller can reach.
    // ---------------------------------------------------------------------

    /// The `#215` fixture. Two tenants, one of them reached under an
    /// external id that differs from its internal one (the alias vector);
    /// two catalogs under `acme` (the sibling vector) and one under `beta`
    /// (the wrong-parent vector); one collection under each `acme` catalog.
    ///
    /// Four static principals, each named so a `RoleBinding` can address it
    /// as `urn:tellurion:static#<principal>` — the identity
    /// `auth::StaticBearerAuthorizer::subject` already derives for a named
    /// static token, so the whole matrix below is exercised without a JWKS
    /// endpoint anywhere in it.
    const POLICY_TEST_CONFIG: &str = r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants:
  - id: acme
    external_id: acme-public
  - id: beta
catalogs:
  - { id: cadastre, tenant: acme, external_id: cadastre-public }
  - { id: zoning, tenant: acme }
  - { id: slashed, tenant: acme, external_id: "with/slash" }
  - { id: bcat, tenant: beta }
collections:
  - id: parcels
    catalog: cadastre
    storage: main
    table: parcels
    geometry: geom
    pk: id
  - id: lots
    catalog: zoning
    storage: main
    table: lots
    geometry: geom
    pk: id
auth:
  bearer_tokens:
    - token: policy-sysadmin-token
      tenants: [acme, beta]
      platform_admin: true
      principal: sysadmin-principal
    - token: policy-operator-token
      tenants: [acme, beta]
      principal: operator-principal
    - token: policy-stranger-token
      tenants: [acme, beta]
      principal: stranger-principal
"#;

    const POLICY_SYSADMIN_TOKEN: &str = "policy-sysadmin-token";
    const POLICY_OPERATOR_TOKEN: &str = "policy-operator-token";
    const POLICY_STRANGER_TOKEN: &str = "policy-stranger-token";

    fn static_principal(subject: &str) -> tellurion_core::PrincipalIdentity {
        tellurion_core::PrincipalIdentity {
            issuer: "urn:tellurion:static".to_string(),
            subject: subject.to_string(),
        }
    }

    fn policy_binding(
        subject: &str,
        role: &str,
        scope: tellurion_core::ControlScope,
    ) -> tellurion_core::RoleBinding {
        tellurion_core::RoleBinding {
            principal: static_principal(subject),
            role: role.to_string(),
            scope,
        }
    }

    fn policy_statement(
        id: &str,
        effect: tellurion_core::PolicyEffect,
        roles: &[&str],
        patterns: &[&str],
    ) -> tellurion_core::PathPolicy {
        tellurion_core::PathPolicy {
            id: id.to_string(),
            role: None,
            scope: None,
            effect,
            methods: vec!["GET".to_string(), "PUT".to_string()],
            patterns: patterns.iter().map(|p| (*p).to_string()).collect(),
            roles: roles.iter().map(|r| (*r).to_string()).collect(),
            conditions: Vec::new(),
        }
    }

    fn tenant_scope_of(tenant: &str) -> tellurion_core::ControlScope {
        tellurion_core::ControlScope::Tenant {
            tenant_id: tenant.to_string(),
        }
    }

    fn catalog_scope_of(tenant: &str, catalog: &str) -> tellurion_core::ControlScope {
        tellurion_core::ControlScope::Catalog {
            tenant_id: tenant.to_string(),
            catalog_id: catalog.to_string(),
        }
    }

    fn collection_scope_of(
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> tellurion_core::ControlScope {
        tellurion_core::ControlScope::Collection {
            tenant_id: tenant.to_string(),
            catalog_id: catalog.to_string(),
            collection_id: collection.to_string(),
        }
    }

    /// Builds the whole app — real router, real middleware stack, real
    /// authorizer — over `POLICY_TEST_CONFIG` plus one compiled policy set.
    fn policy_app(
        bindings: &[tellurion_core::RoleBinding],
        policies: &[tellurion_core::PathPolicy],
    ) -> (Router, Arc<AppContext>, std::path::PathBuf) {
        let path = mutation_test_config_path(POLICY_TEST_CONFIG);
        let store = tellurion_core::FileConfigStore::new(&path);
        let versioned = tellurion_core::ConfigStore::load_versioned(&store).unwrap();
        let config = versioned.config;
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = CoreRouter::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer = tellurion_core::build_authorizer(&config.auth)
            .expect("no bearer principal in this fixture reads a token_env");
        let registry_reader: Arc<dyn tellurion_core::RegistryReader> =
            Arc::new(tellurion_core::FileRegistryReader::build(&config));
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let control_policy = Arc::new(
            tellurion_core::ControlPolicySet::compile(bindings, policies)
                .expect("the fixture's patterns compile"),
        );
        let tenants = config.tenants.clone();
        let ctx = Arc::new(
            AppContext::new_with_registry_version_and_policy(
                config,
                tenants,
                router,
                resolver,
                authorizer,
                registry_reader,
                cache,
                style_store,
                versioned.version,
                control_policy,
                None,
            )
            .with_config_store(Arc::new(tellurion_core::FileConfigStore::new(&path))
                as Arc<dyn tellurion_core::ConfigStore>),
        );
        let app = build(Arc::clone(&ctx), test_metrics_handle(), 60);
        (app, ctx, path)
    }

    // The administrative paths this fixture addresses, by scope. External
    // ids throughout — `acme-public`/`cadastre-public` are deliberately not
    // the internal ids a binding names.
    const PLATFORM_VIEW: &str = "/config/effective";
    const TENANT_VIEW: &str = "/acme-public/config/effective";
    const CATALOG_VIEW: &str = "/acme-public/config/catalogs/cadastre-public/effective";
    const SIBLING_CATALOG_VIEW: &str = "/acme-public/config/catalogs/zoning/effective";
    const COLLECTION_VIEW: &str =
        "/acme-public/config/catalogs/cadastre-public/collections/parcels/effective";
    const OTHER_TENANT_VIEW: &str = "/beta/config/effective";

    /// The complete administrative answer table for this fixture, as status
    /// codes. Captured by issuing every request; used to compare two
    /// deployments byte-for-byte on the axis that matters.
    async fn administrative_answers(app: &Router) -> Vec<(String, u16)> {
        let mut answers = Vec::new();
        for (path, bearer) in [
            (PLATFORM_VIEW, None),
            ("/config/profiles", None),
            (PLATFORM_VIEW, Some(POLICY_OPERATOR_TOKEN)),
            ("/config", None),
            ("/config", Some(POLICY_OPERATOR_TOKEN)),
            ("/config", Some(POLICY_SYSADMIN_TOKEN)),
            (TENANT_VIEW, None),
            (TENANT_VIEW, Some(POLICY_OPERATOR_TOKEN)),
            (CATALOG_VIEW, Some(POLICY_OPERATOR_TOKEN)),
            (SIBLING_CATALOG_VIEW, Some(POLICY_OPERATOR_TOKEN)),
            (COLLECTION_VIEW, Some(POLICY_OPERATOR_TOKEN)),
            (OTHER_TENANT_VIEW, Some(POLICY_OPERATOR_TOKEN)),
            // A catalog that exists, but under the other tenant.
            (
                "/acme-public/config/catalogs/bcat/effective",
                Some(POLICY_OPERATOR_TOKEN),
            ),
            // An encoded separator naming no catalog at all.
            (
                "/acme-public/config/catalogs/cadastre%2Fpublic/effective",
                Some(POLICY_OPERATOR_TOKEN),
            ),
            // An encoded separator that really does name a catalog — the
            // only way to reach one whose external id contains a separator.
            (
                "/acme-public/config/catalogs/with%2Fslash/effective",
                Some(POLICY_OPERATOR_TOKEN),
            ),
        ] {
            let response = get_with_bearer(app, path, bearer).await;
            answers.push((
                format!("{path} bearer={}", bearer.unwrap_or("<none>")),
                response.status().as_u16(),
            ));
        }
        answers
    }

    /// **Rule 1, proved rather than asserted.** A deployment that declares
    /// no path scopes authorises exactly what it authorises today — and so
    /// does one whose statements are all about a subtree this table never
    /// touches (`#215`'s engagement rule R0). The comparison is the whole
    /// administrative answer table, every path and every credential, not a
    /// sampled one.
    ///
    /// The third arrangement is the control: statements that DO mention
    /// these paths must change the table, or the first two comparisons
    /// would be passing vacuously.
    #[tokio::test]
    async fn a_deployment_that_declares_no_path_scopes_answers_exactly_as_it_did() {
        let (undeclared_app, _ctx, undeclared_path) = policy_app(&[], &[]);
        let baseline = administrative_answers(&undeclared_app).await;

        // Every answer the pre-`#215` server gave, named explicitly so this
        // test fails if the baseline itself drifts rather than silently
        // comparing two equally-wrong tables.
        assert_eq!(
            baseline,
            vec![
                ("/config/effective bearer=<none>".to_string(), 200),
                ("/config/profiles bearer=<none>".to_string(), 200),
                (
                    "/config/effective bearer=policy-operator-token".to_string(),
                    200
                ),
                ("/config bearer=<none>".to_string(), 401),
                ("/config bearer=policy-operator-token".to_string(), 403),
                ("/config bearer=policy-sysadmin-token".to_string(), 200),
                (
                    "/acme-public/config/effective bearer=<none>".to_string(),
                    401
                ),
                (
                    "/acme-public/config/effective bearer=policy-operator-token".to_string(),
                    200
                ),
                (
                    "/acme-public/config/catalogs/cadastre-public/effective bearer=policy-operator-token"
                        .to_string(),
                    200
                ),
                (
                    "/acme-public/config/catalogs/zoning/effective bearer=policy-operator-token"
                        .to_string(),
                    200
                ),
                (
                    "/acme-public/config/catalogs/cadastre-public/collections/parcels/effective bearer=policy-operator-token"
                        .to_string(),
                    200
                ),
                ("/beta/config/effective bearer=policy-operator-token".to_string(), 200),
                (
                    "/acme-public/config/catalogs/bcat/effective bearer=policy-operator-token"
                        .to_string(),
                    404
                ),
                (
                    "/acme-public/config/catalogs/cadastre%2Fpublic/effective bearer=policy-operator-token"
                        .to_string(),
                    404
                ),
                (
                    "/acme-public/config/catalogs/with%2Fslash/effective bearer=policy-operator-token"
                        .to_string(),
                    200
                ),
            ]
        );

        // A statement about a subtree none of those paths lies in. R0 says
        // this changes nothing at all — including the encoded-separator
        // answer, which stays the `404` it always was rather than becoming
        // the `400` a governed path would get.
        let (elsewhere_app, _ctx, elsewhere_path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "viewer",
                tenant_scope_of("beta"),
            )],
            &[policy_statement(
                "beta-catalogs-only",
                tellurion_core::PolicyEffect::Allow,
                &["viewer"],
                &["/beta/config/catalogs/**"],
            )],
        );
        assert_eq!(administrative_answers(&elsewhere_app).await, baseline);

        // The control: a statement that DOES mention these paths changes
        // the table, so the two comparisons above are not vacuous.
        let (governed_app, _ctx, governed_path) = policy_app(
            &[],
            &[policy_statement(
                "governs-everything",
                tellurion_core::PolicyEffect::Allow,
                &["nobody-holds-this"],
                &["/**"],
            )],
        );
        assert_ne!(administrative_answers(&governed_app).await, baseline);

        for path in [undeclared_path, elsewhere_path, governed_path] {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Precedence pair **(Allow @ platform, Deny @ tenant)**, judged at
    /// tenant scope. The deny wins, and the loser really cannot reach the
    /// resource: the request is issued and refused, not inspected.
    #[tokio::test]
    async fn a_tenant_deny_beats_a_platform_allow_held_by_the_same_principal() {
        let (app, _ctx, path) = policy_app(
            &[
                policy_binding("operator-principal", "reader", ControlScope::Platform),
                policy_binding("operator-principal", "blocked", tenant_scope_of("acme")),
            ],
            &[
                policy_statement(
                    "read-all",
                    tellurion_core::PolicyEffect::Allow,
                    &["reader"],
                    &["/**"],
                ),
                policy_statement(
                    "block-all",
                    tellurion_core::PolicyEffect::Deny,
                    &["blocked"],
                    &["/**"],
                ),
            ],
        );

        // The winner: denied at tenant scope, 403 (the tenant boundary was
        // already crossed, so its existence is not news — see
        // `control_checkpoint`'s own doc).
        let refused = get_with_bearer(&app, TENANT_VIEW, Some(POLICY_OPERATOR_TOKEN)).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);

        // The loser really is a loser: the same platform allow, unopposed,
        // does reach the same resource for a principal with no tenant deny.
        let (unopposed, _ctx, unopposed_path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                ControlScope::Platform,
            )],
            &[policy_statement(
                "read-all",
                tellurion_core::PolicyEffect::Allow,
                &["reader"],
                &["/**"],
            )],
        );
        assert_eq!(
            get_with_bearer(&unopposed, TENANT_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::OK
        );

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(unopposed_path);
    }

    /// Precedence pair **(Deny @ tenant, Allow @ catalog)**, judged at
    /// catalog scope: the SHALLOWER deny beats the DEEPER allow. Depth
    /// breaks no ties.
    #[tokio::test]
    async fn a_tenant_deny_beats_a_catalog_allow_beneath_it() {
        let (app, _ctx, path) = policy_app(
            &[
                policy_binding("operator-principal", "blocked", tenant_scope_of("acme")),
                policy_binding(
                    "operator-principal",
                    "reader",
                    catalog_scope_of("acme", "cadastre"),
                ),
            ],
            &[
                policy_statement(
                    "read-all",
                    tellurion_core::PolicyEffect::Allow,
                    &["reader"],
                    &["/**"],
                ),
                policy_statement(
                    "block-all",
                    tellurion_core::PolicyEffect::Deny,
                    &["blocked"],
                    &["/**"],
                ),
            ],
        );
        // Catalog scope refuses with a bare `404`: at this depth,
        // `403`-versus-`404` would itself enumerate the tenant's catalogs.
        let refused = get_with_bearer(&app, CATALOG_VIEW, Some(POLICY_OPERATOR_TOKEN)).await;
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_file(path);
    }

    /// Precedence pair **(Allow @ catalog, Deny @ collection)**, judged at
    /// collection scope: the DEEPER deny beats the SHALLOWER allow — the
    /// other direction of the same rule.
    #[tokio::test]
    async fn a_collection_deny_beats_a_catalog_allow_above_it() {
        let (app, _ctx, path) = policy_app(
            &[
                policy_binding(
                    "operator-principal",
                    "reader",
                    catalog_scope_of("acme", "cadastre"),
                ),
                policy_binding(
                    "operator-principal",
                    "blocked",
                    collection_scope_of("acme", "cadastre", "parcels"),
                ),
            ],
            &[
                policy_statement(
                    "read-all",
                    tellurion_core::PolicyEffect::Allow,
                    &["reader"],
                    &["/**"],
                ),
                policy_statement(
                    "block-all",
                    tellurion_core::PolicyEffect::Deny,
                    &["blocked"],
                    &["/**"],
                ),
            ],
        );
        assert_eq!(
            get_with_bearer(&app, COLLECTION_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        // The catalog above it, which the same allow covers and no deny
        // touches, still serves — so this is the collection being refused,
        // not the whole subtree collapsing.
        assert_eq!(
            get_with_bearer(&app, CATALOG_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::OK
        );
        let _ = std::fs::remove_file(path);
    }

    /// Precedence pairs **(Allow @ catalog, request @ its parent tenant)**
    /// and **(Allow @ collection, request @ its parent catalog)** — grants
    /// flow down and never up.
    #[tokio::test]
    async fn a_grant_beneath_a_resource_never_reaches_the_resource_above_it() {
        let read_all = policy_statement(
            "read-all",
            tellurion_core::PolicyEffect::Allow,
            &["reader"],
            &["/**"],
        );

        let (catalog_bound, _ctx, catalog_path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                catalog_scope_of("acme", "cadastre"),
            )],
            std::slice::from_ref(&read_all),
        );
        // Down: allowed.
        assert_eq!(
            get_with_bearer(&catalog_bound, CATALOG_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::OK
        );
        // Up: refused at the parent tenant, which is a tenant-scope refusal
        // and therefore a `403`.
        assert_eq!(
            get_with_bearer(&catalog_bound, TENANT_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );

        let (collection_bound, _ctx, collection_path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                collection_scope_of("acme", "cadastre", "parcels"),
            )],
            &[read_all],
        );
        assert_eq!(
            get_with_bearer(
                &collection_bound,
                COLLECTION_VIEW,
                Some(POLICY_OPERATOR_TOKEN)
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            get_with_bearer(&collection_bound, CATALOG_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        let _ = std::fs::remove_file(catalog_path);
        let _ = std::fs::remove_file(collection_path);
    }

    /// Precedence pairs **(Allow @ tenant A, request @ tenant B)** and
    /// **(Allow @ catalog X, request @ sibling catalog Y)** — the "nested
    /// resources cannot be authorized under the wrong parent" criterion, and
    /// the reason a pattern as wide as `/**` still cannot escape its
    /// binding's scope.
    #[tokio::test]
    async fn a_grant_never_reaches_a_sibling_of_the_resource_it_is_bound_to() {
        let read_all = policy_statement(
            "read-all",
            tellurion_core::PolicyEffect::Allow,
            &["reader"],
            &["/**"],
        );

        let (tenant_bound, _ctx, tenant_path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                tenant_scope_of("acme"),
            )],
            std::slice::from_ref(&read_all),
        );
        assert_eq!(
            get_with_bearer(&tenant_bound, TENANT_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            get_with_bearer(
                &tenant_bound,
                OTHER_TENANT_VIEW,
                Some(POLICY_OPERATOR_TOKEN)
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );

        let (catalog_bound, _ctx, catalog_path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                catalog_scope_of("acme", "cadastre"),
            )],
            &[read_all],
        );
        assert_eq!(
            get_with_bearer(&catalog_bound, CATALOG_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            get_with_bearer(
                &catalog_bound,
                SIBLING_CATALOG_VIEW,
                Some(POLICY_OPERATOR_TOKEN)
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );

        let _ = std::fs::remove_file(tenant_path);
        let _ = std::fs::remove_file(catalog_path);
    }

    /// Downward inheritance is transitive, and the positive control for
    /// every negative above: a platform binding really does reach a
    /// collection three levels beneath it, so the refusals elsewhere are
    /// the scope rule at work and not a policy set that grants nothing.
    #[tokio::test]
    async fn a_platform_grant_reaches_every_scope_beneath_it() {
        let (app, _ctx, path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                ControlScope::Platform,
            )],
            &[policy_statement(
                "read-all",
                tellurion_core::PolicyEffect::Allow,
                &["reader"],
                &["/**"],
            )],
        );
        for target in [
            PLATFORM_VIEW,
            TENANT_VIEW,
            CATALOG_VIEW,
            COLLECTION_VIEW,
            OTHER_TENANT_VIEW,
        ] {
            assert_eq!(
                get_with_bearer(&app, target, Some(POLICY_OPERATOR_TOKEN))
                    .await
                    .status(),
                StatusCode::OK,
                "{target}"
            );
        }
        let _ = std::fs::remove_file(path);
    }

    /// Default-deny within a governed path: a principal holding no binding
    /// at all — and an anonymous caller on a surface that was previously
    /// open to everyone — reach `403`, not the `200` the path used to give.
    /// Absence of an allow is a deny.
    #[tokio::test]
    async fn a_governed_path_denies_a_principal_that_no_statement_allows() {
        let (app, _ctx, path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                ControlScope::Platform,
            )],
            &[policy_statement(
                "platform-reads",
                tellurion_core::PolicyEffect::Allow,
                &["reader"],
                &["/config/effective"],
            )],
        );
        assert_eq!(
            get_with_bearer(&app, PLATFORM_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            get_with_bearer(&app, PLATFORM_VIEW, Some(POLICY_STRANGER_TOKEN))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get_with_bearer(&app, PLATFORM_VIEW, None).await.status(),
            StatusCode::FORBIDDEN
        );
        // And the sibling resource, which no pattern mentions, is untouched.
        assert_eq!(get(&app, "/config/profiles").await.status(), StatusCode::OK);
        let _ = std::fs::remove_file(path);
    }

    /// A refusal at catalog scope is byte-identical to the answer a catalog
    /// that does not exist already gives. Compared as status, headers that
    /// could differ, and body bytes — because a difference in any of them is
    /// the enumeration oracle this refusal shape exists to close.
    #[tokio::test]
    async fn a_catalog_denial_is_indistinguishable_from_a_catalog_that_does_not_exist() {
        let (app, _ctx, path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                catalog_scope_of("acme", "cadastre"),
            )],
            &[policy_statement(
                "read-all",
                tellurion_core::PolicyEffect::Allow,
                &["reader"],
                &["/**"],
            )],
        );

        let denied = get_with_bearer(&app, SIBLING_CATALOG_VIEW, Some(POLICY_OPERATOR_TOKEN)).await;
        let absent = get_with_bearer(
            &app,
            "/acme-public/config/catalogs/no-such-catalog/effective",
            Some(POLICY_OPERATOR_TOKEN),
        )
        .await;

        assert_eq!(denied.status(), absent.status());
        assert_eq!(
            denied.headers().get(header::CONTENT_TYPE),
            absent.headers().get(header::CONTENT_TYPE)
        );
        let denied_body = to_bytes(denied.into_body(), usize::MAX).await.unwrap();
        let absent_body = to_bytes(absent.into_body(), usize::MAX).await.unwrap();
        assert_eq!(denied_body, absent_body);
        let _ = std::fs::remove_file(path);
    }

    /// A refusal names the decision and nothing else: not the statement that
    /// produced it, not the role that failed to match, not the scope, not
    /// the path. The same reasoning `#208` applied to the `Allow` header —
    /// a response an unauthorised caller can read must not be derivable
    /// from that caller's grants, or probing it enumerates the policy
    /// document.
    #[tokio::test]
    async fn a_refusal_names_neither_the_statement_nor_the_role_nor_the_scope() {
        let (app, _ctx, path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "secret-role-name",
                tenant_scope_of("beta"),
            )],
            &[policy_statement(
                "secret-statement-id",
                tellurion_core::PolicyEffect::Allow,
                &["secret-role-name"],
                &["/**"],
            )],
        );
        let refused = get_with_bearer(&app, TENANT_VIEW, Some(POLICY_OPERATOR_TOKEN)).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(refused.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8_lossy(&body).to_string();
        for leak in [
            "secret-statement-id",
            "secret-role-name",
            "tenant/acme",
            "explicit",
            "no_matching_allow",
        ] {
            assert!(
                !body.contains(leak),
                "the refusal disclosed '{leak}': {body}"
            );
        }
        let _ = std::fs::remove_file(path);
    }

    /// The agreement property (`#215`), through the server. A resource
    /// reachable ONLY through an encoded separator — a catalog whose
    /// external id contains one — is decided by exactly the scope rule that
    /// decides every other rendering, because the policy layer decodes the
    /// path the same way axum does and then replaces the external id with
    /// the internal one before matching.
    ///
    /// The decisive half is the negative: the encoded separator does not
    /// smuggle a second segment past the binding's scope. A principal bound
    /// to `cadastre` cannot reach `with/slash` by any spelling of it.
    #[tokio::test]
    async fn an_encoded_separator_cannot_reach_outside_the_scope_it_is_bound_to() {
        let read_all = policy_statement(
            "read-all",
            tellurion_core::PolicyEffect::Allow,
            &["reader"],
            &["/**"],
        );
        let slashed = "/acme-public/config/catalogs/with%2Fslash/effective";

        // Bound to the slash-bearing catalog itself: it serves.
        let (bound, _ctx, bound_path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                catalog_scope_of("acme", "slashed"),
            )],
            std::slice::from_ref(&read_all),
        );
        assert_eq!(
            get_with_bearer(&bound, slashed, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::OK
        );
        // And its sibling still does not, so the wide `/**` pattern is still
        // bounded by the binding rather than by the path's spelling.
        assert_eq!(
            get_with_bearer(&bound, CATALOG_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        // Bound to a DIFFERENT catalog: the encoded separator buys nothing.
        let (elsewhere, _ctx, elsewhere_path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                catalog_scope_of("acme", "cadastre"),
            )],
            &[read_all],
        );
        assert_eq!(
            get_with_bearer(&elsewhere, slashed, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        let _ = std::fs::remove_file(bound_path);
        let _ = std::fs::remove_file(elsewhere_path);
    }

    /// Dot segments, double encoding and duplicate slashes reach no decision
    /// at all: none of them resolves to a resource, so each keeps the answer
    /// it already had rather than acquiring a new refusal — which is what
    /// keeps a governed deployment from changing an answer on a path nobody
    /// governed.
    #[tokio::test]
    async fn a_traversal_or_double_encoding_reaches_no_decision() {
        let (governed, _ctx, governed_path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                catalog_scope_of("acme", "cadastre"),
            )],
            &[policy_statement(
                "read-all",
                tellurion_core::PolicyEffect::Allow,
                &["reader"],
                &["/**"],
            )],
        );
        let (ungoverned, _ctx, ungoverned_path) = policy_app(&[], &[]);

        for probe in [
            "/acme-public/config/catalogs/%2e%2e/effective",
            "/acme-public/config/catalogs/cadastre%252Dpublic/effective",
            "/acme-public/config/catalogs/no-such-catalog/effective",
        ] {
            let with_policy = get_with_bearer(&governed, probe, Some(POLICY_OPERATOR_TOKEN)).await;
            let without_policy =
                get_with_bearer(&ungoverned, probe, Some(POLICY_OPERATOR_TOKEN)).await;
            assert_eq!(
                with_policy.status(),
                StatusCode::NOT_FOUND,
                "{probe} acquired a new answer under policy"
            );
            assert_eq!(with_policy.status(), without_policy.status(), "{probe}");
        }

        let _ = std::fs::remove_file(governed_path);
        let _ = std::fs::remove_file(ungoverned_path);
    }

    /// An external id that differs from the internal one gets the same
    /// decision as the internal one would — because the canonical path a
    /// pattern matches is built from internal ids. A pattern written
    /// against the external id matches nothing, which is the safe direction:
    /// an alias can never be the thing that grants.
    #[tokio::test]
    async fn an_alias_cannot_produce_a_different_decision_than_the_resource_it_names() {
        let (app, _ctx, path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                tenant_scope_of("acme"),
            )],
            &[
                // Written against the INTERNAL id: this is the one that
                // decides, for every external id the tenant answers on.
                policy_statement(
                    "internal-id-statement",
                    tellurion_core::PolicyEffect::Allow,
                    &["reader"],
                    &["/acme/config/**"],
                ),
                // Written against the EXTERNAL id: matches nothing, so it
                // can neither grant nor govern.
                policy_statement(
                    "external-id-statement",
                    tellurion_core::PolicyEffect::Deny,
                    &["reader"],
                    &["/acme-public/config/**"],
                ),
            ],
        );
        // The deny is written against the alias and therefore never takes
        // effect; the allow is written against the internal id and does.
        assert_eq!(
            get_with_bearer(&app, TENANT_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::OK
        );
        // The catalog beneath it, reached under ITS alias, is decided by the
        // same internal-id statement.
        assert_eq!(
            get_with_bearer(&app, CATALOG_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::OK
        );
        let _ = std::fs::remove_file(path);
    }

    /// A statement carrying a condition of a kind this build does not
    /// implement can deny but can never allow (`#215` rule R3d) — proved
    /// through the server, in both directions.
    #[tokio::test]
    async fn a_condition_this_build_cannot_evaluate_never_grants() {
        let mut conditional_allow = policy_statement(
            "conditional-allow",
            tellurion_core::PolicyEffect::Allow,
            &["reader"],
            &["/config/effective"],
        );
        conditional_allow.conditions = vec![tellurion_core::PolicyCondition {
            kind: "ip-range".to_string(),
            config: serde_json::json!({"cidr": "10.0.0.0/8"}),
        }];
        let (app, _ctx, path) = policy_app(
            &[policy_binding(
                "operator-principal",
                "reader",
                ControlScope::Platform,
            )],
            &[conditional_allow],
        );
        assert_eq!(
            get_with_bearer(&app, PLATFORM_VIEW, Some(POLICY_OPERATOR_TOKEN))
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let _ = std::fs::remove_file(path);
    }

    /// `#215`'s audit criterion, through a real mutation: principal,
    /// effective scope, decision context and revision.
    #[tokio::test]
    async fn an_administrative_mutation_records_its_scope_and_decision() {
        let (app, ctx, path) = policy_app(
            &[policy_binding(
                "sysadmin-principal",
                "sysadmin",
                ControlScope::Platform,
            )],
            &[policy_statement(
                "platform-mutation",
                tellurion_core::PolicyEffect::Allow,
                &["sysadmin"],
                &["/config"],
            )],
        );
        let current = get_with_bearer(&app, "/config", Some(POLICY_SYSADMIN_TOKEN)).await;
        assert_eq!(current.status(), StatusCode::OK);
        let version = current_version_header(&current);
        let mut document = json_body(current).await;
        document["settings"] = serde_json::json!({ "cache_ttl_s": 77 });

        let written = put_config_request(
            &app,
            "/config",
            Some(POLICY_SYSADMIN_TOKEN),
            Some(&version),
            &document,
        )
        .await;
        assert_eq!(written.status(), StatusCode::OK);

        let record = ctx.audit_log.recent().remove(0);
        assert_eq!(record.principal, "sysadmin-principal");
        assert_eq!(record.effective_scope, "platform");
        assert!(
            record.decision.contains("explicit_allow")
                && record.decision.contains("platform-mutation")
                && record.decision.contains("sysadmin"),
            "the audit decision context is not the one the checkpoint reached: {}",
            record.decision
        );
        assert_eq!(record.expected_version, version);
        assert!(!record.new_version.is_empty());
        let _ = std::fs::remove_file(path);
    }

    /// The same mutation on a deployment that declared no statements records
    /// `not_engaged` rather than inventing a policy that does not exist.
    #[tokio::test]
    async fn a_mutation_on_an_ungoverned_deployment_records_that_it_was_not_engaged() {
        let (app, ctx, path) = policy_app(&[], &[]);
        let current = get_with_bearer(&app, "/config", Some(POLICY_SYSADMIN_TOKEN)).await;
        let version = current_version_header(&current);
        let mut document = json_body(current).await;
        document["settings"] = serde_json::json!({ "cache_ttl_s": 88 });
        assert_eq!(
            put_config_request(
                &app,
                "/config",
                Some(POLICY_SYSADMIN_TOKEN),
                Some(&version),
                &document
            )
            .await
            .status(),
            StatusCode::OK
        );
        let record = ctx.audit_log.recent().remove(0);
        assert_eq!(record.effective_scope, "platform");
        assert_eq!(record.decision, "not_engaged");
        let _ = std::fs::remove_file(path);
    }
}
