//! Asset domain logic (assets-and-object-storage proposal, first slice):
//! the managed/remote discriminator, the pending -> available -> failed
//! lifecycle, and RFC 9530 `Repr-Digest` declaration/verification — all
//! plain Rust, unit-testable without axum and without a live object store
//! (see this module's own `InMemoryAssetRecordStore`/
//! `objectstore::InMemoryObjectStore`). [`AssetRecordStore`] is the
//! database-backed capability a `StorageDriver` advertises
//! (`StorageDriver::asset_record_store`, `router.rs`) — the assets
//! counterpart of [`crate::outbox::WriteSink`]: same "this driver never
//! claims this capability by default" shape, same "the server never
//! creates the table itself" DDL-ownership rule (`tellurion-ingest`
//! provisions `"<table>_assets"`, a driver refuses by name when it's
//! absent, exactly like `OutboxTableMissing`).
//!
//! ## Idempotent PUT vs. a genuine conflict
//!
//! The proposal's `core` conformance class requires an idempotent PUT.
//! [`register_managed`]/[`register_remote`] implement that literally: a PUT
//! that re-declares a key with the *exact same* representation it already
//! holds is a no-op success (the existing record comes back unchanged, no
//! new internal id minted, no re-registration attempt) — only a PUT that
//! re-declares a key with a *different* representation is a named
//! [`crate::error::Error::Conflict`]. A caller that wants to change an
//! already-registered key's declaration (including retrying a `failed`
//! upload with a new attempt) deletes it first — PATCH, which could relax
//! this, is out of scope for this slice.

#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use uuid::Uuid;

use crate::config::CollectionDecl;
use crate::error::{Error, Result};
use crate::objectstore::{ObjectKey, ObjectStoreError, PresignedObjectStore, ResumableUploadStore};

/// Managed vs. remote (the proposal's first load-bearing distinction): a
/// managed asset's bytes live in an object store this deployment controls;
/// a remote asset is an external `href` registered as metadata only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Managed,
    Remote,
}

/// The explicit managed-asset lifecycle (the proposal's second load-bearing
/// distinction). A remote asset is always [`AssetState::Available`] — it
/// has no byte lifecycle to track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetState {
    Pending,
    Available,
    Failed,
}

/// A declared or computed digest (RFC 9530). This slice supports exactly
/// one algorithm — `sha-256`, the proposal's stated minimum — so this is
/// not an open enum: [`parse_repr_digest`] refuses any other algorithm by
/// name rather than silently accepting or ignoring it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    pub value: [u8; 32],
}

impl Digest {
    pub fn from_sha256_bytes(value: [u8; 32]) -> Self {
        Self { value }
    }
}

/// `sha2::Sha256` over `bytes`, wrapped as this module's own [`Digest`]
/// type — the domain-logic half of checksum verification, callable with no
/// axum request in scope at all (see this module's own doc).
pub fn compute_sha256(bytes: &[u8]) -> Digest {
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    Digest {
        value: hasher.finalize().into(),
    }
}

/// `declared == computed`, byte for byte. Not constant-time — a digest is
/// an integrity check on public bytes, not a secret, so a timing
/// side-channel here reveals nothing an attacker doesn't already have (the
/// bytes themselves).
pub fn verify_digest(declared: &Digest, computed: &Digest) -> bool {
    declared.value == computed.value
}

/// Parses an RFC 9530 `Repr-Digest` header value, requiring it to declare
/// exactly one algorithm, `sha-256` (Structured Fields Dictionary member
/// shape: `sha-256=:<base64>:`). Any other shape — zero members, more than
/// one, an algorithm other than `sha-256`, or a malformed `sf-binary` value
/// — is refused by name rather than partially accepted: this slice's
/// `checksum` class supports exactly the one algorithm the proposal calls
/// the minimum required, never a silent best-effort pick among several.
pub fn parse_repr_digest(header_value: &str) -> Result<Digest> {
    let members: Vec<&str> = header_value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if members.is_empty() {
        return Err(Error::Invalid(
            "Repr-Digest header must declare at least one algorithm".to_string(),
        ));
    }
    if members.len() > 1 {
        return Err(Error::Invalid(format!(
            "Repr-Digest declares {} algorithms; this deployment supports exactly one (sha-256)",
            members.len()
        )));
    }
    let member = members[0];
    let (name, value) = member.split_once('=').ok_or_else(|| {
        Error::Invalid(format!(
            "'{member}' is not a valid Repr-Digest member (expected 'algorithm=:base64:')"
        ))
    })?;
    let name = name.trim().to_ascii_lowercase();
    if name != "sha-256" {
        return Err(Error::Invalid(format!(
            "Repr-Digest algorithm '{name}' is not supported; this deployment requires 'sha-256'"
        )));
    }
    let value = value.trim();
    let inner = value
        .strip_prefix(':')
        .and_then(|v| v.strip_suffix(':'))
        .ok_or_else(|| {
            Error::Invalid(
                "sha-256 Repr-Digest value must be RFC 8941 sf-binary (':base64:')".to_string(),
            )
        })?;
    let decoded = decode_base64(inner).ok_or_else(|| {
        Error::Invalid("sha-256 Repr-Digest value is not valid base64".to_string())
    })?;
    let value: [u8; 32] = decoded.try_into().map_err(|bad: Vec<u8>| {
        Error::Invalid(format!(
            "sha-256 digest must decode to 32 bytes, got {}",
            bad.len()
        ))
    })?;
    Ok(Digest { value })
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Minimal RFC 4648 (padded, standard-alphabet) base64 encoder — this
/// crate's own hand-rolled implementation, the same "no extra crate for a
/// few dozen lines" call `tellurion-stac::search`'s base64url helpers
/// already make for the URL-safe alphabet. Used to render a computed digest
/// into a human-readable mismatch message and by this module's own tests to
/// build `Repr-Digest` fixtures.
pub fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied();
        let b2 = chunk.get(2).copied();
        let n = (b0 as u32) << 16 | (b1.unwrap_or(0) as u32) << 8 | (b2.unwrap_or(0) as u32);
        out.push(BASE64_ALPHABET[(n >> 18 & 0x3f) as usize] as char);
        out.push(BASE64_ALPHABET[(n >> 12 & 0x3f) as usize] as char);
        out.push(if b1.is_some() {
            BASE64_ALPHABET[(n >> 6 & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if b2.is_some() {
            BASE64_ALPHABET[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// The decode half of [`encode_base64`] — also reused by an
/// `AssetRecordStore` implementer (`tellurion-postgis::asset_sql`) to turn
/// a stored digest column's text back into raw bytes when reading a row.
pub fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let input = input.as_bytes();
    if input.is_empty() {
        return Some(Vec::new());
    }
    if !input.len().is_multiple_of(4) {
        return None;
    }
    let mut table = [255u8; 256];
    for (i, &c) in BASE64_ALPHABET.iter().enumerate() {
        table[c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    for chunk in input.chunks(4) {
        let mut vals = [0u8; 4];
        let mut pad = 0u8;
        for (i, &b) in chunk.iter().enumerate() {
            if b == b'=' {
                pad += 1;
                vals[i] = 0;
            } else {
                if pad > 0 {
                    // A '=' padding character can only appear at the end.
                    return None;
                }
                let v = table[b as usize];
                if v == 255 {
                    return None;
                }
                vals[i] = v;
            }
        }
        let n = (vals[0] as u32) << 18
            | (vals[1] as u32) << 12
            | (vals[2] as u32) << 6
            | (vals[3] as u32);
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// A registered asset, as [`AssetRecordStore`] persists and returns it.
/// `href` is `None` for a managed asset — the caller (the handler) derives
/// the `.../data` sub-resource URL from the request's own route rather than
/// storing a URL that would go stale under a mount-point change.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetRecord {
    /// This asset's own immutable internal id — a managed asset's object
    /// key (`objectstore::ObjectKey::new(record.id)`) derives from this and
    /// nothing else. Never serialized to a client.
    pub id: Uuid,
    pub kind: AssetKind,
    pub state: AssetState,
    pub href: Option<String>,
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub roles: Vec<String>,
    /// `Some` only for a managed asset — the byte length declared at
    /// registration (`file:size`), the direct-upload cap this slice bounds
    /// a transfer to.
    pub declared_size: Option<u64>,
    /// `Some` only for a managed asset — the digest declared at
    /// registration, verified when the bytes finish arriving.
    pub digest: Option<Digest>,
    /// `Some` only in [`AssetState::Failed`] — why.
    pub failure_reason: Option<String>,
}

/// What [`AssetRecordStore::register`] persists for a brand-new key. Built
/// by [`register_managed`]/[`register_remote`] after policy validation —
/// never constructed directly by a handler.
#[derive(Debug, Clone)]
pub struct NewAssetRecord {
    pub id: Uuid,
    pub kind: NewAssetKind,
}

#[derive(Debug, Clone)]
pub enum NewAssetKind {
    Managed {
        media_type: Option<String>,
        title: Option<String>,
        description: Option<String>,
        roles: Vec<String>,
        declared_size: u64,
        digest: Digest,
    },
    Remote {
        href: String,
        media_type: Option<String>,
        title: Option<String>,
        description: Option<String>,
        roles: Vec<String>,
    },
}

/// How [`AssetRecordStore::finalize`] transitions a pending managed asset —
/// the only two lifecycle exits a managed asset has (`asset.rs`'s own doc).
#[derive(Debug, Clone)]
pub enum FinalizeOutcome {
    Available,
    Failed { reason: String },
}

/// One record [`AssetRecordStore::list`] reports back, carrying the scoping
/// identity (`item_id`/`key`) that [`AssetRecord`] itself deliberately
/// omits (every other trait method already has that identity from its own
/// arguments) — the reconcile surface's own "everything the database thinks
/// exists" half (`crate::reconcile`), which needs it to name a drift by key,
/// not just by internal id.
#[derive(Debug, Clone)]
pub struct AssetRecordEntry {
    /// `None`: collection-level. `Some(id)`: that item's own asset.
    pub item_id: Option<String>,
    pub key: String,
    pub record: AssetRecord,
}

/// The database-backed asset capability a `StorageDriver` advertises
/// (`router.rs::StorageDriver::asset_record_store`) — see this module's own
/// doc for the `WriteSink`/`OutboxTableMissing` parallel. `item_id: None`
/// means collection-level; `Some(id)` means that item's own asset. `key` is
/// the opaque, per-parent-unique asset key from the URL.
#[async_trait::async_trait]
pub trait AssetRecordStore: Send + Sync {
    /// Create-only: `Ok` on a fresh key, [`Error::Conflict`] when `(item_id,
    /// key)` already holds a record — see this module's own doc for why
    /// idempotent-replay detection lives in [`register_managed`]/
    /// [`register_remote`], one layer up, rather than here.
    async fn register(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
        new_record: NewAssetRecord,
    ) -> Result<AssetRecord>;

    async fn get(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
    ) -> Result<Option<AssetRecord>>;

    /// Transitions a pending managed asset to `available`/`failed`.
    /// [`Error::NotFound`] when `(item_id, key)` names no record.
    async fn finalize(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
        outcome: FinalizeOutcome,
    ) -> Result<AssetRecord>;

    /// Removes the record. `Ok(None)` when `(item_id, key)` named nothing —
    /// the same "absent is a successful no-op, not an error" shape
    /// [`crate::objectstore::ObjectStore::delete`] uses.
    async fn delete(
        &self,
        collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
    ) -> Result<Option<AssetRecord>>;

    /// Every record scoped to `collection` — both collection-level
    /// (`item_id: None`) and every item's own assets — the reconcile
    /// surface's own "everything the database thinks exists" half
    /// (`crate::reconcile`). Unbounded: a collection's asset count is
    /// expected to stay small relative to its feature count (assets are a
    /// keyed sub-resource per item/collection, not a bulk data lane), so
    /// this carries no paging concept, unlike every feature/item listing
    /// surface in this workspace.
    async fn list(&self, collection: &CollectionDecl) -> Result<Vec<AssetRecordEntry>>;

    /// Every *item-scoped* record belonging to any of `item_ids`, in ONE
    /// round trip (`#221`) — the read the STAC lane's Item projection makes
    /// once per page so a page of N items costs one query, not N. PostGIS
    /// compiles it to `item_id = ANY($1)` over a single `text[]` bind,
    /// served by the leading column of the `UNIQUE (item_id, asset_key)`
    /// index the assets table already carries, so this needs no DDL of its
    /// own beyond the table `tellurion-ingest assets create-tables` already
    /// provisions.
    ///
    /// This is deliberately a batched sibling of [`get`](Self::get) rather
    /// than a filter over [`list`](Self::list): `list` is the reconcile
    /// surface's unbounded whole-table walk, and running it per page would
    /// make an Item request's cost scale with the collection's total asset
    /// count instead of the page's.
    ///
    /// Contract:
    ///
    /// - **Item-scoped only.** A collection-level record (`item_id: None`,
    ///   stored by every implementation under the `""` sentinel) is NEVER
    ///   returned, no matter what `item_ids` contains — collection-scoped
    ///   assets belong to the Collection document, and letting one leak
    ///   into an Item is precisely the flattening `#221` exists to prevent.
    ///   Every returned entry therefore has `item_id: Some(_)`.
    /// - **Sparse.** An id with no records is simply absent from the
    ///   result; "this item has no assets" is the ordinary case, not an
    ///   error, and must stay indistinguishable from a collection whose
    ///   items have none.
    /// - **Every state.** `pending`/`failed` managed records come back too:
    ///   which lifecycle states are advertisable is the STAC lane's rule
    ///   (`tellurion-stac::assets`), not a filter this storage capability
    ///   bakes in, exactly as [`list`](Self::list) reports every state to
    ///   reconcile.
    /// - **Empty input, no I/O.** An empty `item_ids` slice MUST answer
    ///   `Ok(vec![])` without touching the backend at all: an empty page
    ///   has nothing to enrich, and a round trip for it would break the
    ///   "one query per page, never one per item" budget in the other
    ///   direction.
    async fn item_assets(
        &self,
        collection: &CollectionDecl,
        item_ids: &[String],
    ) -> Result<Vec<AssetRecordEntry>>;
}

/// Media-type allow-list and size cap, threaded in from the collection's
/// effective settings (`settings::EffectiveSettings`) rather than resolved
/// by this module itself — keeps [`register_managed`]/[`register_remote`]
/// callable from a plain unit test with no `Router` in scope.
pub struct AssetPolicy<'a> {
    pub max_asset_bytes: u64,
    /// `None`: no allow-list configured, every declared media type is
    /// accepted (this slice's default, matching every other whitelisted
    /// setting's "absent means unrestricted" convention). `Some(list)`:
    /// only a media type present in `list` (case-insensitive) is accepted.
    pub allowed_media_types: Option<&'a [String]>,
}

impl AssetPolicy<'_> {
    fn check_media_type(&self, media_type: Option<&str>) -> Result<()> {
        let Some(allowed) = self.allowed_media_types else {
            return Ok(());
        };
        match media_type {
            Some(media_type) if allowed.iter().any(|m| m.eq_ignore_ascii_case(media_type)) => {
                Ok(())
            }
            Some(media_type) => Err(Error::UnsupportedMediaType(media_type.to_string())),
            None => Err(Error::UnsupportedMediaType(
                "no media type declared, and this collection restricts assets to an allow-list"
                    .to_string(),
            )),
        }
    }
}

pub struct RegisterManagedRequest {
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub roles: Vec<String>,
    pub declared_size: u64,
    pub digest: Digest,
}

pub struct RegisterRemoteRequest {
    pub href: String,
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub roles: Vec<String>,
}

fn managed_matches(existing: &AssetRecord, request: &RegisterManagedRequest) -> bool {
    existing.kind == AssetKind::Managed
        && existing.media_type == request.media_type
        && existing.title == request.title
        && existing.description == request.description
        && existing.roles == request.roles
        && existing.declared_size == Some(request.declared_size)
        && existing.digest.as_ref() == Some(&request.digest)
}

fn remote_matches(existing: &AssetRecord, request: &RegisterRemoteRequest) -> bool {
    existing.kind == AssetKind::Remote
        && existing.href.as_deref() == Some(request.href.as_str())
        && existing.media_type == request.media_type
        && existing.title == request.title
        && existing.description == request.description
        && existing.roles == request.roles
}

/// Registers a managed asset: validates the declared media type and size
/// against `policy` (413/415, named, before any storage I/O — the object
/// store is never touched here), then either returns an unchanged existing
/// record (idempotent replay, see this module's own doc) or persists a
/// brand-new `pending` record via `store.register`.
pub async fn register_managed(
    store: &dyn AssetRecordStore,
    policy: &AssetPolicy<'_>,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
    request: RegisterManagedRequest,
) -> Result<AssetRecord> {
    policy.check_media_type(request.media_type.as_deref())?;
    if request.declared_size > policy.max_asset_bytes {
        return Err(Error::PayloadTooLarge {
            limit: policy.max_asset_bytes,
        });
    }

    if let Some(existing) = store.get(collection, item_id, key).await? {
        return if managed_matches(&existing, &request) {
            Ok(existing)
        } else {
            Err(Error::Conflict(format!(
                "asset key '{key}' is already registered with a different declaration"
            )))
        };
    }

    let new_record = NewAssetRecord {
        id: Uuid::new_v4(),
        kind: NewAssetKind::Managed {
            media_type: request.media_type,
            title: request.title,
            description: request.description,
            roles: request.roles,
            declared_size: request.declared_size,
            digest: request.digest,
        },
    };
    store.register(collection, item_id, key, new_record).await
}

/// Registers a remote asset: born available, no byte lifecycle. Same
/// idempotent-replay-vs-conflict rule as [`register_managed`].
pub async fn register_remote(
    store: &dyn AssetRecordStore,
    policy: &AssetPolicy<'_>,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
    request: RegisterRemoteRequest,
) -> Result<AssetRecord> {
    if request.href.trim().is_empty() {
        return Err(Error::Invalid(
            "a remote asset registration requires a non-empty 'href'".to_string(),
        ));
    }
    policy.check_media_type(request.media_type.as_deref())?;

    if let Some(existing) = store.get(collection, item_id, key).await? {
        return if remote_matches(&existing, &request) {
            Ok(existing)
        } else {
            Err(Error::Conflict(format!(
                "asset key '{key}' is already registered with a different declaration"
            )))
        };
    }

    let new_record = NewAssetRecord {
        id: Uuid::new_v4(),
        kind: NewAssetKind::Remote {
            href: request.href,
            media_type: request.media_type,
            title: request.title,
            description: request.description,
            roles: request.roles,
        },
    };
    store.register(collection, item_id, key, new_record).await
}

/// The direct-upload transfer + finalize step (`PUT .../assets/{key}/data`):
/// the caller has already read the request body capped at the record's own
/// `declared_size` (the existing streamed-length body-cap machinery,
/// `tellurion-stac::asset_handlers`) — this function is pure orchestration
/// from there: refuse a key with no pending managed asset, verify the
/// digest, write to the object store only on a match, and finalize.
///
/// A digest mismatch fails the asset by name: `finalize` still runs (the
/// asset ends in [`AssetState::Failed`], queryable), and the object store is
/// never touched — the mismatched bytes are simply dropped.
///
/// This transport always writes: the caller never put anything anywhere
/// before this ran, so [`finish_upload`]'s own `already_committed` is always
/// `false` here. [`complete_resumable_upload`] is the other caller, and the
/// one that sometimes passes `true` — see [`finish_upload`]'s own doc.
pub async fn complete_upload(
    store: &dyn AssetRecordStore,
    objects: &dyn crate::objectstore::ObjectStore,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
    bytes: bytes::Bytes,
) -> Result<AssetRecord> {
    finish_upload(store, objects, collection, item_id, key, bytes, false).await
}

/// [`complete_upload`]'s own shared implementation, parameterized on
/// whether `bytes` is already sitting at the asset's own final object key —
/// `already_committed`, which only ever changes ONE thing: whether the
/// whole-object `put` near the bottom of this function runs at all.
///
/// [`complete_upload`] always passes `false`: the direct-upload transport
/// never writes anywhere before this runs, so that `put` is the only thing
/// that ever gets the bytes into the store. [`complete_resumable_upload`]
/// passes whatever `objects.take_upload_already_committed()` reports —
/// `true` for the `s3` profile, where `ResumableUploadStore::take_upload`'s
/// own `CompleteMultipartUpload` already landed these exact bytes at this
/// exact key before this function ever saw them. Re-`put`-ting them there
/// would not just waste a full second transfer of the object: past S3's
/// 5&nbsp;GiB single-request `PutObject` cap, it would fail outright on an
/// asset whose multipart upload had just succeeded, marking a genuinely
/// complete asset `Failed`.
///
/// The digest-mismatch cleanup above the `put` runs unconditionally either
/// way — for the `already_committed` case that delete is not a best-effort
/// nicety on top of "never wrote anything", it is precisely the mechanism
/// that keeps "a digest mismatch never leaves bytes behind" true for a
/// transport that wrote bytes to the final key before this function ever
/// got a chance to check them.
async fn finish_upload(
    store: &dyn AssetRecordStore,
    objects: &dyn crate::objectstore::ObjectStore,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
    bytes: bytes::Bytes,
    already_committed: bool,
) -> Result<AssetRecord> {
    let Some(record) = store.get(collection, item_id, key).await? else {
        return Err(Error::NotFound);
    };
    if record.kind != AssetKind::Managed {
        // No `.../data` resource exists for a remote asset at all.
        return Err(Error::NotFound);
    }
    if record.state != AssetState::Pending {
        return Err(Error::Conflict(format!(
            "asset '{key}' is not awaiting an upload"
        )));
    }
    let declared = record.digest.clone().expect(
        "a managed record always carries a declared digest (register_managed's own invariant)",
    );
    let object_key = ObjectKey::new(record.id);

    let computed = compute_sha256(&bytes);
    if !verify_digest(&declared, &computed) {
        let reason = format!(
            "declared sha-256 digest {} does not match the uploaded bytes (computed {})",
            encode_base64(&declared.value),
            encode_base64(&computed.value)
        );
        // Best-effort: for the direct-upload transport (`already_committed`
        // is always `false` there) this key never had anything written to
        // it in the first place, so the delete is a harmless no-op. The
        // resumable-upload transport is different when `already_committed`
        // is `true` — completing an S3 multipart upload IS what makes the
        // assembled bytes readable back at all
        // (`ResumableUploadStore::take_upload`'s own doc), so by the time
        // this function ever sees them, `S3ObjectStore` has already
        // committed them to this exact key. This delete is what restores
        // the "a digest mismatch never leaves bytes behind" invariant for
        // that transport; a failure here is swallowed rather than
        // overriding the digest-mismatch error the caller actually asked
        // about.
        let _ = objects.delete(object_key.clone()).await;
        store
            .finalize(
                collection,
                item_id,
                key,
                FinalizeOutcome::Failed {
                    reason: reason.clone(),
                },
            )
            .await?;
        return Err(Error::UnprocessableEntity(reason));
    }

    if !already_committed {
        if let Err(err) = objects.put(object_key, bytes).await {
            let reason = format!("writing the asset to the object store failed: {err}");
            // Best-effort: if even marking it failed doesn't work, the
            // original storage error is still what the caller sees.
            let _ = store
                .finalize(collection, item_id, key, FinalizeOutcome::Failed { reason })
                .await;
            return Err(Error::Storage(Box::new(err)));
        }
    }

    store
        .finalize(collection, item_id, key, FinalizeOutcome::Available)
        .await
}

/// The presigned-upload negotiation step (`presigned-upload` conformance
/// class): mints a time-limited signed `PUT` URL against a `pending`
/// managed asset's own object key, at this store's own configured expiry
/// (`PresignedObjectStore::default_expiry`). Never touches the record's
/// state — presigning is idempotent and side-effect-free; the asset only
/// moves out of `pending` at [`finalize_presigned_upload`].
///
/// `objects` is `&dyn PresignedObjectStore`, not `&dyn ObjectStore` — a
/// compile-time expression of "this only runs against a store that has the
/// capability at all"; the caller (`asset_handlers.rs`) resolves it by
/// calling `ObjectStore::as_presigned` on whatever store this collection
/// configured and refuses by name (`Error::CapabilityUnsupported`) before
/// this function is ever reached when it returns `None` — the `fs`
/// profile's own "no URL space to presign against" refusal.
pub async fn presign_upload(
    store: &dyn AssetRecordStore,
    objects: &dyn PresignedObjectStore,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
    now: SystemTime,
) -> Result<String> {
    let Some(record) = store.get(collection, item_id, key).await? else {
        return Err(Error::NotFound);
    };
    if record.kind != AssetKind::Managed {
        return Err(Error::NotFound);
    }
    if record.state != AssetState::Pending {
        return Err(Error::Conflict(format!(
            "asset '{key}' is not awaiting an upload"
        )));
    }
    objects
        .presign_put(ObjectKey::new(record.id), objects.default_expiry(), now)
        .map_err(|err| Error::Storage(Box::new(err)))
}

/// The presigned-upload commit step (`presigned-upload` conformance class):
/// the server never saw the bytes (the client transferred them straight to
/// the store via the URL [`presign_upload`] minted), so this verifies via
/// `HEAD` rather than a digest computed from a body in hand — existence
/// first (absent -> `failed`, named), then declared-vs-reported size when
/// both are known, then declared-vs-reported sha-256 digest when the store
/// itself reports one (`ObjectMetadata`'s own doc: most S3-compatible
/// stores don't, unless the client's own presigned upload opted into a
/// checksum algorithm the store understands — this is a best-effort
/// integrity check on top of existence/size, never a requirement this
/// slice can enforce end-to-end the way [`complete_upload`]'s in-hand
/// digest check can). Idempotent under retry: finalizing an asset that is
/// already `available`/`failed` is [`Error::Conflict`], the identical shape
/// [`complete_upload`] already uses.
pub async fn finalize_presigned_upload(
    store: &dyn AssetRecordStore,
    objects: &dyn PresignedObjectStore,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
) -> Result<AssetRecord> {
    let Some(record) = store.get(collection, item_id, key).await? else {
        return Err(Error::NotFound);
    };
    if record.kind != AssetKind::Managed {
        return Err(Error::NotFound);
    }
    if record.state != AssetState::Pending {
        return Err(Error::Conflict(format!(
            "asset '{key}' is not awaiting an upload"
        )));
    }

    let object_key = ObjectKey::new(record.id);
    let metadata = objects
        .head(object_key)
        .await
        .map_err(|err| Error::Storage(Box::new(err)))?;

    let Some(metadata) = metadata else {
        let reason = "no object was found at the presigned upload target".to_string();
        store
            .finalize(
                collection,
                item_id,
                key,
                FinalizeOutcome::Failed {
                    reason: reason.clone(),
                },
            )
            .await?;
        return Err(Error::UnprocessableEntity(reason));
    };

    if let (Some(declared), Some(actual)) = (record.declared_size, metadata.size) {
        if declared != actual {
            let reason = format!(
                "declared size {declared} does not match the uploaded object's size {actual}"
            );
            store
                .finalize(
                    collection,
                    item_id,
                    key,
                    FinalizeOutcome::Failed {
                        reason: reason.clone(),
                    },
                )
                .await?;
            return Err(Error::UnprocessableEntity(reason));
        }
    }

    if let (Some(declared), Some(reported_bytes)) = (record.digest.as_ref(), metadata.sha256) {
        let reported = Digest::from_sha256_bytes(reported_bytes);
        if !verify_digest(declared, &reported) {
            let reason = format!(
                "declared sha-256 digest {} does not match the object's reported digest {}",
                encode_base64(&declared.value),
                encode_base64(&reported.value)
            );
            store
                .finalize(
                    collection,
                    item_id,
                    key,
                    FinalizeOutcome::Failed {
                        reason: reason.clone(),
                    },
                )
                .await?;
            return Err(Error::UnprocessableEntity(reason));
        }
    }

    store
        .finalize(collection, item_id, key, FinalizeOutcome::Available)
        .await
}

// -- resumable upload (`resumable-upload` conformance class) ------------
//
// A resumable upload is a subresource of a pending managed asset's own data
// lane — registration (`register_managed`) still happens first, exactly as
// for the direct-upload and presigned-upload transports. What follows is
// the IETF resumable-upload draft's own shape, adapted to this surface:
// create an upload resource, probe its current offset, append chunks at an
// offset (refused by name when it doesn't match what has actually
// accumulated), then complete — which reuses [`complete_upload`]'s own
// digest/cap verification and pending -> available/failed transition
// unchanged, rather than duplicating it for this transport. Implemented for
// [`crate::objectstore::FsObjectStore`] only in this slice — see
// [`ResumableUploadStore`]'s own doc for why `s3` refuses this class by
// name instead.

/// Shared prelude for every function below: fetches the record and refuses
/// (by name) unless it names a managed asset still `pending` — the same two
/// checks [`complete_upload`]/[`presign_upload`] each duplicate inline
/// (this module's own established pattern); factored once here since all
/// five resumable-upload functions need it.
async fn require_pending_managed(
    store: &dyn AssetRecordStore,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
) -> Result<AssetRecord> {
    let Some(record) = store.get(collection, item_id, key).await? else {
        return Err(Error::NotFound);
    };
    if record.kind != AssetKind::Managed {
        // No resumable-upload resource exists for a remote asset at all.
        return Err(Error::NotFound);
    }
    if record.state != AssetState::Pending {
        return Err(Error::Conflict(format!(
            "asset '{key}' is not awaiting an upload"
        )));
    }
    Ok(record)
}

/// `POST .../assets/{key}/data/uploads`: creates a fresh resumable-upload
/// resource for a pending managed asset. Refuses (named [`Error::Conflict`])
/// when one is already in progress for this key — the caller deletes it
/// first ([`abandon_resumable_upload`]) to start clean, the identical
/// "explicit delete before re-declare" shape this module's own doc already
/// establishes for a conflicting registration.
pub async fn create_resumable_upload(
    store: &dyn AssetRecordStore,
    objects: &dyn ResumableUploadStore,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
) -> Result<()> {
    let record = require_pending_managed(store, collection, item_id, key).await?;
    let object_key = ObjectKey::new(record.id);
    let already_in_progress = objects
        .upload_offset(object_key.clone())
        .await
        .map_err(|err| Error::Storage(Box::new(err)))?
        .is_some();
    if already_in_progress {
        return Err(Error::Conflict(format!(
            "an upload is already in progress for asset '{key}'; delete it before starting a new one"
        )));
    }
    objects
        .create_upload(object_key)
        .await
        .map_err(|err| Error::Storage(Box::new(err)))
}

/// `GET .../assets/{key}/data/uploads` (HEAD-style probe): the number of
/// bytes accumulated so far. [`Error::NotFound`] when no upload is in
/// progress for this key — never created, or already consumed by
/// [`complete_resumable_upload`]/[`abandon_resumable_upload`].
pub async fn resumable_upload_offset(
    store: &dyn AssetRecordStore,
    objects: &dyn ResumableUploadStore,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
) -> Result<u64> {
    let record = require_pending_managed(store, collection, item_id, key).await?;
    objects
        .upload_offset(ObjectKey::new(record.id))
        .await
        .map_err(|err| Error::Storage(Box::new(err)))?
        .ok_or(Error::NotFound)
}

/// `PATCH .../assets/{key}/data/uploads`: appends `chunk` at
/// `expected_offset`. The byte cap rides the record's own `declared_size`
/// (the same cap [`register_managed`] already refused an over-cap total
/// against, up front): a chunk that would push the accumulated total past
/// it is refused (named [`Error::PayloadTooLarge`]) before it is ever handed
/// to the store, never buffered past the cap. `expected_offset` not
/// matching what the store actually has accumulated — checked atomically by
/// `objects.append_upload` itself, the concurrency guard — is a named
/// [`Error::Conflict`], covering both directions: out-of-order (the caller
/// is ahead of what has really landed) and stale (the caller is behind,
/// retrying a position another append already advanced past).
pub async fn append_resumable_upload(
    store: &dyn AssetRecordStore,
    objects: &dyn ResumableUploadStore,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
    expected_offset: u64,
    chunk: bytes::Bytes,
) -> Result<u64> {
    let record = require_pending_managed(store, collection, item_id, key).await?;
    let declared_size = record.declared_size.expect(
        "a managed record always carries a declared size (register_managed's own invariant)",
    );
    let prospective_total = expected_offset.saturating_add(chunk.len() as u64);
    if prospective_total > declared_size {
        return Err(Error::PayloadTooLarge {
            limit: declared_size,
        });
    }

    match objects
        .append_upload(ObjectKey::new(record.id), expected_offset, chunk)
        .await
    {
        Ok(new_offset) => Ok(new_offset),
        Err(ObjectStoreError::UploadNotFound) => Err(Error::NotFound),
        Err(ObjectStoreError::UploadOffsetMismatch { expected, actual }) => {
            let reason = if expected > actual {
                format!(
                    "out-of-order append: offset {expected} was declared but only {actual} \
                     bytes have been accumulated so far"
                )
            } else {
                format!(
                    "stale offset: {actual} bytes have already been accumulated, past the \
                     declared offset {expected}"
                )
            };
            Err(Error::Conflict(reason))
        }
        Err(err) => Err(Error::Storage(Box::new(err))),
    }
}

/// `POST .../assets/{key}/data/uploads/complete`: pulls every accumulated
/// byte back out of the upload resource (consuming it, whether what follows
/// succeeds or not) and hands it to [`finish_upload`] — the exact same
/// digest verification, cap-shaped invariants, and pending ->
/// available/failed transition the direct-upload transport already uses,
/// never duplicated for this one. [`Error::NotFound`] when no upload is in
/// progress for this key.
///
/// Reads [`ResumableUploadStore::take_upload_already_committed`] before
/// deciding whether [`finish_upload`]'s own whole-object `put` should run —
/// `false` for `fs` (nothing has been written to the final key yet, `take_
/// upload` only ever read a staging file), `true` for `s3` (`take_upload`'s
/// own `CompleteMultipartUpload` already landed these bytes at the final
/// key). See [`finish_upload`]'s own doc for why re-`put`-ting them in the
/// `s3` case would be worse than merely wasteful.
///
/// For a store where `take_upload` already committed bytes at the key
/// before this ever ran, `objects` also keeps that key marked in-progress
/// (`ResumableUploadStore::release_verifying_upload`'s own doc) for as long
/// as [`finish_upload`] below is still deciding whether those bytes are
/// correct — otherwise a second `create_resumable_upload` for the same key
/// could be admitted into that exact window and have its own correct bytes
/// destroyed by this attempt's delayed cleanup on a mismatch. `release`
/// below runs exactly once, unconditionally, right after the single
/// `.await` on [`finish_upload`] — never threaded through that function's
/// own several exit paths (digest match, digest mismatch, or any error
/// along the way), so there is only one call site to ever forget, not one
/// per branch.
pub async fn complete_resumable_upload(
    store: &dyn AssetRecordStore,
    objects: &dyn ResumableUploadStore,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
) -> Result<AssetRecord> {
    let record = require_pending_managed(store, collection, item_id, key).await?;
    let object_key = ObjectKey::new(record.id);
    let already_committed = objects.take_upload_already_committed();
    let bytes = match objects.take_upload(object_key.clone()).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Err(Error::NotFound),
        Err(err) => {
            // `take_upload` can fail after it already committed bytes at
            // the final key, on its own read-back `GET` (`take_upload`'s
            // own doc) — release unconditionally so that case can never
            // leave this key stuck refusing every future upload. A no-op
            // when this attempt never actually committed anything.
            let _ = objects.release_verifying_upload(object_key).await;
            return Err(Error::Storage(Box::new(err)));
        }
    };
    let result = finish_upload(
        store,
        objects,
        collection,
        item_id,
        key,
        bytes,
        already_committed,
    )
    .await;
    let _ = objects.release_verifying_upload(object_key).await;
    result
}

/// `DELETE .../assets/{key}/data/uploads`: discards an in-progress upload
/// without completing it — the asset stays `pending`, untouched, and a
/// fresh [`create_resumable_upload`] on the same key starts clean.
/// Idempotent: deleting an already-absent (or never-created) upload is
/// still `Ok(())`, the same "already in the target state" contract
/// [`delete_asset`] and [`crate::objectstore::ObjectStore::delete`] both
/// use.
pub async fn abandon_resumable_upload(
    store: &dyn AssetRecordStore,
    objects: &dyn ResumableUploadStore,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
) -> Result<()> {
    let record = require_pending_managed(store, collection, item_id, key).await?;
    objects
        .abandon_upload(ObjectKey::new(record.id))
        .await
        .map_err(|err| Error::Storage(Box::new(err)))
}

/// `DELETE .../assets/{key}`: removes the record, and for a managed asset,
/// the object too (object first, then the record — a transient object-store
/// failure leaves the record intact rather than orphaning bytes with
/// nothing pointing at them). A remote asset's bytes are never touched.
///
/// `objects` is `Option` — not every collection has a `managed-storage`
/// lane at all (a `core`-only, remote-assets-only deployment has no
/// `object_store` configured, `Router::resolve_object_store`'s own doc), so
/// a caller resolves it lazily and passes `None` rather than being forced
/// to fail the whole delete before ever learning the record it's about to
/// remove is remote and never needed one. `None` while the record turns out
/// to be [`AssetKind::Managed`] is [`Error::CapabilityUnsupported`] — an
/// inconsistent deployment (a managed record with no object store to
/// delete its bytes from), never silently skipped.
pub async fn delete_asset(
    store: &dyn AssetRecordStore,
    objects: Option<&dyn crate::objectstore::ObjectStore>,
    collection: &CollectionDecl,
    item_id: Option<&str>,
    key: &str,
) -> Result<Option<AssetRecord>> {
    let Some(record) = store.get(collection, item_id, key).await? else {
        return Ok(None);
    };
    if record.kind == AssetKind::Managed {
        let objects = objects.ok_or_else(|| Error::CapabilityUnsupported {
            collection: collection.id.clone(),
            capability: "managed-storage".to_string(),
        })?;
        objects
            .delete(ObjectKey::new(record.id))
            .await
            .map_err(|err| Error::Storage(Box::new(err)))?;
    }
    store.delete(collection, item_id, key).await
}

/// In-memory [`AssetRecordStore`] for domain-logic tests — the assets
/// counterpart of `objectstore::InMemoryObjectStore`. Ignores `collection`
/// entirely (every test builds its own store instance, so cross-collection
/// isolation is never exercised through this fake).
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct InMemoryAssetRecordStore {
    records: std::sync::Mutex<HashMap<(String, String), AssetRecord>>,
}

#[cfg(any(test, feature = "test-support"))]
fn scope_key(item_id: Option<&str>, key: &str) -> (String, String) {
    (item_id.unwrap_or("").to_string(), key.to_string())
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait::async_trait]
impl AssetRecordStore for InMemoryAssetRecordStore {
    async fn register(
        &self,
        _collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
        new_record: NewAssetRecord,
    ) -> Result<AssetRecord> {
        let scope = scope_key(item_id, key);
        let mut records = self.records.lock().unwrap();
        if records.contains_key(&scope) {
            return Err(Error::Conflict(format!(
                "asset key '{key}' is already registered"
            )));
        }
        let record = match new_record.kind {
            NewAssetKind::Managed {
                media_type,
                title,
                description,
                roles,
                declared_size,
                digest,
            } => AssetRecord {
                id: new_record.id,
                kind: AssetKind::Managed,
                state: AssetState::Pending,
                href: None,
                media_type,
                title,
                description,
                roles,
                declared_size: Some(declared_size),
                digest: Some(digest),
                failure_reason: None,
            },
            NewAssetKind::Remote {
                href,
                media_type,
                title,
                description,
                roles,
            } => AssetRecord {
                id: new_record.id,
                kind: AssetKind::Remote,
                state: AssetState::Available,
                href: Some(href),
                media_type,
                title,
                description,
                roles,
                declared_size: None,
                digest: None,
                failure_reason: None,
            },
        };
        records.insert(scope, record.clone());
        Ok(record)
    }

    async fn get(
        &self,
        _collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
    ) -> Result<Option<AssetRecord>> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .get(&scope_key(item_id, key))
            .cloned())
    }

    async fn finalize(
        &self,
        _collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
        outcome: FinalizeOutcome,
    ) -> Result<AssetRecord> {
        let mut records = self.records.lock().unwrap();
        let record = records
            .get_mut(&scope_key(item_id, key))
            .ok_or(Error::NotFound)?;
        match outcome {
            FinalizeOutcome::Available => {
                record.state = AssetState::Available;
                record.failure_reason = None;
            }
            FinalizeOutcome::Failed { reason } => {
                record.state = AssetState::Failed;
                record.failure_reason = Some(reason);
            }
        }
        Ok(record.clone())
    }

    async fn delete(
        &self,
        _collection: &CollectionDecl,
        item_id: Option<&str>,
        key: &str,
    ) -> Result<Option<AssetRecord>> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .remove(&scope_key(item_id, key)))
    }

    async fn list(&self, _collection: &CollectionDecl) -> Result<Vec<AssetRecordEntry>> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .iter()
            .map(|((item_id, key), record)| AssetRecordEntry {
                item_id: (!item_id.is_empty()).then(|| item_id.clone()),
                key: key.clone(),
                record: record.clone(),
            })
            .collect())
    }

    /// The in-memory twin of the driver's batched read: the `""`
    /// collection-level sentinel is excluded unconditionally, so a caller
    /// that (harmlessly) passes an empty id — `page_feature_ids` degrades a
    /// feature with no `id` member to `""` — still cannot pull a
    /// collection-level asset into an Item. Sorted so a test asserting on
    /// the result has a stable order to compare against, matching the
    /// driver's own `ORDER BY item_id, asset_key`.
    async fn item_assets(
        &self,
        _collection: &CollectionDecl,
        item_ids: &[String],
    ) -> Result<Vec<AssetRecordEntry>> {
        if item_ids.is_empty() {
            return Ok(Vec::new());
        }
        let wanted: std::collections::HashSet<&str> = item_ids
            .iter()
            .map(String::as_str)
            .filter(|id| !id.is_empty())
            .collect();
        let mut entries: Vec<AssetRecordEntry> = self
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|((item_id, _), _)| wanted.contains(item_id.as_str()))
            .map(|((item_id, key), record)| AssetRecordEntry {
                item_id: Some(item_id.clone()),
                key: key.clone(),
                record: record.clone(),
            })
            .collect();
        entries.sort_by(|a, b| (&a.item_id, &a.key).cmp(&(&b.item_id, &b.key)));
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objectstore::{InMemoryObjectStore, ObjectStore as _};

    fn collection() -> CollectionDecl {
        serde_yaml::from_str(
            r#"
id: demo
catalog: default
storage: main
table: demo
"#,
        )
        .unwrap()
    }

    fn open_policy() -> AssetPolicy<'static> {
        AssetPolicy {
            max_asset_bytes: 1_000_000,
            allowed_media_types: None,
        }
    }

    fn managed_request(size: u64, digest: Digest) -> RegisterManagedRequest {
        RegisterManagedRequest {
            media_type: Some("image/png".to_string()),
            title: Some("thumbnail".to_string()),
            description: None,
            roles: vec!["thumbnail".to_string()],
            declared_size: size,
            digest,
        }
    }

    // -- RFC 9530 digest parsing ---------------------------------------

    #[test]
    fn parses_a_well_formed_sha256_repr_digest() {
        let digest = compute_sha256(b"hello world");
        let header = format!("sha-256=:{}:", encode_base64(&digest.value));
        let parsed = parse_repr_digest(&header).unwrap();
        assert_eq!(parsed, digest);
    }

    #[test]
    fn refuses_an_unsupported_digest_algorithm_by_name() {
        let err = parse_repr_digest("sha-512=:AAAA:").unwrap_err();
        assert!(matches!(err, Error::Invalid(msg) if msg.contains("sha-512")));
    }

    #[test]
    fn refuses_more_than_one_declared_algorithm() {
        let digest = compute_sha256(b"x");
        let header = format!("sha-256=:{}:, sha-512=:AAAA:", encode_base64(&digest.value));
        assert!(parse_repr_digest(&header).is_err());
    }

    #[test]
    fn refuses_a_malformed_header_value() {
        assert!(parse_repr_digest("").is_err());
        assert!(parse_repr_digest("sha-256").is_err());
        assert!(parse_repr_digest("sha-256=not-sf-binary").is_err());
        assert!(parse_repr_digest("sha-256=:not base64!!:").is_err());
    }

    #[test]
    fn base64_round_trips() {
        for input in [
            b"".as_slice(),
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
        ] {
            let encoded = encode_base64(input);
            assert_eq!(decode_base64(&encoded).unwrap(), input);
        }
    }

    // -- register -> upload -> available round trip ---------------------

    #[tokio::test]
    async fn register_upload_available_round_trip_verifies_the_digest() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let payload = b"the real bytes".to_vec();
        let digest = compute_sha256(&payload);

        let pending = register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(payload.len() as u64, digest),
        )
        .await
        .unwrap();
        assert_eq!(pending.state, AssetState::Pending);
        assert_eq!(pending.kind, AssetKind::Managed);

        let available = complete_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            bytes::Bytes::from(payload.clone()),
        )
        .await
        .unwrap();
        assert_eq!(available.state, AssetState::Available);
        assert_eq!(available.id, pending.id);

        let stored = objects.get(ObjectKey::new(available.id)).await.unwrap();
        assert_eq!(stored.unwrap(), bytes::Bytes::from(payload));
    }

    #[tokio::test]
    async fn item_level_round_trip_is_isolated_from_the_collection_level_key() {
        let store = InMemoryAssetRecordStore::default();
        let collection = collection();
        let policy = open_policy();
        let payload = b"item bytes".to_vec();
        let digest = compute_sha256(&payload);

        register_managed(
            &store,
            &policy,
            &collection,
            Some("feature-1"),
            "thumb",
            managed_request(payload.len() as u64, digest.clone()),
        )
        .await
        .unwrap();

        // The identical key at collection level (`item_id: None`) is a
        // distinct scope — registering it must not conflict.
        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(payload.len() as u64, digest),
        )
        .await
        .unwrap();

        let item_asset = store
            .get(&collection, Some("feature-1"), "thumb")
            .await
            .unwrap();
        let collection_asset = store.get(&collection, None, "thumb").await.unwrap();
        assert!(item_asset.is_some());
        assert!(collection_asset.is_some());
        assert_ne!(item_asset.unwrap().id, collection_asset.unwrap().id);
    }

    #[tokio::test]
    async fn digest_mismatch_fails_the_asset_by_name_and_never_writes_the_object() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let declared_digest = compute_sha256(b"expected bytes");

        let pending = register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(14, declared_digest),
        )
        .await
        .unwrap();

        let err = complete_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            bytes::Bytes::from_static(b"different!!!!!"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::UnprocessableEntity(_)));

        let failed = store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.state, AssetState::Failed);
        assert!(failed.failure_reason.is_some());
        assert!(
            objects
                .get(ObjectKey::new(pending.id))
                .await
                .unwrap()
                .is_none(),
            "a digest mismatch must never leave bytes in the object store"
        );
    }

    /// The resumable-upload transport's own hazard on the `s3` profile:
    /// `S3ObjectStore::take_upload` must complete its S3 multipart upload
    /// (landing bytes at this exact key) before it can read them back at
    /// all, so unlike the direct-upload transport above, bytes can already
    /// be sitting at `object_key` by the time this function's own digest
    /// check runs. Simulated here without a real `S3ObjectStore` by
    /// pre-seeding the fake with what `take_upload` would already have
    /// written — proves `complete_upload`'s own cleanup-on-mismatch delete
    /// (not merely "never call put") is what the invariant above actually
    /// needs for that transport.
    #[tokio::test]
    async fn digest_mismatch_deletes_bytes_a_resumable_upload_had_already_committed() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let declared_digest = compute_sha256(b"expected bytes");

        let pending = register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(14, declared_digest),
        )
        .await
        .unwrap();

        // Stand in for `S3ObjectStore::take_upload`'s own
        // `CompleteMultipartUpload` + read-back already having landed the
        // (wrong) bytes at this key before this function ever runs.
        objects
            .put(
                ObjectKey::new(pending.id),
                bytes::Bytes::from_static(b"different!!!!!"),
            )
            .await
            .unwrap();

        let err = complete_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            bytes::Bytes::from_static(b"different!!!!!"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::UnprocessableEntity(_)));

        assert!(
            objects
                .get(ObjectKey::new(pending.id))
                .await
                .unwrap()
                .is_none(),
            "the pre-existing (wrong) bytes must be cleaned up on a digest mismatch, \
             not left behind at the asset's own final key"
        );
    }

    // -- registration conflicts / idempotency ----------------------------

    #[tokio::test]
    async fn conflicting_key_is_refused_by_name() {
        let store = InMemoryAssetRecordStore::default();
        let policy = open_policy();
        let collection = collection();
        let digest = compute_sha256(b"a");

        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(1, digest.clone()),
        )
        .await
        .unwrap();

        // A different declaration (different digest) at the same key.
        let other_digest = compute_sha256(b"b");
        let err = register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(1, other_digest),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)));
    }

    #[tokio::test]
    async fn replaying_the_identical_registration_is_idempotent_not_a_conflict() {
        let store = InMemoryAssetRecordStore::default();
        let policy = open_policy();
        let collection = collection();
        let digest = compute_sha256(b"a");

        let first = register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(1, digest.clone()),
        )
        .await
        .unwrap();
        let second = register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(1, digest),
        )
        .await
        .unwrap();
        assert_eq!(
            first.id, second.id,
            "the same internal id, not a fresh registration"
        );
    }

    #[tokio::test]
    async fn a_remote_declaration_at_a_managed_key_conflicts() {
        let store = InMemoryAssetRecordStore::default();
        let policy = open_policy();
        let collection = collection();
        let digest = compute_sha256(b"a");

        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(1, digest),
        )
        .await
        .unwrap();

        let err = register_remote(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            RegisterRemoteRequest {
                href: "https://example.test/thumb.png".to_string(),
                media_type: None,
                title: None,
                description: None,
                roles: vec![],
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)));
    }

    // -- media type / size cap -------------------------------------------

    #[tokio::test]
    async fn media_type_outside_the_allow_list_is_refused() {
        let store = InMemoryAssetRecordStore::default();
        let collection = collection();
        let allowed = vec!["image/png".to_string()];
        let policy = AssetPolicy {
            max_asset_bytes: 1_000_000,
            allowed_media_types: Some(&allowed),
        };
        let mut request = managed_request(1, compute_sha256(b"a"));
        request.media_type = Some("application/x-executable".to_string());

        let err = register_managed(&store, &policy, &collection, None, "bad", request)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedMediaType(_)));
    }

    #[tokio::test]
    async fn declared_size_over_the_cap_is_refused_before_any_storage_io() {
        let store = InMemoryAssetRecordStore::default();
        let collection = collection();
        let policy = AssetPolicy {
            max_asset_bytes: 10,
            allowed_media_types: None,
        };
        let request = managed_request(11, compute_sha256(b"a"));

        let err = register_managed(&store, &policy, &collection, None, "big", request)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PayloadTooLarge { limit: 10 }));
        assert!(store.get(&collection, None, "big").await.unwrap().is_none());
    }

    // -- remote assets: register + delete never touching bytes ----------

    #[tokio::test]
    async fn remote_asset_registers_available_with_no_byte_lifecycle() {
        let store = InMemoryAssetRecordStore::default();
        let collection = collection();
        let policy = open_policy();

        let record = register_remote(
            &store,
            &policy,
            &collection,
            None,
            "external",
            RegisterRemoteRequest {
                href: "https://example.test/data.tif".to_string(),
                media_type: Some("image/tiff".to_string()),
                title: None,
                description: None,
                roles: vec![],
            },
        )
        .await
        .unwrap();
        assert_eq!(record.kind, AssetKind::Remote);
        assert_eq!(record.state, AssetState::Available);
    }

    #[tokio::test]
    async fn deleting_a_remote_asset_never_touches_the_object_store() {
        let store = InMemoryAssetRecordStore::default();
        let collection = collection();
        let policy = open_policy();

        register_remote(
            &store,
            &policy,
            &collection,
            None,
            "external",
            RegisterRemoteRequest {
                href: "https://example.test/data.tif".to_string(),
                media_type: None,
                title: None,
                description: None,
                roles: vec![],
            },
        )
        .await
        .unwrap();

        // `objects: None` — a metadata-only, remote-assets-only collection
        // (`core` conformance class alone) has no object store configured
        // at all; deleting a remote asset must still succeed.
        let deleted = delete_asset(&store, None, &collection, None, "external")
            .await
            .unwrap();
        assert!(deleted.is_some());
        assert!(store
            .get(&collection, None, "external")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn deleting_a_managed_asset_removes_both_the_record_and_the_object() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let payload = b"bytes".to_vec();
        let digest = compute_sha256(&payload);

        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(payload.len() as u64, digest),
        )
        .await
        .unwrap();
        let available = complete_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            bytes::Bytes::from(payload),
        )
        .await
        .unwrap();

        delete_asset(&store, Some(&objects), &collection, None, "thumb")
            .await
            .unwrap();
        assert!(store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .is_none());
        assert!(objects
            .get(ObjectKey::new(available.id))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn deleting_a_managed_asset_with_no_object_store_configured_is_a_named_refusal() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let payload = b"bytes".to_vec();
        let digest = compute_sha256(&payload);

        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(payload.len() as u64, digest),
        )
        .await
        .unwrap();
        complete_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            bytes::Bytes::from(payload),
        )
        .await
        .unwrap();

        let err = delete_asset(&store, None, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::CapabilityUnsupported { .. }));
    }

    #[tokio::test]
    async fn completing_an_upload_for_an_unregistered_key_is_not_found() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let err = complete_upload(
            &store,
            &objects,
            &collection,
            None,
            "nope",
            bytes::Bytes::from_static(b"x"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::NotFound));
    }

    #[tokio::test]
    async fn completing_an_already_available_upload_is_a_named_conflict() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let payload = b"bytes".to_vec();
        let digest = compute_sha256(&payload);

        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(payload.len() as u64, digest),
        )
        .await
        .unwrap();
        complete_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            bytes::Bytes::from(payload),
        )
        .await
        .unwrap();

        let err = complete_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            bytes::Bytes::from_static(b"again"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)));
    }

    // -- presigned upload: register -> presign -> finalize -> available ---
    //
    // `InMemoryObjectStore` honors `s3` semantics for this suite's purposes
    // (`objectstore.rs`'s own doc): a presigned URL is never actually
    // dereferenced — these tests simulate the client's out-of-band upload
    // by calling `ObjectStore::put` on the same fake store directly, then
    // exercise `finalize_presigned_upload` against it exactly the way the
    // real HTTP handler would after a client's real presigned `PUT`
    // succeeded.

    fn fixed_now() -> std::time::SystemTime {
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_440_938_160)
    }

    #[tokio::test]
    async fn register_presign_upload_finalize_available_round_trip() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::with_checksum_reporting();
        let collection = collection();
        let policy = open_policy();
        let payload = b"presigned bytes".to_vec();
        let digest = compute_sha256(&payload);

        let pending = register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(payload.len() as u64, digest),
        )
        .await
        .unwrap();
        assert_eq!(pending.state, AssetState::Pending);

        let href = presign_upload(&store, &objects, &collection, None, "thumb", fixed_now())
            .await
            .unwrap();
        assert!(href.contains("method=PUT"));
        // Presigning itself never changes the record's state.
        assert_eq!(
            store
                .get(&collection, None, "thumb")
                .await
                .unwrap()
                .unwrap()
                .state,
            AssetState::Pending
        );

        // The client's own out-of-band upload, simulated as a direct
        // `put` against the same fake store the presigned URL targets.
        objects
            .put(ObjectKey::new(pending.id), bytes::Bytes::from(payload))
            .await
            .unwrap();

        let available = finalize_presigned_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        assert_eq!(available.state, AssetState::Available);
        assert_eq!(available.id, pending.id);
    }

    #[tokio::test]
    async fn finalize_presigned_upload_fails_by_name_when_the_object_is_absent() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();

        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(5, compute_sha256(b"12345")),
        )
        .await
        .unwrap();

        // No `put` at all this time — the client never actually uploaded.
        let err = finalize_presigned_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::UnprocessableEntity(_)));

        let failed = store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.state, AssetState::Failed);
        assert!(failed.failure_reason.is_some());
    }

    #[tokio::test]
    async fn finalize_presigned_upload_fails_by_name_on_a_size_mismatch() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();

        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(100, compute_sha256(b"whatever, size is checked first")),
        )
        .await
        .unwrap();
        let pending = store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .unwrap();

        // The client uploaded *something*, but not 100 bytes of it.
        objects
            .put(
                ObjectKey::new(pending.id),
                bytes::Bytes::from_static(b"short"),
            )
            .await
            .unwrap();

        let err = finalize_presigned_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::UnprocessableEntity(_)));
        let failed = store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.state, AssetState::Failed);
    }

    #[tokio::test]
    async fn finalize_presigned_upload_fails_by_name_on_a_reported_digest_mismatch() {
        let store = InMemoryAssetRecordStore::default();
        // `with_checksum_reporting`: this fake models a store that DOES
        // report `x-amz-checksum-sha256` on `HEAD` — the store computes it
        // from whatever bytes were actually stored, so a declared digest
        // that doesn't match those bytes is a genuine mismatch.
        let objects = InMemoryObjectStore::with_checksum_reporting();
        let collection = collection();
        let policy = open_policy();

        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(9, compute_sha256(b"expected!")),
        )
        .await
        .unwrap();
        let pending = store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .unwrap();

        // Same length (9 bytes) as declared, so the size check alone
        // wouldn't catch this — only the digest check does.
        objects
            .put(
                ObjectKey::new(pending.id),
                bytes::Bytes::from_static(b"different"),
            )
            .await
            .unwrap();

        let err = finalize_presigned_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::UnprocessableEntity(_)));
        let failed = store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.state, AssetState::Failed);
    }

    #[tokio::test]
    async fn finalize_presigned_upload_skips_the_digest_check_when_the_store_never_reports_one() {
        let store = InMemoryAssetRecordStore::default();
        // Default: no checksum reporting — the common case per
        // `ObjectMetadata`'s own doc. A declared digest that doesn't match
        // the actual bytes must NOT fail finalize here (there is nothing to
        // compare it against), only the size check runs.
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let payload = b"nine-byte".to_vec();
        assert_eq!(payload.len(), 9);

        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(9, compute_sha256(b"expected!")), // deliberately wrong digest
        )
        .await
        .unwrap();
        let pending = store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .unwrap();
        objects
            .put(ObjectKey::new(pending.id), bytes::Bytes::from(payload))
            .await
            .unwrap();

        let available = finalize_presigned_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        assert_eq!(available.state, AssetState::Available);
    }

    #[tokio::test]
    async fn presign_upload_refuses_a_key_that_is_not_pending() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let payload = b"bytes".to_vec();
        let digest = compute_sha256(&payload);

        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(payload.len() as u64, digest),
        )
        .await
        .unwrap();
        complete_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            bytes::Bytes::from(payload),
        )
        .await
        .unwrap();

        let err = presign_upload(&store, &objects, &collection, None, "thumb", fixed_now())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)));
    }

    #[tokio::test]
    async fn presign_upload_for_an_unregistered_key_is_not_found() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let err = presign_upload(&store, &objects, &collection, None, "nope", fixed_now())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound));
    }

    #[tokio::test]
    async fn finalize_presigned_upload_on_an_already_available_asset_is_a_named_conflict() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::with_checksum_reporting();
        let collection = collection();
        let policy = open_policy();
        let payload = b"bytes".to_vec();
        let digest = compute_sha256(&payload);

        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(payload.len() as u64, digest),
        )
        .await
        .unwrap();
        let pending = store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .unwrap();
        objects
            .put(ObjectKey::new(pending.id), bytes::Bytes::from(payload))
            .await
            .unwrap();
        finalize_presigned_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();

        let err = finalize_presigned_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)));
    }

    // -- resumable upload: register -> create -> append* -> complete ------

    #[tokio::test]
    async fn resumable_round_trip_register_create_append_complete_available() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let payload = b"resumable upload bytes".to_vec();
        let digest = compute_sha256(&payload);

        let pending = register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(payload.len() as u64, digest),
        )
        .await
        .unwrap();
        assert_eq!(pending.state, AssetState::Pending);

        create_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        assert_eq!(
            resumable_upload_offset(&store, &objects, &collection, None, "thumb")
                .await
                .unwrap(),
            0
        );

        let first_chunk = &payload[..10];
        let offset = append_resumable_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::copy_from_slice(first_chunk),
        )
        .await
        .unwrap();
        assert_eq!(offset, 10);

        let second_chunk = &payload[10..];
        let offset = append_resumable_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            10,
            bytes::Bytes::copy_from_slice(second_chunk),
        )
        .await
        .unwrap();
        assert_eq!(offset, payload.len() as u64);
        assert_eq!(
            resumable_upload_offset(&store, &objects, &collection, None, "thumb")
                .await
                .unwrap(),
            payload.len() as u64
        );

        let available = complete_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        assert_eq!(available.state, AssetState::Available);
        assert_eq!(available.id, pending.id);

        let stored = objects.get(ObjectKey::new(available.id)).await.unwrap();
        assert_eq!(stored.unwrap(), bytes::Bytes::from(payload));

        // The asset is no longer pending (it's `available`) — probing its
        // upload resource, which completion already consumed, is a named
        // refusal rather than a fresh upload's "nothing here yet".
        let err = resumable_upload_offset(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)));
    }

    #[tokio::test]
    async fn probing_the_offset_of_an_upload_that_was_never_created_is_not_found() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(5, compute_sha256(b"12345")),
        )
        .await
        .unwrap();

        let err = resumable_upload_offset(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound));
    }

    #[tokio::test]
    async fn a_second_create_while_one_is_in_progress_is_a_named_conflict() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(5, compute_sha256(b"12345")),
        )
        .await
        .unwrap();

        create_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        let err = create_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)));
    }

    #[tokio::test]
    async fn out_of_order_append_is_refused_by_name() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(20, compute_sha256(b"whatever twenty long")),
        )
        .await
        .unwrap();
        create_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        append_resumable_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::from_static(b"abc"),
        )
        .await
        .unwrap();

        // Declares an offset past what has actually accumulated (3) — a gap.
        let err = append_resumable_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            10,
            bytes::Bytes::from_static(b"x"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Conflict(msg) if msg.contains("out-of-order")));
    }

    #[tokio::test]
    async fn stale_offset_append_is_refused_by_name() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(20, compute_sha256(b"whatever twenty long")),
        )
        .await
        .unwrap();
        create_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        append_resumable_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::from_static(b"abc"),
        )
        .await
        .unwrap();

        // Retries offset 0 after the server already accumulated 3 bytes —
        // stale, not a gap.
        let err = append_resumable_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::from_static(b"x"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Conflict(msg) if msg.contains("stale")));
    }

    #[tokio::test]
    async fn a_declared_total_over_the_cap_is_refused_up_front_at_registration() {
        let store = InMemoryAssetRecordStore::default();
        let collection = collection();
        let policy = AssetPolicy {
            max_asset_bytes: 10,
            allowed_media_types: None,
        };
        let request = managed_request(11, compute_sha256(b"a"));

        let err = register_managed(&store, &policy, &collection, None, "big", request)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::PayloadTooLarge { limit: 10 }));
    }

    #[tokio::test]
    async fn an_append_that_would_exceed_the_declared_total_is_refused_before_it_lands() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(5, compute_sha256(b"12345")),
        )
        .await
        .unwrap();
        create_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        append_resumable_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::from_static(b"123"),
        )
        .await
        .unwrap();

        // 3 bytes already accumulated + a 5-byte chunk would total 8,
        // past the 5-byte declared size.
        let err = append_resumable_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            3,
            bytes::Bytes::from_static(b"45678"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::PayloadTooLarge { limit: 5 }));

        // Never buffered past the cap: the offset is exactly what it was
        // before the refused append.
        assert_eq!(
            resumable_upload_offset(&store, &objects, &collection, None, "thumb")
                .await
                .unwrap(),
            3
        );
    }

    #[tokio::test]
    async fn deleting_an_incomplete_upload_lets_a_fresh_one_start_clean() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let payload = b"final bytes".to_vec();
        let digest = compute_sha256(&payload);
        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(payload.len() as u64, digest.clone()),
        )
        .await
        .unwrap();

        create_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        append_resumable_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::from_static(b"stale junk!"),
        )
        .await
        .unwrap();

        abandon_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        // Idempotent — abandoning again (nothing left) is still `Ok`.
        abandon_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        let err = resumable_upload_offset(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound));

        // The asset itself is untouched — still pending — so a fresh upload
        // on the same key starts clean.
        create_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        assert_eq!(
            resumable_upload_offset(&store, &objects, &collection, None, "thumb")
                .await
                .unwrap(),
            0
        );
        append_resumable_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::copy_from_slice(&payload),
        )
        .await
        .unwrap();
        let available = complete_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        assert_eq!(available.state, AssetState::Available);
    }

    #[tokio::test]
    async fn a_digest_mismatch_at_complete_fails_the_asset_by_name() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let declared_digest = compute_sha256(b"expected bytes here");
        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(20, declared_digest),
        )
        .await
        .unwrap();

        create_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        append_resumable_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::from_static(b"totally different!!!"),
        )
        .await
        .unwrap();

        let err = complete_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::UnprocessableEntity(_)));

        let failed = store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.state, AssetState::Failed);
        assert!(failed.failure_reason.is_some());
        // A digest mismatch never writes the object, matching
        // `complete_upload`'s own direct-upload contract.
        assert!(objects
            .get(ObjectKey::new(failed.id))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn completing_with_no_upload_in_progress_is_not_found() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(5, compute_sha256(b"12345")),
        )
        .await
        .unwrap();

        let err = complete_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound));
    }

    #[tokio::test]
    async fn creating_an_upload_for_a_non_pending_asset_is_a_named_conflict() {
        let store = InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        let collection = collection();
        let policy = open_policy();
        let payload = b"bytes".to_vec();
        let digest = compute_sha256(&payload);
        register_managed(
            &store,
            &policy,
            &collection,
            None,
            "thumb",
            managed_request(payload.len() as u64, digest),
        )
        .await
        .unwrap();
        complete_upload(
            &store,
            &objects,
            &collection,
            None,
            "thumb",
            bytes::Bytes::from(payload),
        )
        .await
        .unwrap();

        let err = create_resumable_upload(&store, &objects, &collection, None, "thumb")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)));
    }
}
