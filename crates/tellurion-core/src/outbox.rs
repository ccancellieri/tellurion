//! The write side of the transactional outbox contract: a mutation goes
//! through [`WriteSink::apply`], which commits the data change and the
//! outbox obligation in one backend transaction — there is deliberately no
//! "write without outbox" method, so the atomicity is the capability itself,
//! not a convention a caller could skip. [`OutboxSource`] is the matching
//! read side an applier drains to keep a derived index (or any other
//! consumer) converging on the primary without a second, independently
//! written source of truth.
//!
//! `IndexSink` is the read-index half of the same contract (`#67`): the
//! applier (`crate::applier`) drains an `OutboxSource` into an `IndexSink`
//! in strict per-collection sequence order. `SearchSource` is the freshness-
//! gated read side a search lane serves from once that index is caught up
//! enough — see `crate::router::Router::resolve_search` for the gate itself
//! (design doc section 4).
//!
//! `Sequence` is a total order *within one collection* — there is no
//! cross-collection ordering, by design (a global obligation table would be
//! a shared write hotspot with no offsetting benefit for a per-tenant
//! catalog).

use std::time::SystemTime;

use async_trait::async_trait;

use crate::config::CollectionDecl;
use crate::crs::RequestedCrs;
use crate::error::Result;
use crate::locking::RowVersion;

/// The single capability name both halves of the atomic optimistic-locking
/// guard refuse under (`#150`) — one name, so an operator reading a refusal
/// learns "this write lane cannot do optimistic locking", not which of two
/// internal methods was missing. Named once here rather than spelled twice,
/// per this workspace's closed-vocabulary rule.
const OPTIMISTIC_LOCKING_CAPABILITY: &str = "optimistic-locking";

/// OGC API Features — Part 4's Create/Replace/Delete requirements class
/// (OGC 20-002r1, Table 2 "Conformance class URIs"), the first of that
/// document's five and the one every other write class is layered on.
///
/// Never static (`#263`): the class's own Requirement 1 clause A —
/// identified `/req/core/methods` in the published text, which is that
/// document's own inconsistency, since every other requirement inside
/// clause 6 is `/req/create-replace-delete/…` — reads "A server SHALL
/// implement one or more of the methods HTTP POST, PUT and/or DELETE for
/// each mutable resource", and whether this deployment has a mutable
/// resource at all is a routing fact, not a build fact. Earned per
/// deployment by [`crate::router::Router::create_replace_delete_conformance_classes`],
/// the same way `conf/features` and `conf/update` below already are.
pub const CREATE_REPLACE_DELETE_CONFORMANCE_CLASS: &str =
    "http://www.opengis.net/spec/ogcapi-features-4/1.0/conf/create-replace-delete";

/// OGC API Features Part 4's RFC 7396 Update conformance class. It is
/// declared per driver through [`WriteSink::update_conformance_classes`].
pub const UPDATE_CONFORMANCE_CLASS: &str =
    "http://www.opengis.net/spec/ogcapi-features-4/1.0/conf/update";

/// OGC API Features — Part 4's feature-body requirements class. Unlike the
/// protocol crate's static classes, this one is earned by a resolved write
/// sink for a specific collection because default-CRS transformation can
/// depend on that collection's storage CRS.
pub const FEATURES_PART4_FEATURES_CLASS: &str =
    "http://www.opengis.net/spec/ogcapi-features-4/1.0/conf/features";

/// A monotonic, per-collection commit order. Gaps are allowed (it is an
/// order, not a dense counter); what matters is that a later commit compares
/// greater than an earlier one and [`OutboxSource::read_after`] returns
/// every obligation in ascending order, never skipping or reordering one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Sequence(pub u64);

/// One item's mutation: replace it wholesale with `Upsert`'s payload, or
/// remove it with `Delete`. There is no partial-update variant in this first
/// slice — a caller that wants to change one property re-sends the whole
/// feature, the same "PUT replaces" semantics the write endpoint itself
/// exposes.
#[derive(Debug, Clone, PartialEq)]
pub enum MutationKind {
    /// The GeoJSON Feature to derive the stored row (and, eventually, any
    /// derived index document) from.
    Upsert(serde_json::Value),
    Delete,
}

/// A caller's request to `WriteSink::apply`: which item, and what to do with
/// it.
#[derive(Debug, Clone, PartialEq)]
pub struct Mutation {
    /// Stable item identity within the collection — the same id space the
    /// `FeatureSource` item lane already uses (`items/{id}`).
    pub feature_id: String,
    pub kind: MutationKind,
}

/// The write side of the outbox contract. `apply` performs the data
/// mutation AND appends the outbox obligation in ONE backend transaction,
/// returning the sequence it committed at — never two round-trips, never a
/// method that writes the data alone. A driver that cannot uphold this
/// atomicity has nothing honest to advertise here at all.
#[async_trait]
pub trait WriteSink: Send + Sync {
    async fn apply(&self, collection: &CollectionDecl, mutation: Mutation) -> Result<Sequence>;

    /// Creates a new item with a server-assigned id (`#88`, `POST
    /// /collections/{cid}/items`) — the counterpart of [`apply`](Self::apply)
    /// for a caller that doesn't yet know `Mutation::feature_id`, since
    /// `apply`'s own signature requires the caller to already have decided
    /// it (correct for `PUT`'s "id is caller-supplied" upsert/delete
    /// contract, but a server-assigned create has no id to supply before the
    /// row exists). This is not a second, independent write path: a real
    /// implementer still commits the data insert and the outbox obligation
    /// in the SAME one-transaction atomicity `apply` gives, it just decides
    /// the id itself instead of taking one as input. Returns the minted id
    /// alongside the sequence the transaction committed at.
    ///
    /// Default: refuses by name with `Error::CapabilityUnsupported {
    /// capability: "create" }` — the Result-shaped counterpart of
    /// [`FeatureSource::filter_capable`](crate::storage::FeatureSource::filter_capable)'s
    /// default `false`, for a write sink that was never asked to support a
    /// server-assigned create. A real implementer overrides this entirely;
    /// `collection` is only used to name the collection in that default
    /// refusal.
    async fn create(
        &self,
        collection: &CollectionDecl,
        _feature: serde_json::Value,
    ) -> Result<(String, Sequence)> {
        Err(crate::error::Error::CapabilityUnsupported {
            collection: collection.id.clone(),
            capability: "create".to_string(),
        })
    }

    /// Whether this sink can accept a `Content-Crs`-declared geometry
    /// expressed in a collection's own storage CRS rather than the default
    /// CRS84 (OGC API Features Part 4, `/req/features/crs-other-crs`) — the
    /// write-side counterpart of
    /// [`FeatureSource::crs_capable`](crate::storage::FeatureSource::crs_capable),
    /// same `runtime_checkable`-style marker and the same reasoning:
    /// reprojecting an inbound geometry into storage CRS is a refinement of
    /// "can write at all," not a capability with its own resolve entry
    /// point. Default `false`; PostGIS overrides this to `true`.
    /// `tellurion-features`'s write handlers check this before honoring a
    /// `Content-Crs` header naming anything but CRS84, refusing with a named
    /// 400 rather than silently storing coordinates under the wrong SRID —
    /// the same silent-corruption failure mode a driver that ignored this
    /// header entirely would otherwise produce.
    fn crs_capable(&self) -> bool {
        false
    }

    /// Part 4 feature-body conformance classes this sink satisfies for
    /// `collection`. The collection parameter is load-bearing: a driver may
    /// support default CRS84 writes only for a bounded set of storage CRSs.
    /// The default is conservative; sinks opt in only when the complete
    /// Requirements Class is satisfied for this collection.
    fn features_conformance_classes(&self, _collection: &CollectionDecl) -> Vec<&'static str> {
        Vec::new()
    }

    /// The OGC API Features — Part 4 (20-002r1, draft) Optimistic Locking,
    /// ETags requirement class (`#107`) this sink's own `apply`/`create`
    /// genuinely honor, paired with this collection's read lane
    /// (`FeatureSource::item`) — the write-lane counterpart of
    /// [`FeatureSource::cql2_conformance_classes`](crate::storage::FeatureSource::cql2_conformance_classes),
    /// same per-driver declared-subset shape and the same reason it exists
    /// separately from a single `bool`: a future sink could conceivably
    /// commit a write without the freshly-read stored representation
    /// afterward reflecting it byte-for-byte (an eventually-consistent or
    /// write-behind store), which would make `tellurion-features`'
    /// generic If-Match/ETag guard (read current state, hash it, compare)
    /// unsound for that sink even though the guard's own code is identical
    /// for every driver. Unlike CQL2, where a driver's own SQL compiler
    /// genuinely earns a narrower or wider subset, every real implementer
    /// in this workspace either commits synchronously (PostGIS, GeoPackage:
    /// both return `vec![tellurion_core::locking::OPTIMISTIC_LOCKING_ETAGS_CLASS]`)
    /// or, by declining to override this at all, honestly reports it cannot
    /// (the default, `Vec::new()`).
    ///
    /// Read by [`crate::router::Router::locking_conformance_classes`] (the
    /// workspace-wide intersection across every write-capable driver "in
    /// use," folded into a deployment's `/conformance` response) and by
    /// `Router::canonical_descriptor` (this specific collection's own
    /// answer, gated on its write lane actually resolving to this sink AND
    /// its features lane resolving too — the guard needs both). Never
    /// includes the Timestamps class: that one is a per-collection
    /// declaration (`CollectionDecl::modified_column.is_some()`), not
    /// something any driver earns or withholds — see `locking`'s own module
    /// doc.
    ///
    /// `#150` narrowed what earns this: committing synchronously is
    /// necessary but NOT sufficient. Both classes exist to stop a lost
    /// update, and a guard evaluated in Rust before the write transaction
    /// opens cannot — two writers whose checks both pass before either
    /// applies both commit. A sink therefore only declares this once it
    /// also implements [`row_version`](Self::row_version) and
    /// [`apply_conditional`](Self::apply_conditional), which move the
    /// decisive comparison into the write statement itself.
    fn locking_conformance_classes(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// Mints an opaque [`RowVersion`] witness for `feature_id`'s current
    /// stored row, or `Ok(None)` when no such row exists (`#150`) — the
    /// capture half of the atomic optimistic-locking guard.
    ///
    /// Callers MUST read this BEFORE the representation they hash into an
    /// ETag, never after. A witness taken first can only ever be older than
    /// the state that was hashed, so a write that lands between the two
    /// reads makes the witness stale and the guard refuses; a witness taken
    /// afterwards could be NEWER than the hashed state, which would let
    /// exactly the write this guard exists to stop slip through.
    ///
    /// Default: refuses by name with `Error::CapabilityUnsupported` — the
    /// same "honestly decline rather than fake it" shape
    /// [`create`](Self::create) uses. A sink that cannot mint a witness must
    /// not silently fall back to the racy pre-transaction check, so there is
    /// deliberately no `Ok(None)`-shaped "I have no witness" answer here:
    /// `Ok(None)` means "no such row", nothing else.
    async fn row_version(
        &self,
        collection: &CollectionDecl,
        _feature_id: &str,
    ) -> Result<Option<RowVersion>> {
        Err(crate::error::Error::CapabilityUnsupported {
            collection: collection.id.clone(),
            capability: OPTIMISTIC_LOCKING_CAPABILITY.to_string(),
        })
    }

    /// [`apply_with_crs`](Self::apply_with_crs) guarded by `expected`
    /// (`#150`): the implementer re-verifies the witness as a predicate the
    /// BACKEND evaluates atomically with the write — a `WHERE` term on the
    /// mutating statement, not a second round trip — so no concurrent writer
    /// can slip between the check and the apply.
    ///
    /// - `Ok(Some(sequence))` — the row still carried `expected`; the data
    ///   mutation and the outbox obligation committed exactly as
    ///   `apply_with_crs` would have.
    /// - `Ok(None)` — somebody else got there first. NOTHING was written
    ///   (the transaction rolls back, no outbox obligation, no data change).
    ///   An ordinary outcome, not an error, for the same reason
    ///   [`crate::lease::Lease::try_acquire`] reserves `Err` for "I could
    ///   not find out": a caller must never be able to confuse "the
    ///   precondition no longer holds" with "the database was unreachable",
    ///   and the return type — not a convention — is what keeps it honest.
    ///   The caller maps this to `412 Precondition Failed`.
    /// - `Err(_)` — the write could not be attempted at all.
    ///
    /// Only ever called with a precondition a caller already evaluated and
    /// found satisfied; a request carrying none goes through
    /// [`apply_with_crs`](Self::apply_with_crs) unchanged.
    ///
    /// Default: refuses by name, exactly as
    /// [`row_version`](Self::row_version) does and for the same reason.
    async fn apply_conditional(
        &self,
        collection: &CollectionDecl,
        _mutation: Mutation,
        _requested_crs: RequestedCrs,
        _expected: &RowVersion,
    ) -> Result<Option<Sequence>> {
        Err(crate::error::Error::CapabilityUnsupported {
            collection: collection.id.clone(),
            capability: OPTIMISTIC_LOCKING_CAPABILITY.to_string(),
        })
    }

    /// Part 4 Update classes this synchronous sink can support when paired
    /// with its driver's `FeatureSource`. The protocol handler performs the
    /// RFC 7396 merge; this declaration confirms read-after-write can supply
    /// the committed response representation and validators. Default empty.
    fn update_conformance_classes(&self) -> Vec<&'static str> {
        Vec::new()
    }

    /// [`apply`](Self::apply) with an explicit declared input CRS — the
    /// write-lane mirror of
    /// [`FeatureSource::item_with_crs`](crate::storage::FeatureSource::item_with_crs):
    /// a driver whose [`crs_capable`](Self::crs_capable) stays `false` never
    /// needs to know about this at all, so the default implementation
    /// ignores `requested_crs` entirely and delegates to `apply` — correct
    /// because the caller (`tellurion-features`'s write handlers) already
    /// refuses a non-CRS84 `Content-Crs` before this is ever reached for
    /// such a driver. PostGIS overrides both together.
    async fn apply_with_crs(
        &self,
        collection: &CollectionDecl,
        mutation: Mutation,
        _requested_crs: RequestedCrs,
    ) -> Result<Sequence> {
        self.apply(collection, mutation).await
    }

    /// [`create`](Self::create)'s counterpart of
    /// [`apply_with_crs`](Self::apply_with_crs) — same default-ignores-and-
    /// delegates shape, for the server-assigned-id create path.
    async fn create_with_crs(
        &self,
        collection: &CollectionDecl,
        feature: serde_json::Value,
        _requested_crs: RequestedCrs,
    ) -> Result<(String, Sequence)> {
        self.create(collection, feature).await
    }

    /// Applies every `mutations` entry as ONE chunk of a batch ingest
    /// (`#114`): all of it commits — or, for a refused item, is cleanly
    /// discarded — inside a SINGLE backend transaction, via a per-item
    /// SAVEPOINT (or the driver's native equivalent) nested inside that one
    /// transaction. This is still [`apply`](Self::apply)'s own atomicity
    /// contract (data + outbox, one transaction), just amortized over many
    /// items instead of one — the batch lane is a *pacing* layer over this
    /// same write path, never a second one with its own semantics.
    ///
    /// Every mutation in `mutations` is caller-supplied-id (`Mutation::
    /// feature_id` is always already set): batch ingest never mints a
    /// server-assigned id (see [`create`](Self::create)'s own doc for why
    /// that's a different shape of write, and the GeoJSON wire format this
    /// lane accepts has no delete representation of its own, so every
    /// mutation here is an `Upsert`). Each is therefore an idempotent
    /// upsert under `apply`'s own "same id twice converges" rule — exactly
    /// what makes it safe for a caller to re-send an unapplied tail after a
    /// network cut, with no server-side resume state anywhere in this
    /// contract.
    ///
    /// `strict`: `false` attempts every mutation in `mutations` regardless
    /// of any earlier refusal in this same call, returning one
    /// [`BatchItemResult`] per input mutation, in the same order. `true`
    /// stops attempting further mutations the moment one is refused — the
    /// transaction still commits everything successfully applied UP TO that
    /// point (nothing already committed is ever rolled back over a LATER
    /// item's refusal), and the returned `Vec` is therefore SHORTER than
    /// `mutations` by however many were never attempted; the caller (which
    /// still has the original `mutations` in order) is responsible for
    /// reporting that remainder as unapplied and, per this trait's own
    /// no-resume-state contract, for stopping the batch entirely rather
    /// than sending further chunks.
    ///
    /// The outer `Result` is for a failure that isn't attributable to any
    /// one mutation — the transaction itself never opening, or its final
    /// `COMMIT` failing (a pool exhausted, a connection dropped) — in which
    /// case NONE of `mutations` committed, batch or otherwise (dropping a
    /// transaction without an explicit commit rolls it back in full); a
    /// per-item validation or constraint failure is always `Ok` with that
    /// item's own `BatchItemOutcome::Refused` instead, never bubbled up as
    /// this outer `Err`.
    ///
    /// Default: refuses by name with `Error::CapabilityUnsupported {
    /// capability: "batch_apply" }` — the same "honestly decline rather than
    /// fake it" default [`create`](Self::create) uses. A driver that hasn't
    /// implemented the per-item-savepoint machinery this needs has nothing
    /// sound to offer here, and a naive N-single-item-transaction fallback
    /// would silently break this method's own one-transaction-per-chunk
    /// contract instead of admitting it.
    async fn apply_batch(
        &self,
        collection: &CollectionDecl,
        mutations: Vec<Mutation>,
        _requested_crs: RequestedCrs,
        _strict: bool,
    ) -> Result<Vec<BatchItemResult>> {
        let _ = mutations;
        Err(crate::error::Error::CapabilityUnsupported {
            collection: collection.id.clone(),
            capability: "batch_apply".to_string(),
        })
    }
}

/// One input [`Mutation`]'s outcome from [`WriteSink::apply_batch`], paired
/// with the id it was submitted under — a batch has no per-item path
/// segment to read that back from the way a single `PUT`/`DELETE`'s `{fid}`
/// does, so it travels on the result instead.
#[derive(Debug)]
pub struct BatchItemResult {
    pub feature_id: String,
    pub outcome: BatchItemOutcome,
}

/// One mutation's outcome inside a batch chunk apply (`#114`) — the
/// transactional-outbox contract's bulk-pacing counterpart of
/// [`WriteSink::apply`]'s single `Result<Sequence>`: a whole chunk of
/// mutations commits in ONE transaction ([`WriteSink::apply_batch`]'s own
/// doc), but a caller loading a real, imperfect dataset still needs to know
/// WHICH rows in that chunk actually stuck and which didn't.
#[derive(Debug)]
pub enum BatchItemOutcome {
    /// This mutation's row and outbox obligation committed inside the
    /// chunk's shared transaction, at this `Sequence`.
    Applied(Sequence),
    /// This mutation was attempted and refused — the identical
    /// `tellurion_core::Error` a single `PUT` against the same bad input
    /// would have produced, so `Problem::from_core_error` maps it the same
    /// way for either lane. Rolls back only this one mutation's own
    /// savepoint, never anything else already committed in the chunk.
    Refused(crate::error::Error),
}

/// Where a mutated feature's geometry was, and where it now is, expressed in
/// CRS84 (`#141`, `#142`) — recorded by the storage itself, inside the SAME
/// transaction that performed the mutation, and carried on the obligation
/// rather than re-derived by a consumer.
///
/// ## Why a consumer cannot derive this itself
///
/// An [`Obligation`]'s `Upsert` payload is the client-submitted feature
/// verbatim, in whatever CRS that client declared on `Content-Crs` (OGC API
/// Features Part 4, `#116`): CRS84 by default, but equally the collection's
/// own storage CRS — which may be projected (metres, `#142`) or merely
/// authority-ordered latitude-before-longitude (EPSG:4326, see
/// [`crate::crs`]'s "Axis order" section). The payload carries no record of
/// which, so reading its coordinates as CRS84 is a **guess**, and a wrong
/// guess maps a write to the wrong tile-cache buckets — which does not fail
/// loudly, it serves a stale tile with a `200` forever. A `Delete` payload is
/// worse still: it is `NULL`, so a consumer has no geometry at all for the
/// feature that just vanished (`#141`).
///
/// The storage, by contrast, knows exactly what it stored and in which CRS,
/// and can express both in CRS84 without any consumer growing a projection
/// dependency. So it does, once, at write time.
///
/// ## Reading it
///
/// [`Unrecorded`](Self::Unrecorded) is NOT "no geometry" — it is "this
/// storage did not record an answer": an outbox row written before the
/// extent column existed, or a driver that cannot express its storage CRS in
/// CRS84. A consumer must treat it as *unknown* and degrade conservatively,
/// never as an empty extent. `Crs84 { prior: None, current: None }`, by
/// contrast, IS an answer: the feature genuinely had no geometry before this
/// mutation and has none after it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ObligationExtent {
    /// The storage recorded nothing for this obligation — unknown, never
    /// empty. The default, so an obligation built by a producer that never
    /// heard of this field says the honest thing.
    #[default]
    Unrecorded,
    /// `[minlon, minlat, maxlon, maxlat]` for the feature's geometry as it
    /// stood immediately BEFORE this mutation (`prior`) and as it stands
    /// immediately AFTER it (`current`), both in CRS84 (longitude first).
    /// `None` on either side means "there was genuinely no geometry there" —
    /// a feature that did not exist yet, one whose geometry column is
    /// `NULL`, or (for `current`) one this obligation deleted.
    Crs84 {
        prior: Option<[f64; 4]>,
        current: Option<[f64; 4]>,
    },
}

/// One committed change, as an [`OutboxSource`] reads it back out of the
/// outbox.
#[derive(Debug, Clone, PartialEq)]
pub struct Obligation {
    pub sequence: Sequence,
    pub feature_id: String,
    pub kind: MutationKind,
    /// The CRS84 extents this mutation moved the feature between (`#141`,
    /// `#142`) — see [`ObligationExtent`] for why this travels on the
    /// obligation instead of being derived from `kind`'s payload.
    /// [`ObligationExtent::Unrecorded`] for every outbox row written before
    /// the extent column existed, which is exactly the signal a consumer
    /// needs to fall back rather than guess.
    pub extent: ObligationExtent,
    /// Dedup/version stamp a derived-index consumer applies idempotently
    /// against: a document store only overwrites when the incoming version
    /// exceeds the one it already has stored, so a re-delivered or
    /// out-of-order obligation converges rather than corrupts. In this first
    /// slice `version` is simply the committing [`Sequence`] — see the
    /// design doc's open questions for when the two might need to diverge.
    pub version: Sequence,
    /// When this obligation committed (`"<table>_outbox".committed_at`,
    /// `DEFAULT now()`) — carried on the record itself rather than through a
    /// second query, per this module's own "the record grows, a sibling log
    /// never appears" rule (`#115`): the change-feed/webhook consumers need
    /// a timestamp in their compact envelope and the outbox already stores
    /// one, so this struct grows a field instead of either lane inventing
    /// its own way to ask for it.
    pub committed_at: SystemTime,
}

/// The read side of a source-of-truth storage's outbox — what an applier (or
/// any other at-least-once consumer) drains.
#[async_trait]
pub trait OutboxSource: Send + Sync {
    /// Obligations with sequence strictly greater than `after`, ascending,
    /// at most `limit`. MUST NOT skip or reorder. `Ok(vec![])` means "caught
    /// up" — there is nothing newer than `after` right now.
    async fn read_after(
        &self,
        collection: &CollectionDecl,
        after: Sequence,
        limit: u32,
    ) -> Result<Vec<Obligation>>;

    /// The highest sequence committed to the source of truth for this
    /// collection — the primary high-water mark a freshness check would
    /// compare a derived index's own applied high-water mark against.
    async fn primary_high_water(&self, collection: &CollectionDecl) -> Result<Sequence>;

    /// Removes every obligation with `sequence <= floor` from this
    /// collection's outbox, in bounded batches of at most `batch_size` rows
    /// per call, returning how many rows this call actually removed (`0`
    /// means either nothing qualified or the outbox was already pruned up to
    /// `floor`). `floor` is always a
    /// [`crate::retention`]-computed consumer-aware bound, never a bare TTL —
    /// see that module's own doc for why pruning past a registered
    /// consumer's own cursor is never silently attempted by a well-behaved
    /// caller (this method trusts `floor`, it does not re-derive it).
    ///
    /// Default: refuses by name with `Error::CapabilityUnsupported {
    /// capability: "outbox-retention" }`, the same "narrow, opt-in
    /// capability a driver earns by overriding this" shape
    /// [`WriteSink::create`]'s own default uses — a driver that never
    /// implements pruning changes nothing about its existing behavior by
    /// this method existing at all.
    async fn prune_before(
        &self,
        collection: &CollectionDecl,
        _floor: Sequence,
        _batch_size: u32,
    ) -> Result<u64> {
        Err(crate::error::Error::CapabilityUnsupported {
            collection: collection.id.clone(),
            capability: "outbox-retention".to_string(),
        })
    }
}

/// A derived read index (`#67`) — never a source of truth, always
/// rebuildable from the primary. `apply` MUST be idempotent: applying the
/// same [`Obligation`] twice (at-least-once redelivery — a crash between
/// apply and cursor advance, a replayed gap, a retried batch) converges to
/// the same stored state rather than corrupting it. The dedup mechanism is
/// a version compare, not a blind upsert: a sink only overwrites a
/// `feature_id`'s stored document when the incoming `obligation.version`
/// exceeds the one already stored — see `Obligation::version`'s own doc. A
/// `Delete` is itself a versioned tombstone (never a row deletion), so an
/// out-of-order or replayed delete can neither resurrect a newer write nor
/// drop one.
///
/// `applied_high_water` is this sink's own durable high-water mark: the
/// highest primary [`Sequence`] it has durably applied. It MUST be
/// consistent with what `apply` has actually committed — read back right
/// after an `apply` returns `Ok`, it must reflect that obligation — since
/// the applier (`crate::applier`) uses it, unmodified, as its restart-safe
/// resume cursor: a crash between an `apply` and any bookkeeping the sink
/// does internally must never leave `applied_high_water` ahead of what was
/// actually durably applied (that would silently skip an obligation on
/// restart, the one thing this whole contract forbids).
#[async_trait]
pub trait IndexSink: Send + Sync {
    async fn apply(&self, collection: &CollectionDecl, obligation: &Obligation) -> Result<()>;

    async fn applied_high_water(&self, collection: &CollectionDecl) -> Result<Sequence>;
}

/// A search request against a derived index (`#67`). Deliberately minimal —
/// a page size and, since `#181`, an optional free-text query; no
/// filter/bbox/datetime — because a caller with anything richer to ask has
/// no honest way to know a given `SearchSource` would actually honor it:
/// `FeatureSource` solves that with per-capability markers
/// (`filter_capable`, `crs_capable`) precisely so a driver that can't
/// compile a filter is never silently handed one; reusing
/// `crate::storage::ItemsQuery` here instead of this narrower type would
/// have meant either building that same negotiation twice or letting a
/// caller's filter/bbox silently do nothing. Widening this further is
/// future work, not something to fake now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub limit: u32,
    /// Free-text query over the derived index's text content (`#181`).
    /// `None` means "no text predicate" — byte-for-byte the pre-`#181`
    /// behavior, per that grown-field-changes-nothing rule
    /// [`Obligation::committed_at`] already follows. A caller MUST check
    /// [`SearchSource::text_search_capable`] before ever setting this: the
    /// exact same negotiation `FeatureSource::filter_capable` exists for —
    /// a source that never opted in is never silently handed a text
    /// predicate it would have to drop (a dropped predicate makes the
    /// result set describe a different selection than the request named,
    /// the one thing `#181`'s agreement gates forbid).
    pub q: Option<String>,
}

/// One page of search results, as a [`SearchSource`] answers them — the
/// derived index's stored documents, not a fresh read of the primary.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchPage {
    pub features_geojson: Vec<serde_json::Value>,
}

/// Freshness-aware search reads over a derived index (`#67`, design doc
/// section 3.2/4) — the capability a search lane serves from once
/// `crate::router::Router::resolve_search`'s freshness gate finds the
/// routed index caught up enough. `applied_high_water` MUST agree with
/// [`IndexSink::applied_high_water`] for the same collection whenever both
/// are advertised by the same driver (the only case in this workspace
/// today) — it lives on this trait too, rather than forcing the freshness
/// gate to separately resolve `index_sink()` just to read the cursor.
#[async_trait]
pub trait SearchSource: Send + Sync {
    async fn search(&self, collection: &CollectionDecl, query: &SearchQuery) -> Result<SearchPage>;

    async fn applied_high_water(&self, collection: &CollectionDecl) -> Result<Sequence>;

    /// Whether this source can honor [`SearchQuery::q`] — a free-text
    /// predicate compiled against the derived index's own text index
    /// (`#181`; PostGIS: a `tsvector`/GIN column on `"<table>_index"`).
    /// Same `runtime_checkable`-style marker shape as
    /// [`FeatureSource::filter_capable`](crate::storage::FeatureSource::filter_capable),
    /// and for the same reason: the dispatch path checks this BEFORE
    /// building a `q`-bearing query, so a source that can't compile the
    /// predicate is refused by name upstream, never silently handed a `q`
    /// it would have to drop or approximate. Default `false` — a source
    /// that never opted in changes nothing about its existing behavior by
    /// this method existing at all; PostGIS overrides this to `true`.
    fn text_search_capable(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    /// A `WriteSink` that only implements `apply` — every driver in this
    /// workspace that predates `#88` (and any future one that never opts
    /// into server-assigned create).
    struct ApplyOnlySink;

    #[async_trait]
    impl WriteSink for ApplyOnlySink {
        async fn apply(
            &self,
            _collection: &CollectionDecl,
            _mutation: Mutation,
        ) -> Result<Sequence> {
            Ok(Sequence(1))
        }
    }

    fn collection() -> CollectionDecl {
        serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main\n").unwrap()
    }

    #[tokio::test]
    async fn create_defaults_to_a_named_capability_unsupported_refusal() {
        let sink = ApplyOnlySink;
        let result = sink
            .create(&collection(), serde_json::json!({"type": "Feature"}))
            .await;
        match result {
            Err(Error::CapabilityUnsupported {
                collection,
                capability,
            }) => {
                assert_eq!(collection, "demo");
                assert_eq!(capability, "create");
            }
            other => panic!("expected a named CapabilityUnsupported refusal, got {other:?}"),
        }
    }

    #[test]
    fn crs_capable_defaults_to_false() {
        assert!(!ApplyOnlySink.crs_capable());
    }

    #[test]
    fn features_conformance_defaults_to_empty() {
        assert!(ApplyOnlySink
            .features_conformance_classes(&collection())
            .is_empty());
    }

    #[tokio::test]
    async fn apply_with_crs_ignores_the_requested_crs_and_delegates_to_apply() {
        let sink = ApplyOnlySink;
        let result = sink
            .apply_with_crs(
                &collection(),
                Mutation {
                    feature_id: "1".to_string(),
                    kind: MutationKind::Delete,
                },
                RequestedCrs::Storage,
            )
            .await;
        assert_eq!(result.unwrap(), Sequence(1));
    }

    #[tokio::test]
    async fn create_with_crs_ignores_the_requested_crs_and_delegates_to_create() {
        let sink = ApplyOnlySink;
        let result = sink
            .create_with_crs(
                &collection(),
                serde_json::json!({"type": "Feature"}),
                RequestedCrs::Storage,
            )
            .await;
        // `create` itself is unimplemented on this fake, so its own default
        // (named `CapabilityUnsupported`) is what should surface — proving
        // `create_with_crs`'s default really does delegate to `create`
        // rather than doing something else with the CRS it was handed.
        assert!(matches!(
            result,
            Err(Error::CapabilityUnsupported { capability, .. }) if capability == "create"
        ));
    }

    /// `#150`: a sink that never implemented the atomic guard must refuse
    /// by name rather than answer something a caller could mistake for "no
    /// row" — the whole point of `row_version` returning `Err` here instead
    /// of `Ok(None)`.
    #[tokio::test]
    async fn row_version_defaults_to_a_named_capability_unsupported_refusal() {
        let result = ApplyOnlySink.row_version(&collection(), "1").await;
        match result {
            Err(Error::CapabilityUnsupported {
                collection,
                capability,
            }) => {
                assert_eq!(collection, "demo");
                assert_eq!(capability, "optimistic-locking");
            }
            other => panic!("expected a named CapabilityUnsupported refusal, got {other:?}"),
        }
    }

    /// `#150`: and the apply half refuses under the SAME capability name —
    /// an operator reading either refusal learns the same fact about the
    /// write lane.
    #[tokio::test]
    async fn apply_conditional_defaults_to_a_named_capability_unsupported_refusal() {
        let result = ApplyOnlySink
            .apply_conditional(
                &collection(),
                Mutation {
                    feature_id: "1".to_string(),
                    kind: MutationKind::Delete,
                },
                RequestedCrs::Omitted,
                &RowVersion::new("42"),
            )
            .await;
        match result {
            Err(Error::CapabilityUnsupported {
                collection,
                capability,
            }) => {
                assert_eq!(collection, "demo");
                assert_eq!(capability, "optimistic-locking");
            }
            other => panic!("expected a named CapabilityUnsupported refusal, got {other:?}"),
        }
    }

    /// `#150`: crucially, the default must NOT silently degrade to the racy
    /// `apply` path — a sink that inherits it writes nothing at all.
    #[tokio::test]
    async fn apply_conditional_default_never_falls_back_to_an_unguarded_apply() {
        struct CountingSink(std::sync::atomic::AtomicUsize);

        #[async_trait]
        impl WriteSink for CountingSink {
            async fn apply(
                &self,
                _collection: &CollectionDecl,
                _mutation: Mutation,
            ) -> Result<Sequence> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Sequence(1))
            }
        }

        let sink = CountingSink(std::sync::atomic::AtomicUsize::new(0));
        let _ = sink
            .apply_conditional(
                &collection(),
                Mutation {
                    feature_id: "1".to_string(),
                    kind: MutationKind::Delete,
                },
                RequestedCrs::Omitted,
                &RowVersion::new("42"),
            )
            .await;
        assert_eq!(
            sink.0.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the refusing default must never reach the unguarded write path"
        );
    }

    /// An `OutboxSource` that only implements the two required methods —
    /// every driver in this workspace that predates `#115`'s retention
    /// floor, and any future one that never opts into pruning.
    struct ReadOnlyOutbox;

    #[async_trait]
    impl OutboxSource for ReadOnlyOutbox {
        async fn read_after(
            &self,
            _collection: &CollectionDecl,
            _after: Sequence,
            _limit: u32,
        ) -> Result<Vec<Obligation>> {
            Ok(Vec::new())
        }

        async fn primary_high_water(&self, _collection: &CollectionDecl) -> Result<Sequence> {
            Ok(Sequence(0))
        }
    }

    #[tokio::test]
    async fn prune_before_defaults_to_a_named_capability_unsupported_refusal() {
        let source = ReadOnlyOutbox;
        let result = source.prune_before(&collection(), Sequence(10), 100).await;
        match result {
            Err(Error::CapabilityUnsupported {
                collection,
                capability,
            }) => {
                assert_eq!(collection, "demo");
                assert_eq!(capability, "outbox-retention");
            }
            other => panic!("expected a named CapabilityUnsupported refusal, got {other:?}"),
        }
    }

    /// A `SearchSource` that only implements the two required methods —
    /// any implementer that predates `#181`'s free-text slice (or never
    /// opts into it).
    struct PlainSearchSource;

    #[async_trait]
    impl SearchSource for PlainSearchSource {
        async fn search(
            &self,
            _collection: &CollectionDecl,
            _query: &SearchQuery,
        ) -> Result<SearchPage> {
            Ok(SearchPage {
                features_geojson: Vec::new(),
            })
        }

        async fn applied_high_water(&self, _collection: &CollectionDecl) -> Result<Sequence> {
            Ok(Sequence(0))
        }
    }

    /// `#181`: a source that never opted into free text never advertises
    /// it — the dispatch path's capability check is what keeps a `q` from
    /// ever reaching such a source, so the default must be the honest "no".
    #[test]
    fn text_search_capable_defaults_to_false() {
        assert!(!PlainSearchSource.text_search_capable());
    }

    #[tokio::test]
    async fn apply_batch_defaults_to_a_named_capability_unsupported_refusal() {
        let sink = ApplyOnlySink;
        let mutations = vec![Mutation {
            feature_id: "1".to_string(),
            kind: MutationKind::Delete,
        }];
        let result = sink
            .apply_batch(&collection(), mutations, RequestedCrs::Omitted, false)
            .await;
        match result {
            Err(Error::CapabilityUnsupported {
                collection,
                capability,
            }) => {
                assert_eq!(collection, "demo");
                assert_eq!(capability, "batch_apply");
            }
            other => panic!("expected a named CapabilityUnsupported refusal, got {other:?}"),
        }
    }
}
