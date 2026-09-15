//! Item write handlers (`#25`, the transactional-outbox design's write
//! slice): `PUT /collections/{cid}/items/{fid}` replaces an item (creating
//! it if `fid` is new — an upsert, matching `WriteSink::apply`'s own
//! `MutationKind::Upsert` semantics exactly, and the same "PUT replaces"
//! contract this endpoint's id-is-caller-supplied shape implies),
//! `DELETE /collections/{cid}/items/{fid}` removes one, and
//! `POST /collections/{cid}/items` (`#88`) creates a new item with a
//! server-assigned id — create-only, never an upsert (`PUT` stays the
//! idempotent replace/create-by-caller-supplied-id endpoint; `POST` never
//! accepts an `{fid}` and never overwrites an existing item). All three
//! resolve storage through `Router::resolve_write` — the write counterpart
//! of every read handler's `resolve_features`/`resolve_tiles` in
//! `handlers.rs` — so a collection whose write lane isn't routed to a
//! `WriteSink`-capable driver refuses with the identical
//! `CapabilityUnsupported` 404 a read lane without its capability already
//! gives. `create_item` itself no longer inspects `CollectionDecl::id_type`
//! at all (`#87`, `#94`): every `id_type` value a driver can declare
//! (`Integer`, `Uuid`, `Text`) has a real create-path implementation on the
//! PostGIS driver now — `Text`'s is caller-supplied rather than server-
//! minted, but it's still the driver's own call, not this handler's — so
//! whether a `POST` is servable is entirely `WriteSink::create`'s own
//! capability/config-mismatch call to make, the same way `apply` already
//! decides `PUT`/`DELETE` servability with no handler-level id-type check at
//! all — see `IdType`'s own doc and `tellurion-postgis`'s
//! `validate_id_type_for_create`.
//!
//! Schema validation (`#44`) runs BEFORE `WriteSink::apply` is ever called:
//! a collection with a declared schema (`CollectionDecl::schema`) has its
//! inbound feature's `properties` checked against it
//! (`SchemaDecl::validate_feature_properties`), and a violation is rejected
//! with a 400 naming every offending property — no outbox obligation is
//! committed for a feature that fails this check. A collection with no
//! declared schema (`schema: None`, the default) skips this entirely and
//! accepts the feature as-is, exactly as issue `#44` specifies.
//!
//! `PUT`'s body is capped before it is ever fully buffered (`#91`): once the
//! collection is resolved and the caller cleared `authorize_write_lane`,
//! `read_capped_body` reads at most `settings.max_request_body_bytes` (the
//! same platform -> tenant -> catalog -> collection chain every other
//! whitelisted setting resolves through) and refuses a body that runs past
//! it with a named `413` — checked against the streamed length, not
//! buffer-then-measure. Placed after the auth checkpoint deliberately: a
//! cheap, header-only reject shouldn't have to wait behind reading an
//! attacker-sized body first, and a request destined for a `403` shouldn't
//! cost the server a large read either.
//!
//! `put_item`/`delete_item` both honor `If-Match`/`If-Unmodified-Since`
//! through `evaluate_write_preconditions` (OGC API Features — Part 4,
//! 20-002r1 draft, `#107`) — one read of the target's current state
//! (`FeatureSource::item`, `filter: None`: an existence/state check for a
//! write-authorized caller is not itself a filtered read, so the read
//! lane's own grant filter never applies here), then up to three checks in
//! RFC 7232 section 5's precedence order:
//!
//! 1. The narrow existence guard this crate already had (Requirement 12
//!    clause B, `/req/create-replace-delete/put-rid-exception`): since
//!    `put_item`'s `PUT` creates on a missing id (the upsert this module's
//!    own doc above describes), a caller sending `If-Match` is explicitly
//!    asking the server to refuse rather than silently create when the
//!    target doesn't yet exist — preserved byte-for-byte, and now applied
//!    to `delete_item` too (see `evaluate_write_preconditions`'s own doc
//!    for why a missing-resource `If-Match` refuses on either verb).
//! 2. `req/optimistic-locking-etags`: once a target exists, `If-Match`'s
//!    value is compared against a content-derived hash of its current
//!    state (`tellurion_core::locking::compute_feature_etag`), not merely
//!    its existence.
//! 3. `req/optimistic-locking-timestamps`, only when `If-Match` was never
//!    sent: `If-Unmodified-Since` compared against this collection's own
//!    declared `modified_column`, when one exists.
//!
//! `POST` (`create_item`) has no equivalent requirement class for either —
//! there is no existing target for a create to compare against.
//!
//! Those three checks all run over ONE read, and that read is not the write.
//! `#150` closes the gap: a precondition this module actually ENFORCED comes
//! back paired with a `tellurion_core::locking::RowVersion` witness of the
//! target row (captured through `WriteSink::row_version` BEFORE the read
//! being hashed, never after — see that method's own doc), and the write
//! goes out through `WriteSink::apply_conditional`, which re-verifies the
//! witness as a predicate the backend evaluates atomically with the write.
//! `Ok(None)` from that call — somebody else wrote first, nothing committed
//! — is the `412` this guard exists to produce. A write lane whose driver
//! cannot do that refuses by name (`CapabilityUnsupported`) instead of
//! quietly falling back to a check that cannot hold; a request carrying no
//! enforceable precondition never touches any of this and behaves exactly as
//! it always has.
//!
//! This module deliberately does not call `handlers.rs`'s own
//! `resolve_tenant_catalog`/`authorize_lane`/`extract_credential` helpers —
//! they are private to that module (a read-lane file this lane does not
//! own), so the small handful this file needs are reimplemented locally
//! rather than exported from it. `authorize_write_lane` below is this
//! module's own counterpart to `handlers.rs`'s `authorize_lane`: it runs the
//! identical `policy::authorize_resource` isolation/RBAC checkpoint against
//! `PolicyLane::Write` (`#68`) and builds the same 401/403 problem+json on
//! `Deny`. `lane_supports_filter` is always passed as `false` — a write
//! grant can never carry a filter (`validate_grant` rejects that combination
//! at config-load time; row-level write conditions are out of scope until a
//! real caller needs them), and `WriteSink::apply` has no filter parameter
//! to receive one even if a grant somehow matched with one. A deployment
//! with no access control configured (`state.authorizer` is `None`) keeps
//! the open-by-default behavior every other lane has without `auth:`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{OriginalUri, Path, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use tellurion_core::auth::Credential;
use tellurion_core::policy::{self, PolicyDecision, ResourceContext};
use tellurion_core::problem::Problem;
use tellurion_core::{
    crs, locking, AppContext, ContextState, Error as CoreError, Mutation, MutationKind, PolicyLane,
    RateCharge, RateCounter, RateVerdict, RequestedCrs, DEFAULT_MAX_REQUEST_BODY_BYTES,
};

use crate::handlers::{DEFAULT_CATALOG, DEFAULT_TENANT};
use crate::problem::ApiError;

/// Extracts a [`Credential`] from `Authorization: Bearer <token>` — mirrors
/// `handlers.rs`'s own `extract_credential` exactly (duplicated, not
/// shared, per this module's own doc).
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

/// The `#34`/`#68` policy checkpoint both write handlers call right after
/// resolving `(tenant_id, catalog_id, collection_id)` — identical in shape
/// to `handlers.rs`'s own `authorize_lane`, evaluated against
/// `PolicyLane::Write` with `lane_supports_filter: false` always (see this
/// module's own doc for why). `state.authorizer` being `None` skips straight
/// to unrestricted access, the same "byte-for-byte unchanged" rule every
/// read lane's checkpoint follows.
pub(crate) async fn authorize_write_lane(
    state: &ContextState,
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
        lane: PolicyLane::Write,
        visibility: &visibility,
    };
    match policy::authorize_resource(&state.config, &resource, &subject, false)? {
        PolicyDecision::Allow { .. } => {}
        PolicyDecision::Deny => return Err(crate::problem::policy_denied(&credential)),
    }
    // `#188`: a write is always one served request — there is no listing
    // variant of this checkpoint — so it always charges.
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

fn precondition_failed(detail: impl Into<String>) -> ApiError {
    ApiError {
        status: StatusCode::PRECONDITION_FAILED,
        problem: Problem::new(412, "PreconditionFailed", detail.into()),
    }
}

/// Whether either precondition header could still be COMPARED against
/// something for this collection — a deliberately cheap over-approximation
/// of what [`evaluate_write_preconditions_against`] will actually enforce,
/// answerable before the target is ever read (`#150`).
///
/// It only has to be a SUPERSET of the cases that end up enforcing
/// something, because it decides whether a [`locking::RowVersion`] witness is
/// captured, and a captured-but-unused witness costs one query while a
/// missing one would leave the guard unable to close its own window. It must
/// never be true for a request that enforces nothing, though: that would
/// turn a header this crate has always silently ignored — an
/// `If-Unmodified-Since` on a collection that declares no `modified_column`,
/// RFC 7232 section 3.4's unparseable date — into a capability refusal on a
/// driver that never needed the capability.
fn precondition_is_enforceable(headers: &HeaderMap, modified_column: Option<&str>) -> bool {
    if headers.contains_key(header::IF_MATCH) {
        return true;
    }
    let Some(since) = headers.get(header::IF_UNMODIFIED_SINCE) else {
        return false;
    };
    modified_column.is_some()
        && since
            .to_str()
            .ok()
            .and_then(locking::parse_http_date)
            .is_some()
}

/// The full Optimistic Locking precondition evaluation `put_item`/
/// `delete_item` both run before `apply` (OGC API Features — Part 4,
/// 20-002r1 draft): three checks over ONE read of `fid`'s current state,
/// evaluated in RFC 7232 section 5's own precedence order (`If-Match`,
/// when sent, is authoritative; `If-Unmodified-Since` is only consulted
/// when `If-Match` was NOT sent at all — never both).
///
/// 1. **The narrow existence guard** (`/req/create-replace-delete/put-rid-
///    exception` clause B, preserved byte-for-byte from before this
///    module gained real Optimistic Locking support): `If-Match` sent
///    against a target that does not exist at all refuses with `412`
///    rather than letting a `PUT` silently create it. Part 4's own text
///    states this guard for `PUT` specifically, but the reasoning is
///    RFC 7232 section 3.1's own general rule for ANY `If-Match` value
///    against a resource with no current representation to match at all
///    (whether the header carries `*` or a concrete tag, there is nothing
///    to satisfy it against) — so this refusal applies to `delete_item`
///    too, not only `put_item`.
/// 2. **`req/optimistic-locking-etags`** (`#107`): once a current
///    representation DOES exist, `If-Match`'s value is compared against
///    `tellurion_core::locking::compute_feature_etag`'s own hash of it
///    (`locking::if_match_satisfied`) — a mismatch is `412`, same status,
///    different reason (drift, not absence).
/// 3. **`req/optimistic-locking-timestamps`** (`#107`), only reached when
///    `If-Match` was never sent at all: `If-Unmodified-Since` is parsed as
///    an RFC 7231 HTTP-date and compared against `decl.modified_column`'s
///    own stored value on the current representation
///    (`locking::is_unmodified_since`) — but ONLY when this collection
///    actually declares a `modified_column` at all; with none declared,
///    or a stored value that doesn't parse, or a header that doesn't
///    parse, this step is silently skipped (RFC 7232 section 3.4 requires
///    ignoring an unparseable precondition date, and this crate's own
///    "never fabricate a timestamp" rule requires the same for a
///    collection with no real source) rather than ever failing the
///    request over it.
///
/// A collection with no read lane at all (write-only routing, no
/// `FeatureSource`) can't answer any of this, so it fails the same way any
/// other unresolvable dependency does — propagated as the underlying
/// `CapabilityUnsupported`/`NotFound` error, not silently skipped, since
/// silently skipping would mean a guard the caller explicitly asked for
/// (by sending either header) never actually ran. Callers only invoke this
/// at all when at least one of `If-Match`/`If-Unmodified-Since` is present
/// — see `put_item`/`delete_item` — so the common case (neither header
/// sent) costs no extra read.
///
/// `#150`: whatever this returns is only ever true of the instant it read.
/// A precondition it actually enforced therefore comes back paired with the
/// [`locking::RowVersion`] witness the write must re-verify in-transaction (see
/// `capture_row_version`), and `Ok(None)` means nothing was enforced — the
/// write proceeds through the ordinary unguarded path exactly as before.
async fn evaluate_write_preconditions(
    state: &ContextState,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
    fid: &str,
    headers: &HeaderMap,
) -> Result<Option<locking::RowVersion>, ApiError> {
    let (decl, source) = state
        .router
        .resolve_features(tenant_id, catalog_id, collection_id)
        .await?;

    let witness = capture_row_version(
        state,
        tenant_id,
        catalog_id,
        collection_id,
        fid,
        headers,
        decl.modified_column.as_deref(),
    )
    .await?;

    let current = source
        .item(&decl, fid, None)
        .await
        .map_err(ApiError::from)?;

    let enforced = evaluate_write_preconditions_against(
        headers,
        decl.modified_column.as_deref(),
        current.as_ref(),
    )?;
    resolve_witness(enforced, witness)
}

/// Reads the write lane's [`locking::RowVersion`] witness for `fid` — but only when
/// this request carries a precondition that could actually be enforced
/// (`precondition_is_enforceable`), so a request whose headers this crate
/// has always ignored still costs nothing and still reaches a driver with no
/// such capability unchanged (`#150`).
///
/// Called BEFORE the read whose representation gets hashed into an ETag,
/// never after — see `WriteSink::row_version`'s own doc for why the order is
/// load-bearing rather than incidental.
async fn capture_row_version(
    state: &ContextState,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
    fid: &str,
    headers: &HeaderMap,
    modified_column: Option<&str>,
) -> Result<Option<locking::RowVersion>, ApiError> {
    if !precondition_is_enforceable(headers, modified_column) {
        return Ok(None);
    }
    let (write_decl, sink) = state
        .router
        .resolve_write(tenant_id, catalog_id, collection_id)
        .await?;
    // A driver that cannot mint a witness refuses HERE, by name
    // (`CapabilityUnsupported`), rather than letting the request fall
    // through to a guard that cannot actually hold.
    sink.row_version(&write_decl, fid)
        .await
        .map_err(ApiError::from)
}

/// Pairs "a precondition was genuinely enforced" with the witness that has
/// to survive into the write (`#150`).
///
/// The `(true, None)` case is the one worth naming: the witness is captured
/// before the representation is read, so a `None` witness alongside an
/// enforced precondition means the row did not exist when the witness was
/// taken but did by the time it was hashed — somebody created it in between.
/// Refusing is the only safe answer; the caller's validator was never
/// compared against a state this server can promise is still current.
fn resolve_witness(
    enforced: bool,
    witness: Option<locking::RowVersion>,
) -> Result<Option<locking::RowVersion>, ApiError> {
    if !enforced {
        return Ok(None);
    }
    match witness {
        Some(witness) => Ok(Some(witness)),
        None => Err(precondition_failed(
            "the target resource was created concurrently with this request's own \
             precondition check; refusing rather than writing over a version this \
             request's caller never saw",
        )),
    }
}

/// `Ok(true)` when a precondition was genuinely compared and satisfied —
/// the caller must then re-verify it atomically with the write (`#150`).
/// `Ok(false)` when there was nothing to compare (no precondition header, a
/// collection with no declared `modified_column`, an unparseable date RFC
/// 7232 section 3.4 requires ignoring): the request is byte-for-byte an
/// unguarded write, exactly as it has always been.
fn evaluate_write_preconditions_against(
    headers: &HeaderMap,
    modified_column: Option<&str>,
    current: Option<&serde_json::Value>,
) -> Result<bool, ApiError> {
    if let Some(if_match) = headers.get(header::IF_MATCH) {
        let Some(current) = current else {
            return Err(precondition_failed(
                "If-Match was sent but the target resource does not exist;                  refusing rather than silently treating this as an insert                  or a no-op",
            ));
        };
        let raw = if_match.to_str().map_err(|_| {
            ApiError::from(CoreError::Invalid(
                "If-Match header value is not valid ASCII".to_string(),
            ))
        })?;
        let etag = locking::compute_feature_etag(current);
        if !locking::if_match_satisfied(raw, &etag) {
            return Err(precondition_failed(
                "If-Match does not match the target resource's current ETag;                  it has changed since this request's caller last read it",
            ));
        }
        return Ok(true);
    }

    let Some(since_header) = headers.get(header::IF_UNMODIFIED_SINCE) else {
        return Ok(false);
    };
    let Some(modified_column) = modified_column else {
        return Ok(false);
    };
    let Some(current) = current else {
        // No `If-Match` was sent, so the missing-resource guard above never
        // applies here either — an ordinary insert-by-`PUT` against a new
        // id proceeds regardless of `If-Unmodified-Since`.
        return Ok(false);
    };
    let Ok(since_raw) = since_header.to_str() else {
        return Ok(false);
    };
    let Some(since) = locking::parse_http_date(since_raw) else {
        return Ok(false);
    };
    let Some(stored_raw) = current["properties"][modified_column].as_str() else {
        return Ok(false);
    };
    let Some(modified_at) = locking::parse_stored_timestamp(stored_raw) else {
        return Ok(false);
    };
    if !locking::is_unmodified_since(modified_at, since) {
        return Err(precondition_failed(format!(
            "If-Unmodified-Since was {since_raw}, but the target resource's own              '{modified_column}' has since changed; refusing rather than              overwriting a version this request's caller never saw"
        )));
    }
    Ok(true)
}

/// [`evaluate_write_preconditions`], but only when the request actually
/// carries one of the two headers — the "neither header sent costs no extra
/// read" shortcut `put_item`/`delete_item` both apply, named once here
/// instead of repeated at each call site.
async fn evaluate_write_preconditions_if_sent(
    state: &ContextState,
    tenant_id: &str,
    catalog_id: &str,
    collection_id: &str,
    fid: &str,
    headers: &HeaderMap,
) -> Result<Option<locking::RowVersion>, ApiError> {
    if !headers.contains_key(header::IF_MATCH) && !headers.contains_key(header::IF_UNMODIFIED_SINCE)
    {
        return Ok(None);
    }
    evaluate_write_preconditions(state, tenant_id, catalog_id, collection_id, fid, headers).await
}

/// The single write call every mutating handler in this module makes
/// (`#150`), so that "a satisfied precondition is re-verified inside the
/// write transaction" is structural rather than something three handlers
/// each have to remember.
///
/// - `expected: None` — `WriteSink::apply_with_crs`, byte-for-byte the call
///   this module always made for a request carrying no enforceable
///   precondition.
/// - `expected: Some(_)` — `WriteSink::apply_conditional`, whose `Ok(None)`
///   ("somebody else got there first", the driver's own ordinary outcome
///   rather than an error — see `tellurion_core::lease` for the discipline
///   it follows) becomes the `412` the caller's precondition earned. The
///   status is deliberately the same one a precondition that failed the
///   FIRST check produces: from the client's side these are one fact — the
///   validator it sent no longer describes the resource — and splitting them
///   into two statuses would tell a client to behave differently over a
///   difference in server-side timing it cannot observe.
async fn apply_guarded(
    sink: &dyn tellurion_core::WriteSink,
    decl: &tellurion_core::CollectionDecl,
    mutation: Mutation,
    resolved_crs: RequestedCrs,
    expected: Option<&locking::RowVersion>,
) -> Result<(), ApiError> {
    let Some(expected) = expected else {
        sink.apply_with_crs(decl, mutation, resolved_crs)
            .await
            .map_err(ApiError::from)?;
        return Ok(());
    };
    match sink
        .apply_conditional(decl, mutation, resolved_crs, expected)
        .await
        .map_err(ApiError::from)?
    {
        Some(_sequence) => Ok(()),
        None => Err(precondition_failed(
            "the target resource changed between this request's precondition check \
             and its write; refusing rather than overwriting a version this request's \
             caller never saw",
        )),
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

pub(crate) fn require_param(
    params: &HashMap<String, String>,
    name: &str,
) -> Result<String, ApiError> {
    params
        .get(name)
        .cloned()
        .ok_or(CoreError::NotFound)
        .map_err(ApiError::from)
}

/// Resolves this request's `(tenant, catalog)` path segments to internal ids
/// (`#39`) — see `handlers.rs`'s own `resolve_tenant_catalog` for why this
/// is every handler's first move; duplicated here rather than imported
/// (this module's own doc explains why).
pub(crate) async fn resolve_tenant_catalog(
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

/// Parses the request body as a JSON object and pulls out its `properties`
/// member — the only structural check this endpoint makes on a feature's
/// shape beyond what `SchemaDecl::validate_feature_properties` itself
/// checks for a declared-schema collection. `properties` absent or JSON
/// `null` is treated as an empty object (a feature with no properties at
/// all is not itself malformed). Any other shape (not an object, or
/// `properties` present but not an object/null) is `Error::Invalid` — a 400
/// naming the problem, same as every other request-shape check in this
/// crate.
pub(crate) fn parse_feature_body(
    body: &[u8],
) -> Result<
    (
        serde_json::Value,
        serde_json::Map<String, serde_json::Value>,
    ),
    ApiError,
> {
    let value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| CoreError::Invalid(format!("request body is not valid JSON: {e}")))?;
    if !value.is_object() {
        return Err(ApiError::from(CoreError::Invalid(
            "request body must be a JSON object (a GeoJSON Feature)".to_string(),
        )));
    }
    let properties = match value.get("properties") {
        None | Some(serde_json::Value::Null) => serde_json::Map::new(),
        Some(serde_json::Value::Object(map)) => map.clone(),
        Some(_) => {
            return Err(ApiError::from(CoreError::Invalid(
                "feature 'properties' must be a JSON object".to_string(),
            )))
        }
    };
    Ok((value, properties))
}

/// Applies an RFC 7396 JSON Merge Patch to `target` in place. Object-valued
/// patches recurse, `null` removes an object member, and every non-object
/// patch replaces the complete target value (including arrays and scalars).
fn apply_json_merge_patch(target: &mut serde_json::Value, patch: &serde_json::Value) {
    let serde_json::Value::Object(patch_members) = patch else {
        *target = patch.clone();
        return;
    };

    if !target.is_object() {
        *target = serde_json::Value::Object(serde_json::Map::new());
    }
    let target_members = target
        .as_object_mut()
        .expect("target was replaced with an object above");
    for (name, patch_value) in patch_members {
        if patch_value.is_null() {
            target_members.remove(name);
            continue;
        }
        apply_json_merge_patch(
            target_members
                .entry(name.clone())
                .or_insert(serde_json::Value::Null),
            patch_value,
        );
    }
}

const MERGE_PATCH_MEDIA_TYPE: &str = "application/merge-patch+json";
const GEOJSON_MEDIA_TYPE: &str = "application/geo+json";

fn require_merge_patch_content_type(headers: &HeaderMap) -> Result<(), ApiError> {
    let media_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if media_type.is_some_and(|value| value.eq_ignore_ascii_case(MERGE_PATCH_MEDIA_TYPE)) {
        return Ok(());
    }
    Err(ApiError::from(CoreError::UnsupportedMediaType(format!(
        "PATCH requires Content-Type {MERGE_PATCH_MEDIA_TYPE}"
    ))))
}

fn validate_patched_feature(
    feature: &mut serde_json::Value,
    fid: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, ApiError> {
    let Some(object) = feature.as_object_mut() else {
        return Err(ApiError::from(CoreError::Invalid(
            "the merge patch result must be a GeoJSON Feature object".to_string(),
        )));
    };
    if object.get("type").and_then(serde_json::Value::as_str) != Some("Feature") {
        return Err(ApiError::from(CoreError::Invalid(
            "the merge patch result must retain type 'Feature'".to_string(),
        )));
    }
    if !matches!(
        object.get("geometry"),
        Some(serde_json::Value::Null | serde_json::Value::Object(_))
    ) {
        return Err(ApiError::from(CoreError::Invalid(
            "the merge patch result must retain a GeoJSON 'geometry' object or null".to_string(),
        )));
    }
    if let Some(serde_json::Value::Object(geometry)) = object.get("geometry") {
        serde_json::from_value::<geojson::Geometry>(serde_json::Value::Object(geometry.clone()))
            .map_err(|error| {
                ApiError::from(CoreError::Invalid(format!(
                    "feature 'geometry' is not a valid GeoJSON geometry: {error}"
                )))
            })?;
    }
    object.insert("id".to_string(), serde_json::Value::String(fid.to_string()));
    match object.get("properties") {
        None => Err(ApiError::from(CoreError::Invalid(
            "the merge patch result must retain the GeoJSON 'properties' member".to_string(),
        ))),
        Some(serde_json::Value::Null) => Ok(serde_json::Map::new()),
        Some(serde_json::Value::Object(properties)) => Ok(properties.clone()),
        Some(_) => Err(ApiError::from(CoreError::Invalid(
            "feature 'properties' must be a JSON object".to_string(),
        ))),
    }
}

/// `read_crs_capable` is the **read** source's `FeatureSource::crs_capable`,
/// not the write sink's: this body came back from `source.item_with_crs`
/// just above the call, so the CRS its coordinates are in is decided by
/// whichever driver produced them, exactly as it is on the read lane
/// (`handlers.rs`'s `set_content_crs`, `#227`).
fn patched_feature_response(
    feature: serde_json::Value,
    canonical_feature: &serde_json::Value,
    modified_column: Option<&str>,
    response_crs: RequestedCrs,
    storage_srid: Option<i32>,
    read_crs_capable: bool,
) -> Response {
    let etag = locking::compute_feature_etag(canonical_feature);
    let mut response = (StatusCode::OK, Json(feature)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(GEOJSON_MEDIA_TYPE),
    );
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    if let Some(column) = modified_column {
        if let Some(raw) = canonical_feature["properties"][column].as_str() {
            if let Some(modified_at) = locking::parse_stored_timestamp(raw) {
                if let Ok(value) = HeaderValue::from_str(&locking::format_http_date(modified_at)) {
                    response.headers_mut().insert(header::LAST_MODIFIED, value);
                }
            }
        }
    }
    let content_crs = format!(
        "<{}>",
        crs::content_crs_uri(response_crs, storage_srid, read_crs_capable)
    );
    if let Ok(value) = HeaderValue::from_str(&content_crs) {
        response.headers_mut().insert(CONTENT_CRS_HEADER, value);
    }
    response
}

fn property_tombstones(current: &serde_json::Value, patch: &serde_json::Value) -> Vec<String> {
    let Some(patch_properties) = patch.get("properties") else {
        return Vec::new();
    };
    let Some(current_properties) = current
        .get("properties")
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };
    match patch_properties {
        serde_json::Value::Null => Vec::new(),
        serde_json::Value::Object(properties) => properties
            .iter()
            .filter(|(name, value)| value.is_null() && current_properties.contains_key(*name))
            .map(|(name, _)| name.clone())
            .collect(),
        _ => Vec::new(),
    }
}

fn preserve_relational_property_tombstones(feature: &mut serde_json::Value, tombstones: &[String]) {
    if tombstones.is_empty() {
        return;
    }
    let Some(feature) = feature.as_object_mut() else {
        return;
    };
    let properties = feature
        .entry("properties")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if !properties.is_object() {
        *properties = serde_json::Value::Object(serde_json::Map::new());
    }
    let properties = properties
        .as_object_mut()
        .expect("properties was normalized to an object above");
    for name in tombstones {
        properties.insert(name.clone(), serde_json::Value::Null);
    }
}

/// Reads a write-lane request body, refusing before it is fully buffered
/// once the streamed length runs past `limit` bytes (`#91`) — never
/// buffer-then-measure. Built on axum's own body-limit machinery
/// (`axum::body::to_bytes`, the same `http_body_util::Limited` wrapper
/// `DefaultBodyLimit` applies) rather than a bespoke stream guard. A read
/// failure that isn't the length limit — a genuine transport error — surfaces
/// as `Error::Invalid` instead of misreporting a dropped connection as an
/// oversized body.
pub(crate) async fn read_capped_body(
    body: axum::body::Body,
    limit: u64,
) -> Result<bytes::Bytes, ApiError> {
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

/// The request-side `Content-Crs` header (OGC API Features Part 4,
/// Requirement 40, `/req/features/content-crs-header`). `handlers.rs` sets
/// the same header name on responses (its own `CONTENT_CRS_HEADER`,
/// duplicated here rather than imported — see this module's own doc on why
/// `handlers.rs`'s private helpers aren't shared).
const CONTENT_CRS_HEADER: HeaderName = HeaderName::from_static("content-crs");

/// Resolves the request's declared write-side CRS (OGC API Features Part 4,
/// Requirements 39-42): an absent header resolves to `RequestedCrs::
/// Omitted` (Requirement 41, `/req/features/default-crs`) — byte-for-byte
/// the CRS84 interpretation every write in this module assumed before this
/// header was ever inspected (Requirement 39, `/req/features/crs-crs84`,
/// which applies verbatim here since this crate declares Part 2 CRS by
/// Reference support, making the *conditional* Requirement 41 the live one
/// instead). A present header is parsed with `crs::parse_content_crs_header`
/// — the read-side counterpart of the `"<" URI ">"` shape `handlers.rs`'s
/// `set_content_crs` already writes for responses (Requirement 15/16) — and
/// validated against `storage_srid` through `crs::resolve`, the identical
/// seam `handlers.rs`'s `list_items`/`get_item` already run for the `crs`/
/// `bbox-crs` query parameters. A CRS this collection doesn't even advertise
/// (neither CRS84 nor its own storage CRS) refuses right here (Requirement
/// 42 clause B, `/req/features/crs-other-crs`) with `crs::resolve`'s own
/// "unsupported crs" message — never a second, write-specific rejection
/// message for the same failure `handlers.rs` already names one way.
pub(crate) fn resolve_content_crs(
    headers: &HeaderMap,
    storage_srid: Option<i32>,
) -> Result<RequestedCrs, ApiError> {
    let Some(value) = headers.get(CONTENT_CRS_HEADER) else {
        return Ok(RequestedCrs::Omitted);
    };
    let raw = value.to_str().map_err(|_| {
        ApiError::from(CoreError::Invalid(
            "Content-Crs header value is not valid ASCII".to_string(),
        ))
    })?;
    let uri = crs::parse_content_crs_header(raw).map_err(ApiError::from)?;
    crs::resolve(Some(uri), storage_srid).map_err(ApiError::from)
}

/// Refuses a declared write-side CRS the resolved write lane's driver cannot
/// actually reproject from (Requirement 42 clause B) — the write-lane
/// mirror of `handlers.rs`'s own `crs_capable` gate for `crs`/`bbox-crs`
/// (see `list_items`/`get_item`), evaluated against `WriteSink::crs_capable`
/// rather than a second, independent capability check. PostGIS and the
/// narrowly 4326/3857-capable GeoPackage write sink answer `true`.
/// `RequestedCrs::Omitted`/`::Crs84` always proceeds regardless of driver
/// capability because it is the required default input contract; a driver
/// must either store it honestly or refuse its unsupported storage CRS.
pub(crate) fn refuse_unreprojectable_content_crs(
    resolved: RequestedCrs,
    storage_srid: Option<i32>,
    crs_capable: bool,
    cid: &str,
) -> Result<(), ApiError> {
    if resolved == RequestedCrs::Storage && !crs_capable {
        return Err(ApiError::from(CoreError::Invalid(format!(
            "collection '{cid}' cannot accept a Content-Crs of '{}': its write \
             lane does not support reprojecting into that coordinate reference \
             system",
            // The `Storage` arm names this collection's own storage CRS
            // whatever the capability flag says, which is what this refusal
            // has to quote back: the value the client actually sent.
            crs::content_crs_uri(resolved, storage_srid, crs_capable)
        ))));
    }
    Ok(())
}

/// `POST /collections/{cid}/items` (`#88`) — creates one feature with a
/// server-assigned id, 201 with a `Location` header pointing at the created
/// item. Mirrors `put_item`'s own shape through the auth checkpoint, the
/// body cap, and schema validation; diverges only after `resolve_write`,
/// where a `PUT`/`DELETE` already has an `{fid}` to hand `WriteSink` and
/// this doesn't — see `WriteSink::create`'s own doc for why that's a
/// distinct method on the same trait rather than a new write path.
///
/// Together with `put_item`/`delete_item`, this is what `lib.rs`'s
/// `CONFORMANCE_CLASSES` now declares as OGC API Features — Part 4's
/// Create/Replace/Delete requirements class — see that constant's own doc
/// for the exact URI and what's still withheld.
pub async fn create_item(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: axum::body::Body,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    authorize_write_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
    )
    .await?;

    let limit = state
        .router
        .effective_settings(&collection_id)
        .map(|settings| settings.max_request_body_bytes)
        .unwrap_or(DEFAULT_MAX_REQUEST_BODY_BYTES);
    let body = read_capped_body(body, limit).await?;

    let (decl, sink) = state
        .router
        .resolve_write(&tenant_id, &catalog_id, &collection_id)
        .await?;

    let resolved_crs = resolve_content_crs(&headers, decl.srid)?;
    refuse_unreprojectable_content_crs(resolved_crs, decl.srid, sink.crs_capable(), &cid)?;

    let (feature, properties) = parse_feature_body(&body)?;
    if let Some(schema) = &decl.schema {
        schema
            .validate_feature_properties(&properties)
            .map_err(ApiError::from)?;
    }

    let (new_id, _sequence) = sink
        .create_with_crs(&decl, feature, resolved_crs)
        .await
        .map_err(ApiError::from)?;

    let location_path = format!("{}/{new_id}", uri.path().trim_end_matches('/'));
    let location = state.config.server.public_href(&location_path);
    let mut response = StatusCode::CREATED.into_response();
    let location_value = HeaderValue::from_str(&location).map_err(|_| {
        ApiError::from(CoreError::Invalid(
            "the minted item id is not a valid Location header value".to_string(),
        ))
    })?;
    response
        .headers_mut()
        .insert(header::LOCATION, location_value);
    Ok(response)
}

/// `PUT /collections/{cid}/items/{fid}` — replaces (or creates) one feature.
/// Schema-validates the request body against the collection's declared
/// schema, when it has one, before ever calling `WriteSink::apply` — a
/// malformed feature never becomes a committed outbox obligation (`#44`).
/// Inspects `Content-Crs` exactly like `create_item` — see
/// `resolve_content_crs`/`refuse_unreprojectable_content_crs`'s own docs.
pub async fn put_item(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let fid = require_param(&params, "fid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    authorize_write_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
    )
    .await?;

    let expected_version = evaluate_write_preconditions_if_sent(
        &state,
        &tenant_id,
        &catalog_id,
        &collection_id,
        &fid,
        &headers,
    )
    .await?;

    let limit = state
        .router
        .effective_settings(&collection_id)
        .map(|settings| settings.max_request_body_bytes)
        .unwrap_or(DEFAULT_MAX_REQUEST_BODY_BYTES);
    let body = read_capped_body(body, limit).await?;

    let (decl, sink) = state
        .router
        .resolve_write(&tenant_id, &catalog_id, &collection_id)
        .await?;

    let resolved_crs = resolve_content_crs(&headers, decl.srid)?;
    refuse_unreprojectable_content_crs(resolved_crs, decl.srid, sink.crs_capable(), &cid)?;

    let (feature, properties) = parse_feature_body(&body)?;
    if let Some(schema) = &decl.schema {
        schema
            .validate_feature_properties(&properties)
            .map_err(ApiError::from)?;
    }

    apply_guarded(
        sink.as_ref(),
        &decl,
        Mutation {
            feature_id: fid,
            kind: MutationKind::Upsert(feature),
        },
        resolved_crs,
        expected_version.as_ref(),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `PATCH /collections/{cid}/items/{fid}` — applies an RFC 7396 JSON Merge
/// Patch to an existing feature. The path identifier is authoritative:
/// an `id` member in the patch document is ignored, and the final feature
/// is normalized back to `{fid}` before schema validation and persistence.
pub async fn patch_item(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let fid = require_param(&params, "fid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    authorize_write_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
    )
    .await?;
    require_merge_patch_content_type(&headers)?;

    let limit = state
        .router
        .effective_settings(&collection_id)
        .map(|settings| settings.max_request_body_bytes)
        .unwrap_or(DEFAULT_MAX_REQUEST_BODY_BYTES);
    let body = read_capped_body(body, limit).await?;
    let mut patch: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|error| CoreError::Invalid(format!("request body is not valid JSON: {error}")))?;
    if let Some(object) = patch.as_object_mut() {
        object.remove("id");
    }

    let (read_decl, source) = state
        .router
        .resolve_features(&tenant_id, &catalog_id, &collection_id)
        .await?;
    // `#150`: captured BEFORE the read whose representation is hashed —
    // `WriteSink::row_version`'s own doc explains why a witness taken
    // afterwards would let through exactly the write this guard exists to
    // stop.
    let witness = capture_row_version(
        &state,
        &tenant_id,
        &catalog_id,
        &collection_id,
        &fid,
        &headers,
        read_decl.modified_column.as_deref(),
    )
    .await?;
    let Some(current_canonical) = source
        .item(&read_decl, &fid, None)
        .await
        .map_err(ApiError::from)?
    else {
        return Err(ApiError::from(CoreError::NotFound));
    };
    let expected_version = resolve_witness(
        evaluate_write_preconditions_against(
            &headers,
            read_decl.modified_column.as_deref(),
            Some(&current_canonical),
        )?,
        witness,
    )?;

    let (decl, sink) = state
        .router
        .resolve_write(&tenant_id, &catalog_id, &collection_id)
        .await?;
    let resolved_crs = resolve_content_crs(&headers, decl.srid)?;
    refuse_unreprojectable_content_crs(resolved_crs, decl.srid, sink.crs_capable(), &cid)?;

    let working_crs = if source.crs_capable() {
        RequestedCrs::Crs84
    } else {
        RequestedCrs::Omitted
    };
    let mut updated = if working_crs == RequestedCrs::Omitted {
        current_canonical
    } else {
        source
            .item_with_crs(&read_decl, &fid, None, working_crs)
            .await
            .map_err(ApiError::from)?
            .ok_or(CoreError::NotFound)
            .map_err(ApiError::from)?
    };
    let tombstones = property_tombstones(&updated, &patch);
    apply_json_merge_patch(&mut updated, &patch);
    preserve_relational_property_tombstones(&mut updated, &tombstones);
    let properties = validate_patched_feature(&mut updated, &fid)?;
    if let Some(schema) = &decl.schema {
        schema
            .validate_feature_properties(&properties)
            .map_err(ApiError::from)?;
    }

    // `Content-Crs` describes spatial values present in the patch document.
    // When geometry is untouched, the merged geometry came from the
    // canonical read representation (CRS84), not from the request body.
    let mutation_crs = if patch.get("geometry").is_some() {
        resolved_crs
    } else {
        working_crs
    };
    apply_guarded(
        sink.as_ref(),
        &decl,
        Mutation {
            feature_id: fid.clone(),
            kind: MutationKind::Upsert(updated),
        },
        mutation_crs,
        expected_version.as_ref(),
    )
    .await?;

    let final_canonical = source
        .item(&read_decl, &fid, None)
        .await
        .map_err(ApiError::from)?
        .ok_or(CoreError::NotFound)
        .map_err(ApiError::from)?;
    let final_feature = if working_crs == RequestedCrs::Omitted {
        final_canonical.clone()
    } else {
        source
            .item_with_crs(&read_decl, &fid, None, working_crs)
            .await
            .map_err(ApiError::from)?
            .ok_or(CoreError::NotFound)
            .map_err(ApiError::from)?
    };
    Ok(patched_feature_response(
        final_feature,
        &final_canonical,
        read_decl.modified_column.as_deref(),
        working_crs,
        read_decl.srid,
        source.crs_capable(),
    ))
}

/// `DELETE /collections/{cid}/items/{fid}` — removes one feature. No schema
/// validation applies (there is no inbound feature body to check).
pub async fn delete_item(
    State(ctx): State<Arc<AppContext>>,
    Path(params): Path<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (tenant_id, catalog_id) = resolve_tenant_catalog(&ctx, &params).await?;
    let cid = require_param(&params, "cid")?;
    let fid = require_param(&params, "fid")?;
    let state = ctx.current();
    let collection_id = state.resolver.resolve_collection(&catalog_id, &cid).await?;
    authorize_write_lane(
        &state,
        ctx.rate_counter.as_ref(),
        &headers,
        &tenant_id,
        &catalog_id,
        &collection_id,
    )
    .await?;

    // OGC API Features — Part 4 Optimistic Locking (`#107`): the exact same
    // preconditions `put_item` evaluates, ahead of the same
    // `Router::resolve_write` — see `evaluate_write_preconditions`'s own
    // doc for why this applies to `DELETE` too, not only `PUT`.
    let expected_version = evaluate_write_preconditions_if_sent(
        &state,
        &tenant_id,
        &catalog_id,
        &collection_id,
        &fid,
        &headers,
    )
    .await?;

    let (decl, sink) = state
        .router
        .resolve_write(&tenant_id, &catalog_id, &collection_id)
        .await?;

    // `RequestedCrs::Omitted` is what `WriteSink::apply` itself passes on
    // this lane (a `DELETE` carries no geometry to interpret), so routing
    // through `apply_guarded` changes nothing for an unguarded delete.
    apply_guarded(
        sink.as_ref(),
        &decl,
        Mutation {
            feature_id: fid,
            kind: MutationKind::Delete,
        },
        RequestedCrs::Omitted,
        expected_version.as_ref(),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_feature_body_defaults_absent_properties_to_empty() {
        let (_value, properties) = parse_feature_body(br#"{"type":"Feature","geometry":null}"#)
            .expect("valid feature body");
        assert!(properties.is_empty());
    }

    #[test]
    fn parse_feature_body_treats_a_null_properties_member_as_empty() {
        let (_value, properties) =
            parse_feature_body(br#"{"type":"Feature","properties":null}"#).expect("valid body");
        assert!(properties.is_empty());
    }

    #[test]
    fn parse_feature_body_extracts_a_present_properties_object() {
        let (_value, properties) =
            parse_feature_body(br#"{"type":"Feature","properties":{"name":"a"}}"#)
                .expect("valid body");
        assert_eq!(properties.get("name").unwrap(), "a");
    }

    #[test]
    fn parse_feature_body_rejects_malformed_json() {
        assert!(parse_feature_body(b"not json").is_err());
    }

    #[test]
    fn parse_feature_body_rejects_a_non_object_top_level_value() {
        assert!(parse_feature_body(b"[1, 2, 3]").is_err());
    }

    #[test]
    fn parse_feature_body_rejects_a_non_object_properties_member() {
        assert!(parse_feature_body(br#"{"type":"Feature","properties":"nope"}"#).is_err());
    }

    #[test]
    fn json_merge_patch_recurses_removes_nulls_and_replaces_arrays() {
        let mut target = serde_json::json!({
            "type": "Feature",
            "geometry": null,
            "properties": {
                "name": "old",
                "nested": { "keep": true, "remove": 1 },
                "tags": ["old"]
            }
        });
        let patch = serde_json::json!({
            "properties": {
                "name": "new",
                "nested": { "remove": null, "added": 2 },
                "tags": ["new", "array"]
            }
        });

        apply_json_merge_patch(&mut target, &patch);

        assert_eq!(
            target,
            serde_json::json!({
                "type": "Feature",
                "geometry": null,
                "properties": {
                    "name": "new",
                    "nested": { "keep": true, "added": 2 },
                    "tags": ["new", "array"]
                }
            })
        );
    }

    #[test]
    fn json_merge_patch_replaces_the_whole_target_with_a_scalar() {
        let mut target = serde_json::json!({"type": "Feature"});
        apply_json_merge_patch(&mut target, &serde_json::json!(false));
        assert_eq!(target, serde_json::json!(false));
    }

    #[test]
    fn json_merge_patch_rejects_removing_the_required_properties_member() {
        let mut target = serde_json::json!({
            "type": "Feature",
            "id": "x",
            "geometry": null,
            "properties": {"name": "old"}
        });
        apply_json_merge_patch(&mut target, &serde_json::json!({"properties": null}));
        assert!(validate_patched_feature(&mut target, "x").is_err());
    }

    // -- `Content-Crs` (OGC API Features Part 4, `/req/features/
    // content-crs-header`, `/req/features/crs-other-crs`) ------------------

    #[test]
    fn resolve_content_crs_is_omitted_when_the_header_is_absent() {
        let headers = HeaderMap::new();
        assert_eq!(
            resolve_content_crs(&headers, Some(3857)).unwrap(),
            RequestedCrs::Omitted
        );
    }

    #[test]
    fn resolve_content_crs_accepts_explicit_crs84() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_CRS_HEADER,
            HeaderValue::from_static("<http://www.opengis.net/def/crs/OGC/1.3/CRS84>"),
        );
        assert_eq!(
            resolve_content_crs(&headers, Some(3857)).unwrap(),
            RequestedCrs::Crs84
        );
    }

    #[test]
    fn resolve_content_crs_accepts_the_collections_own_storage_crs() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_CRS_HEADER,
            HeaderValue::from_static("<http://www.opengis.net/def/crs/EPSG/0/3857>"),
        );
        assert_eq!(
            resolve_content_crs(&headers, Some(3857)).unwrap(),
            RequestedCrs::Storage
        );
    }

    #[test]
    fn resolve_content_crs_refuses_a_crs_the_collection_does_not_advertise_at_all() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_CRS_HEADER,
            HeaderValue::from_static("<http://www.opengis.net/def/crs/EPSG/0/25832>"),
        );
        assert!(resolve_content_crs(&headers, Some(3857)).is_err());
    }

    #[test]
    fn resolve_content_crs_refuses_a_header_with_no_angle_brackets() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_CRS_HEADER,
            HeaderValue::from_static("http://www.opengis.net/def/crs/OGC/1.3/CRS84"),
        );
        let err = resolve_content_crs(&headers, Some(3857)).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn resolve_content_crs_refuses_an_empty_header_value() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_CRS_HEADER, HeaderValue::from_static(""));
        assert!(resolve_content_crs(&headers, Some(3857)).is_err());
    }

    #[test]
    fn refuse_unreprojectable_content_crs_allows_omitted_regardless_of_capability() {
        assert!(refuse_unreprojectable_content_crs(
            RequestedCrs::Omitted,
            Some(3857),
            false,
            "demo"
        )
        .is_ok());
    }

    #[test]
    fn refuse_unreprojectable_content_crs_allows_explicit_crs84_regardless_of_capability() {
        assert!(
            refuse_unreprojectable_content_crs(RequestedCrs::Crs84, Some(3857), false, "demo")
                .is_ok()
        );
    }

    #[test]
    fn refuse_unreprojectable_content_crs_allows_storage_when_the_driver_is_capable() {
        assert!(refuse_unreprojectable_content_crs(
            RequestedCrs::Storage,
            Some(3857),
            true,
            "demo"
        )
        .is_ok());
    }

    #[test]
    fn refuse_unreprojectable_content_crs_refuses_storage_when_the_driver_is_not_capable() {
        let err =
            refuse_unreprojectable_content_crs(RequestedCrs::Storage, Some(3857), false, "demo")
                .unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(
            err.problem.detail.contains("demo")
                && err
                    .problem
                    .detail
                    .contains("http://www.opengis.net/def/crs/EPSG/0/3857"),
            "detail should name both the collection and the refused crs: {}",
            err.problem.detail
        );
    }
}
