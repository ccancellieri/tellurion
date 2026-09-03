//! Asset handlers (assets-and-object-storage proposal, first slice: `core`,
//! `managed-storage`, `direct-upload`, `checksum`, and `object-store-
//! profile: fs`, collection- and item-level, on the primary database-backed
//! driver):
//!
//! ```text
//! GET/PUT/DELETE  /collections/{cid}/assets/{key}
//! GET/PUT/DELETE  /collections/{cid}/items/{fid}/assets/{key}
//! GET/PUT         /collections/{cid}/assets/{key}/data
//! GET/PUT         /collections/{cid}/items/{fid}/assets/{key}/data
//! ```
//!
//! ## Resumable upload (`resumable-upload` conformance class, `fs`- or
//! `s3`-profile object stores — see `tellurion_core::objectstore::
//! ResumableUploadStore`'s own doc for how each profile backs it: `fs`
//! appends to a real file, `s3` drives a real multipart upload)
//!
//! ```text
//! POST            /collections/{cid}/assets/{key}/data/uploads
//! GET             /collections/{cid}/assets/{key}/data/uploads
//! PATCH           /collections/{cid}/assets/{key}/data/uploads
//! DELETE          /collections/{cid}/assets/{key}/data/uploads
//! POST            /collections/{cid}/assets/{key}/data/uploads/complete
//! ```
//!
//! (and the identical set under `.../items/{fid}/assets/{key}/data/uploads`).
//! A subresource of a pending managed asset's own `.../data` lane, the third
//! upload transport alongside direct-upload and presigned-upload: `POST
//! .../uploads` creates the upload resource; `GET .../uploads` probes the
//! accumulated offset (HEAD-style — axum serves a literal `HEAD` against
//! this same handler automatically, carrying the same `Upload-Offset`
//! header this `GET` sets, with no body); `PATCH .../uploads` appends a
//! chunk at the offset named by its own `Upload-Offset` request header,
//! refusing (named `409`) an offset that doesn't match what has actually
//! accumulated; `DELETE .../uploads` abandons an incomplete upload,
//! idempotently; `POST .../uploads/complete` pulls the accumulated bytes
//! back out and hands them to the exact same digest/cap verification the
//! direct-upload lane uses (`tellurion_core::complete_resumable_upload`
//! delegates into `complete_upload` unchanged), flipping pending ->
//! available/failed by name. Every verb here sits on `PolicyLane::Write` —
//! even the offset probe, since it introspects a write-in-progress, not
//! published data.
//!
//! ## Download redirect (`download-redirect` conformance class, `s3`-profile
//! object stores only)
//!
//! No new route: `get_asset_data` itself answers a `307 Temporary Redirect`
//! to a presigned `GET` URL when the resolved object store has the
//! presigned-URL capability, and proxies bytes unchanged otherwise — see
//! that handler's own doc for the status-code choice and the fs/s3 split.
//!
//! ## Reconcile (read-only report, `#93`)
//!
//! ```text
//! GET  /collections/{cid}/assets/reconcile
//! ```
//!
//! Collection-level only — a walk of the whole collection's own asset
//! records (both collection- and item-level, `tellurion_core::AssetRecordStore::
//! list`) against its object store's managed namespace
//! (`tellurion_core::ListableObjectStore::list_all`), reporting drift both
//! ways (`tellurion_core::reconcile`'s own doc). Read-only: no repair
//! action, no deletion, no state flip lives here or in the domain function
//! it calls. `PolicyLane::Write`, not `Stac` — it introspects write-side
//! state (pending/failed records, in-progress upload staging files), the
//! same lane `get_upload_offset`'s own doc already reasons about for the
//! identical "read, but of write-in-progress state" shape. Refuses by name
//! (`CapabilityUnsupported("managed-storage")`) when this collection has no
//! `object_store` at all, and again (`"listable-storage"`) when the
//! resolved store has no listing capability — both shipped profiles do, so
//! this second refusal is currently unreachable in practice, kept for a
//! future profile that genuinely cannot list
//! (`tellurion_core::ObjectStore::as_listable`'s own doc).
//!
//! Metadata and bytes live at separate URLs, per the proposal's own
//! wire-contract rule. One handler per verb serves BOTH collection- and
//! item-level routes (`Path<HashMap<String, String>>` captures whichever
//! named segments a given route declares; `params.get("fid")` is `Some`
//! only on the item-level mount) — there is exactly one asset domain, keyed
//! by an optional item id, not two parallel handler sets.
//!
//! Every handler resolves storage through `Router::resolve_assets` (the
//! `AssetRecordStore` capability) exactly the way `write_handlers.rs`
//! resolves `resolve_write` — a collection whose anchor driver isn't
//! `AssetRecordStore`-capable, or whose `"<table>_assets"` table was never
//! provisioned, refuses by name (`CapabilityUnsupported`/`Config`) rather
//! than 500ing. This module deliberately does not call `handlers.rs`'s own
//! `resolve_tenant_catalog`/`authorize_lane`/`extract_credential` — they are
//! private to that module — so the small handful this file needs are
//! reimplemented locally, the identical arrangement
//! `tellurion-features::write_handlers`'s own module doc documents and
//! justifies for the same reason.
//!
//! ## Registration wire contract (`PUT .../assets/{key}`)
//!
//! The request body is a STAC Asset Object (`href`, `type`, `title`,
//! `description`, `roles`), plus the STAC `file` extension's `file:size`
//! for a managed asset's declared byte length. Managed vs. remote is
//! discriminated by whether the request carries an RFC 9530 `Repr-Digest`
//! header (`asset::parse_repr_digest`):
//!
//! - **Present** → managed registration. `href` MUST be absent (there are
//!   no bytes yet — the server derives the eventual `.../data` href, never
//!   the client), `file:size` is required, `type` is checked against this
//!   collection's asset media-type allow-list, and the declared size
//!   against its cap — both named refusals (415/413) before any storage
//!   I/O, per `asset::register_managed`.
//! - **Absent** → remote registration. `href` is required (STAC's own
//!   requirement); born available, no byte lifecycle.
//!
//! Idempotent replay vs. a genuine conflict is `asset::register_managed`/
//! `register_remote`'s own concern — see that module's doc.
//!
//! ## Read-surface unification with declared assets
//!
//! A collection-level `GET .../assets/{key}` checks `stac.assets` (`#36`
//! slice 1, operator-declared config assets) FIRST: a declared asset is, in
//! this proposal's terms, a remote asset that happens to live in config —
//! same read surface, always `available`, no byte lifecycle. A declared key
//! is read-only through this API (`PUT`/`DELETE` against it refuse with a
//! named `Conflict`) — config is its source of truth, this API is not a
//! second way to edit it. Declared assets are a collection-level-only
//! config concept (`config::StacConf::assets` lives on the collection, not
//! per-item), so this check never runs for an item-level request.
//!
//! ## What this slice does not do
//!
//! `PATCH` is deferred — a caller that wants to change an already-
//! registered key's declaration deletes it first (`asset.rs`'s own doc).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use tellurion_core::policy::{self, PolicyDecision, ResourceContext};
use tellurion_core::{
    abandon_resumable_upload, append_resumable_upload, complete_resumable_upload, complete_upload,
    create_resumable_upload, delete_asset as domain_delete_asset, finalize_presigned_upload,
    parse_repr_digest, presign_upload, reconcile as domain_reconcile, register_managed,
    register_remote, resumable_upload_offset, AppContext, AssetDecl, AssetKind, AssetPolicy,
    AssetRecord, AssetState, ContextState, Credential, Error as CoreError, ObjectKey, PolicyLane,
    RateCharge, RateCounter, RateVerdict, ReconcileReport, RegisterManagedRequest,
    RegisterRemoteRequest,
};

use crate::handlers::{DEFAULT_CATALOG, DEFAULT_TENANT};
use crate::problem::ApiError;

/// `Repr-Digest` (RFC 9530) — HTTP header names are matched case-
/// insensitively by `HeaderMap`, so this literal covers every casing a
/// client sends.
const REPR_DIGEST_HEADER: &str = "repr-digest";

// -- request/response shapes -------------------------------------------

/// The wire request body for `PUT .../assets/{key}`: the STAC Asset Object
/// plus the `file` extension's `file:size` (declared byte length, managed
/// registration only) — see this module's own doc for the full
/// registration contract.
#[derive(Debug, Deserialize)]
struct AssetObjectInput {
    #[serde(default)]
    href: Option<String>,
    #[serde(rename = "type", default)]
    media_type: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(rename = "file:size", default)]
    declared_size: Option<u64>,
}

/// The wire response body: the STAC Asset Object plus this extension's own
/// `status`/`status_detail` — the same "JSON objects are extensible, add a
/// bare custom field rather than invent a namespace" precedent
/// `model::StacAsset::templated` already sets in this crate.
#[derive(Debug, Serialize)]
struct AssetObjectResponse {
    href: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    roles: Vec<String>,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_detail: Option<String>,
}

fn state_label(state: AssetState) -> &'static str {
    match state {
        AssetState::Pending => "pending",
        AssetState::Available => "available",
        AssetState::Failed => "failed",
    }
}

fn asset_record_to_response(record: &AssetRecord, data_href: &str) -> AssetObjectResponse {
    let href = match record.kind {
        AssetKind::Managed => data_href.to_string(),
        AssetKind::Remote => record.href.clone().unwrap_or_default(),
    };
    AssetObjectResponse {
        href,
        media_type: record.media_type.clone(),
        title: record.title.clone(),
        description: record.description.clone(),
        roles: record.roles.clone(),
        status: state_label(record.state),
        status_detail: record.failure_reason.clone(),
    }
}

fn declared_asset_to_response(decl: &AssetDecl) -> AssetObjectResponse {
    AssetObjectResponse {
        href: decl.href.clone(),
        media_type: decl.media_type.clone(),
        title: decl.title.clone(),
        description: None,
        roles: decl.roles.clone(),
        status: "available",
        status_detail: None,
    }
}

fn json_response<T: Serialize>(status: StatusCode, body: T) -> Response {
    (status, Json(body)).into_response()
}

/// The wire response body for `.../assets/{key}/data/presign` (both verbs):
/// the negotiated transfer target and how long it stays valid — never the
/// underlying store's credentials, only the already-signed URL.
#[derive(Debug, Serialize)]
struct PresignedTransferResponse {
    href: String,
    method: &'static str,
    expires_in_s: u64,
}

/// The `presigned-upload` conformance class's own capability refusal — the
/// same shape every other optional capability in this workspace refuses
/// with (`Router::resolve_object_store`'s own `"managed-storage"`), raised
/// here rather than inside `tellurion_core` because only the HTTP layer
/// resolves an `Arc<dyn ObjectStore>` down to its borrowed presign
/// capability (`ObjectStore::as_presigned`'s own doc): the `fs` profile's
/// refusal-by-name for this class.
fn presign_capability_unsupported(collection_id: &str) -> ApiError {
    ApiError::from(CoreError::CapabilityUnsupported {
        collection: collection_id.to_string(),
        capability: "presigned-upload".to_string(),
    })
}

/// The `resumable-upload` conformance class's own capability refusal —
/// raised here rather than inside `tellurion_core` for the identical reason
/// [`presign_capability_unsupported`] is: only the HTTP layer resolves an
/// `Arc<dyn ObjectStore>` down to its borrowed resumable-upload capability
/// (`ObjectStore::as_resumable`'s own doc). Currently unreachable in
/// practice: both shipped profiles implement
/// `tellurion_core::objectstore::ResumableUploadStore` (`fs` since the
/// third slice, `s3` via real multipart-upload signing as of this one) —
/// kept, the same way [`listable_capability_unsupported`] already is, for a
/// future profile that genuinely cannot resume.
fn resumable_capability_unsupported(collection_id: &str) -> ApiError {
    ApiError::from(CoreError::CapabilityUnsupported {
        collection: collection_id.to_string(),
        capability: "resumable-upload".to_string(),
    })
}

/// The reconcile surface's own listing-capability refusal — raised here for
/// the identical reason [`presign_capability_unsupported`]/
/// [`resumable_capability_unsupported`] are (only the HTTP layer resolves
/// the borrowed capability). Currently unreachable in practice: both
/// shipped profiles implement `tellurion_core::ListableObjectStore`
/// (`objectstore.rs`'s own doc) — kept for a future profile that genuinely
/// cannot list.
fn listable_capability_unsupported(collection_id: &str) -> ApiError {
    ApiError::from(CoreError::CapabilityUnsupported {
        collection: collection_id.to_string(),
        capability: "listable-storage".to_string(),
    })
}

/// `Upload-Offset` — the IETF resumable-upload draft's own header name,
/// reused here on both requests (`PATCH .../uploads`, naming where a chunk
/// starts) and responses (every resumable-upload verb, naming the
/// accumulated offset after the call) — `HeaderMap`'s own case-insensitive
/// matching covers whatever casing a client sends.
const UPLOAD_OFFSET_HEADER: &str = "upload-offset";

/// Parses the request's own `Upload-Offset` header — required on `PATCH
/// .../uploads` (`#93` resumable-upload, "the offset check is the guard").
/// Missing or malformed is a plain `400`, distinct from the `409`-family
/// refusals an offset that parses but doesn't match reality gets
/// (`asset::append_resumable_upload`'s own doc).
fn require_upload_offset_header(headers: &HeaderMap) -> Result<u64, ApiError> {
    let value = headers.get(UPLOAD_OFFSET_HEADER).ok_or_else(|| {
        ApiError::from(CoreError::Invalid(
            "an append requires an 'Upload-Offset' header naming the offset this chunk starts at"
                .to_string(),
        ))
    })?;
    let value = value.to_str().map_err(|_| {
        ApiError::from(CoreError::Invalid(
            "Upload-Offset header is not valid UTF-8".to_string(),
        ))
    })?;
    value.parse::<u64>().map_err(|_| {
        ApiError::from(CoreError::Invalid(format!(
            "'{value}' is not a valid Upload-Offset"
        )))
    })
}

/// The wire response body every resumable-upload verb below returns —
/// mirrored onto an `Upload-Offset` response header too, so a client that
/// only reads headers (the literal IETF draft shape) and a client that only
/// reads JSON both see the same number.
#[derive(Debug, Serialize)]
struct UploadOffsetResponse {
    offset: u64,
}

fn upload_offset_response(status: StatusCode, offset: u64) -> Response {
    let mut response = json_response(status, UploadOffsetResponse { offset });
    if let Ok(value) = HeaderValue::from_str(&offset.to_string()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(UPLOAD_OFFSET_HEADER), value);
    }
    response
}

// -- shared request-scope resolution (mirrors write_handlers.rs) --------

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

fn require_param(params: &HashMap<String, String>, name: &str) -> Result<String, ApiError> {
    params
        .get(name)
        .cloned()
        .ok_or(CoreError::NotFound)
        .map_err(ApiError::from)
}

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

/// The `#34`/`#68` policy checkpoint, generalized over the lane: `Stac` for
/// a read (`GET`), `Write` for anything that mutates (`PUT`/`DELETE`) — "Asset
/// writes go through the write policy lane like every other write."
/// `lane_supports_filter` is always `false`: an asset has no row-level ABAC
/// filter surface, the same reasoning `write_handlers.rs`'s own
/// `authorize_write_lane` gives.
async fn authorize_asset_lane(
    state: &ContextState,
    rate_counter: &dyn RateCounter,
    headers: &HeaderMap,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
    lane: PolicyLane,
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
        lane,
        visibility: &visibility,
    };
    match policy::authorize_resource(&state.config, &resource, &subject, false)? {
        PolicyDecision::Allow { .. } => {}
        PolicyDecision::Deny => return Err(crate::problem::policy_denied(&credential)),
    }
    // `#188`: an asset operation is one served request — there is no
    // listing variant of this checkpoint — so it always charges.
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

/// One request's resolved scope: internal ids for `Router`, external ids
/// (echoed straight back from the path, never an internal id) for building
/// this response's own `.../data` href. `fid` is `Some` only on an
/// item-level route.
struct AssetScope {
    tenant_id: String,
    catalog_id: String,
    collection_id: String,
    tenant_ext: String,
    catalog_ext: String,
    cid: String,
    fid: Option<String>,
}

async fn resolve_scope(
    ctx: &AppContext,
    params: &HashMap<String, String>,
) -> Result<AssetScope, ApiError> {
    let tenant_ext = tenant_of(params);
    let catalog_ext = catalog_of(params);
    let (tenant_id, catalog_id) = resolve_tenant_catalog(ctx, params).await?;
    let cid = require_param(params, "cid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    let fid = params.get("fid").cloned();
    Ok(AssetScope {
        tenant_id,
        catalog_id,
        collection_id,
        tenant_ext,
        catalog_ext,
        cid,
        fid,
    })
}

/// `.../assets/{key}/data`'s own href — this scope's arguments handed to
/// `assets::asset_data_href`, which owns the one definition of that URL
/// shape (`#221`). Sharing it is what makes the href a client reads off an
/// Item's `assets` map byte-identical to the one this API returns for the
/// same record, rather than two hand-built strings that could drift apart.
fn data_href(state: &ContextState, scope: &AssetScope, key: &str) -> String {
    crate::assets::asset_data_href(
        &state.config.server,
        &scope.tenant_ext,
        &scope.catalog_ext,
        &scope.cid,
        scope.fid.as_deref(),
        key,
    )
}

/// Collection-level-only: `stac.assets` (`#36` slice 1) has no per-item
/// concept — see this module's own doc.
fn declared_asset_for(state: &ContextState, collection_id: &str, key: &str) -> Option<AssetDecl> {
    state
        .router
        .effective_settings(collection_id)
        .and_then(|settings| settings.stac.as_ref())
        .and_then(|stac| stac.assets.get(key).cloned())
}

/// Reads a request body capped at `limit` bytes, refusing before it is
/// fully buffered once the streamed length runs past it (`#91`) — the
/// identical helper `write_handlers.rs` defines for the same reason
/// (duplicated per module, not shared; see this module's own doc).
async fn read_capped_body(body: axum::body::Body, limit: u64) -> Result<bytes::Bytes, ApiError> {
    let capped = usize::try_from(limit).unwrap_or(usize::MAX);
    axum::body::to_bytes(body, capped).await.map_err(|err| {
        let exceeded_limit = std::error::Error::source(&err)
            .is_some_and(|source| source.is::<http_body_util::LengthLimitError>());
        if exceeded_limit {
            ApiError::from(CoreError::PayloadTooLarge { limit })
        } else {
            ApiError::from(CoreError::Invalid(format!(
                "failed to read request body: {err}"
            )))
        }
    })
}

// -- handlers -------------------------------------------------------------

/// `GET .../assets/{key}` (both levels): declared asset first (collection
/// level only), then the database-backed record.
pub async fn get_asset(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Stac,
    )
    .await?;
    let key = require_param(&params, "key")?;

    if scope.fid.is_none() {
        if let Some(declared) = declared_asset_for(&state, &scope.collection_id, &key) {
            return Ok(json_response(
                StatusCode::OK,
                declared_asset_to_response(&declared),
            ));
        }
    }

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let record = store
        .get(&decl, scope.fid.as_deref(), &key)
        .await?
        .ok_or(CoreError::NotFound)?;
    let href = data_href(&state, &scope, &key);
    Ok(json_response(
        StatusCode::OK,
        asset_record_to_response(&record, &href),
    ))
}

/// `PUT .../assets/{key}` (both levels): register — see this module's own
/// doc for the full managed-vs-remote wire contract.
pub async fn put_asset(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Write,
    )
    .await?;
    let key = require_param(&params, "key")?;

    if scope.fid.is_none() && declared_asset_for(&state, &scope.collection_id, &key).is_some() {
        return Err(ApiError::from(CoreError::Conflict(format!(
            "asset key '{key}' is a config-declared asset and is read-only through this API"
        ))));
    }

    let settings = state
        .router
        .effective_settings(&scope.collection_id)
        .cloned()
        .unwrap_or_default();
    let body_bytes = read_capped_body(body, settings.max_request_body_bytes).await?;
    let input: AssetObjectInput = serde_json::from_slice(&body_bytes).map_err(|err| {
        ApiError::from(CoreError::Invalid(format!(
            "request body is not a valid Asset Object: {err}"
        )))
    })?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let policy = AssetPolicy {
        max_asset_bytes: settings.max_asset_bytes,
        allowed_media_types: if settings.asset_media_types.is_empty() {
            None
        } else {
            Some(&settings.asset_media_types)
        },
    };

    let digest_header = headers
        .get(REPR_DIGEST_HEADER)
        .map(|value| {
            value.to_str().map_err(|_| {
                ApiError::from(CoreError::Invalid(
                    "Repr-Digest header is not valid UTF-8".to_string(),
                ))
            })
        })
        .transpose()?;

    let record = if let Some(digest_header) = digest_header {
        if input.href.is_some() {
            return Err(ApiError::from(CoreError::Invalid(
                "a managed asset registration (Repr-Digest present) must not declare 'href'"
                    .to_string(),
            )));
        }
        let declared_size = input.declared_size.ok_or_else(|| {
            ApiError::from(CoreError::Invalid(
                "a managed asset registration requires 'file:size'".to_string(),
            ))
        })?;
        let digest = parse_repr_digest(digest_header)?;
        register_managed(
            &*store,
            &policy,
            &decl,
            scope.fid.as_deref(),
            &key,
            RegisterManagedRequest {
                media_type: input.media_type,
                title: input.title,
                description: input.description,
                roles: input.roles,
                declared_size,
                digest,
            },
        )
        .await?
    } else {
        let href = input.href.ok_or_else(|| {
            ApiError::from(CoreError::Invalid(
                "a remote asset registration requires 'href'".to_string(),
            ))
        })?;
        register_remote(
            &*store,
            &policy,
            &decl,
            scope.fid.as_deref(),
            &key,
            RegisterRemoteRequest {
                href,
                media_type: input.media_type,
                title: input.title,
                description: input.description,
                roles: input.roles,
            },
        )
        .await?
    };

    let href = data_href(&state, &scope, &key);
    Ok(json_response(
        StatusCode::OK,
        asset_record_to_response(&record, &href),
    ))
}

/// `DELETE .../assets/{key}` (both levels): remote deletes the record only;
/// managed deletes the record and the object (`asset::delete_asset`'s own
/// doc).
pub async fn delete_asset(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Write,
    )
    .await?;
    let key = require_param(&params, "key")?;

    if scope.fid.is_none() && declared_asset_for(&state, &scope.collection_id, &key).is_some() {
        return Err(ApiError::from(CoreError::Conflict(format!(
            "asset key '{key}' is a config-declared asset and is read-only through this API"
        ))));
    }

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    // Resolved lazily and passed through as `Option` — a remote-only delete
    // must succeed even when this collection declares no `object_store` at
    // all (`asset::delete_asset`'s own doc).
    let objects = state
        .router
        .resolve_object_store(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .ok();
    let deleted = domain_delete_asset(
        &*store,
        objects.as_deref(),
        &decl,
        scope.fid.as_deref(),
        &key,
    )
    .await?;
    if deleted.is_none() {
        return Err(ApiError::from(CoreError::NotFound));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `PUT .../assets/{key}/data` (both levels, managed only): the direct-
/// upload transfer. Capped at the record's own declared size (`asset::
/// complete_upload`'s own doc), the existing streamed-length body-cap
/// machinery.
pub async fn put_asset_data(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Write,
    )
    .await?;
    let key = require_param(&params, "key")?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let record = store
        .get(&decl, scope.fid.as_deref(), &key)
        .await?
        .ok_or(CoreError::NotFound)?;
    if record.kind != AssetKind::Managed {
        return Err(ApiError::from(CoreError::NotFound));
    }
    let cap = record.declared_size.unwrap_or(0);
    let body_bytes = read_capped_body(body, cap).await?;

    let objects = state.router.resolve_object_store(
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
    )?;
    let updated = complete_upload(
        &*store,
        &*objects,
        &decl,
        scope.fid.as_deref(),
        &key,
        body_bytes,
    )
    .await?;

    let href = data_href(&state, &scope, &key);
    Ok(json_response(
        StatusCode::OK,
        asset_record_to_response(&updated, &href),
    ))
}

/// `GET .../assets/{key}/data` (both levels, managed only): on an object
/// store with the presigned-URL capability (the `s3` profile), answers the
/// `download-redirect` conformance class — a `307 Temporary Redirect` to a
/// time-limited presigned `GET` URL, never proxying bytes through this
/// server. `307`, not `302`: RFC 9110 defines `307` as both temporary
/// (unlike `301`/`308`, a client must not cache this as the resource's
/// permanent location — a presigned URL's own expiry makes that doubly
/// true) and method-preserving by requirement rather than by long-standing
/// convention (`302`'s own method-on-redirect behavior is still governed by
/// "for historical reasons" language in the spec, and real HTTP/1.0-era
/// clients really did rewrite it) — the deliberate, unambiguous choice for
/// a route that is always `GET` and whose target is genuinely temporary,
/// never a stable proxy URL this server itself serves.
///
/// A store with no presigned-URL capability (the `fs` profile —
/// `ObjectStore::as_presigned`'s own doc: `fs` has no URL space of its own
/// to redirect to) proxies the bytes directly instead, byte-for-byte the
/// same as before this class existed — the honest behavior for a profile
/// with nothing else to redirect to, never a degraded fallback.
///
/// Either way, a `pending`/`failed` asset still `404`s before ever reaching
/// the object store — there is nothing at the target yet (or ever again) to
/// redirect to or proxy.
pub async fn get_asset_data(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Stac,
    )
    .await?;
    let key = require_param(&params, "key")?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let record = store
        .get(&decl, scope.fid.as_deref(), &key)
        .await?
        .ok_or(CoreError::NotFound)?;
    if record.kind != AssetKind::Managed || record.state != AssetState::Available {
        return Err(ApiError::from(CoreError::NotFound));
    }

    let objects = state.router.resolve_object_store(
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
    )?;

    if let Some(presigned) = objects.as_presigned() {
        let expires_in = presigned.default_expiry();
        let href = presigned
            .presign_get(ObjectKey::new(record.id), expires_in, SystemTime::now())
            .map_err(|err| ApiError::from(CoreError::Storage(Box::new(err))))?;
        let mut response = StatusCode::TEMPORARY_REDIRECT.into_response();
        if let Ok(value) = HeaderValue::from_str(&href) {
            response.headers_mut().insert(header::LOCATION, value);
        }
        return Ok(response);
    }

    let bytes = objects
        .get(ObjectKey::new(record.id))
        .await
        .map_err(|err| ApiError::from(CoreError::Storage(Box::new(err))))?
        .ok_or(CoreError::NotFound)?;

    let mut response = (StatusCode::OK, bytes).into_response();
    let media_type = record
        .media_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    if let Ok(value) = HeaderValue::from_str(media_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    Ok(response)
}

/// `PUT .../assets/{key}/data/presign` (both levels, managed only, `s3`-
/// profile object stores only): the presigned-upload negotiation step —
/// mints a time-limited signed `PUT` URL the client transfers bytes to
/// directly, the alternative to `put_asset_data`'s own direct-upload byte
/// lane. `object_store_id`s resolving to the `fs` profile refuse this by
/// name (`presign_capability_unsupported`) — `fs` has no URL space of its
/// own to mint a signed URL against (`ObjectStore::as_presigned`'s own
/// doc), the `presigned-upload` conformance class's own refusal-by-name
/// contract.
pub async fn put_asset_presign(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Write,
    )
    .await?;
    let key = require_param(&params, "key")?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let objects = state.router.resolve_object_store(
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
    )?;
    let presigned = objects
        .as_presigned()
        .ok_or_else(|| presign_capability_unsupported(&scope.collection_id))?;

    let href = presign_upload(
        &*store,
        presigned,
        &decl,
        scope.fid.as_deref(),
        &key,
        SystemTime::now(),
    )
    .await?;
    Ok(json_response(
        StatusCode::OK,
        PresignedTransferResponse {
            href,
            method: "PUT",
            expires_in_s: presigned.default_expiry().as_secs(),
        },
    ))
}

/// `GET .../assets/{key}/data/presign` (both levels, managed only, `s3`-
/// profile object stores only): the read-side companion to
/// `put_asset_presign` — mints a time-limited signed `GET` URL for an
/// already-`available` managed asset, alongside (never in place of) the
/// unchanged direct-proxy `get_asset_data`. Refuses the same way for the
/// `fs` profile, and `404`s (never presigns) a `pending`/`failed` asset —
/// there is nothing at the target yet to hand a download URL for.
pub async fn get_asset_presign(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Stac,
    )
    .await?;
    let key = require_param(&params, "key")?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let record = store
        .get(&decl, scope.fid.as_deref(), &key)
        .await?
        .ok_or(CoreError::NotFound)?;
    if record.kind != AssetKind::Managed || record.state != AssetState::Available {
        return Err(ApiError::from(CoreError::NotFound));
    }

    let objects = state.router.resolve_object_store(
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
    )?;
    let presigned = objects
        .as_presigned()
        .ok_or_else(|| presign_capability_unsupported(&scope.collection_id))?;
    let expires_in = presigned.default_expiry();
    let href = presigned
        .presign_get(ObjectKey::new(record.id), expires_in, SystemTime::now())
        .map_err(|err| ApiError::from(CoreError::Storage(Box::new(err))))?;
    Ok(json_response(
        StatusCode::OK,
        PresignedTransferResponse {
            href,
            method: "GET",
            expires_in_s: expires_in.as_secs(),
        },
    ))
}

/// `POST .../assets/{key}/finalize` (both levels, managed only, `s3`-
/// profile object stores only): the presigned-upload commit step — the
/// server never saw the bytes, so this verifies via the store's own `HEAD`
/// (`tellurion_core::finalize_presigned_upload`'s own doc) and flips
/// `pending` to `available`/`failed` by name. No request body.
pub async fn post_asset_finalize(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Write,
    )
    .await?;
    let key = require_param(&params, "key")?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let objects = state.router.resolve_object_store(
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
    )?;
    let presigned = objects
        .as_presigned()
        .ok_or_else(|| presign_capability_unsupported(&scope.collection_id))?;

    let updated =
        finalize_presigned_upload(&*store, presigned, &decl, scope.fid.as_deref(), &key).await?;

    let href = data_href(&state, &scope, &key);
    Ok(json_response(
        StatusCode::OK,
        asset_record_to_response(&updated, &href),
    ))
}

// -- resumable-upload conformance class ---------------------------------

/// `POST .../assets/{key}/data/uploads` (both levels, managed only, `fs`-
/// or `s3`-profile object stores): creates the upload resource — see this
/// module's own doc for the full resumable-upload wire contract.
pub async fn post_create_upload(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Write,
    )
    .await?;
    let key = require_param(&params, "key")?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let objects = state.router.resolve_object_store(
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
    )?;
    let resumable = objects
        .as_resumable()
        .ok_or_else(|| resumable_capability_unsupported(&scope.collection_id))?;

    create_resumable_upload(&*store, resumable, &decl, scope.fid.as_deref(), &key).await?;
    Ok(upload_offset_response(StatusCode::CREATED, 0))
}

/// `GET .../assets/{key}/data/uploads` (both levels, managed only): probes
/// the accumulated offset — HEAD-style; axum serves a literal `HEAD` against
/// this same handler automatically (dropping the body, keeping the
/// `Upload-Offset` header this sets). `404` when no upload is in progress
/// for this key.
pub async fn get_upload_offset(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    // `PolicyLane::Write`, not `Stac`: an upload-in-progress is a write
    // artifact this collection's write policy gates, never published data
    // (this module's own doc).
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Write,
    )
    .await?;
    let key = require_param(&params, "key")?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let objects = state.router.resolve_object_store(
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
    )?;
    let resumable = objects
        .as_resumable()
        .ok_or_else(|| resumable_capability_unsupported(&scope.collection_id))?;

    let offset =
        resumable_upload_offset(&*store, resumable, &decl, scope.fid.as_deref(), &key).await?;
    Ok(upload_offset_response(StatusCode::OK, offset))
}

/// `PATCH .../assets/{key}/data/uploads` (both levels, managed only):
/// appends one chunk at the offset its own `Upload-Offset` request header
/// names. Capped at whatever room remains below the asset's own declared
/// size — the same streamed-length body-cap machinery `put_asset_data`
/// uses, sized to the remaining budget rather than the full declared size,
/// so an oversized chunk is refused (`413`) before it is ever fully
/// buffered, never after.
pub async fn patch_append_upload(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Write,
    )
    .await?;
    let key = require_param(&params, "key")?;
    let expected_offset = require_upload_offset_header(&headers)?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let record = store
        .get(&decl, scope.fid.as_deref(), &key)
        .await?
        .ok_or(CoreError::NotFound)?;
    if record.kind != AssetKind::Managed {
        return Err(ApiError::from(CoreError::NotFound));
    }
    let declared_size = record.declared_size.unwrap_or(0);
    let remaining_cap = declared_size.saturating_sub(expected_offset);
    let chunk = read_capped_body(body, remaining_cap).await?;

    let objects = state.router.resolve_object_store(
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
    )?;
    let resumable = objects
        .as_resumable()
        .ok_or_else(|| resumable_capability_unsupported(&scope.collection_id))?;

    let new_offset = append_resumable_upload(
        &*store,
        resumable,
        &decl,
        scope.fid.as_deref(),
        &key,
        expected_offset,
        chunk,
    )
    .await?;
    Ok(upload_offset_response(StatusCode::OK, new_offset))
}

/// `DELETE .../assets/{key}/data/uploads` (both levels, managed only):
/// abandons an incomplete upload — idempotent, `204` whether or not one was
/// actually in progress. The asset itself stays `pending`, untouched.
pub async fn delete_upload(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Write,
    )
    .await?;
    let key = require_param(&params, "key")?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let objects = state.router.resolve_object_store(
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
    )?;
    let resumable = objects
        .as_resumable()
        .ok_or_else(|| resumable_capability_unsupported(&scope.collection_id))?;

    abandon_resumable_upload(&*store, resumable, &decl, scope.fid.as_deref(), &key).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST .../assets/{key}/data/uploads/complete` (both levels, managed
/// only): the resumable-upload commit step — pulls the accumulated bytes
/// back out and hands them to `tellurion_core::complete_upload` unchanged
/// (`asset::complete_resumable_upload`'s own doc), flipping pending ->
/// available/failed by name exactly as the direct-upload lane does. No
/// request body.
pub async fn post_complete_upload(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Write,
    )
    .await?;
    let key = require_param(&params, "key")?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let objects = state.router.resolve_object_store(
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
    )?;
    let resumable = objects
        .as_resumable()
        .ok_or_else(|| resumable_capability_unsupported(&scope.collection_id))?;

    let updated =
        complete_resumable_upload(&*store, resumable, &decl, scope.fid.as_deref(), &key).await?;

    let href = data_href(&state, &scope, &key);
    Ok(json_response(
        StatusCode::OK,
        asset_record_to_response(&updated, &href),
    ))
}

// -- reconcile (read-only report) ---------------------------------------

/// One [`tellurion_core::BrokenAsset`] on the wire.
#[derive(Debug, Serialize)]
struct BrokenAssetResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    item_id: Option<String>,
    key: String,
    id: String,
}

/// One [`tellurion_core::OrphanedObject`] on the wire.
#[derive(Debug, Serialize)]
struct OrphanedObjectResponse {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    staging: bool,
}

#[derive(Debug, Serialize)]
struct ReconcileReportResponse {
    broken: Vec<BrokenAssetResponse>,
    orphaned: Vec<OrphanedObjectResponse>,
}

impl From<ReconcileReport> for ReconcileReportResponse {
    fn from(report: ReconcileReport) -> Self {
        Self {
            broken: report
                .broken
                .into_iter()
                .map(|entry| BrokenAssetResponse {
                    item_id: entry.item_id,
                    key: entry.key,
                    id: entry.id.to_string(),
                })
                .collect(),
            orphaned: report
                .orphaned
                .into_iter()
                .map(|entry| OrphanedObjectResponse {
                    name: entry.raw_name,
                    id: entry.id.map(|id| id.to_string()),
                    staging: entry.is_staging,
                })
                .collect(),
        }
    }
}

/// `GET .../assets/reconcile` (collection level only) — the reconcile
/// surface's own read-only drift report; see this module's own doc for the
/// full contract (`PolicyLane::Write`, the two capability refusals, and why
/// this walks the whole collection rather than taking an item id).
pub async fn get_reconcile_report(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let scope = resolve_scope(&ctx, &params).await?;
    let state = ctx.current();
    authorize_asset_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
        PolicyLane::Write,
    )
    .await?;

    let (decl, store) = state
        .router
        .resolve_assets(&scope.tenant_id, &scope.catalog_id, &scope.collection_id)
        .await?;
    let objects = state.router.resolve_object_store(
        &scope.tenant_id,
        &scope.catalog_id,
        &scope.collection_id,
    )?;
    let listable = objects
        .as_listable()
        .ok_or_else(|| listable_capability_unsupported(&scope.collection_id))?;

    let report = domain_reconcile(&*store, listable, &decl).await?;
    Ok(json_response(
        StatusCode::OK,
        ReconcileReportResponse::from(report),
    ))
}
