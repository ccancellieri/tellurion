//! The object-store PORT (assets-and-object-storage proposal) and its
//! ADAPTERS: [`FsObjectStore`] (first slice: `core` + `managed-storage` +
//! `direct-upload` + `checksum` + `object-store-profile: fs`) and
//! [`S3ObjectStore`] (second slice: `object-store-profile: s3` +
//! `presigned-upload` — the plain HTTP protocol any S3-compatible store
//! speaks, MinIO/Ceph/R2 included, signed with hand-rolled AWS Signature
//! Version 4 (`sigv4.rs`), never a vendor SDK).
//!
//! [`ObjectStore`] covers put/get/delete/exists/head whole-object I/O.
//! [`ResumableUploadStore`] (third slice: `resumable-upload`) extends it
//! with a chunked-append lane for a pending managed asset's bytes,
//! implemented for both shipped profiles: [`FsObjectStore`] appends to a
//! real file on disk; [`S3ObjectStore`] (this slice) backs it with a real
//! S3 multipart upload — `CreateMultipartUpload`/`UploadPart`/
//! `CompleteMultipartUpload`/`AbortMultipartUpload`, signed with the same
//! SigV4 signer every other `s3` verb uses — rather than refusing the class
//! by name the way the second slice's presigned-upload-only `s3` profile
//! did. See [`ResumableUploadStore`]'s own doc for the shared contract and
//! each `impl` block's own doc for the profile-specific mechanics. A
//! managed asset's bytes live under an [`ObjectKey`], which
//! can only ever be built from an already-generated [`Uuid`] — the asset's
//! own immutable internal id (`asset.rs`), never a client-supplied filename
//! or asset key. That is what makes path traversal impossible by
//! construction on the filesystem profile: [`FsObjectStore::object_path`]
//! joins the store's root with a [`Uuid`]'s canonical hyphenated form, a
//! string that can never contain a path separator or a `..` segment.
//! [`ObjectKey::from_raw`] exists only so this module's own tests can prove
//! that even a second, defense-in-depth check inside `object_path` would
//! reject a hostile string — production code never calls it.
//!
//! Presigned URLs ([`PresignedObjectStore`]) are a strict extension only
//! `s3` can satisfy: `fs` has no URL space of its own to mint a signed URL
//! against, so [`ObjectStore::as_presigned`] defaults to `None` and only
//! [`S3ObjectStore`] overrides it — a caller resolves the capability by
//! calling that method on whatever `Arc<dyn ObjectStore>` `Router::
//! resolve_object_store` already handed it, the same borrowed-capability
//! shape `StorageDriver`'s own `asset_record_store`/`write_sink` accessors
//! use, refusing by name (never a downcast/`Any` probe) when the resolved
//! profile doesn't have it.
//!
//! [`ListableObjectStore`] (the reconcile surface's own list primitive) and
//! [`ResumableUploadStore`] are both implemented by BOTH shipped profiles as
//! of this slice — `fs` walks its one flat directory / appends to a real
//! file, `s3` speaks `ListObjectsV2` under this store's own `key_prefix` /
//! drives a real multipart upload. Presigned URLs
//! ([`PresignedObjectStore`]) stay the odd one out, a strict extension only
//! `s3` can satisfy (`fs` has no URL space of its own to mint a signed URL
//! against). `ObjectStore::as_listable`/`as_resumable`'s `None` defaults are
//! still there for a future profile that genuinely lacks one of these
//! capabilities, the identical refuse-by-name shape [`ObjectStore::
//! as_presigned`]'s own `fs`-side refusal already establishes.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use uuid::Uuid;

use crate::sigv4;

/// Errors this port's adapters can raise. Deliberately small and
/// framework-free — `tellurion-stac`'s asset handlers map these onto HTTP
/// status the same way they map `tellurion_core::Error`.
#[derive(Debug, thiserror::Error)]
pub enum ObjectStoreError {
    #[error("object store I/O error: {0}")]
    Io(#[source] std::io::Error),
    /// [`ObjectKey::from_raw`] was handed a string that cannot safely become
    /// a single path segment — only reachable through that constructor
    /// (never through the normal [`Uuid`]-derived path), see this module's
    /// own doc.
    #[error("'{0}' is not a valid object key")]
    InvalidKey(String),
    /// `s3` profile only: the store rejected the request's credentials
    /// (HTTP 403) — the message is this fixed string, never the
    /// credentials, the signature, or any response body the store sent
    /// back (`asset_handlers.rs`'s `Error::Storage` mapping already logs
    /// this server-side and returns a generic 500 to the client, so nothing
    /// here ever reaches a caller either way).
    #[error("storage credentials rejected")]
    CredentialsRejected,
    /// `s3` profile only: any HTTP response this module doesn't otherwise
    /// map (not 2xx, not the per-call 404 case, not 403) — carries only the
    /// status code, never the response body.
    #[error("object store returned HTTP {status}")]
    Storage { status: u16 },
    /// `s3` profile only: the HTTP request itself never got a response
    /// (connection refused, TLS failure, timeout, DNS).
    #[error("object store request failed: {0}")]
    Http(#[source] reqwest::Error),

    /// `resumable-upload` conformance class only: `upload_offset`/
    /// `append_upload`/`take_upload` was called for a key with no live
    /// upload resource — never created, or already consumed by a prior
    /// `take_upload`/`abandon_upload`. Distinct from a plain missing-file
    /// `Io` error so `asset.rs` can map it straight to `Error::NotFound`
    /// without inspecting an `io::ErrorKind`.
    #[error("no resumable upload is in progress for this object")]
    UploadNotFound,

    /// `resumable-upload` conformance class only: [`ResumableUploadStore::
    /// append_upload`]'s own compare-and-append guard — the caller's
    /// declared offset does not match the number of bytes actually
    /// accumulated so far. Carries both values so `asset.rs` can name the
    /// direction (stale vs. out-of-order) in the refusal it builds.
    #[error("upload offset {expected} does not match the {actual} bytes already accumulated")]
    UploadOffsetMismatch { expected: u64, actual: u64 },

    /// `s3` profile only, `resumable-upload` conformance class: a
    /// `CreateMultipartUpload`/`CompleteMultipartUpload` response this store
    /// expected a specific XML element in did not contain one — the HTTP
    /// call itself succeeded (2xx), but this store's own hand-rolled parser
    /// (the same `extract_first`/`extract_all` idiom
    /// [`ListableObjectStore::list_all`] already uses for `ListObjectsV2`)
    /// found nothing to extract. Distinct from [`Self::Storage`], which
    /// covers a non-2xx status; this is a 2xx response shaped unlike what
    /// this store's own parser understands.
    #[error("object store returned a response this store's own parser could not read: {0}")]
    MultipartResponseMalformed(String),
}

pub type Result<T> = std::result::Result<T, ObjectStoreError>;

/// A HEAD probe's result — only the fields the store actually reports
/// populated; `fs` (`FsObjectStore::head`) only ever fills `size` (a plain
/// `stat`, no checksum concept); `s3` (`S3ObjectStore::head`) fills `size`
/// from `Content-Length` always, and `sha256` from an `x-amz-checksum-
/// sha256` response header only when the store reports one — S3's
/// additional-checksums feature is opt-in per upload, so this is `None` far
/// more often than not, and [`crate::asset::finalize_presigned_upload`]
/// treats a `None` digest as "nothing to check", never as a mismatch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectMetadata {
    pub size: Option<u64>,
    pub sha256: Option<[u8; 32]>,
}

/// A managed asset's object-store key: the asset's own internal [`Uuid`],
/// nothing else. Immutable and server-generated — see this module's own doc
/// for why that is the path-traversal invariant, not a filter applied to
/// client input.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectKey(ObjectKeyRepr);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ObjectKeyRepr {
    Id(Uuid),
    /// Only [`ObjectKey::from_raw`] (test-only) ever produces this variant —
    /// see that constructor's own doc.
    #[cfg(test)]
    Raw(String),
}

impl ObjectKey {
    pub fn new(id: Uuid) -> Self {
        Self(ObjectKeyRepr::Id(id))
    }

    /// `None` for a test-only raw key (see [`ObjectKey::from_raw`]) — a real
    /// `ObjectKey` (every one production code ever builds) always has one.
    pub fn id(&self) -> Option<Uuid> {
        match &self.0 {
            ObjectKeyRepr::Id(id) => Some(*id),
            #[cfg(test)]
            ObjectKeyRepr::Raw(_) => None,
        }
    }

    /// Test-only escape hatch: builds an `ObjectKey` from an arbitrary
    /// string rather than a real `Uuid`, so a test can hand a hostile
    /// string (`"../../etc/passwd"`, an absolute path, ...) to
    /// [`FsObjectStore::object_path`] and prove it is rejected rather than
    /// resolved outside the store's root. Never called by production code —
    /// every real `ObjectKey` is built from [`ObjectKey::new`] with a
    /// freshly generated `Uuid`.
    #[cfg(test)]
    fn from_raw(raw: &str) -> Self {
        Self(ObjectKeyRepr::Raw(raw.to_string()))
    }
}

/// The single path segment `FsObjectStore` writes an object under —
/// [`Uuid::hyphenated`] for a real key, always exactly 36 characters from
/// `{0-9a-f-}`, which can never contain `/`, `\`, or a `..` sequence. Only
/// this module's own `#[cfg(test)]` escape hatch ([`ObjectKey::from_raw`])
/// can make this diverge from that guarantee — a defense-in-depth check in
/// `FsObjectStore::object_path` rejects the divergent case outright rather
/// than trusting the string.
fn path_segment(key: &ObjectKey) -> String {
    match &key.0 {
        ObjectKeyRepr::Id(id) => id.hyphenated().to_string(),
        #[cfg(test)]
        ObjectKeyRepr::Raw(raw) => raw.clone(),
    }
}

/// What an object store must do: put/get whole bytes, delete (idempotent —
/// deleting an already-absent key is `Ok(())`, never a distinct error,
/// matching `WriteSink`-style capability contracts that treat "already in
/// the target state" as success), exists, and head (existence plus
/// whatever metadata the store can report without transferring the object's
/// bytes — [`crate::asset::finalize_presigned_upload`]'s own verification
/// step). No streaming-multipart or resumable surface — those are later
/// slices per the proposal's own scoping.
#[async_trait::async_trait]
pub trait ObjectStore: Send + Sync {
    async fn put(&self, key: ObjectKey, bytes: bytes::Bytes) -> Result<()>;
    async fn get(&self, key: ObjectKey) -> Result<Option<bytes::Bytes>>;
    async fn delete(&self, key: ObjectKey) -> Result<()>;
    async fn exists(&self, key: ObjectKey) -> Result<bool>;
    async fn head(&self, key: ObjectKey) -> Result<Option<ObjectMetadata>>;

    /// Advertises this store's presigned-URL capability (the
    /// `presigned-upload` conformance class) — `None` (the default) for
    /// every profile with no URL space of its own to mint a signed URL
    /// against; only [`S3ObjectStore`] overrides it. See this module's own
    /// doc for why this is a borrowed capability accessor, not an
    /// `Arc`-cloning one.
    fn as_presigned(&self) -> Option<&dyn PresignedObjectStore> {
        None
    }

    /// Advertises this store's resumable-upload capability (the
    /// `resumable-upload` conformance class) — `None` (the default) for
    /// every profile that does not implement [`ResumableUploadStore`]; both
    /// [`FsObjectStore`] and [`S3ObjectStore`] override it as of this
    /// slice. Same borrowed-capability, refuse-by-name shape
    /// [`as_presigned`](Self::as_presigned) already establishes, for a
    /// future profile that genuinely cannot resume.
    fn as_resumable(&self) -> Option<&dyn ResumableUploadStore> {
        None
    }

    /// Advertises this store's listing capability (the reconcile surface) —
    /// `None` (the default) for a profile with no enumeration primitive;
    /// both profiles this slice ships override it — see
    /// [`ListableObjectStore`]'s own doc for why this one, unlike
    /// [`as_presigned`](Self::as_presigned)/[`as_resumable`](Self::as_resumable),
    /// isn't split by profile.
    fn as_listable(&self) -> Option<&dyn ListableObjectStore> {
        None
    }

    /// Advertises this store's path-addressed READ capability — `None` (the
    /// default) for every profile that cannot safely resolve an arbitrary
    /// caller-supplied object path, which as of this slice is every profile
    /// but `s3`. See [`PathAddressedObjectStore`]'s own doc for why
    /// [`FsObjectStore`] deliberately stays `None` here rather than growing
    /// a nested-path resolver. Same borrowed-capability, refuse-by-name
    /// shape [`as_presigned`](Self::as_presigned) already establishes.
    fn as_path_addressed(&self) -> Option<&dyn PathAddressedObjectStore> {
        None
    }
}

/// The presigned-URL capability (`presigned-upload` conformance class) —
/// a strict extension of [`ObjectStore`] only the `s3` profile implements.
/// Pure computation, no I/O: both methods take `now` as an argument rather
/// than reading a clock themselves, the same clock-injection rule
/// `sigv4.rs`'s own signer follows (production passes `SystemTime::now()`;
/// tests pass a fixed instant so the resulting URL is reproducible).
pub trait PresignedObjectStore: ObjectStore {
    /// A time-limited signed URL a client can `GET` directly against the
    /// store to download `key`'s bytes — the download half of the
    /// negotiation the proposal's presigned transport describes.
    fn presign_get(&self, key: ObjectKey, expires_in: Duration, now: SystemTime) -> Result<String>;

    /// A time-limited signed URL a client can `PUT` bytes directly against
    /// the store — the upload half, minted against a `pending` managed
    /// asset's own key (`asset::presign_upload`).
    fn presign_put(&self, key: ObjectKey, expires_in: Duration, now: SystemTime) -> Result<String>;

    /// This store's own configured presigned-URL lifetime
    /// (`config::ObjectStoreProfile::S3.presign_expiry_s`) — callers pass
    /// this straight to [`presign_get`](Self::presign_get)/
    /// [`presign_put`](Self::presign_put) rather than inventing their own
    /// default, so the expiry advertised to a client always matches this
    /// deployment's own configuration.
    fn default_expiry(&self) -> Duration;
}

/// The resumable-upload capability (`resumable-upload` conformance class) —
/// a strict extension of [`ObjectStore`], implemented by both shipped
/// profiles: [`FsObjectStore`] appends straight to a real file on disk,
/// guarded by a single coarse lock per store instance so two concurrent
/// operations against the SAME upload can never interleave (a lock per
/// upload id would let unrelated uploads proceed concurrently; this slice
/// trades that throughput for a much smaller, obviously-correct
/// implementation). [`S3ObjectStore`] backs the identical trait with a real
/// S3 multipart upload — see that `impl` block's own doc for the semantic
/// mismatches this profile must bridge to do it (S3's own 5&nbsp;MiB part
/// floor vs. this trait's arbitrary-chunk-size contract, and a
/// verify-before-commit ordering `fs` gets for free but `s3` cannot) and
/// the lock discipline it borrows from `fs`'s own choice above.
///
/// Every method is keyed by the SAME [`ObjectKey`] the eventual completed
/// object lives at — an in-progress upload can never collide with it
/// ([`FsObjectStore`]'s own `.upload`-suffixed path; [`S3ObjectStore`]'s own
/// in-memory upload-id bookkeeping, keyed the same way). The accumulated
/// bytes are pulled back out whole via [`take_upload`](Self::take_upload),
/// which `crate::asset::complete_resumable_upload` hands straight to
/// `crate::asset::complete_upload` for the same digest/cap verification
/// every other transport already uses — never duplicated for this one.
/// On `fs` that verification runs strictly before the object exists at its
/// final key at all (`take_upload` only ever reads a `.upload`-suffixed
/// staging file). `s3` cannot offer that ordering — see [`S3ObjectStore`]'s
/// own `impl` block doc for why, and for how `complete_upload`'s own
/// digest-mismatch branch closes the resulting gap rather than leaving it
/// open.
#[async_trait::async_trait]
pub trait ResumableUploadStore: ObjectStore {
    /// Creates a fresh, empty upload resource for `key`, overwriting any
    /// prior state at that path unconditionally — mechanical, like
    /// [`ObjectStore::put`]. The "an upload is already in progress" refusal
    /// is `asset::create_resumable_upload`'s own check, made by probing
    /// [`upload_offset`](Self::upload_offset) first.
    async fn create_upload(&self, key: ObjectKey) -> Result<()>;

    /// The number of bytes accumulated so far, or `None` when no upload
    /// resource exists for `key` — never created, or already consumed by
    /// [`take_upload`](Self::take_upload)/[`abandon_upload`](Self::abandon_upload).
    async fn upload_offset(&self, key: ObjectKey) -> Result<Option<u64>>;

    /// Appends `bytes` at `expected_offset`, checked atomically against the
    /// upload's actual current length under this store's own lock — the
    /// concurrency guard: a caller whose `expected_offset` no longer matches
    /// gets [`ObjectStoreError::UploadOffsetMismatch`], never a silent
    /// interleave. [`ObjectStoreError::UploadNotFound`] when no upload
    /// resource exists for `key` at all. Returns the new accumulated length
    /// on success.
    async fn append_upload(
        &self,
        key: ObjectKey,
        expected_offset: u64,
        bytes: bytes::Bytes,
    ) -> Result<u64>;

    /// Reads back every byte accumulated so far and removes the upload
    /// resource in the same step — `None` when no upload resource exists.
    /// [`crate::asset::complete_resumable_upload`] calls this exactly once,
    /// consuming the upload whether the digest check that follows passes or
    /// not.
    async fn take_upload(&self, key: ObjectKey) -> Result<Option<bytes::Bytes>>;

    /// Discards an in-progress upload without completing it — idempotent,
    /// the same "already-absent is a successful no-op" contract
    /// [`ObjectStore::delete`] uses.
    async fn abandon_upload(&self, key: ObjectKey) -> Result<()>;

    /// Whether [`take_upload`](Self::take_upload) already wrote its
    /// returned bytes to `key`'s own final object path — `false` by
    /// default, which is exactly right for [`FsObjectStore`]: its own
    /// `take_upload` only ever reads a separate `.upload`-suffixed staging
    /// file, so the real key is still untouched by the time this returns.
    /// [`S3ObjectStore`] overrides this to `true` — see that `impl`
    /// block's own doc for why completing its multipart upload IS what
    /// makes the assembled bytes exist at the real key at all, before this
    /// trait's own caller ever gets a chance to look at them.
    ///
    /// [`crate::asset::complete_resumable_upload`] reads this right after
    /// `take_upload` returns, to decide whether [`crate::asset::
    /// complete_upload`]'s own whole-object `put` would be a correct write
    /// (`fs`) or a wasted — and, past S3's 5&nbsp;GiB single-request PUT
    /// cap, outright failing — re-transfer of bytes already sitting at
    /// that exact key (`s3`). Pure and synchronous on purpose: a fixed
    /// property of the profile, never a network round trip of its own.
    fn take_upload_already_committed(&self) -> bool {
        false
    }

    /// Releases the "bytes already committed, not yet verified" hold that
    /// [`take_upload`](Self::take_upload) leaves at `key` whenever
    /// [`take_upload_already_committed`](Self::take_upload_already_committed)
    /// is `true`. Until this runs, [`upload_offset`](Self::upload_offset)
    /// keeps reporting `key` as in-progress, so
    /// `crate::asset::create_resumable_upload`'s own "already in progress"
    /// admission check keeps refusing a second attempt at the same key —
    /// which is exactly the property that closes the gap
    /// [`take_upload`](Self::take_upload)'s own doc describes: a second,
    /// legitimate attempt admitted into the window between this store
    /// committing one attempt's bytes and the caller finishing its own
    /// digest check on them could have its correct bytes destroyed by the
    /// first attempt's own cleanup.
    ///
    /// `crate::asset::complete_resumable_upload` calls this exactly once,
    /// unconditionally, immediately after the digest-verify-and-maybe-
    /// delete step (`crate::asset::finish_upload`) returns — regardless of
    /// whether that step found the digest matched, found a mismatch and
    /// deleted the object, or failed outright. It is structured that way
    /// specifically so this call can never be skipped by an early `?`
    /// return inside that step: there is only the one call site, sitting
    /// after the single `.await` on the whole step, not threaded through
    /// each of that step's own exit paths.
    ///
    /// A no-op by default — exactly right for [`FsObjectStore`], whose own
    /// `take_upload` never leaves anything behind to release in the first
    /// place (`take_upload_already_committed`'s own default `false`).
    /// [`S3ObjectStore`] overrides this to actually clear its hold.
    async fn release_verifying_upload(&self, key: ObjectKey) -> Result<()> {
        let _ = key;
        Ok(())
    }
}

/// One entry [`ListableObjectStore::list_all`] reports back — the reconcile
/// surface's own "look at everything actually there" primitive
/// (`crate::reconcile`). `raw_name` is the object's bare path segment,
/// deliberately unvalidated: the whole point of listing is to see what is
/// REALLY present in the store's managed prefix, including garbage a
/// hostile client or a half-finished write left behind, never filtered
/// through [`ObjectKey`]'s own construction rules the way every other verb
/// on [`ObjectStore`] is. `id` is `Some` only when `raw_name` (with the
/// `.upload` suffix stripped, when `is_staging`) parses as a [`Uuid`] — the
/// shape every object this deployment itself ever wrote has; `None` covers
/// anything else found there, which `crate::reconcile` still reports (an
/// orphan with no recoverable identity is still an orphan).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedObject {
    pub raw_name: String,
    pub id: Option<Uuid>,
    /// `true` for an `fs`-profile resumable-upload staging file
    /// ([`FsObjectStore::upload_path`]'s own `.upload` suffix) — always
    /// `false` on the `s3` profile: [`S3ObjectStore`]'s own
    /// [`ResumableUploadStore`] implementation keeps its in-progress state
    /// as S3 multipart-upload parts, which `ListObjectsV2` (this trait's
    /// own `s3` implementation) never enumerates at all — they simply don't
    /// exist as listable objects until `CompleteMultipartUpload` lands them
    /// at their own final key, at which point they are indistinguishable
    /// from any other completed object. There is no separate S3-side
    /// staging key the way `fs`'s `.upload` suffix is one.
    pub is_staging: bool,
}

/// The listing capability (assets-and-object-storage proposal, reconcile
/// surface) — a strict extension of [`ObjectStore`], implemented by both
/// shipped profiles (see this module's own doc for why, unlike
/// [`PresignedObjectStore`]/[`ResumableUploadStore`], it isn't split by
/// profile). `crate::reconcile`'s own caller (`tellurion-stac::
/// asset_handlers::get_reconcile_report`) resolves this the same
/// borrowed-capability, refuse-by-name way those two traits already
/// establish, for a future profile that genuinely cannot list.
#[async_trait::async_trait]
pub trait ListableObjectStore: ObjectStore {
    /// Every entry currently present in this store's own managed
    /// namespace — unbounded by any cap this deployment applies to a
    /// single asset's bytes (a listing is metadata, not a byte transfer).
    async fn list_all(&self) -> Result<Vec<ListedObject>>;
}

/// The path-addressed READ capability — a strict extension of
/// [`ObjectStore`] for a caller that already holds a full, externally
/// minted object path and needs to read it, rather than an
/// [`ObjectKey`] this deployment itself generated.
///
/// This exists for exactly one caller: `tellurion-iceberg`'s `FileIO`
/// layer. An Iceberg table's own metadata names every manifest and data
/// file by absolute URI (`s3://bucket/warehouse/db/tbl/data/…parquet`),
/// laid down by whatever engine wrote the table. Those keys are nested,
/// arbitrarily deep, and emphatically not [`Uuid`]s — so they cannot go
/// through [`ObjectStore::get`], whose whole key space is "one `Uuid`,
/// generated here" (this module's own path-traversal invariant). Rather
/// than stand up a second S3 client beside this one, that driver borrows
/// this capability off the same [`S3ObjectStore`], and therefore the same
/// hand-rolled SigV4 signer (`sigv4.rs`), the same error mapping, and the
/// same `reqwest` client every other `s3` verb already uses.
///
/// READ ONLY, deliberately. There is no `put_path`/`delete_path` and there
/// will not be one: the only consumer is a read-only driver, and a
/// write verb over an arbitrary caller-supplied path would hand away
/// exactly the invariant [`ObjectKey`] exists to enforce. Ingest owns
/// physical layout; nothing on the serving path writes an object at a
/// path it did not generate.
///
/// [`ObjectStore::as_path_addressed`] is the accessor, `None` by default —
/// [`FsObjectStore`] does NOT implement this and must not: its key space is
/// one flat directory of [`Uuid`]-named files, and resolving a nested
/// caller-supplied path underneath its root is precisely the traversal this
/// module refuses to allow. A local-filesystem Iceberg table is served by
/// `iceberg`'s own `LocalFsStorageFactory` instead, never through here.
#[async_trait::async_trait]
pub trait PathAddressedObjectStore: ObjectStore {
    /// Whole-object read at `path` (the key inside this store's bucket, no
    /// leading `/`). `Ok(None)` for a key the store reports absent — same
    /// shape [`ObjectStore::get`] uses, never an error.
    async fn get_path(&self, path: &str) -> Result<Option<bytes::Bytes>>;

    /// Byte-range read at `path`: `offset` and `length` become one HTTP
    /// `Range: bytes=<offset>-<offset+length-1>` header. Iceberg's Parquet
    /// reader issues these constantly (footer, then per-column-chunk), so
    /// this — not [`Self::get_path`] — is the hot verb. A `length` of `0`
    /// reads nothing and returns empty bytes without issuing a request:
    /// there is no such thing as an empty HTTP byte range, and asking the
    /// store for one is how you get a whole-object read by accident.
    async fn get_path_range(
        &self,
        path: &str,
        offset: u64,
        length: u64,
    ) -> Result<Option<bytes::Bytes>>;

    /// Existence plus whatever metadata the store reports without
    /// transferring bytes — the [`ObjectStore::head`] contract, at an
    /// arbitrary path.
    async fn head_path(&self, path: &str) -> Result<Option<ObjectMetadata>>;
}

/// The `fs` object-store profile: one flat directory, one file per object,
/// named after its [`ObjectKey`]'s [`path_segment`]. No sub-directory
/// nesting — a `Uuid` v4 has no meaningful prefix to shard on, and this
/// slice's scale (a single filesystem, not a multi-million-object bucket)
/// doesn't need it.
pub struct FsObjectStore {
    root: PathBuf,
    /// Serializes every [`ResumableUploadStore`] operation on this store
    /// instance (see that trait's own doc for why one coarse lock rather
    /// than one per upload id) — irrelevant to plain
    /// put/get/delete/exists/head, which never touch it.
    uploads: tokio::sync::Mutex<()>,
}

impl FsObjectStore {
    /// `root` must already exist and be a writable directory — checked once
    /// here (a named, actionable startup failure) rather than surfacing as
    /// a confusing per-request I/O error the first time a caller uploads.
    pub fn new(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        let metadata = std::fs::metadata(&root)?;
        if !metadata.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("object store root '{}' is not a directory", root.display()),
            ));
        }
        Ok(Self {
            root,
            uploads: tokio::sync::Mutex::new(()),
        })
    }

    /// Joins `key`'s single path segment onto `root`, then verifies the
    /// result is still lexically inside `root` — defense in depth beyond
    /// the "always a bare `Uuid` segment" invariant [`ObjectKey`] already
    /// gives every real caller (see this module's own doc), and the reason
    /// [`ObjectKey::from_raw`] exists at all: a test can prove this check
    /// independently of whether `ObjectKey`'s own constructor would ever
    /// let a hostile string through in the first place.
    fn object_path(&self, key: &ObjectKey) -> Result<PathBuf> {
        let segment = path_segment(key);
        if segment.is_empty()
            || segment.contains('/')
            || segment.contains('\\')
            || segment == "."
            || segment == ".."
        {
            return Err(ObjectStoreError::InvalidKey(segment));
        }
        let candidate = self.root.join(&segment);
        // Lexical containment check: `candidate`'s parent must be exactly
        // `root`. `Path::join` with a single traversal-free segment already
        // guarantees this given the character check above, but this stays
        // as an explicit assertion rather than trusting that reasoning
        // silently — the whole point of "impossible by construction" is
        // that it is checked, not merely argued.
        if candidate.parent() != Some(self.root.as_path()) {
            return Err(ObjectStoreError::InvalidKey(segment));
        }
        Ok(candidate)
    }

    /// The resumable-upload staging path for `key`: [`Self::object_path`]'s
    /// own already-validated path with a `.upload` extension appended — a
    /// real object's filename is always a bare `Uuid` with no extension
    /// ([`path_segment`]'s own doc), so this can never collide with a
    /// completed object's own path.
    fn upload_path(&self, key: &ObjectKey) -> Result<PathBuf> {
        Ok(self.object_path(key)?.with_extension("upload"))
    }
}

#[async_trait::async_trait]
impl ObjectStore for FsObjectStore {
    async fn put(&self, key: ObjectKey, bytes: bytes::Bytes) -> Result<()> {
        let path = self.object_path(&key)?;
        tokio::fs::write(&path, bytes)
            .await
            .map_err(ObjectStoreError::Io)
    }

    async fn get(&self, key: ObjectKey) -> Result<Option<bytes::Bytes>> {
        let path = self.object_path(&key)?;
        match tokio::fs::read(&path).await {
            Ok(data) => Ok(Some(bytes::Bytes::from(data))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(ObjectStoreError::Io(err)),
        }
    }

    async fn delete(&self, key: ObjectKey) -> Result<()> {
        let path = self.object_path(&key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(ObjectStoreError::Io(err)),
        }
    }

    async fn exists(&self, key: ObjectKey) -> Result<bool> {
        let path = self.object_path(&key)?;
        Ok(tokio::fs::try_exists(&path).await.unwrap_or(false))
    }

    /// A plain `stat` — size only, no checksum concept on a local
    /// filesystem (`ObjectMetadata`'s own doc).
    async fn head(&self, key: ObjectKey) -> Result<Option<ObjectMetadata>> {
        let path = self.object_path(&key)?;
        match tokio::fs::metadata(&path).await {
            Ok(meta) => Ok(Some(ObjectMetadata {
                size: Some(meta.len()),
                sha256: None,
            })),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(ObjectStoreError::Io(err)),
        }
    }

    fn as_resumable(&self) -> Option<&dyn ResumableUploadStore> {
        Some(self)
    }

    fn as_listable(&self) -> Option<&dyn ListableObjectStore> {
        Some(self)
    }
}

#[async_trait::async_trait]
impl ListableObjectStore for FsObjectStore {
    /// One flat `read_dir` over the store's own root — see
    /// [`FsObjectStore`]'s own doc for why this profile needs no
    /// subdirectory sharding to walk. Both a completed object (a bare
    /// [`Uuid`] filename) and a resumable-upload staging file (the same
    /// [`Uuid`] with a `.upload` suffix, [`FsObjectStore::upload_path`])
    /// are reported — the reconcile surface's own requirement to see
    /// leftover staging debris, not just finished objects.
    async fn list_all(&self) -> Result<Vec<ListedObject>> {
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&self.root)
            .await
            .map_err(ObjectStoreError::Io)?;
        while let Some(entry) = dir.next_entry().await.map_err(ObjectStoreError::Io)? {
            let file_type = entry.file_type().await.map_err(ObjectStoreError::Io)?;
            if !file_type.is_file() {
                // This profile never creates a subdirectory under its own
                // root — a stray one is not this listing's concern, and
                // `raw_name`'s only real consumer (`crate::reconcile`) only
                // ever compares object identities, which a directory has
                // none of.
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let (stem, is_staging) = match name.strip_suffix(".upload") {
                Some(stem) => (stem.to_string(), true),
                None => (name.clone(), false),
            };
            entries.push(ListedObject {
                id: Uuid::parse_str(&stem).ok(),
                raw_name: name,
                is_staging,
            });
        }
        Ok(entries)
    }
}

#[async_trait::async_trait]
impl ResumableUploadStore for FsObjectStore {
    async fn create_upload(&self, key: ObjectKey) -> Result<()> {
        let path = self.upload_path(&key)?;
        let _guard = self.uploads.lock().await;
        tokio::fs::write(&path, [])
            .await
            .map_err(ObjectStoreError::Io)
    }

    async fn upload_offset(&self, key: ObjectKey) -> Result<Option<u64>> {
        let path = self.upload_path(&key)?;
        let _guard = self.uploads.lock().await;
        match tokio::fs::metadata(&path).await {
            Ok(meta) => Ok(Some(meta.len())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(ObjectStoreError::Io(err)),
        }
    }

    async fn append_upload(
        &self,
        key: ObjectKey,
        expected_offset: u64,
        bytes: bytes::Bytes,
    ) -> Result<u64> {
        use tokio::io::AsyncWriteExt as _;

        let path = self.upload_path(&key)?;
        let _guard = self.uploads.lock().await;
        let actual = match tokio::fs::metadata(&path).await {
            Ok(meta) => meta.len(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(ObjectStoreError::UploadNotFound);
            }
            Err(err) => return Err(ObjectStoreError::Io(err)),
        };
        if actual != expected_offset {
            return Err(ObjectStoreError::UploadOffsetMismatch {
                expected: expected_offset,
                actual,
            });
        }
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .map_err(ObjectStoreError::Io)?;
        let appended_len = bytes.len() as u64;
        file.write_all(&bytes).await.map_err(ObjectStoreError::Io)?;
        file.flush().await.map_err(ObjectStoreError::Io)?;
        Ok(actual + appended_len)
    }

    async fn take_upload(&self, key: ObjectKey) -> Result<Option<bytes::Bytes>> {
        let path = self.upload_path(&key)?;
        let _guard = self.uploads.lock().await;
        match tokio::fs::read(&path).await {
            Ok(data) => {
                tokio::fs::remove_file(&path)
                    .await
                    .map_err(ObjectStoreError::Io)?;
                Ok(Some(bytes::Bytes::from(data)))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(ObjectStoreError::Io(err)),
        }
    }

    async fn abandon_upload(&self, key: ObjectKey) -> Result<()> {
        let path = self.upload_path(&key)?;
        let _guard = self.uploads.lock().await;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(ObjectStoreError::Io(err)),
        }
    }
}

/// S3's own hard floor on a multipart-upload part: every part except the
/// last must be at least this many bytes. [`ResumableUploadStore::
/// append_upload`]'s own contract accepts arbitrary chunk sizes, so
/// [`S3ObjectStore`]'s implementation buffers appends in memory and only
/// flushes a real `UploadPart` once the buffer reaches this threshold — see
/// that `impl` block's own doc for the full accumulator design. Not a
/// config knob: this is AWS's own protocol minimum, not a deployment
/// choice, the same way `sigv4::UNSIGNED_PAYLOAD` isn't one either.
const S3_MULTIPART_PART_FLOOR: u64 = 5 * 1024 * 1024;

/// One in-progress s3 multipart upload's local bookkeeping
/// ([`S3ObjectStore::uploads`]) — unlike [`FsObjectStore`], which persists
/// the accumulated bytes to a real `.upload`-suffixed file and can read the
/// total straight back from `stat`, a multipart upload has nothing local to
/// stat: the bytes already flushed live in S3 itself, addressed only by
/// `upload_id` and part number, so this store must remember that mapping
/// itself. This bookkeeping is process-memory only, never persisted — see
/// [`S3ObjectStore::uploads`]'s own doc for the honest gap that implies.
struct S3UploadState {
    /// S3's own multipart-upload identifier, minted by
    /// `CreateMultipartUpload` and required on every subsequent
    /// `UploadPart`/`CompleteMultipartUpload`/`AbortMultipartUpload` call.
    upload_id: String,
    /// `(part_number, etag)` for every part already flushed to S3, in the
    /// order they were flushed (part numbers are always assigned
    /// sequentially from 1, so this is already sorted) — `etag` is the
    /// exact `ETag` response header value `UploadPart` returned (quotes
    /// included), which `CompleteMultipartUpload`'s own request body must
    /// echo back verbatim for S3 to accept it as that part's manifest
    /// entry.
    completed_parts: Vec<(i32, String)>,
    /// Bytes appended since the last flush — not yet a real S3 part. Never
    /// as large as [`S3_MULTIPART_PART_FLOOR`] once
    /// [`S3ObjectStore::append_upload`] returns: a full threshold's worth
    /// is always flushed immediately.
    buffer: Vec<u8>,
    /// Total bytes already flushed as completed parts. Added to
    /// `buffer.len()`, this is what [`ResumableUploadStore::upload_offset`]
    /// reports — computed purely from this local state, no network round
    /// trip, the same "cheap probe" contract `fs`'s own `stat`-based
    /// `upload_offset` already gives.
    flushed_len: u64,
}

/// One entry in [`S3ObjectStore::uploads`] — either a multipart upload
/// still accumulating appended bytes, or one whose bytes
/// [`ResumableUploadStore::take_upload`] has already committed at the
/// key's own final path but whose caller has not yet finished verifying
/// them (`ResumableUploadStore::release_verifying_upload`'s own doc).
/// Splitting these into two variants, rather than just removing the map
/// entry once bytes are committed, is the fix this enum exists for: a
/// removed entry made [`ResumableUploadStore::upload_offset`] report `key`
/// as free, which is exactly the admission gate a second, concurrent
/// attempt at the same key depends on staying closed.
enum S3UploadEntry {
    /// Accumulating appended bytes — the state this store has always kept,
    /// unchanged.
    InProgress(S3UploadState),
    /// [`ResumableUploadStore::take_upload`] already completed (or, for a
    /// zero-length upload, plain-`PUT`) its bytes at this key's own final
    /// path. `total_len` is purely informational — what
    /// [`ResumableUploadStore::upload_offset`] reports while in this state,
    /// mirroring what a genuinely in-progress upload of the same total size
    /// would report, since a caller probing offset mid-verification has no
    /// reason to be told anything different. Cleared only by
    /// [`ResumableUploadStore::release_verifying_upload`].
    Verifying { total_len: u64 },
}

/// The `s3` object-store profile: the plain HTTP protocol any
/// S3-compatible store speaks (MinIO, Ceph RGW, Cloudflare R2, AWS S3
/// itself), signed with hand-rolled AWS Signature Version 4 (`sigv4.rs`) —
/// never a vendor SDK. Always path-style addressing
/// (`{endpoint}/{bucket}/{key_prefix}{key}`), never virtual-hosted, so one
/// `endpoint` setting works against a store with no DNS wildcarding of its
/// own. Credentials are read once, eagerly, at [`S3ObjectStore::new`] time
/// (server boot) — a named startup failure when the configured environment
/// variable is unset, the same "at load, not at request time" rule
/// [`FsObjectStore::new`]'s missing-root check already follows.
pub struct S3ObjectStore {
    http: reqwest::Client,
    endpoint: url::Url,
    bucket: String,
    region: String,
    key_prefix: String,
    access_key: String,
    secret_key: String,
    presign_expiry: Duration,
    /// [`ResumableUploadStore`]'s own bookkeeping, one coarse lock guarding
    /// every in-progress upload this store instance knows about — the same
    /// "correctness first, a per-upload lock map is the obvious upgrade if
    /// throughput ever matters" trade-off [`FsObjectStore::uploads`]'s own
    /// doc already makes, necessary here too: an `append_upload` that
    /// flushes a part must hold this lock across that flush so a second
    /// concurrent append to the SAME key can never observe a half-updated
    /// offset (`ResumableUploadStore::append_upload`'s own doc). Unlike
    /// `fs`, this bookkeeping lives only in process memory — a server
    /// restart mid-upload orphans the underlying multipart upload
    /// server-side (S3 keeps billing storage for its parts until something
    /// aborts it, either a bucket lifecycle rule the deployment configures
    /// or a future reconcile-surface enhancement); documented honestly here
    /// rather than solved this slice.
    uploads: tokio::sync::Mutex<std::collections::HashMap<Uuid, S3UploadEntry>>,
}

/// Hand-written rather than derived: `access_key`/`secret_key` must never
/// appear in a debug print (a stray `{:?}` in a log line must not leak
/// credentials the same way `ObjectStoreError`'s own doc already promises
/// for error messages).
impl std::fmt::Debug for S3ObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3ObjectStore")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("key_prefix", &self.key_prefix)
            .field("presign_expiry", &self.presign_expiry)
            .finish_non_exhaustive()
    }
}

impl S3ObjectStore {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        endpoint: &str,
        bucket: impl Into<String>,
        region: impl Into<String>,
        key_prefix: impl Into<String>,
        access_key_env: &str,
        secret_key_env: &str,
        presign_expiry_s: u64,
    ) -> std::io::Result<Self> {
        let access_key = std::env::var(access_key_env).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("environment variable '{access_key_env}' is not set"),
            )
        })?;
        let secret_key = std::env::var(secret_key_env).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("environment variable '{secret_key_env}' is not set"),
            )
        })?;
        Self::build(
            endpoint,
            bucket,
            region,
            key_prefix,
            access_key,
            secret_key,
            presign_expiry_s,
        )
    }

    /// A store built for [`PathAddressedObjectStore`] reads ONLY, against
    /// already-resolved credentials.
    ///
    /// Exists for `tellurion-iceberg`'s `FileIO` layer, whose keys are
    /// whole in-bucket paths recorded in an Iceberg table's own metadata
    /// rather than [`ObjectKey`]s this deployment generated — see
    /// [`PathAddressedObjectStore`]'s own doc. Two deliberate differences
    /// from [`S3ObjectStore::new`]:
    ///
    /// - `key_prefix` is empty, and the path-addressed verbs ignore it
    ///   anyway ([`S3ObjectStore::raw_object_path`]) — there is no prefix
    ///   to file someone else's objects under.
    /// - Credentials arrive already resolved rather than as variable names
    ///   to look up here. The caller reads them from the environment (never
    ///   from `config.yaml`) so that a missing variable refuses with ITS
    ///   error type, naming the table and the collection it belongs to,
    ///   instead of an `io::Error` this module would have to invent a
    ///   caller-shaped message for.
    ///
    /// `presign_expiry` is [`Duration::ZERO`]: presigning is meaningless for
    /// a read-through store nothing hands a URL out of, and zero makes any
    /// accidental future call mint an already-expired URL rather than a
    /// silently long-lived one.
    pub fn for_path_reads(
        endpoint: &str,
        bucket: impl Into<String>,
        region: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> std::io::Result<Self> {
        Self::build(endpoint, bucket, region, "", access_key, secret_key, 0)
    }

    /// The credential-resolution-free half of [`S3ObjectStore::new`] — split
    /// out so this module's own tests can build an instance against fixed,
    /// known credentials without touching the process environment at all
    /// (`tests::s3_presign_shape` below).
    #[allow(clippy::too_many_arguments)]
    fn build(
        endpoint: &str,
        bucket: impl Into<String>,
        region: impl Into<String>,
        key_prefix: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        presign_expiry_s: u64,
    ) -> std::io::Result<Self> {
        let endpoint = url::Url::parse(endpoint).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("object store endpoint '{endpoint}' is not a valid URL: {err}"),
            )
        })?;
        // Same timeout budget `OidcValidator`'s own `reqwest::Client`
        // builder uses for its (much smaller) discovery/JWKS calls, widened
        // for object bodies that can be genuinely large — see `auth.rs`'s
        // own `Client::builder` for the identical `.unwrap_or_default()`
        // fallback (a client that never got its timeout configured is still
        // a usable client, never a startup failure over this alone).
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Ok(Self {
            http,
            endpoint,
            bucket: bucket.into(),
            region: region.into(),
            key_prefix: key_prefix.into(),
            access_key: access_key.into(),
            secret_key: secret_key.into(),
            presign_expiry: Duration::from_secs(presign_expiry_s),
            uploads: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// The path-style object path this key resolves to:
    /// `/{bucket}/{key_prefix}{segment}`. `key_prefix` is inserted
    /// literally (already validated non-hostile at config-load-adjacent
    /// `S3ObjectStore::new` time is unnecessary — it comes from this
    /// deployment's own config, not a client), `segment` is always a bare
    /// [`Uuid`] the same way [`FsObjectStore::object_path`] requires (this
    /// module's own path-traversal invariant, see the module doc).
    fn object_path(&self, key: &ObjectKey) -> Result<String> {
        let segment = path_segment(key);
        if segment.is_empty() || segment.contains('/') || segment.contains('\\') {
            return Err(ObjectStoreError::InvalidKey(segment));
        }
        Ok(format!("/{}/{}{}", self.bucket, self.key_prefix, segment))
    }

    /// The `Host` header value this store signs and sends — includes a
    /// non-default port explicitly (SigV4 signs whatever `Host` value the
    /// request actually carries, and a mismatch here would just make every
    /// signature invalid against a non-standard-port endpoint like a local
    /// MinIO on `:9000`).
    fn host_header(&self) -> String {
        match self.endpoint.port() {
            Some(port) => format!("{}:{port}", self.endpoint.host_str().unwrap_or_default()),
            None => self.endpoint.host_str().unwrap_or_default().to_string(),
        }
    }

    fn full_url(&self, path: &str) -> String {
        format!("{}://{}{path}", self.endpoint.scheme(), self.host_header())
    }

    fn credentials(&self) -> sigv4::Credentials<'_> {
        sigv4::Credentials {
            access_key: &self.access_key,
            secret_key: &self.secret_key,
        }
    }

    /// Signs and sends one request against `path` with no query string —
    /// every plain put/get/delete/head verb's own shape. `body` also
    /// doubles as this request's payload-hash input — `None` (GET/DELETE/
    /// HEAD) hashes the empty string, matching every SigV4 implementation's
    /// own convention for a bodyless request.
    async fn signed_request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<bytes::Bytes>,
    ) -> Result<reqwest::Response> {
        self.signed_object_request(method, path, &[], body).await
    }

    /// Signs and sends one request against `path` with `query` attached —
    /// the multipart-upload verbs' own request shape
    /// (`?uploads`/`?partNumber=N&uploadId=...`/`?uploadId=...`), generalized
    /// from [`Self::signed_request`] (which always sends an empty query).
    /// Builds the wire query string with the exact same
    /// [`sigv4::canonical_query_string`] encoder the signature itself is
    /// computed over, the same "the string a signature covers and the
    /// string actually sent can never diverge" discipline
    /// [`Self::signed_list_request`] already establishes for the
    /// `list-objects` flow.
    async fn signed_object_request(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<bytes::Bytes>,
    ) -> Result<reqwest::Response> {
        let now = SystemTime::now();
        let payload_hash = sigv4::sha256_hex(body.as_deref().unwrap_or(&[]));
        let input = sigv4::SignRequestInput {
            method: method.as_str(),
            host: &self.host_header(),
            path,
            query,
            payload_hash: &payload_hash,
        };
        let headers = sigv4::sign_headers(&input, &self.credentials(), &self.region, "s3", now);
        let url = if query.is_empty() {
            self.full_url(path)
        } else {
            format!(
                "{}?{}",
                self.full_url(path),
                sigv4::canonical_query_string(query)
            )
        };
        let mut request = self.http.request(method, url);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        request.send().await.map_err(ObjectStoreError::Http)
    }

    /// The `403 -> CredentialsRejected`, `5xx/other -> Storage` half of
    /// every verb's own status handling — factored out since all four share
    /// it identically; each verb still handles its own 200/404 shape.
    fn map_error_status(status: reqwest::StatusCode) -> ObjectStoreError {
        if status == reqwest::StatusCode::FORBIDDEN {
            ObjectStoreError::CredentialsRejected
        } else {
            ObjectStoreError::Storage {
                status: status.as_u16(),
            }
        }
    }
}

#[async_trait::async_trait]
impl ObjectStore for S3ObjectStore {
    async fn put(&self, key: ObjectKey, bytes: bytes::Bytes) -> Result<()> {
        let path = self.object_path(&key)?;
        let response = self
            .signed_request(reqwest::Method::PUT, &path, Some(bytes))
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Self::map_error_status(response.status()))
        }
    }

    async fn get(&self, key: ObjectKey) -> Result<Option<bytes::Bytes>> {
        let path = self.object_path(&key)?;
        let response = self
            .signed_request(reqwest::Method::GET, &path, None)
            .await?;
        match response.status() {
            reqwest::StatusCode::OK => Ok(Some(
                response.bytes().await.map_err(ObjectStoreError::Http)?,
            )),
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            status => Err(Self::map_error_status(status)),
        }
    }

    async fn delete(&self, key: ObjectKey) -> Result<()> {
        let path = self.object_path(&key)?;
        let response = self
            .signed_request(reqwest::Method::DELETE, &path, None)
            .await?;
        let status = response.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            // Idempotent — an already-absent key is success, matching
            // `FsObjectStore::delete`'s own contract (this trait's doc).
            Ok(())
        } else {
            Err(Self::map_error_status(status))
        }
    }

    async fn exists(&self, key: ObjectKey) -> Result<bool> {
        Ok(self.head(key).await?.is_some())
    }

    async fn head(&self, key: ObjectKey) -> Result<Option<ObjectMetadata>> {
        let path = self.object_path(&key)?;
        let response = self
            .signed_request(reqwest::Method::HEAD, &path, None)
            .await?;
        match response.status() {
            reqwest::StatusCode::OK => {
                let size = response
                    .headers()
                    .get(reqwest::header::CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());
                // `x-amz-checksum-sha256`: S3's additional-checksums
                // feature, opt-in per upload — only present when the
                // client's own presigned PUT declared this algorithm. Not
                // every S3-compatible store implements it at all
                // (`ObjectMetadata`'s own doc), so absence here is the
                // common case, not an error.
                let sha256 = response
                    .headers()
                    .get("x-amz-checksum-sha256")
                    .and_then(|v| v.to_str().ok())
                    .and_then(crate::asset::decode_base64)
                    .and_then(|bytes| bytes.try_into().ok());
                Ok(Some(ObjectMetadata { size, sha256 }))
            }
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            status => Err(Self::map_error_status(status)),
        }
    }

    fn as_presigned(&self) -> Option<&dyn PresignedObjectStore> {
        Some(self)
    }

    fn as_listable(&self) -> Option<&dyn ListableObjectStore> {
        Some(self)
    }

    fn as_resumable(&self) -> Option<&dyn ResumableUploadStore> {
        Some(self)
    }

    fn as_path_addressed(&self) -> Option<&dyn PathAddressedObjectStore> {
        Some(self)
    }
}

/// `s3` is the only shipped profile that can honor
/// [`PathAddressedObjectStore`] — see that trait's own doc for why `fs`
/// deliberately does not. Every verb here reuses the identical
/// [`S3ObjectStore::signed_object_request`] machinery (hand-rolled SigV4,
/// one `reqwest::Client`, the same 403/other status mapping) the
/// [`ObjectKey`]-addressed verbs above already use; the ONLY difference is
/// which path string gets signed.
#[async_trait::async_trait]
impl PathAddressedObjectStore for S3ObjectStore {
    async fn get_path(&self, path: &str) -> Result<Option<bytes::Bytes>> {
        let path = self.raw_object_path(path)?;
        let response = self
            .signed_raw_request(reqwest::Method::GET, &path, None)
            .await?;
        match response.status() {
            reqwest::StatusCode::OK => Ok(Some(
                response.bytes().await.map_err(ObjectStoreError::Http)?,
            )),
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            status => Err(Self::map_error_status(status)),
        }
    }

    async fn get_path_range(
        &self,
        path: &str,
        offset: u64,
        length: u64,
    ) -> Result<Option<bytes::Bytes>> {
        // A zero-length range has no HTTP spelling: `bytes=N-(N-1)` is
        // malformed, and omitting the header entirely would silently fetch
        // the WHOLE object instead of nothing — exactly the "an absent
        // value quietly becomes a different, larger behaviour" trap. Answer
        // it locally instead of asking the store.
        if length == 0 {
            return Ok(Some(bytes::Bytes::new()));
        }
        let raw = self.raw_object_path(path)?;
        let last = offset + length - 1;
        let response = self
            .signed_raw_request(
                reqwest::Method::GET,
                &raw,
                Some(format!("bytes={offset}-{last}")),
            )
            .await?;
        match response.status() {
            // 206 is what a store that honored the range answers; 200 means
            // it ignored `Range` and sent the whole object, which some
            // S3-compatible stores do when the range covers the entire
            // object. Both are read as success and the caller gets exactly
            // the bytes the store sent — never silently truncated, never
            // silently widened, because the one caller (Iceberg's Parquet
            // reader) checks the length it got.
            reqwest::StatusCode::PARTIAL_CONTENT | reqwest::StatusCode::OK => Ok(Some(
                response.bytes().await.map_err(ObjectStoreError::Http)?,
            )),
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            status => Err(Self::map_error_status(status)),
        }
    }

    async fn head_path(&self, path: &str) -> Result<Option<ObjectMetadata>> {
        let path = self.raw_object_path(path)?;
        let response = self
            .signed_raw_request(reqwest::Method::HEAD, &path, None)
            .await?;
        match response.status() {
            reqwest::StatusCode::OK => {
                let size = response
                    .headers()
                    .get(reqwest::header::CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());
                // `sha256` stays `None` unconditionally here, unlike
                // `ObjectStore::head`: that digest is this deployment's own
                // upload-verification concern (`asset.rs`), and an object
                // some other engine wrote has no reason to carry it. `None`
                // is "this store reports no digest", never "the digest did
                // not match".
                Ok(Some(ObjectMetadata { size, sha256: None }))
            }
            reqwest::StatusCode::NOT_FOUND => Ok(None),
            status => Err(Self::map_error_status(status)),
        }
    }
}

impl S3ObjectStore {
    /// The path-style object path an ALREADY-COMPLETE in-bucket key
    /// resolves to: `/{bucket}/{path}`.
    ///
    /// Deliberately does NOT prepend `key_prefix`, unlike
    /// [`S3ObjectStore::object_path`]. That prefix is where THIS deployment
    /// files the objects it generates; a [`PathAddressedObjectStore`] key is
    /// the complete key some other writer already minted and recorded in
    /// its own metadata, so prefixing it would name an object that does not
    /// exist.
    ///
    /// Rejects the three shapes that could resolve somewhere other than the
    /// key named: an empty key, a leading `/` (which would produce a
    /// `//`-rooted path), and any `..` segment. None can arise from the one
    /// caller this exists for (Iceberg table metadata), which is exactly
    /// why checking is cheap and worth doing anyway.
    fn raw_object_path(&self, path: &str) -> Result<String> {
        if path.is_empty()
            || path.starts_with('/')
            || path.split('/').any(|segment| segment == "..")
        {
            return Err(ObjectStoreError::InvalidKey(path.to_string()));
        }
        Ok(format!("/{}/{path}", self.bucket))
    }

    /// [`Self::signed_request`] for a path that may contain characters
    /// SigV4 percent-encodes, plus an optional `Range` header.
    ///
    /// The wire URL is built from [`sigv4::canonical_uri`] — the SAME
    /// encoder the signature is computed over — rather than
    /// [`Self::full_url`]'s literal interpolation. For an
    /// [`ObjectKey`]-addressed path (a bare `Uuid` under this deployment's
    /// own `key_prefix`) those two agree character for character, which is
    /// why the verbs above keep using the literal form unchanged; for an
    /// arbitrary externally minted key they need not, and a divergence
    /// between the string signed and the string sent is an unexplainable
    /// 403.
    ///
    /// `Range` is intentionally NOT a signed header: SigV4 signs exactly the
    /// header set [`sigv4::sign_headers`] returns, and every S3-compatible
    /// store accepts additional unsigned headers.
    async fn signed_raw_request(
        &self,
        method: reqwest::Method,
        path: &str,
        range: Option<String>,
    ) -> Result<reqwest::Response> {
        let now = SystemTime::now();
        let payload_hash = sigv4::sha256_hex(&[]);
        let input = sigv4::SignRequestInput {
            method: method.as_str(),
            host: &self.host_header(),
            path,
            query: &[],
            payload_hash: &payload_hash,
        };
        let headers = sigv4::sign_headers(&input, &self.credentials(), &self.region, "s3", now);
        let url = format!(
            "{}://{}{}",
            self.endpoint.scheme(),
            self.host_header(),
            sigv4::canonical_uri(path)
        );
        let mut request = self.http.request(method, url);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if let Some(range) = range {
            request = request.header(reqwest::header::RANGE, range);
        }
        request.send().await.map_err(ObjectStoreError::Http)
    }
}

impl PresignedObjectStore for S3ObjectStore {
    fn presign_get(&self, key: ObjectKey, expires_in: Duration, now: SystemTime) -> Result<String> {
        self.presign(reqwest::Method::GET, key, expires_in, now)
    }

    fn presign_put(&self, key: ObjectKey, expires_in: Duration, now: SystemTime) -> Result<String> {
        self.presign(reqwest::Method::PUT, key, expires_in, now)
    }

    fn default_expiry(&self) -> Duration {
        self.presign_expiry
    }
}

impl S3ObjectStore {
    fn presign(
        &self,
        method: reqwest::Method,
        key: ObjectKey,
        expires_in: Duration,
        now: SystemTime,
    ) -> Result<String> {
        let path = self.object_path(&key)?;
        let input = sigv4::PresignInput {
            method: method.as_str(),
            scheme: self.endpoint.scheme(),
            host: &self.host_header(),
            path: &path,
        };
        Ok(sigv4::presign_url(
            &input,
            &self.credentials(),
            &self.region,
            "s3",
            now,
            expires_in,
        ))
    }

    /// Signs and sends a `GET` on this store's own bucket root with `query`
    /// attached — the `list-objects` flow's own request shape, distinct
    /// from [`Self::signed_request`] (which always targets one object's own
    /// path and never carries query parameters). Builds the wire query
    /// string with the exact same [`sigv4::canonical_query_string`] encoder
    /// the signature itself is computed over, so the string a signature
    /// covers and the string actually sent can never diverge.
    async fn signed_list_request(&self, query: &[(&str, &str)]) -> Result<reqwest::Response> {
        let now = SystemTime::now();
        let path = format!("/{}", self.bucket);
        let payload_hash = sigv4::sha256_hex(&[]);
        let input = sigv4::SignRequestInput {
            method: "GET",
            host: &self.host_header(),
            path: &path,
            query,
            payload_hash: &payload_hash,
        };
        let headers = sigv4::sign_headers(&input, &self.credentials(), &self.region, "s3", now);
        let query_string = sigv4::canonical_query_string(query);
        let mut request = self
            .http
            .get(format!("{}?{query_string}", self.full_url(&path)));
        for (name, value) in headers {
            request = request.header(name, value);
        }
        request.send().await.map_err(ObjectStoreError::Http)
    }
}

#[async_trait::async_trait]
impl ListableObjectStore for S3ObjectStore {
    /// `ListObjectsV2` under this store's own `key_prefix`, looping on
    /// `NextContinuationToken` until the store reports `IsTruncated=false`
    /// — a single response page (S3's own per-request cap, 1000 keys unless
    /// a smaller `max-keys` is requested, which this profile never sends)
    /// would silently under-report drift in exactly the deployment where
    /// the reconcile surface matters most: a bucket holding more than 1000
    /// managed objects.
    async fn list_all(&self) -> Result<Vec<ListedObject>> {
        let mut entries = Vec::new();
        let mut continuation_token: Option<String> = None;
        loop {
            let mut query: Vec<(&str, &str)> =
                vec![("list-type", "2"), ("prefix", &self.key_prefix)];
            if let Some(token) = continuation_token.as_deref() {
                query.push(("continuation-token", token));
            }
            let response = self.signed_list_request(&query).await?;
            let status = response.status();
            if !status.is_success() {
                return Err(Self::map_error_status(status));
            }
            let body = response.text().await.map_err(ObjectStoreError::Http)?;
            let page = parse_list_bucket_result(&body);
            for key in page.keys {
                let raw_name = key
                    .strip_prefix(&self.key_prefix)
                    .unwrap_or(&key)
                    .to_string();
                entries.push(ListedObject {
                    id: Uuid::parse_str(&raw_name).ok(),
                    raw_name,
                    // `ListObjectsV2` only ever enumerates completed
                    // objects — an in-progress multipart upload's parts
                    // live in a separate namespace this call never touches
                    // (S3's own `ListMultipartUploads`, which this slice
                    // does not implement), so every key reported here is a
                    // finished object, never resumable-upload staging
                    // debris the way `fs`'s own `.upload`-suffixed files
                    // are.
                    is_staging: false,
                });
            }
            match page.next_continuation_token.filter(|_| page.is_truncated) {
                Some(token) => continuation_token = Some(token),
                None => break,
            }
        }
        Ok(entries)
    }
}

impl S3ObjectStore {
    /// Flushes bytes out of `state.buffer` as one real `UploadPart` call
    /// (`PUT {path}?partNumber=N&uploadId=...`), updating `state` in place.
    /// `force_remainder` is [`ResumableUploadStore::take_upload`]'s own
    /// final flush — S3's own rule that only the LAST part of a multipart
    /// upload may be smaller than [`S3_MULTIPART_PART_FLOOR`], so that
    /// flush takes whatever remains regardless of size (including zero,
    /// for an upload that never accumulated a single full part — S3 still
    /// requires at least one part to complete). Every other caller
    /// ([`ResumableUploadStore::append_upload`]'s own loop) only ever
    /// flushes once the buffer has reached the floor, so it always takes
    /// exactly that much.
    async fn flush_part(
        &self,
        key: &ObjectKey,
        state: &mut S3UploadState,
        force_remainder: bool,
    ) -> Result<()> {
        let take_len = if force_remainder {
            state.buffer.len()
        } else {
            S3_MULTIPART_PART_FLOOR as usize
        };
        // Copy the leading `take_len` bytes rather than draining them out
        // of `state.buffer` up front: this call is about to await a real
        // network round trip, and until that `UploadPart` PUT comes back
        // confirmed successful, these bytes are still exactly what
        // `append_upload` already told its own caller was durably
        // accumulated. Draining first and only afterward discovering the
        // request failed (a transport error, or a non-2xx status) would
        // silently throw that acknowledgment away — `state.flushed_len`
        // was never incremented for them, so they would vanish from both
        // the buffer AND the completed-parts count at once, regressing an
        // offset a caller was already told was safe. Every early return
        // below (the `?` on the request itself, and the explicit
        // non-success check) happens before `state` is touched at all, so
        // a transient failure here leaves the buffer and every other field
        // exactly as it was.
        let chunk: Vec<u8> = state.buffer[..take_len].to_vec();
        let chunk_len = chunk.len() as u64;
        let part_number = state.completed_parts.len() as i32 + 1;
        let path = self.object_path(key)?;
        let part_number_str = part_number.to_string();
        let query = [
            ("partNumber", part_number_str.as_str()),
            ("uploadId", state.upload_id.as_str()),
        ];
        let response = self
            .signed_object_request(
                reqwest::Method::PUT,
                &path,
                &query,
                Some(bytes::Bytes::from(chunk)),
            )
            .await?;
        if !response.status().is_success() {
            return Err(Self::map_error_status(response.status()));
        }
        // S3 identifies a completed part by its `ETag`, echoed back
        // verbatim (quotes included) in `CompleteMultipartUpload`'s own
        // request body — never recomputed locally, only carried forward
        // from what the store itself reported for this exact part.
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        // Only now, with the part confirmed landed, do the bytes actually
        // leave the buffer.
        state.buffer.drain(..take_len);
        state.completed_parts.push((part_number, etag));
        state.flushed_len += chunk_len;
        Ok(())
    }

    /// `AbortMultipartUpload` (`DELETE {path}?uploadId=...`) — tolerant of
    /// a `404` the same way [`ObjectStore::delete`]'s own idempotent
    /// contract already is: an upload id S3 no longer recognizes (already
    /// aborted, already completed, or never real) is not this call's
    /// problem to report.
    async fn abort_multipart(&self, path: &str, upload_id: &str) -> Result<()> {
        let response = self
            .signed_object_request(
                reqwest::Method::DELETE,
                path,
                &[("uploadId", upload_id)],
                None,
            )
            .await?;
        let status = response.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(Self::map_error_status(status))
        }
    }

    /// `CompleteMultipartUpload` (`POST {path}?uploadId=...`) — the request
    /// body lists every part this store flushed, in order, exactly as S3's
    /// own API requires. Not handled: AWS's own documented quirk where a
    /// `CompleteMultipartUpload` failure can arrive as a `200 OK` with an
    /// error embedded in the response body — this store treats any 2xx
    /// status as success, an honest simplification this slice does not
    /// close.
    async fn complete_multipart(
        &self,
        path: &str,
        upload_id: &str,
        parts: &[(i32, String)],
    ) -> Result<()> {
        let body = complete_multipart_body(parts);
        let response = self
            .signed_object_request(
                reqwest::Method::POST,
                path,
                &[("uploadId", upload_id)],
                Some(bytes::Bytes::from(body)),
            )
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(Self::map_error_status(response.status()))
        }
    }
}

/// `CompleteMultipartUpload`'s own request body: every part in order,
/// carrying back the exact `ETag` [`S3ObjectStore::flush_part`]'s own
/// `UploadPart` call recorded — S3 rejects the request unless this matches.
/// No XML-escaping: `part_number` is a plain decimal integer and `etag` is
/// whatever the store itself returned, never client-supplied text (the same
/// "every value here is server-controlled" reasoning
/// [`parse_list_bucket_result`]'s own doc already gives for skipping
/// entity-decoding).
fn complete_multipart_body(parts: &[(i32, String)]) -> String {
    let mut body = String::from("<CompleteMultipartUpload>");
    for (part_number, etag) in parts {
        body.push_str(&format!(
            "<Part><PartNumber>{part_number}</PartNumber><ETag>{etag}</ETag></Part>"
        ));
    }
    body.push_str("</CompleteMultipartUpload>");
    body
}

/// [`ResumableUploadStore`] for the `s3` profile: a real S3 multipart
/// upload underneath the same trait `fs` backs with a plain file. Every
/// method is keyed by `key`'s own [`Uuid`] into [`S3ObjectStore::uploads`];
/// [`create_upload`](Self::create_upload) mints the multipart upload id,
/// [`append_upload`](Self::append_upload) buffers and flushes parts at
/// [`S3_MULTIPART_PART_FLOOR`], and [`take_upload`](Self::take_upload)
/// completes the multipart upload and reads the assembled object straight
/// back — see each method's own doc for the specifics.
///
/// Two places where mapping this trait onto real S3 multipart upload is
/// genuine but imperfect, both accepted deliberately rather than solved
/// this slice:
///
/// - **Verification-before-commit is inverted for this transport.**
///   [`FsObjectStore::take_upload`] only ever reads a `.upload`-suffixed
///   staging file — the object's real, final key is untouched until
///   `crate::asset::complete_upload`'s own digest check passes and it
///   calls [`ObjectStore::put`]. S3 multipart upload has no equivalent
///   staging area: `CompleteMultipartUpload` is what makes the assembled
///   bytes exist and readable at all, and it can only ever write to the
///   exact key the upload was created against. So
///   [`take_upload`](Self::take_upload) below completes the multipart
///   upload — landing bytes at the real key — before `complete_upload`
///   ever gets a chance to check whether those are even the bytes the
///   client declared. This is not silently swept away: `complete_upload`'s
///   own digest-mismatch branch (`asset.rs`) deletes the object on a
///   mismatch rather than merely skipping a `put` that, for this
///   transport, already happened — restoring the "a digest mismatch never
///   leaves bytes behind" invariant every other transport gets for free. A
///   reader racing the narrow window between completion and that cleanup
///   is a known, accepted gap. A *writer* racing that same window — a
///   second `create_upload` for the same key, admitted before this
///   mismatch cleanup finishes, whose own correct bytes the delayed delete
///   could then destroy — is not accepted: [`S3UploadEntry::Verifying`]
///   and [`release_verifying_upload`](Self::release_verifying_upload) keep
///   [`upload_offset`](Self::upload_offset) reporting the key in-progress
///   for exactly as long as that cleanup takes, so the domain layer's own
///   admission check refuses the second attempt outright rather than
///   letting the two interleave.
/// - **This store's own upload bookkeeping is process-memory only.**
///   [`S3ObjectStore::uploads`] is a plain in-memory map, not a file on
///   disk the way `FsObjectStore`'s staging file is — a server restart
///   mid-upload orphans the underlying S3 multipart upload with no local
///   trace left to resume or abort it by. [`abandon_upload`](Self::abandon_upload)
///   and S3's own `AbortMultipartUpload` cover the ordinary give-up path;
///   recovering an orphan left by a crash is not this slice's job — the
///   standard production mitigation is a bucket lifecycle rule that
///   auto-aborts incomplete multipart uploads past some configured age, an
///   operator concern this server has no way to enforce from inside one
///   process's memory.
#[async_trait::async_trait]
impl ResumableUploadStore for S3ObjectStore {
    async fn create_upload(&self, key: ObjectKey) -> Result<()> {
        let id = key
            .id()
            .ok_or_else(|| ObjectStoreError::InvalidKey("non-uuid object key".to_string()))?;
        let path = self.object_path(&key)?;
        let mut uploads = self.uploads.lock().await;
        if let Some(S3UploadEntry::InProgress(stale)) = uploads.remove(&id) {
            // Best-effort: this store's own bookkeeping still remembered a
            // prior upload for this key (a caller that bypasses the
            // domain layer's own "already in progress" guard, or a
            // leftover from a process that crashed before `abandon_upload`
            // ran) — abort it server-side first so a fresh `create_upload`
            // never orphans a second, now-unreachable multipart upload.
            // Errors here are deliberately swallowed: this is a best-effort
            // cleanup of SOMEONE ELSE's leftover state, not this call's own
            // work, and `abort_multipart`'s own idempotent contract means
            // there is nothing useful to report even when it does run.
            //
            // A stale `Verifying` entry (removed above but not matched
            // here) has no `upload_id` left to abort — its own multipart
            // upload completed long ago; dropping the entry is all a
            // caller that bypassed the admission guard leaves this store
            // to do.
            let _ = self.abort_multipart(&path, &stale.upload_id).await;
        }
        let response = self
            .signed_object_request(reqwest::Method::POST, &path, &[("uploads", "")], None)
            .await?;
        if !response.status().is_success() {
            return Err(Self::map_error_status(response.status()));
        }
        let body = response.text().await.map_err(ObjectStoreError::Http)?;
        let upload_id = extract_first(&body, "<UploadId>", "</UploadId>").ok_or_else(|| {
            ObjectStoreError::MultipartResponseMalformed(
                "CreateMultipartUpload response had no <UploadId>".to_string(),
            )
        })?;
        uploads.insert(
            id,
            S3UploadEntry::InProgress(S3UploadState {
                upload_id,
                completed_parts: Vec::new(),
                buffer: Vec::new(),
                flushed_len: 0,
            }),
        );
        Ok(())
    }

    async fn upload_offset(&self, key: ObjectKey) -> Result<Option<u64>> {
        let Some(id) = key.id() else { return Ok(None) };
        let uploads = self.uploads.lock().await;
        Ok(uploads.get(&id).map(|entry| match entry {
            S3UploadEntry::InProgress(state) => state.flushed_len + state.buffer.len() as u64,
            S3UploadEntry::Verifying { total_len } => *total_len,
        }))
    }

    async fn append_upload(
        &self,
        key: ObjectKey,
        expected_offset: u64,
        bytes: bytes::Bytes,
    ) -> Result<u64> {
        let id = key
            .id()
            .ok_or_else(|| ObjectStoreError::InvalidKey("non-uuid object key".to_string()))?;
        let mut uploads = self.uploads.lock().await;
        let state = match uploads.get_mut(&id) {
            Some(S3UploadEntry::InProgress(state)) => state,
            // `None` (never created, or already taken) and `Verifying`
            // (taken, and not yet released) both mean the same thing from
            // this call's own point of view: there is no accumulating
            // upload resource left at `key` to append to.
            Some(S3UploadEntry::Verifying { .. }) | None => {
                return Err(ObjectStoreError::UploadNotFound)
            }
        };
        let actual = state.flushed_len + state.buffer.len() as u64;
        if actual != expected_offset {
            return Err(ObjectStoreError::UploadOffsetMismatch {
                expected: expected_offset,
                actual,
            });
        }
        state.buffer.extend_from_slice(&bytes);
        // Flush every full part-sized chunk now sitting in the buffer —
        // S3's own floor applies to every part except the last, so this
        // never flushes the tail; `take_upload`'s own final flush handles
        // that one. Held across the flush's own network round trip: two
        // concurrent appends to the SAME key must never interleave, the
        // identical coarse-lock trade-off `FsObjectStore::uploads`'s own
        // doc already makes.
        while state.buffer.len() as u64 >= S3_MULTIPART_PART_FLOOR {
            self.flush_part(&key, state, false).await?;
        }
        Ok(state.flushed_len + state.buffer.len() as u64)
    }

    async fn take_upload(&self, key: ObjectKey) -> Result<Option<bytes::Bytes>> {
        let id = key
            .id()
            .ok_or_else(|| ObjectStoreError::InvalidKey("non-uuid object key".to_string()))?;
        let path = self.object_path(&key)?;
        let mut state = {
            let mut uploads = self.uploads.lock().await;
            match uploads.remove(&id) {
                Some(S3UploadEntry::InProgress(state)) => state,
                // Already taken by a previous call and still being
                // verified (or already released with nothing to take
                // again) — either way, there is nothing here for a second
                // `take_upload` to consume.
                Some(S3UploadEntry::Verifying { .. }) => return Ok(None),
                None => return Ok(None),
            }
        };
        // From here `state` is exclusively owned by this call — the map no
        // longer has an entry for `id`, so nothing else can observe or
        // mutate it. Unlike `append_upload`'s own CAS, which must hold the
        // store's coarse lock across its flush, the remaining work below
        // (a possible final flush, `CompleteMultipartUpload`, and the
        // read-back `GET`) runs without holding it, so a slow completion
        // on one key never blocks resumable-upload operations against
        // every other key.
        //
        // Every fallible step between here and a successful completion
        // reinstates `state` into `self.uploads` before returning its
        // error: this call already removed it from the map above, and
        // completion failing must not ALSO make the upload disappear. Left
        // as the original bug had it, a retry would find nothing, get
        // `Ok(None)`, and the caller would surface `NotFound` — the asset
        // stuck `Pending` forever with no way to resume it, and the real
        // S3 multipart upload behind it orphaned. Re-acquiring the lock
        // just to put one entry back is cheap and brief, unlike the flush/
        // complete work itself, so this does not reintroduce the
        // coarse-lock-for-the-whole-duration cost the paragraph above
        // deliberately avoids. Every reinstatement below puts `state` back
        // as `InProgress` — a failure at these steps never got as far as
        // committing anything, so the upload is exactly as resumable as it
        // was before this call.
        if state.completed_parts.is_empty() && state.buffer.is_empty() {
            // Nothing was ever appended (a zero-length managed asset, or a
            // completion called with no prior append at all). S3 requires
            // a multipart upload to assemble from at least one part, and a
            // genuinely empty part is not a request this store trusts
            // every S3-compatible implementation to accept even as the
            // sole/last part. Rather than gamble on that, abort the now-
            // pointless multipart upload outright and write the object the
            // same direct way `ObjectStore::put` always would for an
            // empty body — a plain zero-byte `PUT`, no multipart machinery
            // involved at all.
            if let Err(err) = self.abort_multipart(&path, &state.upload_id).await {
                self.uploads
                    .lock()
                    .await
                    .insert(id, S3UploadEntry::InProgress(state));
                return Err(err);
            }
            let response = match self
                .signed_object_request(reqwest::Method::PUT, &path, &[], Some(bytes::Bytes::new()))
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    self.uploads
                        .lock()
                        .await
                        .insert(id, S3UploadEntry::InProgress(state));
                    return Err(err);
                }
            };
            if !response.status().is_success() {
                let err = Self::map_error_status(response.status());
                self.uploads
                    .lock()
                    .await
                    .insert(id, S3UploadEntry::InProgress(state));
                return Err(err);
            }
        } else {
            if !state.buffer.is_empty() {
                // A genuine tail below the floor — S3 allows only the LAST
                // part of a multipart upload to be smaller than
                // `S3_MULTIPART_PART_FLOOR`, which is exactly what this
                // flush is.
                if let Err(err) = self.flush_part(&key, &mut state, true).await {
                    self.uploads
                        .lock()
                        .await
                        .insert(id, S3UploadEntry::InProgress(state));
                    return Err(err);
                }
            }
            if let Err(err) = self
                .complete_multipart(&path, &state.upload_id, &state.completed_parts)
                .await
            {
                self.uploads
                    .lock()
                    .await
                    .insert(id, S3UploadEntry::InProgress(state));
                return Err(err);
            }
        }
        // Completion has now genuinely succeeded: the assembled bytes are
        // readable at `key`'s own final path, before this store's own
        // caller (`crate::asset::complete_upload`, by way of
        // `crate::asset::finish_upload`) has verified anything about them
        // — see [`ResumableUploadStore`]'s own doc for why. This is the
        // exact defect the `Verifying` variant exists to close: reinstate
        // the map entry as `Verifying` — not remove it outright — so
        // `upload_offset` keeps reporting `key` as in-progress for as long
        // as those bytes remain unverified, refusing a second attempt at
        // the same key rather than admitting one into this window.
        // `crate::asset::complete_resumable_upload` is the only caller
        // that clears it, via `release_verifying_upload`, once its own
        // digest check (and, on a mismatch, its own cleanup delete) has
        // actually finished.
        self.uploads.lock().await.insert(
            id,
            S3UploadEntry::Verifying {
                total_len: state.flushed_len,
            },
        );
        // `complete_upload`'s own digest verification needs the assembled
        // object's actual bytes; a completed multipart upload has no
        // cheaper way to hand them back than reading the finished object
        // straight out again — an extra GET round trip the direct-upload
        // transport never pays. A failure in this last read is
        // deliberately not reinstated as resumable: the multipart upload's
        // own `upload_id` is already spent server-side either way, and the
        // assembled object genuinely exists at `key`'s own final path
        // regardless of whether this particular `GET` manages to read it
        // back — there is nothing left here for a retried `take_upload` to
        // usefully resume. The `Verifying` entry just inserted stays in
        // place either way, so a caller that sees this error and still
        // wants the key freed up must go through `release_verifying_upload`
        // exactly like the success path does.
        self.get(key).await
    }

    async fn abandon_upload(&self, key: ObjectKey) -> Result<()> {
        let id = key
            .id()
            .ok_or_else(|| ObjectStoreError::InvalidKey("non-uuid object key".to_string()))?;
        let path = self.object_path(&key)?;
        let state = {
            let mut uploads = self.uploads.lock().await;
            match uploads.get(&id) {
                Some(S3UploadEntry::Verifying { .. }) => {
                    // Owned by an in-flight `take_upload`/digest-
                    // verification — leave it untouched. There is no
                    // multipart upload left here for THIS call to abort
                    // (`take_upload` already completed it), and removing
                    // the hold out from under that in-flight completion
                    // would reopen exactly the admission window
                    // `release_verifying_upload`'s own doc exists to keep
                    // shut.
                    None
                }
                _ => uploads.remove(&id).map(|entry| match entry {
                    S3UploadEntry::InProgress(state) => state,
                    S3UploadEntry::Verifying { .. } => {
                        unreachable!("Verifying is matched and left in place above")
                    }
                }),
            }
        };
        let Some(state) = state else {
            return Ok(());
        };
        if let Err(err) = self.abort_multipart(&path, &state.upload_id).await {
            // The abort itself failed — the multipart upload is still
            // alive server-side, so this call has not actually cleaned
            // anything up. Put the state back rather than letting it stay
            // removed: a caller that sees this error and retries the same
            // DELETE must reach a SECOND real `AbortMultipartUpload`
            // attempt. Left as the original bug had it, the retry would
            // find nothing here (already removed above) and return
            // `Ok(())` — a false "cleaned up" signal that silently
            // orphans the server-side multipart upload forever.
            self.uploads
                .lock()
                .await
                .insert(id, S3UploadEntry::InProgress(state));
            return Err(err);
        }
        Ok(())
    }

    /// `true`: `take_upload`'s own `CompleteMultipartUpload` is what lands
    /// the assembled bytes at `key`'s own final path in the first place —
    /// see this `impl` block's own doc for the full "verification-before-
    /// commit is inverted for this transport" explanation.
    fn take_upload_already_committed(&self) -> bool {
        true
    }

    /// Clears the `Verifying` hold `take_upload` left at `key` — a no-op
    /// (not an error) when there is nothing there to clear, the same
    /// idempotent shape [`abandon_upload`](Self::abandon_upload) already
    /// uses, since a caller that already released (or never committed
    /// anything, e.g. the `fs` profile calling through the trait's default)
    /// has nothing left to do here either. Only removes a `Verifying`
    /// entry specifically — an `InProgress` entry at the same id belongs to
    /// a genuinely different, later upload attempt and must never be
    /// disturbed by a release call that logically belongs to an earlier
    /// one.
    async fn release_verifying_upload(&self, key: ObjectKey) -> Result<()> {
        let Some(id) = key.id() else { return Ok(()) };
        let mut uploads = self.uploads.lock().await;
        if matches!(uploads.get(&id), Some(S3UploadEntry::Verifying { .. })) {
            uploads.remove(&id);
        }
        Ok(())
    }
}

/// One `ListObjectsV2` response page, as [`parse_list_bucket_result`]
/// extracts it.
struct ListBucketPage {
    keys: Vec<String>,
    is_truncated: bool,
    next_continuation_token: Option<String>,
}

/// Hand-rolled `ListObjectsV2` XML response parser — no XML crate, the same
/// "a few dozen lines beats a new dependency" call this crate's own
/// `asset::encode_base64`/`decode_base64` already make for base64.
/// Deliberately naive: extracts `<Key>`/`<IsTruncated>`/
/// `<NextContinuationToken>` by substring search, with no entity-decoding —
/// safe because every key this deployment itself ever writes is a bare
/// [`Uuid`] (optionally under this deployment's own operator-configured
/// `key_prefix`), never client-supplied text that could carry an
/// XML-special character requiring `&amp;`-style unescaping.
fn parse_list_bucket_result(xml: &str) -> ListBucketPage {
    ListBucketPage {
        keys: extract_all(xml, "<Key>", "</Key>"),
        is_truncated: extract_first(xml, "<IsTruncated>", "</IsTruncated>").as_deref()
            == Some("true"),
        next_continuation_token: extract_first(
            xml,
            "<NextContinuationToken>",
            "</NextContinuationToken>",
        ),
    }
}

fn extract_first(xml: &str, open: &str, close: &str) -> Option<String> {
    let start = xml.find(open)? + open.len();
    let end = xml[start..].find(close)? + start;
    Some(xml[start..end].to_string())
}

fn extract_all(xml: &str, open: &str, close: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(open) {
        let after_open = &rest[start + open.len()..];
        let Some(end) = after_open.find(close) else {
            break;
        };
        out.push(after_open[..end].to_string());
        rest = &after_open[end + close.len()..];
    }
    out
}

/// Builds the object store this collection's `object_store` config id
/// resolves to — `fs` or `s3` as of this slice; see
/// `crate::config::ObjectStoreProfile`'s own doc for why any other
/// `profile:` value already refuses at config-load time (an unknown serde
/// tag), before this function is ever reached.
pub fn build_object_store(
    decl: &crate::config::ObjectStoreDecl,
) -> std::io::Result<std::sync::Arc<dyn ObjectStore>> {
    match &decl.profile {
        crate::config::ObjectStoreProfile::Fs { root } => {
            Ok(std::sync::Arc::new(FsObjectStore::new(root)?))
        }
        crate::config::ObjectStoreProfile::S3 {
            endpoint,
            bucket,
            region,
            key_prefix,
            access_key_env,
            secret_key_env,
            presign_expiry_s,
        } => Ok(std::sync::Arc::new(S3ObjectStore::new(
            endpoint,
            bucket.clone(),
            region.clone(),
            key_prefix.clone(),
            access_key_env,
            secret_key_env,
            *presign_expiry_s,
        )?)),
    }
}

/// In-memory `ObjectStore` for domain-logic tests (`asset.rs`) that must
/// run without touching a real filesystem — the object-store counterpart of
/// how `tellurion-core`'s other capability traits get a fake implementer in
/// tests rather than a live backend. Also implements
/// [`PresignedObjectStore`] unconditionally, honoring `s3` semantics (a
/// fake presign step never actually transfers bytes; a test simulates the
/// out-of-band client upload by calling [`ObjectStore::put`] on this same
/// store directly, then exercises `asset::finalize_presigned_upload`
/// against it) — see `asset.rs`'s own presign test suite.
#[cfg(any(test, feature = "test-support"))]
pub struct InMemoryObjectStore {
    objects: std::sync::Mutex<std::collections::HashMap<Uuid, bytes::Bytes>>,
    /// Whether [`ObjectStore::head`] reports a `sha256` digest — real
    /// S3-compatible stores only do when the client's own presigned upload
    /// declared a checksum algorithm the store understands
    /// (`ObjectMetadata`'s own doc); `false` by default, matching the more
    /// common "store doesn't report it" case. `with_checksum_reporting`
    /// flips it for the tests that specifically exercise the digest-aware
    /// path.
    report_checksum: bool,
    /// Staged resumable-upload bytes, keyed the same way `objects` is —
    /// this fake also implements [`ResumableUploadStore`] unconditionally
    /// (honoring `fs` semantics, the profile that actually ships it) so
    /// `asset.rs`'s own resumable-upload domain tests run hermetically, the
    /// same reasoning this struct's own doc already gives for implementing
    /// [`PresignedObjectStore`] unconditionally.
    uploads: std::sync::Mutex<std::collections::HashMap<Uuid, Vec<u8>>>,
}

#[cfg(any(test, feature = "test-support"))]
impl Default for InMemoryObjectStore {
    fn default() -> Self {
        Self {
            objects: std::sync::Mutex::new(std::collections::HashMap::new()),
            report_checksum: false,
            uploads: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl InMemoryObjectStore {
    pub fn with_checksum_reporting() -> Self {
        Self {
            report_checksum: true,
            ..Self::default()
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait::async_trait]
impl ObjectStore for InMemoryObjectStore {
    async fn put(&self, key: ObjectKey, bytes: bytes::Bytes) -> Result<()> {
        let id = key
            .id()
            .ok_or_else(|| ObjectStoreError::InvalidKey("raw test key".to_string()))?;
        self.objects.lock().unwrap().insert(id, bytes);
        Ok(())
    }

    async fn get(&self, key: ObjectKey) -> Result<Option<bytes::Bytes>> {
        let Some(id) = key.id() else { return Ok(None) };
        Ok(self.objects.lock().unwrap().get(&id).cloned())
    }

    async fn delete(&self, key: ObjectKey) -> Result<()> {
        if let Some(id) = key.id() {
            self.objects.lock().unwrap().remove(&id);
        }
        Ok(())
    }

    async fn exists(&self, key: ObjectKey) -> Result<bool> {
        let Some(id) = key.id() else { return Ok(false) };
        Ok(self.objects.lock().unwrap().contains_key(&id))
    }

    async fn head(&self, key: ObjectKey) -> Result<Option<ObjectMetadata>> {
        let Some(id) = key.id() else { return Ok(None) };
        let objects = self.objects.lock().unwrap();
        let Some(bytes) = objects.get(&id) else {
            return Ok(None);
        };
        let sha256 = self
            .report_checksum
            .then(|| crate::asset::compute_sha256(bytes).value);
        Ok(Some(ObjectMetadata {
            size: Some(bytes.len() as u64),
            sha256,
        }))
    }

    fn as_presigned(&self) -> Option<&dyn PresignedObjectStore> {
        Some(self)
    }

    fn as_resumable(&self) -> Option<&dyn ResumableUploadStore> {
        Some(self)
    }

    fn as_listable(&self) -> Option<&dyn ListableObjectStore> {
        Some(self)
    }
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait::async_trait]
impl ListableObjectStore for InMemoryObjectStore {
    /// Reports both maps this fake tracks — completed objects (`objects`)
    /// and staged resumable-upload bytes (`uploads`) — the identical two
    /// categories [`FsObjectStore::list_all`] reports for real, so a
    /// `crate::reconcile` domain test can exercise the same logic against
    /// this hermetic fake as against a real filesystem.
    async fn list_all(&self) -> Result<Vec<ListedObject>> {
        let mut entries: Vec<ListedObject> = self
            .objects
            .lock()
            .unwrap()
            .keys()
            .map(|id| ListedObject {
                raw_name: id.hyphenated().to_string(),
                id: Some(*id),
                is_staging: false,
            })
            .collect();
        entries.extend(self.uploads.lock().unwrap().keys().map(|id| ListedObject {
            raw_name: format!("{}.upload", id.hyphenated()),
            id: Some(*id),
            is_staging: true,
        }));
        Ok(entries)
    }
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait::async_trait]
impl ResumableUploadStore for InMemoryObjectStore {
    async fn create_upload(&self, key: ObjectKey) -> Result<()> {
        let id = key
            .id()
            .ok_or_else(|| ObjectStoreError::InvalidKey("raw test key".to_string()))?;
        self.uploads.lock().unwrap().insert(id, Vec::new());
        Ok(())
    }

    async fn upload_offset(&self, key: ObjectKey) -> Result<Option<u64>> {
        let Some(id) = key.id() else { return Ok(None) };
        Ok(self
            .uploads
            .lock()
            .unwrap()
            .get(&id)
            .map(|bytes| bytes.len() as u64))
    }

    async fn append_upload(
        &self,
        key: ObjectKey,
        expected_offset: u64,
        bytes: bytes::Bytes,
    ) -> Result<u64> {
        let id = key
            .id()
            .ok_or_else(|| ObjectStoreError::InvalidKey("raw test key".to_string()))?;
        let mut uploads = self.uploads.lock().unwrap();
        let staged = uploads
            .get_mut(&id)
            .ok_or(ObjectStoreError::UploadNotFound)?;
        let actual = staged.len() as u64;
        if actual != expected_offset {
            return Err(ObjectStoreError::UploadOffsetMismatch {
                expected: expected_offset,
                actual,
            });
        }
        staged.extend_from_slice(&bytes);
        Ok(staged.len() as u64)
    }

    async fn take_upload(&self, key: ObjectKey) -> Result<Option<bytes::Bytes>> {
        let Some(id) = key.id() else { return Ok(None) };
        Ok(self
            .uploads
            .lock()
            .unwrap()
            .remove(&id)
            .map(bytes::Bytes::from))
    }

    async fn abandon_upload(&self, key: ObjectKey) -> Result<()> {
        if let Some(id) = key.id() {
            self.uploads.lock().unwrap().remove(&id);
        }
        Ok(())
    }
}

#[cfg(any(test, feature = "test-support"))]
impl PresignedObjectStore for InMemoryObjectStore {
    /// A synthetic URL — never dereferenced over the network by any test,
    /// only checked for shape/expiry (`asset.rs`'s own presign tests
    /// simulate the client's out-of-band upload with a direct `put` call,
    /// per this struct's own doc).
    fn presign_get(&self, key: ObjectKey, expires_in: Duration, now: SystemTime) -> Result<String> {
        self.fake_presigned_url("GET", key, expires_in, now)
    }

    fn presign_put(&self, key: ObjectKey, expires_in: Duration, now: SystemTime) -> Result<String> {
        self.fake_presigned_url("PUT", key, expires_in, now)
    }

    fn default_expiry(&self) -> Duration {
        Duration::from_secs(900)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl InMemoryObjectStore {
    fn fake_presigned_url(
        &self,
        method: &str,
        key: ObjectKey,
        expires_in: Duration,
        now: SystemTime,
    ) -> Result<String> {
        let id = key
            .id()
            .ok_or_else(|| ObjectStoreError::InvalidKey("raw test key".to_string()))?;
        let issued_at = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Ok(format!(
            "https://fake-object-store.test/{id}?method={method}&issued_at={issued_at}&expires_in={}",
            expires_in.as_secs()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, FsObjectStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FsObjectStore::new(dir.path()).expect("build fs store");
        (dir, store)
    }

    #[tokio::test]
    async fn round_trips_put_get_delete_exists() {
        let (_dir, store) = store();
        let key = ObjectKey::new(Uuid::new_v4());

        assert!(!store.exists(key.clone()).await.unwrap());
        assert!(store.get(key.clone()).await.unwrap().is_none());

        store
            .put(key.clone(), bytes::Bytes::from_static(b"hello"))
            .await
            .unwrap();
        assert!(store.exists(key.clone()).await.unwrap());
        assert_eq!(
            store.get(key.clone()).await.unwrap(),
            Some(bytes::Bytes::from_static(b"hello"))
        );

        store.delete(key.clone()).await.unwrap();
        assert!(!store.exists(key.clone()).await.unwrap());
        assert!(store.get(key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn deleting_an_absent_key_is_not_an_error() {
        let (_dir, store) = store();
        let key = ObjectKey::new(Uuid::new_v4());
        store.delete(key).await.expect("idempotent delete");
    }

    #[tokio::test]
    async fn two_distinct_uuids_never_collide_on_disk() {
        let (dir, store) = store();
        let a = ObjectKey::new(Uuid::new_v4());
        let b = ObjectKey::new(Uuid::new_v4());
        store
            .put(a.clone(), bytes::Bytes::from_static(b"a"))
            .await
            .unwrap();
        store
            .put(b.clone(), bytes::Bytes::from_static(b"b"))
            .await
            .unwrap();
        assert_eq!(store.get(a).await.unwrap().unwrap(), "a");
        assert_eq!(store.get(b).await.unwrap().unwrap(), "b");

        let mut entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        entries.sort();
        assert_eq!(entries.len(), 2, "one file per distinct object key");
    }

    /// The load-bearing traversal test: a hostile key built through the
    /// test-only `ObjectKey::from_raw` escape hatch (see its own doc — no
    /// production path ever constructs a key this way) must never resolve
    /// to a path outside `store`'s root, whether the write actually happens
    /// or is refused outright.
    #[tokio::test]
    async fn a_hostile_key_cannot_escape_the_store_root() {
        let (dir, store) = store();
        let escape_target = dir.path().parent().unwrap().join("escaped.txt");
        std::fs::remove_file(&escape_target).ok();

        for hostile in [
            "../../../../../../etc/passwd",
            "../escaped",
            "..",
            "sub/../../escaped",
            "/etc/passwd",
            "a/b",
        ] {
            let key = ObjectKey::from_raw(hostile);
            let result = store.put(key, bytes::Bytes::from_static(b"pwned")).await;
            assert!(
                result.is_err(),
                "hostile key '{hostile}' must be refused, not written"
            );
        }

        assert!(
            !escape_target.exists(),
            "no hostile key may have written outside the store root"
        );
    }

    #[test]
    fn build_object_store_rejects_a_root_that_does_not_exist() {
        let decl = crate::config::ObjectStoreDecl {
            id: "main".to_string(),
            profile: crate::config::ObjectStoreProfile::Fs {
                root: "/this/path/does/not/exist/hopefully".to_string(),
            },
        };
        assert!(build_object_store(&decl).is_err());
    }

    #[test]
    fn build_object_store_accepts_an_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let decl = crate::config::ObjectStoreDecl {
            id: "main".to_string(),
            profile: crate::config::ObjectStoreProfile::Fs {
                root: dir.path().to_string_lossy().to_string(),
            },
        };
        assert!(build_object_store(&decl).is_ok());
    }

    // -- ResumableUploadStore (fs only) ----------------------------------

    #[test]
    fn fs_advertises_the_resumable_upload_capability() {
        let (_dir, fs_store) = store();
        assert!(fs_store.as_resumable().is_some());
        // `S3ObjectStore` also implements `ResumableUploadStore` as of this
        // slice — proven separately in `s3_tests` (real multipart-upload
        // signing needs a live mock, not just the trait-default check this
        // one covers).
    }

    #[tokio::test]
    async fn create_probe_append_complete_round_trip() {
        let (_dir, store) = store();
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());

        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), None);

        resumable.create_upload(key.clone()).await.unwrap();
        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), Some(0));

        let offset = resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from_static(b"hello "))
            .await
            .unwrap();
        assert_eq!(offset, 6);
        let offset = resumable
            .append_upload(key.clone(), 6, bytes::Bytes::from_static(b"world"))
            .await
            .unwrap();
        assert_eq!(offset, 11);
        assert_eq!(
            resumable.upload_offset(key.clone()).await.unwrap(),
            Some(11)
        );

        let taken = resumable.take_upload(key.clone()).await.unwrap().unwrap();
        assert_eq!(taken, bytes::Bytes::from_static(b"hello world"));
        // Consumed: the upload resource is gone.
        assert_eq!(resumable.upload_offset(key).await.unwrap(), None);
    }

    #[tokio::test]
    async fn appending_past_the_current_offset_is_a_named_mismatch() {
        let (_dir, store) = store();
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());
        resumable.create_upload(key.clone()).await.unwrap();
        resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from_static(b"abc"))
            .await
            .unwrap();

        // Out-of-order: the client believes more has landed than truly has.
        let err = resumable
            .append_upload(key.clone(), 10, bytes::Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ObjectStoreError::UploadOffsetMismatch {
                expected: 10,
                actual: 3
            }
        ));

        // Stale: the client is retrying a position the server already moved
        // past.
        let err = resumable
            .append_upload(key, 0, bytes::Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ObjectStoreError::UploadOffsetMismatch {
                expected: 0,
                actual: 3
            }
        ));
    }

    #[tokio::test]
    async fn appending_or_probing_with_no_live_upload_is_named_not_found() {
        let (_dir, store) = store();
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());

        let err = resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(matches!(err, ObjectStoreError::UploadNotFound));
        assert_eq!(resumable.take_upload(key.clone()).await.unwrap(), None);
        // Idempotent: abandoning a never-created (or already-consumed)
        // upload is a successful no-op, matching `ObjectStore::delete`'s
        // own contract.
        resumable.abandon_upload(key).await.unwrap();
    }

    #[tokio::test]
    async fn abandoning_an_incomplete_upload_lets_a_fresh_one_start_clean() {
        let (_dir, store) = store();
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());

        resumable.create_upload(key.clone()).await.unwrap();
        resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from_static(b"stale bytes"))
            .await
            .unwrap();
        resumable.abandon_upload(key.clone()).await.unwrap();
        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), None);

        // A fresh upload on the same key starts at offset 0, with none of
        // the abandoned bytes still present.
        resumable.create_upload(key.clone()).await.unwrap();
        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), Some(0));
        let taken = resumable.take_upload(key).await.unwrap().unwrap();
        assert_eq!(taken, bytes::Bytes::new());
    }

    /// Two concurrent appends racing the identical `expected_offset`: this
    /// store's own coarse lock (`FsObjectStore`'s own doc) guarantees
    /// exactly one wins and the other observes the loser's own advance as a
    /// named mismatch — never a torn write, never both silently applied.
    #[tokio::test]
    async fn two_concurrent_appends_at_the_same_offset_never_both_succeed() {
        let (_dir, store) = store();
        let store = std::sync::Arc::new(store);
        let key = ObjectKey::new(Uuid::new_v4());
        store
            .as_resumable()
            .unwrap()
            .create_upload(key.clone())
            .await
            .unwrap();

        let store_a = std::sync::Arc::clone(&store);
        let key_a = key.clone();
        let task_a = tokio::spawn(async move {
            store_a
                .as_resumable()
                .unwrap()
                .append_upload(key_a, 0, bytes::Bytes::from_static(b"aaaa"))
                .await
        });
        let store_b = std::sync::Arc::clone(&store);
        let key_b = key.clone();
        let task_b = tokio::spawn(async move {
            store_b
                .as_resumable()
                .unwrap()
                .append_upload(key_b, 0, bytes::Bytes::from_static(b"bbbb"))
                .await
        });

        let (result_a, result_b) = tokio::join!(task_a, task_b);
        let results = [result_a.unwrap(), result_b.unwrap()];
        let successes = results.iter().filter(|r| r.is_ok()).count();
        let mismatches = results
            .iter()
            .filter(|r| matches!(r, Err(ObjectStoreError::UploadOffsetMismatch { .. })))
            .count();
        assert_eq!(successes, 1, "exactly one racing append must win");
        assert_eq!(mismatches, 1, "the loser must see a named offset mismatch");

        // Whichever chunk won, the accumulated bytes are exactly that one
        // chunk — never a mix of both, never neither.
        let taken = store
            .as_resumable()
            .unwrap()
            .take_upload(key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(taken.len(), 4);
        assert!(
            taken == bytes::Bytes::from_static(b"aaaa")
                || taken == bytes::Bytes::from_static(b"bbbb")
        );
    }

    // -- ListableObjectStore (reconcile surface) -------------------------

    #[tokio::test]
    async fn fs_lists_completed_objects_and_upload_staging_files_separately() {
        let (_dir, store) = store();
        let completed = ObjectKey::new(Uuid::new_v4());
        let staging = ObjectKey::new(Uuid::new_v4());
        store
            .put(completed.clone(), bytes::Bytes::from_static(b"done"))
            .await
            .unwrap();
        store
            .as_resumable()
            .unwrap()
            .create_upload(staging.clone())
            .await
            .unwrap();

        let listed = store.as_listable().unwrap().list_all().await.unwrap();
        assert_eq!(listed.len(), 2, "one completed object, one staging file");

        let completed_entry = listed
            .iter()
            .find(|entry| entry.id == completed.id())
            .expect("the completed object is listed");
        assert!(!completed_entry.is_staging);

        let staging_entry = listed
            .iter()
            .find(|entry| entry.id == staging.id())
            .expect("the staging file is listed");
        assert!(staging_entry.is_staging);
        assert!(staging_entry.raw_name.ends_with(".upload"));
    }

    #[tokio::test]
    async fn fs_list_all_is_empty_on_a_fresh_store() {
        let (_dir, store) = store();
        assert!(store
            .as_listable()
            .unwrap()
            .list_all()
            .await
            .unwrap()
            .is_empty());
    }
}

/// `s3` object-store profile tests. Two kinds:
///
/// - Hermetic, always-run: a hand-rolled HTTP/1.1 loopback mock (the same
///   `std::net::TcpListener`-based idiom `tellurion-cog`'s own
///   `test_support::MockServer` uses for its ranged-GET tests, rather than
///   pulling a mocking crate into this workspace) drives `S3ObjectStore`'s
///   real signed-request code path against a fake bucket, and a
///   clock-fixed golden test checks presigned-URL shape with no network at
///   all.
/// - Skippable live-store integration tests, gated on the
///   `TELLURION_TEST_S3_*` environment variables — skip cleanly with a
///   printed notice when unset, so the suite stays green on a machine with
///   no object store running.
#[cfg(test)]
mod s3_tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    // Only for `.get(...)` calls on `crate::asset::InMemoryAssetRecordStore`
    // in the Defect 2 domain-layer tests below — `as _` since only the
    // trait's methods are needed in scope, never its name.
    use crate::asset::AssetRecordStore as _;

    // -- a minimal in-process S3-shaped HTTP/1.1 server ---------------------

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum MockBehavior {
        /// PUT stores, GET/HEAD/DELETE answer from the same in-memory map —
        /// a real (if tiny) object store, letting these tests drive
        /// `S3ObjectStore`'s actual put/get/delete/head/exists code paths
        /// end to end rather than asserting on request shape alone.
        Store,
        /// Every request gets `403 Forbidden` regardless of path or
        /// method — the credentials-rejected fixture.
        RejectAll,
    }

    /// One in-progress multipart upload this mock is tracking — the mock's
    /// own counterpart to [`S3UploadState`], keyed by the fake `upload_id`
    /// this mock itself mints in the `CreateMultipartUpload` arm below.
    struct MultipartUpload {
        object_path: String,
        parts: std::collections::HashMap<i32, Vec<u8>>,
    }

    /// One-shot fault injection: lets a test force a SPECIFIC request kind
    /// to fail exactly once (a `500`, and — for `UploadPart`/
    /// `CompleteMultipartUpload`/`AbortMultipartUpload` — genuinely without
    /// taking effect server-side, the same way a real transient failure
    /// would leave S3's own state untouched), then succeed on every
    /// subsequent attempt of the same kind. `MockBehavior::Store`'s own
    /// "everything always succeeds" shape and `MockBehavior::RejectAll`'s
    /// own "everything always 403s" shape bookend "always ok" and "always
    /// broken" — neither can reach a mid-flush failure, which is exactly
    /// the class of bug this hook exists to make reachable in a test:
    /// `S3ObjectStore::flush_part`'s own "don't drain before the network
    /// call confirms" fix, `take_upload`'s own "don't drop state before
    /// completion confirms" fix, and `abandon_upload`'s own "a retry
    /// actually retries the abort" fix all only matter on a path that
    /// fails once and then succeeds.
    #[derive(Default)]
    struct FaultInjector {
        fail_next_upload_part: bool,
        fail_next_complete_multipart: bool,
        fail_next_abort_multipart: bool,
    }

    struct MockS3 {
        addr: SocketAddr,
        objects: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
        // Kept alive here (not just in the listener thread's own clone) for
        // symmetry with `objects`/`aborted` above and to leave room for a
        // future test that inspects in-progress multipart state directly;
        // no test does yet, so `#[allow(dead_code)]` rather than a false
        // "unused" signal on genuinely load-bearing shared state.
        #[allow(dead_code)]
        multipart: Arc<Mutex<std::collections::HashMap<String, MultipartUpload>>>,
        /// Every `upload_id` this mock has received an `AbortMultipartUpload`
        /// for — `MockS3::was_aborted`'s own backing store, this crate's own
        /// "abandon aborts the multipart upload" assertion hook.
        aborted: Arc<Mutex<std::collections::HashSet<String>>>,
        #[allow(dead_code)]
        next_upload_id: Arc<Mutex<u64>>,
        /// See [`FaultInjector`]'s own doc.
        faults: Arc<Mutex<FaultInjector>>,
        /// Every plain (non-multipart) `PUT` this mock has received, in
        /// request order — `MockS3::plain_put_count`'s own backing store,
        /// the "`complete_upload` must not re-PUT bytes a completed
        /// multipart upload already committed" assertion hook.
        plain_puts: Arc<Mutex<Vec<String>>>,
    }

    impl MockS3 {
        fn spawn(behavior: MockBehavior) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("binds a loopback port");
            let addr = listener.local_addr().expect("bound listener has an addr");
            let objects = Arc::new(Mutex::new(
                std::collections::HashMap::<String, Vec<u8>>::new(),
            ));
            let multipart = Arc::new(Mutex::new(std::collections::HashMap::<
                String,
                MultipartUpload,
            >::new()));
            let aborted = Arc::new(Mutex::new(std::collections::HashSet::<String>::new()));
            let next_upload_id = Arc::new(Mutex::new(0u64));
            let faults = Arc::new(Mutex::new(FaultInjector::default()));
            let plain_puts = Arc::new(Mutex::new(Vec::<String>::new()));
            let objects_for_thread = Arc::clone(&objects);
            let multipart_for_thread = Arc::clone(&multipart);
            let aborted_for_thread = Arc::clone(&aborted);
            let next_upload_id_for_thread = Arc::clone(&next_upload_id);
            let faults_for_thread = Arc::clone(&faults);
            let plain_puts_for_thread = Arc::clone(&plain_puts);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { break };
                    handle_one(
                        stream,
                        &objects_for_thread,
                        &multipart_for_thread,
                        &aborted_for_thread,
                        &next_upload_id_for_thread,
                        &faults_for_thread,
                        &plain_puts_for_thread,
                        behavior,
                    );
                }
            });
            Self {
                addr,
                objects,
                multipart,
                aborted,
                next_upload_id,
                faults,
                plain_puts,
            }
        }

        fn endpoint(&self) -> String {
            format!("http://{}", self.addr)
        }

        fn seed(&self, path: &str, bytes: &[u8]) {
            self.objects
                .lock()
                .unwrap()
                .insert(path.to_string(), bytes.to_vec());
        }

        /// Whether this mock has received an `AbortMultipartUpload` for
        /// `upload_id` — the assertion hook `abandon_upload`'s own test
        /// ("abandon aborts the multipart upload") needs, since aborting is
        /// otherwise unobservable from the client side (a successful abort
        /// and a successful no-op both just return `Ok(())`).
        fn was_aborted(&self, upload_id: &str) -> bool {
            self.aborted.lock().unwrap().contains(upload_id)
        }

        /// Arms a one-shot failure for the next `UploadPart` this mock
        /// receives — see [`FaultInjector`]'s own doc.
        fn fail_next_upload_part(&self) {
            self.faults.lock().unwrap().fail_next_upload_part = true;
        }

        /// Arms a one-shot failure for the next `CompleteMultipartUpload`.
        fn fail_next_complete_multipart(&self) {
            self.faults.lock().unwrap().fail_next_complete_multipart = true;
        }

        /// Arms a one-shot failure for the next `AbortMultipartUpload`.
        fn fail_next_abort_multipart(&self) {
            self.faults.lock().unwrap().fail_next_abort_multipart = true;
        }

        /// How many plain (non-multipart) `PUT` requests this mock has
        /// received against `path` — `0` is what `complete_resumable_
        /// upload` must achieve on the `s3` profile once the multipart
        /// upload it drove has already committed the object.
        fn plain_put_count(&self, path: &str) -> usize {
            self.plain_puts
                .lock()
                .unwrap()
                .iter()
                .filter(|seen| seen.as_str() == path)
                .count()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_one(
        mut stream: TcpStream,
        objects: &Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
        multipart: &Arc<Mutex<std::collections::HashMap<String, MultipartUpload>>>,
        aborted: &Arc<Mutex<std::collections::HashSet<String>>>,
        next_upload_id: &Arc<Mutex<u64>>,
        faults: &Arc<Mutex<FaultInjector>>,
        plain_puts: &Arc<Mutex<Vec<String>>>,
        behavior: MockBehavior,
    ) {
        let mut reader = BufReader::new(stream.try_clone().expect("clone loopback stream"));
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();

        let mut content_length: usize = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line
                .strip_prefix("Content-Length: ")
                .or_else(|| line.strip_prefix("content-length: "))
            {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            let _ = reader.read_exact(&mut body);
        }

        if behavior == MockBehavior::RejectAll {
            write_response(&mut stream, "403 Forbidden", &[], &[]);
            return;
        }

        // Every query-bearing verb this mock understands (list-objects,
        // plus the multipart-upload verbs' own `?uploads`/`?partNumber=N&
        // uploadId=...`/`?uploadId=...`) shares this one parse — `base_path`
        // is a fresh owned `String` (not borrowed from `path`) so the plain
        // put/get/head/delete arms below can still move `path` freely.
        let (base_path, raw_query) = path.split_once('?').unwrap_or((path.as_str(), ""));
        let base_path = base_path.to_string();
        let query = parse_query(raw_query);
        let has = |name: &str| query.iter().any(|(k, _)| k == name);
        let query_value = |name: &str| {
            query
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };

        match method.as_str() {
            "POST" if has("uploads") => {
                // `CreateMultipartUpload` (`S3ObjectStore::create_upload`'s
                // own request shape: `POST {path}?uploads`).
                let mut counter = next_upload_id.lock().unwrap();
                *counter += 1;
                let upload_id = format!("mock-upload-{}", *counter);
                drop(counter);
                multipart.lock().unwrap().insert(
                    upload_id.clone(),
                    MultipartUpload {
                        object_path: base_path.clone(),
                        parts: std::collections::HashMap::new(),
                    },
                );
                let xml = format!(
                    "<InitiateMultipartUploadResult><UploadId>{upload_id}</UploadId>\
                     </InitiateMultipartUploadResult>"
                );
                write_response(&mut stream, "200 OK", &[], xml.as_bytes());
            }
            "PUT" if has("partNumber") && has("uploadId") => {
                // `UploadPart` (`S3ObjectStore::flush_part`'s own request
                // shape: `PUT {path}?partNumber=N&uploadId=...`) — stores
                // this part under its own part number, keyed by
                // `upload_id`, and returns a fake but stable `ETag` this
                // mock can later recognize in `CompleteMultipartUpload`'s
                // own request body (the real store never inspects the
                // `ETag`'s own content, only echoes it back verbatim, so
                // this mock doesn't need a real one either).
                //
                // A one-shot injected failure (`FaultInjector::
                // fail_next_upload_part`) responds without ever recording
                // the part — a real transient failure never landed on S3's
                // side either, which is exactly the state `flush_part`'s
                // own fix relies on being true.
                if std::mem::take(&mut faults.lock().unwrap().fail_next_upload_part) {
                    write_response(&mut stream, "500 Internal Server Error", &[], &[]);
                    return;
                }
                let upload_id = query_value("uploadId").unwrap_or_default();
                let part_number: i32 = query_value("partNumber")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
                let mut in_progress = multipart.lock().unwrap();
                match in_progress.get_mut(&upload_id) {
                    Some(entry) => {
                        entry.parts.insert(part_number, body.clone());
                        let etag = format!("\"etag-{upload_id}-{part_number}\"");
                        write_response(&mut stream, "200 OK", &[format!("ETag: {etag}")], &[]);
                    }
                    None => write_response(&mut stream, "404 Not Found", &[], &[]),
                }
            }
            "POST" if has("uploadId") => {
                // `CompleteMultipartUpload` (`S3ObjectStore::
                // complete_multipart`'s own request shape: `POST {path}?
                // uploadId=...`) — reassembles the parts this upload's own
                // request body names, in the order it names them, and
                // places the result at the object path this upload was
                // created against.
                //
                // A one-shot injected failure responds before this upload
                // is removed from `multipart` — a real
                // `CompleteMultipartUpload` failure leaves the multipart
                // upload exactly as completable as it was before the
                // attempt, which is what `take_upload`'s own fix (put the
                // state back rather than drop it) needs a retry to find.
                if std::mem::take(&mut faults.lock().unwrap().fail_next_complete_multipart) {
                    write_response(&mut stream, "500 Internal Server Error", &[], &[]);
                    return;
                }
                let upload_id = query_value("uploadId").unwrap_or_default();
                let mut in_progress = multipart.lock().unwrap();
                match in_progress.remove(&upload_id) {
                    Some(entry) => {
                        let body_str = String::from_utf8_lossy(&body);
                        let mut assembled = Vec::new();
                        for part_number in parse_complete_multipart_body(&body_str) {
                            if let Some(part_bytes) = entry.parts.get(&part_number) {
                                assembled.extend_from_slice(part_bytes);
                            }
                        }
                        objects.lock().unwrap().insert(entry.object_path, assembled);
                        let xml = "<CompleteMultipartUploadResult>\
                                   <ETag>\"final\"</ETag></CompleteMultipartUploadResult>";
                        write_response(&mut stream, "200 OK", &[], xml.as_bytes());
                    }
                    None => write_response(&mut stream, "404 Not Found", &[], &[]),
                }
            }
            "DELETE" if has("uploadId") => {
                // `AbortMultipartUpload` (`S3ObjectStore::abort_multipart`'s
                // own request shape: `DELETE {path}?uploadId=...`) —
                // idempotent, the same "removing an absent entry is still
                // success" contract every plain `DELETE` arm below already
                // follows.
                //
                // A one-shot injected failure responds before this upload
                // is removed from `multipart`/recorded in `aborted` — a
                // real `AbortMultipartUpload` failure leaves the multipart
                // upload alive server-side, which is what `abandon_upload`'s
                // own fix (put the state back so a retry re-issues the
                // abort for real) needs to be true.
                if std::mem::take(&mut faults.lock().unwrap().fail_next_abort_multipart) {
                    write_response(&mut stream, "500 Internal Server Error", &[], &[]);
                    return;
                }
                let upload_id = query_value("uploadId").unwrap_or_default();
                multipart.lock().unwrap().remove(&upload_id);
                aborted.lock().unwrap().insert(upload_id);
                write_response(&mut stream, "204 No Content", &[], &[]);
            }
            "PUT" => {
                plain_puts.lock().unwrap().push(path.clone());
                objects.lock().unwrap().insert(path, body);
                write_response(&mut stream, "200 OK", &[], &[]);
            }
            "GET" if has("list-type") => {
                // `ListObjectsV2` (`ListableObjectStore::list_all`'s own
                // request shape): a query string on the bucket root, never
                // on a single object's own path (an object key is always a
                // bare `Uuid`, which can never contain `?`) — a single,
                // untruncated page, `Contents`/`Key` entries filtered by
                // the request's own `prefix` parameter, mirroring a real
                // `ListObjectsV2` response closely enough for
                // `S3ObjectStore::list_all`'s own XML parser.
                //
                // Real `ListObjectsV2` `<Key>` entries (and its own
                // `prefix` filter) are relative to the bucket, never
                // carrying the bucket segment itself — `objects`' own keys
                // here are full request paths (`/{bucket}/{key}`, the same
                // shape `S3ObjectStore::object_path` builds), so this
                // strips that leading `/{bucket}/` before
                // filtering/reporting.
                let bucket_root = format!("{base_path}/");
                let prefix = query_value("prefix").unwrap_or_default();
                let keys: Vec<String> = objects
                    .lock()
                    .unwrap()
                    .keys()
                    .filter_map(|key| key.strip_prefix(&bucket_root))
                    .filter(|relative| relative.starts_with(prefix.as_str()))
                    .map(str::to_string)
                    .collect();
                let contents: String = keys
                    .iter()
                    .map(|key| format!("<Contents><Key>{key}</Key></Contents>"))
                    .collect();
                let xml = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                     <ListBucketResult>{contents}<IsTruncated>false</IsTruncated></ListBucketResult>"
                );
                write_response(&mut stream, "200 OK", &[], xml.as_bytes());
            }
            "GET" => {
                let found = objects.lock().unwrap().get(&path).cloned();
                match found {
                    Some(bytes) => write_response(&mut stream, "200 OK", &[], &bytes),
                    None => write_response(&mut stream, "404 Not Found", &[], &[]),
                }
            }
            "HEAD" => {
                let found = objects.lock().unwrap().get(&path).cloned();
                match found {
                    // A real HEAD response has no body but still reports the
                    // object's true size in `Content-Length` — write the
                    // status line/header by hand here rather than through
                    // `write_response` (which always derives `Content-
                    // Length` from the body it is actually sending, `0` for
                    // an empty HEAD body) so this mock's `Content-Length`
                    // means the same thing `S3ObjectStore::head` reads it
                    // for: the object's size, not the response's.
                    Some(bytes) => {
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            bytes.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    None => write_response(&mut stream, "404 Not Found", &[], &[]),
                }
            }
            "DELETE" => {
                objects.lock().unwrap().remove(&path);
                write_response(&mut stream, "204 No Content", &[], &[]);
            }
            _ => write_response(&mut stream, "405 Method Not Allowed", &[], &[]),
        }
    }

    /// Splits a raw (still percent-encoded) query string into decoded
    /// `(key, value)` pairs — this mock's own read-side counterpart to
    /// `sigv4::canonical_query_string`'s own encoding, shared by every
    /// query-bearing verb `handle_one` now understands.
    fn parse_query(raw_query: &str) -> Vec<(String, String)> {
        raw_query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                (percent_decode(key), percent_decode(value))
            })
            .collect()
    }

    /// `CompleteMultipartUpload`'s own request body: every `<Part>`'s own
    /// `<PartNumber>`, in document order — this mock doesn't validate the
    /// `ETag` its own `UploadPart` response minted, only reassembles parts
    /// in the order the client declares them.
    fn parse_complete_multipart_body(xml: &str) -> Vec<i32> {
        let mut part_numbers = Vec::new();
        let mut rest = xml;
        while let Some(start) = rest.find("<Part>") {
            let after = &rest[start + "<Part>".len()..];
            let Some(end) = after.find("</Part>") else {
                break;
            };
            let part_xml = &after[..end];
            if let Some(number) = extract_first(part_xml, "<PartNumber>", "</PartNumber>") {
                if let Ok(number) = number.parse::<i32>() {
                    part_numbers.push(number);
                }
            }
            rest = &after[end + "</Part>".len()..];
        }
        part_numbers
    }

    fn write_response(stream: &mut TcpStream, status: &str, extra_headers: &[String], body: &[u8]) {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for header in extra_headers {
            response.push_str(header);
            response.push_str("\r\n");
        }
        response.push_str("\r\n");
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
    }

    /// Minimal percent-decoder — this mock's own counterpart to
    /// `sigv4::canonical_query_string`'s percent-encoding, needed only to
    /// read back a `list-objects` request's raw (still-encoded) `prefix`
    /// query value (`handle_one`'s own `"GET" if path.contains('?')` arm).
    fn percent_decode(input: &str) -> String {
        let bytes = input.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                    if let Ok(byte) = u8::from_str_radix(hex, 16) {
                        out.push(byte);
                        i += 3;
                        continue;
                    }
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn store_against(mock: &MockS3) -> S3ObjectStore {
        S3ObjectStore::build(
            &mock.endpoint(),
            "bucket",
            "us-east-1",
            "",
            "test-access-key",
            "test-secret-key",
            900,
        )
        .expect("builds against a valid loopback endpoint")
    }

    // -- hermetic: real signed requests against the in-process mock --------

    #[tokio::test]
    async fn put_get_delete_exists_round_trip_against_the_mock_store() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let key = ObjectKey::new(Uuid::new_v4());

        assert!(!store.exists(key.clone()).await.unwrap());
        assert!(store.get(key.clone()).await.unwrap().is_none());

        store
            .put(key.clone(), bytes::Bytes::from_static(b"hello s3"))
            .await
            .unwrap();
        assert!(store.exists(key.clone()).await.unwrap());
        assert_eq!(
            store.get(key.clone()).await.unwrap(),
            Some(bytes::Bytes::from_static(b"hello s3"))
        );

        let meta = store.head(key.clone()).await.unwrap().unwrap();
        assert_eq!(meta.size, Some(8));

        store.delete(key.clone()).await.unwrap();
        assert!(!store.exists(key.clone()).await.unwrap());
    }

    #[tokio::test]
    async fn deleting_an_absent_key_is_not_an_error() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        store
            .delete(ObjectKey::new(Uuid::new_v4()))
            .await
            .expect("idempotent delete, matching the ObjectStore trait's own contract");
    }

    #[tokio::test]
    async fn a_key_prefix_is_written_into_every_object_path() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = S3ObjectStore::build(
            &mock.endpoint(),
            "bucket",
            "us-east-1",
            "assets/",
            "test-access-key",
            "test-secret-key",
            900,
        )
        .unwrap();
        let key = ObjectKey::new(Uuid::new_v4());
        store
            .put(key.clone(), bytes::Bytes::from_static(b"x"))
            .await
            .unwrap();
        let expected_path = format!("/bucket/assets/{}", key.id().unwrap().hyphenated());
        assert!(mock.objects.lock().unwrap().contains_key(&expected_path));
    }

    #[tokio::test]
    async fn a_rejected_credential_maps_to_the_named_refusal() {
        let mock = MockS3::spawn(MockBehavior::RejectAll);
        let store = store_against(&mock);
        let key = ObjectKey::new(Uuid::new_v4());

        let err = store
            .put(key.clone(), bytes::Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(matches!(err, ObjectStoreError::CredentialsRejected));
        assert_eq!(err.to_string(), "storage credentials rejected");

        let err = store.get(key.clone()).await.unwrap_err();
        assert!(matches!(err, ObjectStoreError::CredentialsRejected));

        let err = store.head(key).await.unwrap_err();
        assert!(matches!(err, ObjectStoreError::CredentialsRejected));
    }

    #[tokio::test]
    async fn head_reports_size_and_a_missing_object_is_none() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let key = ObjectKey::new(Uuid::new_v4());

        assert!(store.head(key.clone()).await.unwrap().is_none());

        mock.seed(
            &format!("/bucket/{}", key.id().unwrap().hyphenated()),
            b"twelve bytes",
        );
        let meta = store.head(key).await.unwrap().unwrap();
        assert_eq!(meta.size, Some(12));
        // This mock never sends `x-amz-checksum-sha256` — the common case
        // per `ObjectMetadata`'s own doc.
        assert_eq!(meta.sha256, None);
    }

    // -- ResumableUploadStore (s3 multipart) --------------------------------

    #[test]
    fn as_resumable_is_advertised_by_both_shipped_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let fs_store = FsObjectStore::new(dir.path()).unwrap();
        assert!(fs_store.as_resumable().is_some());

        let mock = MockS3::spawn(MockBehavior::Store);
        let s3_store = store_against(&mock);
        assert!(s3_store.as_resumable().is_some());
    }

    /// Full round trip through real `CreateMultipartUpload`/`UploadPart`/
    /// `CompleteMultipartUpload` calls against the mock: three appends that
    /// together cross [`S3_MULTIPART_PART_FLOOR`] once, forcing one real
    /// mid-upload `UploadPart` flush at the floor plus a genuine
    /// smaller-than-the-floor final part at completion — both halves of
    /// the "small chunks crossing the part threshold" requirement in one
    /// test, since a wrong buffer/offset accounting in either would corrupt
    /// the reassembled bytes this test checks byte-for-byte.
    #[tokio::test]
    async fn resumable_round_trip_crosses_the_multipart_part_threshold() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());

        let first: Vec<u8> = vec![b'a'; 2 * 1024 * 1024];
        let second: Vec<u8> = vec![b'b'; 2 * 1024 * 1024];
        let third: Vec<u8> =
            vec![b'c'; S3_MULTIPART_PART_FLOOR as usize + 1000 - first.len() - second.len()];
        let total_len = (first.len() + second.len() + third.len()) as u64;

        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), None);
        resumable.create_upload(key.clone()).await.unwrap();
        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), Some(0));

        let mut offset = 0u64;
        for chunk in [&first, &second, &third] {
            offset = resumable
                .append_upload(key.clone(), offset, bytes::Bytes::from(chunk.clone()))
                .await
                .unwrap();
        }
        assert_eq!(offset, total_len);
        assert_eq!(
            resumable.upload_offset(key.clone()).await.unwrap(),
            Some(total_len)
        );

        let taken = resumable.take_upload(key.clone()).await.unwrap().unwrap();
        let mut expected = first;
        expected.extend_from_slice(&second);
        expected.extend_from_slice(&third);
        assert_eq!(taken.as_ref(), expected.as_slice());

        // The assembled object really landed at this key's own path in the
        // mock's backing store — `take_upload`'s own `CompleteMultipartUpload`
        // + read-back `GET`, exercised end to end. `upload_offset` still
        // reports the key in-progress: `take_upload` leaves a `Verifying`
        // hold in place rather than freeing the key outright, precisely so
        // a second `create_upload` stays refused for as long as those
        // bytes are uncommitted-and-unverified from the domain layer's own
        // point of view (`ResumableUploadStore::release_verifying_upload`'s
        // own doc).
        assert_eq!(
            resumable.upload_offset(key.clone()).await.unwrap(),
            Some(total_len)
        );
        assert!(store.exists(key.clone()).await.unwrap());

        // `release_verifying_upload` clears that hold, the same step
        // `crate::asset::complete_resumable_upload` takes once its own
        // digest check has finished with these bytes.
        resumable
            .release_verifying_upload(key.clone())
            .await
            .unwrap();
        assert_eq!(resumable.upload_offset(key).await.unwrap(), None);
    }

    /// An append that lands EXACTLY at [`S3_MULTIPART_PART_FLOOR`] must
    /// flush precisely one part with nothing left in the buffer —
    /// completion then has no genuine tail to flush, only the part
    /// `append_upload` itself already sent.
    #[tokio::test]
    async fn append_flushes_a_part_exactly_at_the_threshold_with_nothing_left_over() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());
        resumable.create_upload(key.clone()).await.unwrap();

        let exact = vec![b'x'; S3_MULTIPART_PART_FLOOR as usize];
        let offset = resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from(exact.clone()))
            .await
            .unwrap();
        assert_eq!(offset, S3_MULTIPART_PART_FLOOR);

        let taken = resumable.take_upload(key).await.unwrap().unwrap();
        assert_eq!(taken.as_ref(), exact.as_slice());
    }

    #[tokio::test]
    async fn append_upload_offset_mismatch_both_directions_on_s3() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());
        resumable.create_upload(key.clone()).await.unwrap();
        resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from_static(b"abc"))
            .await
            .unwrap();

        // Out-of-order: the client believes more has landed than truly has.
        let err = resumable
            .append_upload(key.clone(), 10, bytes::Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ObjectStoreError::UploadOffsetMismatch {
                expected: 10,
                actual: 3
            }
        ));

        // Stale: the client is retrying a position the server already
        // moved past.
        let err = resumable
            .append_upload(key, 0, bytes::Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ObjectStoreError::UploadOffsetMismatch {
                expected: 0,
                actual: 3
            }
        ));
    }

    #[tokio::test]
    async fn appending_or_probing_with_no_live_upload_is_named_not_found_on_s3() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());

        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), None);
        let err = resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(matches!(err, ObjectStoreError::UploadNotFound));
        assert_eq!(resumable.take_upload(key.clone()).await.unwrap(), None);
        // Idempotent — abandoning a never-created upload is a successful
        // no-op, matching `ObjectStore::delete`'s own contract.
        resumable.abandon_upload(key).await.unwrap();
    }

    /// `abandon_upload` must reach the store as a real `AbortMultipartUpload`
    /// — otherwise the underlying multipart upload lingers on S3, billed
    /// until something aborts it (`S3ObjectStore::uploads`'s own doc).
    #[tokio::test]
    async fn abandon_upload_aborts_the_multipart_upload_on_the_store() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());

        resumable.create_upload(key.clone()).await.unwrap();
        resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from_static(b"partial"))
            .await
            .unwrap();

        // This mock mints upload ids as `mock-upload-{n}`, deterministically
        // counted from a freshly spawned instance — the single
        // `CreateMultipartUpload` above minted `mock-upload-1`.
        assert!(!mock.was_aborted("mock-upload-1"));
        resumable.abandon_upload(key.clone()).await.unwrap();
        assert!(
            mock.was_aborted("mock-upload-1"),
            "AbortMultipartUpload must have reached the store"
        );
        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), None);

        // Idempotent — abandoning again (nothing left locally to abort) is
        // still `Ok(())`.
        resumable.abandon_upload(key).await.unwrap();
    }

    /// A fresh `create_upload` on a key this store's own bookkeeping still
    /// remembers a prior upload for aborts that stale upload server-side
    /// first — `create_upload`'s own "never orphan a second, now-
    /// unreachable multipart upload" guard.
    #[tokio::test]
    async fn recreating_an_upload_aborts_the_stale_one_first() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());

        resumable.create_upload(key.clone()).await.unwrap();
        assert!(!mock.was_aborted("mock-upload-1"));

        resumable.create_upload(key.clone()).await.unwrap();
        assert!(
            mock.was_aborted("mock-upload-1"),
            "the first (now-orphaned) multipart upload must have been aborted"
        );
        // The second upload is a genuinely fresh one — offset `0`, and it
        // completes normally.
        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), Some(0));
        resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from_static(b"fresh"))
            .await
            .unwrap();
        let taken = resumable.take_upload(key).await.unwrap().unwrap();
        assert_eq!(taken, bytes::Bytes::from_static(b"fresh"));
    }

    /// A `create_upload` immediately followed by `take_upload`, with no
    /// append in between (a zero-length managed asset, or a client that
    /// completes without ever sending a chunk): S3 requires at least one
    /// part to complete a multipart upload, so this must not attempt
    /// `CompleteMultipartUpload` with an empty manifest — `take_upload`'s
    /// own zero-byte fallback (abort the now-pointless multipart upload,
    /// write the object directly) must still land a real, empty, readable
    /// object at the key.
    #[tokio::test]
    async fn take_upload_with_nothing_ever_appended_falls_back_to_a_plain_empty_put() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());

        resumable.create_upload(key.clone()).await.unwrap();
        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), Some(0));

        let taken = resumable.take_upload(key.clone()).await.unwrap().unwrap();
        assert_eq!(taken, bytes::Bytes::new());
        assert!(
            mock.was_aborted("mock-upload-1"),
            "the pointless multipart upload must be aborted, not left dangling"
        );
        assert!(store.exists(key.clone()).await.unwrap());
        // Still held in-progress (offset `0`, matching the empty object
        // just committed) until `release_verifying_upload` runs — the same
        // hold `take_upload`'s own non-empty path leaves in place, exercised
        // in `resumable_round_trip_crosses_the_multipart_part_threshold`.
        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), Some(0));
        resumable
            .release_verifying_upload(key.clone())
            .await
            .unwrap();
        assert_eq!(resumable.upload_offset(key).await.unwrap(), None);
    }

    // -- durability: a mid-flush failure must never lose or duplicate bytes -

    /// The bug this reproduces: `flush_part` used to `drain` the flushed
    /// bytes out of `state.buffer` BEFORE issuing the `UploadPart` PUT, so
    /// a failed PUT lost them for good while `flushed_len` stayed
    /// un-incremented — an offset already handed back to a caller as
    /// durably accumulated would silently regress. With the fix, a failed
    /// `UploadPart` leaves `state` exactly as it was about to become
    /// (the newly appended byte safely sitting in the buffer, nothing
    /// flushed, nothing lost); a later successful flush of that SAME
    /// buffered data lands the correct, non-duplicated object.
    #[tokio::test]
    async fn a_failed_upload_part_loses_no_bytes_and_a_later_flush_still_completes_correctly() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());
        resumable.create_upload(key.clone()).await.unwrap();

        // Fill the buffer to one byte short of the floor — no flush yet.
        let below_floor = vec![b'x'; S3_MULTIPART_PART_FLOOR as usize - 1];
        let offset = resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from(below_floor.clone()))
            .await
            .unwrap();
        assert_eq!(offset, S3_MULTIPART_PART_FLOOR - 1);

        // The next single byte crosses the floor, triggering exactly one
        // `UploadPart` — armed to fail once.
        mock.fail_next_upload_part();
        let err = resumable
            .append_upload(key.clone(), offset, bytes::Bytes::from_static(b"y"))
            .await
            .unwrap_err();
        assert!(matches!(err, ObjectStoreError::Storage { status: 500 }));

        // Nothing was lost: the byte the failed flush was about to send is
        // still sitting in the buffer, so the store's own accumulated
        // total already reflects it — it never regresses back toward
        // `S3_MULTIPART_PART_FLOOR - 1`, the value this offset was already
        // reported as before the failing call.
        assert_eq!(
            resumable.upload_offset(key.clone()).await.unwrap(),
            Some(S3_MULTIPART_PART_FLOOR)
        );

        // A retry (an empty append at the now-current offset re-enters the
        // flush loop against the same still-buffered bytes) succeeds this
        // time, since the mock's one-shot failure already fired.
        let offset = resumable
            .append_upload(key.clone(), S3_MULTIPART_PART_FLOOR, bytes::Bytes::new())
            .await
            .unwrap();
        assert_eq!(offset, S3_MULTIPART_PART_FLOOR);

        let taken = resumable.take_upload(key).await.unwrap().unwrap();
        let mut expected = below_floor;
        expected.push(b'y');
        assert_eq!(
            taken.as_ref(),
            expected.as_slice(),
            "the complete object must have every byte exactly once — none lost, none duplicated"
        );
    }

    /// The bug this reproduces: `take_upload` removed the upload's state
    /// from `self.uploads` BEFORE its own final flush, so a failure there
    /// left the state gone — a retry got `Ok(None)`, which the caller
    /// (`asset::complete_resumable_upload`) surfaces as `NotFound`, and the
    /// asset was stuck `Pending` forever with the real S3 multipart upload
    /// orphaned behind it.
    #[tokio::test]
    async fn take_upload_stays_resumable_when_its_own_final_flush_fails() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());
        resumable.create_upload(key.clone()).await.unwrap();

        // A genuine tail below the floor: `take_upload` must flush it as
        // the final (undersized-is-allowed) part.
        resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from_static(b"a tail"))
            .await
            .unwrap();

        mock.fail_next_upload_part();
        let err = resumable.take_upload(key.clone()).await.unwrap_err();
        assert!(matches!(err, ObjectStoreError::Storage { status: 500 }));

        // Still resumable — not vanished. `upload_offset` proves the state
        // is still there, byte for byte what it was before this call.
        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), Some(6));

        // A retried completion succeeds now that the one-shot failure has
        // fired, and produces the exact original bytes.
        let taken = resumable.take_upload(key).await.unwrap().unwrap();
        assert_eq!(taken.as_ref(), b"a tail");
    }

    /// The same hazard as the final-flush case above, one step later:
    /// `CompleteMultipartUpload` itself fails after every part already
    /// landed successfully (so there is no tail left to flush) — this must
    /// still leave the upload resumable rather than dropping it.
    #[tokio::test]
    async fn take_upload_stays_resumable_when_complete_multipart_fails() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());
        resumable.create_upload(key.clone()).await.unwrap();

        // Exactly one full part, nothing left over — `take_upload` below
        // goes straight to `CompleteMultipartUpload` with no final flush.
        let exact = vec![b'z'; S3_MULTIPART_PART_FLOOR as usize];
        resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from(exact.clone()))
            .await
            .unwrap();

        mock.fail_next_complete_multipart();
        let err = resumable.take_upload(key.clone()).await.unwrap_err();
        assert!(matches!(err, ObjectStoreError::Storage { status: 500 }));
        assert_eq!(
            resumable.upload_offset(key.clone()).await.unwrap(),
            Some(S3_MULTIPART_PART_FLOOR),
            "the upload must still be there for a retry to find"
        );

        let taken = resumable.take_upload(key).await.unwrap().unwrap();
        assert_eq!(taken.as_ref(), exact.as_slice());
    }

    /// The bug this reproduces: `abandon_upload` removed the upload's
    /// state from `self.uploads` BEFORE issuing `AbortMultipartUpload`, so
    /// a failed abort still left the state gone — a retried `DELETE` found
    /// nothing local to act on and returned `Ok(())`, a false "cleaned up"
    /// signal that silently orphaned the real multipart upload on S3.
    #[tokio::test]
    async fn a_retried_abandon_upload_actually_reissues_the_abort() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let resumable = store.as_resumable().unwrap();
        let key = ObjectKey::new(Uuid::new_v4());
        resumable.create_upload(key.clone()).await.unwrap();
        resumable
            .append_upload(key.clone(), 0, bytes::Bytes::from_static(b"partial"))
            .await
            .unwrap();

        mock.fail_next_abort_multipart();
        let err = resumable.abandon_upload(key.clone()).await.unwrap_err();
        assert!(matches!(err, ObjectStoreError::Storage { status: 500 }));
        assert!(
            !mock.was_aborted("mock-upload-1"),
            "the injected failure must not have taken effect server-side"
        );
        // The upload must still be known locally so the retry below can
        // reach a real `AbortMultipartUpload`, not silently no-op.
        assert_eq!(resumable.upload_offset(key.clone()).await.unwrap(), Some(7));

        resumable
            .abandon_upload(key.clone())
            .await
            .expect("the retry hits the store for real and succeeds");
        assert!(
            mock.was_aborted("mock-upload-1"),
            "the retry must have actually reissued AbortMultipartUpload"
        );
        assert_eq!(resumable.upload_offset(key).await.unwrap(), None);
    }

    // -- Defect 2: completing a resumable upload must not re-PUT the object -

    /// The bug this reproduces: `complete_resumable_upload` always handed
    /// the bytes `take_upload` read back to `complete_upload`, which
    /// unconditionally `put`s them again. For the `s3` profile that is a
    /// THIRD transfer of the same bytes over a `CompleteMultipartUpload`
    /// that had already landed the correct object at the exact same key —
    /// wasteful at best, and past S3's 5&nbsp;GiB single-request `PutObject`
    /// cap, an asset whose multipart upload completed correctly would then
    /// fail this redundant `put` and end up `Failed` regardless.
    #[tokio::test]
    async fn resumable_s3_completion_never_re_puts_the_already_committed_object() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let asset_store = crate::asset::InMemoryAssetRecordStore::default();
        let collection = s3_durability_test_collection();
        let policy = crate::asset::AssetPolicy {
            max_asset_bytes: 1_000_000,
            allowed_media_types: None,
        };
        let payload = b"bytes that only ever cross the wire through the multipart lane".to_vec();
        let digest = crate::asset::compute_sha256(&payload);

        let pending = crate::asset::register_managed(
            &asset_store,
            &policy,
            &collection,
            None,
            "thumb",
            crate::asset::RegisterManagedRequest {
                media_type: Some("application/octet-stream".to_string()),
                title: None,
                description: None,
                roles: vec![],
                declared_size: payload.len() as u64,
                digest,
            },
        )
        .await
        .unwrap();

        crate::asset::create_resumable_upload(&asset_store, &store, &collection, None, "thumb")
            .await
            .unwrap();
        crate::asset::append_resumable_upload(
            &asset_store,
            &store,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::from(payload.clone()),
        )
        .await
        .unwrap();

        let available = crate::asset::complete_resumable_upload(
            &asset_store,
            &store,
            &collection,
            None,
            "thumb",
        )
        .await
        .unwrap();
        assert_eq!(available.state, crate::asset::AssetState::Available);
        assert_eq!(available.id, pending.id);

        let object_path = format!("/bucket/{}", pending.id.hyphenated());
        assert_eq!(
            mock.plain_put_count(&object_path),
            0,
            "the multipart upload already committed the object; complete_upload must not PUT it again"
        );
        assert_eq!(
            store.get(ObjectKey::new(pending.id)).await.unwrap(),
            Some(bytes::Bytes::from(payload)),
            "the correct, complete object must still be exactly what was uploaded"
        );
    }

    /// The `fs` profile's own counterpart to the test above: `complete_
    /// upload`'s `put` must still run there — `InMemoryObjectStore` honors
    /// `fs` semantics (`take_upload` only ever reads a separate staging
    /// area, never the real key), so the `already_committed` plumbing
    /// Defect 2's fix threads through `complete_resumable_upload` must not
    /// have quietly turned `false` into `true` for every profile.
    #[tokio::test]
    async fn resumable_fs_completion_still_puts_the_whole_object() {
        let asset_store = crate::asset::InMemoryAssetRecordStore::default();
        let objects = InMemoryObjectStore::default();
        assert!(!objects.take_upload_already_committed());
        let collection = s3_durability_test_collection();
        let policy = crate::asset::AssetPolicy {
            max_asset_bytes: 1_000_000,
            allowed_media_types: None,
        };
        let payload = b"fs profile bytes".to_vec();
        let digest = crate::asset::compute_sha256(&payload);

        crate::asset::register_managed(
            &asset_store,
            &policy,
            &collection,
            None,
            "thumb",
            crate::asset::RegisterManagedRequest {
                media_type: None,
                title: None,
                description: None,
                roles: vec![],
                declared_size: payload.len() as u64,
                digest,
            },
        )
        .await
        .unwrap();

        crate::asset::create_resumable_upload(&asset_store, &objects, &collection, None, "thumb")
            .await
            .unwrap();
        crate::asset::append_resumable_upload(
            &asset_store,
            &objects,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::from(payload.clone()),
        )
        .await
        .unwrap();
        let available = crate::asset::complete_resumable_upload(
            &asset_store,
            &objects,
            &collection,
            None,
            "thumb",
        )
        .await
        .unwrap();

        assert_eq!(available.state, crate::asset::AssetState::Available);
        assert_eq!(
            objects.get(ObjectKey::new(available.id)).await.unwrap(),
            Some(bytes::Bytes::from(payload)),
            "fs's own take_upload never writes the real key itself — this can only be here \
             because complete_upload's put ran"
        );
    }

    /// A digest mismatch on the already-committed (`s3`) path must still
    /// delete the object and fail the record by name — this is the case
    /// `complete_upload`'s cleanup-on-mismatch delete exists for in the
    /// first place (see `finish_upload`'s own doc): `take_upload`'s
    /// `CompleteMultipartUpload` already landed the (wrong) bytes at the
    /// final key before this check ever runs, and skipping the redundant
    /// `put` on the SUCCESS path (Defect 2's fix) must not have also
    /// disturbed the FAILURE path's cleanup.
    #[tokio::test]
    async fn resumable_s3_digest_mismatch_still_deletes_the_already_committed_object() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let asset_store = crate::asset::InMemoryAssetRecordStore::default();
        let collection = s3_durability_test_collection();
        let policy = crate::asset::AssetPolicy {
            max_asset_bytes: 1_000_000,
            allowed_media_types: None,
        };
        let declared_digest = crate::asset::compute_sha256(b"expected bytes");

        let pending = crate::asset::register_managed(
            &asset_store,
            &policy,
            &collection,
            None,
            "thumb",
            crate::asset::RegisterManagedRequest {
                media_type: None,
                title: None,
                description: None,
                roles: vec![],
                declared_size: 14,
                digest: declared_digest,
            },
        )
        .await
        .unwrap();

        crate::asset::create_resumable_upload(&asset_store, &store, &collection, None, "thumb")
            .await
            .unwrap();
        crate::asset::append_resumable_upload(
            &asset_store,
            &store,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::from_static(b"different!!!!!"),
        )
        .await
        .unwrap();

        let err = crate::asset::complete_resumable_upload(
            &asset_store,
            &store,
            &collection,
            None,
            "thumb",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, crate::error::Error::UnprocessableEntity(_)));

        assert!(
            !store.exists(ObjectKey::new(pending.id)).await.unwrap(),
            "the already-committed (wrong) bytes must be cleaned up on a digest mismatch"
        );
        let failed = asset_store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(failed.state, crate::asset::AssetState::Failed);
        assert!(failed.failure_reason.is_some());
    }

    /// Reproduces the successor-admission race the two facts above leave
    /// open when nothing closes the gap between them: `take_upload`
    /// commits a first attempt's (wrong) bytes at the asset's real key
    /// while the record is still `pending` (`finish_upload` only marks it
    /// `failed` afterward, once its own cleanup delete has actually run —
    /// see `complete_upload`'s own doc). Calling `take_upload` directly
    /// here, rather than through `complete_resumable_upload`, holds the
    /// process at exactly that moment — the same window a slow real
    /// `DELETE` over the network leaves open in production, reproduced
    /// deterministically instead of by hoping for a timing accident.
    ///
    /// Without the fix, the object store's own bookkeeping for this key is
    /// gone the instant `take_upload` returns, so `upload_offset` reports
    /// `None` and a second `create_resumable_upload` for the still-pending
    /// asset is wrongly admitted right here — the exact opening a
    /// legitimate successor could be admitted through and have its own
    /// correct bytes destroyed by attempt one's still-pending delete.
    #[tokio::test]
    async fn resumable_s3_successor_upload_stays_refused_while_a_mismatched_attempt_is_still_verifying(
    ) {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        let asset_store = crate::asset::InMemoryAssetRecordStore::default();
        let collection = s3_durability_test_collection();
        let policy = crate::asset::AssetPolicy {
            max_asset_bytes: 1_000_000,
            allowed_media_types: None,
        };
        // `wrong` is derived byte-for-byte from `correct` so the two are
        // guaranteed the same length (both must clear
        // `append_resumable_upload`'s own declared-size cap) while still
        // differing in content — and so in digest — in every position,
        // exactly as attempt one's mistake vs. a corrected retry would.
        let correct = b"the right eventual bytes".to_vec();
        let wrong: Vec<u8> = correct.iter().map(|byte| byte.wrapping_add(1)).collect();
        assert_eq!(correct.len(), wrong.len());
        assert_ne!(correct, wrong);
        let declared_digest = crate::asset::compute_sha256(&correct);

        let pending = crate::asset::register_managed(
            &asset_store,
            &policy,
            &collection,
            None,
            "thumb",
            crate::asset::RegisterManagedRequest {
                media_type: None,
                title: None,
                description: None,
                roles: vec![],
                declared_size: correct.len() as u64,
                digest: declared_digest,
            },
        )
        .await
        .unwrap();
        let key = ObjectKey::new(pending.id);

        // Attempt one uploads the wrong bytes.
        crate::asset::create_resumable_upload(&asset_store, &store, &collection, None, "thumb")
            .await
            .unwrap();
        crate::asset::append_resumable_upload(
            &asset_store,
            &store,
            &collection,
            None,
            "thumb",
            0,
            bytes::Bytes::from(wrong.clone()),
        )
        .await
        .unwrap();

        // Drive exactly the first step `complete_resumable_upload` takes —
        // `take_upload` — directly, parking the test between that commit
        // and the digest check that would normally follow it immediately.
        let taken = store.take_upload(key.clone()).await.unwrap();
        assert_eq!(taken, Some(bytes::Bytes::from(wrong.clone())));
        assert_eq!(
            store.get(key.clone()).await.unwrap(),
            Some(bytes::Bytes::from(wrong)),
            "s3's take_upload already committed the wrong bytes at the real key"
        );
        let record_mid_verification = asset_store
            .get(&collection, None, "thumb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            record_mid_verification.state,
            crate::asset::AssetState::Pending,
            "finalize has not run yet — the delete-then-finalize order is what leaves this \
             window open in the first place"
        );

        // The key assertion: with nothing standing in the way but the
        // object store's own bookkeeping, a second, legitimate attempt for
        // this same still-pending asset must stay refused for as long as
        // attempt one's wrong bytes are uncleaned and unverified.
        let successor =
            crate::asset::create_resumable_upload(&asset_store, &store, &collection, None, "thumb")
                .await;
        assert!(
            matches!(successor, Err(crate::error::Error::Conflict(_))),
            "a successor must stay refused while attempt one's wrong bytes are still \
             uncleaned and unverified at the same key, got {successor:?}"
        );

        // Attempt one now finishes for real: the digest mismatch is
        // detected, the wrong bytes are deleted, the record is marked
        // failed, and the store's hold is released — the same sequence
        // `finish_upload`/`complete_resumable_upload` run, reproduced
        // explicitly here since `finish_upload` is a private `asset`
        // helper this test module cannot call directly.
        store.delete(key.clone()).await.unwrap();
        asset_store
            .finalize(
                &collection,
                None,
                "thumb",
                crate::asset::FinalizeOutcome::Failed {
                    reason: "declared digest does not match the uploaded bytes".to_string(),
                },
            )
            .await
            .unwrap();
        store.release_verifying_upload(key.clone()).await.unwrap();

        // Nothing was ever admitted into the vulnerable window, so there
        // was never a successor's correct bytes for that stale cleanup to
        // destroy — the key is simply gone, exactly as an unraced
        // digest-mismatch cleanup leaves it, and the store's own hold is
        // now genuinely released.
        assert!(!store.exists(key.clone()).await.unwrap());
        assert_eq!(store.upload_offset(key).await.unwrap(), None);
    }

    /// Shared collection fixture for the `s3_tests`-local domain-layer
    /// tests above — the same shape `asset.rs`'s own `collection()` test
    /// helper builds, duplicated locally rather than shared across module
    /// boundaries since neither module's test helpers are `pub`.
    fn s3_durability_test_collection() -> crate::config::CollectionDecl {
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

    // -- golden: presigned URL shape at a fixed clock, no network at all ---

    #[test]
    fn s3_presign_shape_is_deterministic_at_a_fixed_clock() {
        let store = S3ObjectStore::build(
            "https://minio.example.test:9000",
            "photos",
            "eu-west-1",
            "originals/",
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            300,
        )
        .unwrap();
        let id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        let key = ObjectKey::new(id);
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_440_938_160); // 2015-08-30T12:36:00Z

        let put_url = store
            .presign_put(key.clone(), Duration::from_secs(300), now)
            .unwrap();
        assert!(put_url.starts_with(
            "https://minio.example.test:9000/photos/originals/11111111-2222-3333-4444-555555555555?"
        ));
        assert!(put_url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
        assert!(put_url.contains("X-Amz-Expires=300"));
        assert!(put_url
            .contains("X-Amz-Credential=AKIDEXAMPLE%2F20150830%2Feu-west-1%2Fs3%2Faws4_request"));
        assert!(put_url.contains("X-Amz-SignedHeaders=host"));
        assert!(put_url.contains("&X-Amz-Signature="));

        // Same key/clock, a different verb and expiry -> a different
        // signature; `default_expiry()` reflects this store's own config
        // (300s, from `build` above) rather than a hardcoded value.
        assert_eq!(store.default_expiry(), Duration::from_secs(300));
        let get_url = store.presign_get(key, store.default_expiry(), now).unwrap();
        assert!(get_url.contains("X-Amz-Expires=300"));
        assert_ne!(
            put_url.rsplit("X-Amz-Signature=").next(),
            get_url.rsplit("X-Amz-Signature=").next()
        );
    }

    #[test]
    fn as_presigned_is_only_advertised_by_the_s3_profile() {
        let dir = tempfile::tempdir().unwrap();
        let fs_store = FsObjectStore::new(dir.path()).unwrap();
        assert!(fs_store.as_presigned().is_none());

        let mock = MockS3::spawn(MockBehavior::Store);
        let s3_store = store_against(&mock);
        assert!(s3_store.as_presigned().is_some());
    }

    #[test]
    fn new_refuses_a_missing_access_key_environment_variable() {
        // A variable name essentially guaranteed not to be set in any test
        // runner's environment.
        let err = S3ObjectStore::new(
            "http://127.0.0.1:1",
            "bucket",
            "us-east-1",
            "",
            "TELLURION_TEST_NEVER_SET_ACCESS_KEY_9f3c",
            "TELLURION_TEST_NEVER_SET_SECRET_KEY_9f3c",
            900,
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn build_object_store_wires_the_s3_profile() {
        let mock = MockS3::spawn(MockBehavior::Store);
        std::env::set_var("TELLURION_TEST_S3_WIRING_ACCESS", "access");
        std::env::set_var("TELLURION_TEST_S3_WIRING_SECRET", "secret");
        let decl = crate::config::ObjectStoreDecl {
            id: "main".to_string(),
            profile: crate::config::ObjectStoreProfile::S3 {
                endpoint: mock.endpoint(),
                bucket: "bucket".to_string(),
                region: "us-east-1".to_string(),
                key_prefix: String::new(),
                access_key_env: "TELLURION_TEST_S3_WIRING_ACCESS".to_string(),
                secret_key_env: "TELLURION_TEST_S3_WIRING_SECRET".to_string(),
                presign_expiry_s: 900,
            },
        };
        let store = build_object_store(&decl).expect("builds an s3-profile store");
        assert!(store.as_presigned().is_some());
        std::env::remove_var("TELLURION_TEST_S3_WIRING_ACCESS");
        std::env::remove_var("TELLURION_TEST_S3_WIRING_SECRET");
    }

    // -- PathAddressedObjectStore (the iceberg `FileIO` surface, `#123`) -----

    /// The capability is `Option`-shaped and honest about which profile has
    /// it: `s3` does, `fs` deliberately does not (see
    /// `PathAddressedObjectStore`'s own doc — a nested caller-supplied path
    /// under `fs`'s root is exactly the traversal this module refuses).
    #[tokio::test]
    async fn only_the_s3_profile_advertises_the_path_addressed_capability() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let s3 = store_against(&mock);
        assert!(s3.as_path_addressed().is_some());

        let root = tempfile::tempdir().unwrap();
        let fs = FsObjectStore::new(root.path()).unwrap();
        assert!(
            fs.as_path_addressed().is_none(),
            "fs must report the capability absent, not fabricate one"
        );
    }

    /// A path-addressed key is the COMPLETE in-bucket key some other writer
    /// minted — never re-filed under this deployment's own `key_prefix`,
    /// which would name an object that does not exist.
    #[test]
    fn a_path_addressed_key_is_not_re_prefixed_the_way_an_object_key_is() {
        let store = S3ObjectStore::build(
            "http://localhost:9000",
            "lake",
            "us-east-1",
            "assets/",
            "test-access-key",
            "test-secret-key",
            900,
        )
        .unwrap();
        assert_eq!(
            store
                .raw_object_path("warehouse/geo/points/data/x.parquet")
                .unwrap(),
            "/lake/warehouse/geo/points/data/x.parquet"
        );
        // …while an `ObjectKey` still is, unchanged.
        let id = Uuid::new_v4();
        assert_eq!(
            store.object_path(&ObjectKey::new(id)).unwrap(),
            format!("/lake/assets/{}", id.hyphenated())
        );
    }

    #[test]
    fn a_path_addressed_key_that_could_escape_its_own_name_is_refused() {
        let store = S3ObjectStore::build(
            "http://localhost:9000",
            "lake",
            "us-east-1",
            "",
            "test-access-key",
            "test-secret-key",
            900,
        )
        .unwrap();
        for hostile in ["", "/absolute/key", "a/../../b", ".."] {
            assert!(
                matches!(
                    store.raw_object_path(hostile),
                    Err(ObjectStoreError::InvalidKey(_))
                ),
                "must refuse {hostile:?}"
            );
        }
    }

    /// A zero-length range is answered locally, without a request — because
    /// the alternative (omitting `Range`) silently fetches the WHOLE object.
    /// The endpoint here is a port nothing is listening on, so any request
    /// at all would fail rather than return empty bytes.
    #[tokio::test]
    async fn a_zero_length_range_reads_nothing_without_asking_the_store() {
        let store = S3ObjectStore::build(
            // Port 0 is not connectable; reaching the network here fails.
            "http://127.0.0.1:0",
            "lake",
            "us-east-1",
            "",
            "test-access-key",
            "test-secret-key",
            900,
        )
        .unwrap();
        let bytes = store
            .get_path_range("warehouse/x.parquet", 0, 0)
            .await
            .unwrap()
            .unwrap();
        assert!(bytes.is_empty());
    }

    // -- ListableObjectStore (reconcile surface) -----------------------------

    #[tokio::test]
    async fn s3_list_all_reports_every_object_under_the_key_prefix() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = S3ObjectStore::build(
            &mock.endpoint(),
            "bucket",
            "us-east-1",
            "assets/",
            "test-access-key",
            "test-secret-key",
            900,
        )
        .unwrap();
        let in_prefix = ObjectKey::new(Uuid::new_v4());
        store
            .put(in_prefix.clone(), bytes::Bytes::from_static(b"a"))
            .await
            .unwrap();
        // Seeded directly, bypassing `key_prefix` entirely — proves the
        // listing is genuinely prefix-scoped, not just "everything in the
        // bucket".
        mock.seed(&format!("/bucket/{}", Uuid::new_v4()), b"outside");

        let listed = store.as_listable().unwrap().list_all().await.unwrap();
        assert_eq!(listed.len(), 1, "only the in-prefix object is reported");
        assert_eq!(listed[0].id, in_prefix.id());
        assert!(!listed[0].is_staging, "s3 has no staging-file concept");
    }

    #[tokio::test]
    async fn s3_list_all_is_empty_on_a_fresh_bucket() {
        let mock = MockS3::spawn(MockBehavior::Store);
        let store = store_against(&mock);
        assert!(store
            .as_listable()
            .unwrap()
            .list_all()
            .await
            .unwrap()
            .is_empty());
    }

    #[test]
    fn as_listable_is_advertised_by_both_shipped_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let fs_store = FsObjectStore::new(dir.path()).unwrap();
        assert!(fs_store.as_listable().is_some());

        let mock = MockS3::spawn(MockBehavior::Store);
        let s3_store = store_against(&mock);
        assert!(s3_store.as_listable().is_some());
    }

    // -- pure XML parsing (no network) ---------------------------------------

    #[test]
    fn parse_list_bucket_result_extracts_keys_from_an_untruncated_page() {
        let xml = "<?xml version=\"1.0\"?><ListBucketResult>\
             <Contents><Key>assets/one</Key></Contents>\
             <Contents><Key>assets/two</Key></Contents>\
             <IsTruncated>false</IsTruncated></ListBucketResult>";
        let page = parse_list_bucket_result(xml);
        assert_eq!(page.keys, vec!["assets/one", "assets/two"]);
        assert!(!page.is_truncated);
        assert_eq!(page.next_continuation_token, None);
    }

    /// A truncated page must carry a continuation token forward —
    /// `S3ObjectStore::list_all`'s own paging loop relies on this shape to
    /// know both "there is more" and "where to resume from".
    #[test]
    fn parse_list_bucket_result_carries_the_continuation_token_when_truncated() {
        let xml = "<ListBucketResult><Contents><Key>a</Key></Contents>\
             <IsTruncated>true</IsTruncated>\
             <NextContinuationToken>token-123</NextContinuationToken></ListBucketResult>";
        let page = parse_list_bucket_result(xml);
        assert_eq!(page.keys, vec!["a"]);
        assert!(page.is_truncated);
        assert_eq!(page.next_continuation_token.as_deref(), Some("token-123"));
    }

    #[test]
    fn parse_list_bucket_result_on_an_empty_bucket_has_no_keys() {
        let xml = "<ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>";
        let page = parse_list_bucket_result(xml);
        assert!(page.keys.is_empty());
        assert!(!page.is_truncated);
    }

    // -- skippable live-store integration tests -----------------------------
    //
    // Gated on `TELLURION_TEST_S3_ENDPOINT`/`_BUCKET`/`_ACCESS_KEY_ENV`/
    // `_SECRET_KEY_ENV` (the last two name further environment variables
    // holding the actual credentials, the same indirection this profile's
    // own config uses) — unset on a machine with no object store running,
    // so this test skips cleanly with a printed notice rather than failing.

    struct LiveConfig {
        endpoint: String,
        bucket: String,
        access_key_env: String,
        secret_key_env: String,
    }

    fn live_config() -> Option<LiveConfig> {
        Some(LiveConfig {
            endpoint: std::env::var("TELLURION_TEST_S3_ENDPOINT").ok()?,
            bucket: std::env::var("TELLURION_TEST_S3_BUCKET").ok()?,
            access_key_env: std::env::var("TELLURION_TEST_S3_ACCESS_KEY_ENV").ok()?,
            secret_key_env: std::env::var("TELLURION_TEST_S3_SECRET_KEY_ENV").ok()?,
        })
    }

    #[tokio::test]
    async fn live_store_put_get_delete_exists_round_trip() {
        let Some(cfg) = live_config() else {
            eprintln!(
                "skipping live s3 test: TELLURION_TEST_S3_ENDPOINT/_BUCKET/_ACCESS_KEY_ENV/\
                 _SECRET_KEY_ENV are not all set"
            );
            return;
        };
        let store = S3ObjectStore::new(
            &cfg.endpoint,
            cfg.bucket,
            "us-east-1",
            "tellurion-live-test/",
            &cfg.access_key_env,
            &cfg.secret_key_env,
            900,
        )
        .expect("builds against the configured live endpoint");
        let key = ObjectKey::new(Uuid::new_v4());

        assert!(!store.exists(key.clone()).await.unwrap());
        store
            .put(key.clone(), bytes::Bytes::from_static(b"live round trip"))
            .await
            .unwrap();
        assert!(store.exists(key.clone()).await.unwrap());
        assert_eq!(
            store.get(key.clone()).await.unwrap(),
            Some(bytes::Bytes::from_static(b"live round trip"))
        );
        let meta = store.head(key.clone()).await.unwrap().unwrap();
        assert_eq!(meta.size, Some(15));
        store.delete(key.clone()).await.unwrap();
        assert!(!store.exists(key).await.unwrap());
    }
}
