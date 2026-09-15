//! The seam between protocol handlers and concrete storage. Handlers call
//! `Router::resolve_features` / `resolve_tiles`; everything downstream is a
//! trait object. A collection whose driver lacks a capability is refused here
//! — at resolve time — never partway through a handler.
//!
//! `Router` resolves `(tenant, collection, lane)` rather than just
//! `(tenant, collection)` (`#21`): each collection has a `features` lane and
//! a `tiles` lane, each bound to an ordered, non-empty chain of drivers — a
//! primary plus an optional read-only fallback tail consulted only when the
//! primary's call errors, never on an empty result. A lane with no explicit
//! `routing` entry defaults to the collection's single `storage`, so a
//! single-storage collection resolves exactly as it did before lanes
//! existed. See `config::RoutingDecl` and the driver-contract design doc,
//! section 3.
//!
//! `Router` also owns derived collection descriptors (`#19`/`#27`): a
//! collection's table/geometry/pk, filled from config overrides where
//! declared and from the storage's `CatalogSource` otherwise, plus its
//! spatial extent (always backend-derived, never configured). Descriptors
//! are cached with a TTL (`AppConfig::server.descriptor_ttl_s`) and lazily
//! re-derived on expiry — see `descriptor.rs`. Introspection is anchored to
//! the features lane's primary driver (see `RoutedCollection::anchor`),
//! since that lane's storage is the collection's canonical one whenever
//! lanes diverge.
//!
//! The descriptor cache itself (`Router::descriptor_cache`) is a single,
//! count-bounded `moka` cache shared across every collection
//! (`AppConfig::server.descriptor_cache_capacity`, `#42` registry
//! scale-out) — not a field on each `RoutedCollection` — so a registry that
//! grows past what fits comfortably in memory as fully-derived descriptors
//! evicts its coldest entries instead of holding one forever per
//! collection. It also caches a failed derivation's `Error::Config` message
//! (never a transient error), which is what makes `registry.validation:
//! lazy` (`config::RegistryValidationMode`) affordable: a misconfigured
//! collection is validated once, on its first request, and every
//! subsequent request against it replays the cached verdict instead of
//! repeating the backend round trip. See `resolved_descriptor`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::asset::AssetRecordStore;
use crate::catalog::CatalogSource;
use crate::config::{
    AppConfig, CatalogDecl, CollectionDecl, CollectionKind, LaneRouting, ObjectStoreDecl,
    ProtocolsConf, RegistryValidationMode, SettingsDecl, StorageDecl, TenantDecl, VisibilityDecl,
};
use crate::descriptor::{self, CachedDescriptor, CollectionDescriptor};
use crate::error::{Error, Result};
use crate::filter::Filter;
use crate::hint::Hints;
use crate::items_budget::budget_feature_source;
use crate::job::JobStore;
use crate::lease::Lease;
use crate::objectstore::{self, ObjectStore};
use crate::observability::{
    enter_phase, observe_feature_source, observe_tile_source, observe_volume_source, Phase,
};
use crate::outbox::{IndexSink, OutboxSource, SearchSource, WriteSink};
use crate::settings::{self, EffectiveSettings, EffectiveSettingsProvenance, SettingsProvenance};
use crate::stac_sidecar::StacMetadataSource;
use crate::storage::{
    FeaturePage, FeatureSource, ItemsQuery, RasterSource, TileCoord, TileSource, VolumeSource,
};

/// A built storage backend. Optional capabilities (`FeatureSource`,
/// `TileSource`, ...) default to `None` — "this driver never claims this
/// capability" — so a fake test driver only needs to override what it
/// supports. `CatalogSource` is not optional: every driver, real or fake,
/// must be able to say what it physically holds, because `Router::
/// validate_catalog` cross-checks configured collections against it once at
/// boot.
pub trait StorageDriver: Send + Sync {
    /// Mandatory: what this storage can serve, with enough physical
    /// metadata to validate a configured collection's `table` at boot. See
    /// `catalog.rs`.
    fn catalog_source(&self) -> Arc<dyn CatalogSource>;

    fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
        None
    }

    fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
        None
    }

    /// Raw raster pixel windows for this collection's tiles lane (`#37`) —
    /// the PNG-lane counterpart of [`tile_source`](Self::tile_source)'s MVT
    /// capability. A driver in this workspace advertises at most one of the
    /// two: a collection's tiles lane is either vector (MVT, optionally
    /// PNG-rendered from it by `tellurion-tiles`) or raster (PNG only, MVT
    /// refused as an unsupported capability) — see [`RasterSource`]'s own
    /// doc. Default `None`, the same "this driver never claims this
    /// capability" convention every other accessor here uses.
    fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
        None
    }

    /// True solid geometry for this collection's 3D places lane (`#15`),
    /// when this driver has any — see `VolumeSource`'s own docs for what
    /// that means and when the places3d lane falls back to extrusion
    /// instead. Independent of `tile_source`: every driver in this
    /// workspace today advertises at most MVT tiles, never volumes, but
    /// nothing here requires a driver to advertise both. Default `None`,
    /// the same "this driver never claims this capability" convention
    /// `feature_source`/`tile_source` use.
    fn volume_source(&self) -> Option<Arc<dyn VolumeSource>> {
        None
    }

    /// A storage that can accept writes AND commit the outbox obligation in
    /// the same transaction (the transactional-outbox design doc, `#25`).
    /// Advertising this is the machine-checkable meaning of "supports the
    /// outbox invariant" — same `Option`-shaped, "this driver never claims
    /// this capability" default every other capability accessor here uses.
    fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
        None
    }

    /// The read side of that storage's outbox. A driver that advertises
    /// [`write_sink`](Self::write_sink) also advertises this — `crate::
    /// applier::run_applier` drains it.
    fn outbox_source(&self) -> Option<Arc<dyn OutboxSource>> {
        None
    }

    /// A derived index this driver can apply outbox obligations into
    /// (`#67`). Same `Option`-shaped "this driver never claims this
    /// capability" default every other accessor here uses — advertising it
    /// says nothing about whether any given collection's index table has
    /// actually been provisioned yet (`crate::applier` / a driver's own
    /// `IndexSink::apply` refuses that case at request time, by name,
    /// exactly the way `write_sink`'s outbox table does).
    fn index_sink(&self) -> Option<Arc<dyn IndexSink>> {
        None
    }

    /// Freshness-aware search reads over this driver's derived index
    /// (`#67`) — same `Option`-shaped "this driver never claims this
    /// capability" default every other accessor here uses. Advertising this
    /// says nothing about whether any given collection's index table is
    /// actually provisioned, mirroring [`index_sink`](Self::index_sink)'s
    /// own doc, and nothing about whether a given collection's `routing.
    /// search` is even entitled to route to it — `Router::resolve_search`'s
    /// config-load provisioning check (a search lane naming an index this
    /// collection never declares via `routing.index` is a named refusal)
    /// covers that, not this method.
    fn search_source(&self) -> Option<Arc<dyn SearchSource>> {
        None
    }

    /// Database-backed asset-record persistence (assets-and-object-storage
    /// proposal, first slice) — the `AssetRecordStore` capability half of
    /// the managed/remote asset model; `crate::objectstore::ObjectStore` (a
    /// collection's configured `object_store`, resolved independently by
    /// `Router::resolve_object_store`) is the other half, for a managed
    /// asset's actual bytes. Same `Option`-shaped "this driver never claims
    /// this capability" default every other accessor here uses.
    /// Advertising this says nothing about whether a given collection's
    /// `"<table>_assets"` table has actually been provisioned — a driver's
    /// own `AssetRecordStore` methods refuse that case at request time, by
    /// name, exactly the way `write_sink`'s outbox table does.
    fn asset_record_store(&self) -> Option<Arc<dyn AssetRecordStore>> {
        None
    }

    /// Batched per-item STAC metadata reads over a collection's
    /// `"<table>_stac"` sidecar (`#202`) — see `crate::stac_sidecar`'s own
    /// module doc for what the sidecar is and why only the STAC lane reads
    /// it. Same `Option`-shaped "this driver never claims this capability"
    /// default every other accessor here uses; only PostGIS advertises it
    /// in this slice. Advertising it says nothing about whether a given
    /// collection's `"<table>_stac"` table has actually been provisioned —
    /// a driver's own `StacMetadataSource::stac_metadata` refuses that case
    /// at request time, by name, exactly the way `asset_record_store`'s
    /// assets table does — and nothing about whether any collection has
    /// opted into a sidecar at all, which is
    /// `CollectionDecl::stac_metadata`'s question, answered by
    /// `Router::resolve_stac_metadata` before this method is ever called.
    fn stac_metadata_source(&self) -> Option<Arc<dyn StacMetadataSource>> {
        None
    }

    /// Single-leader leases this backend can hand out (`#193`) — the
    /// coordination primitive that lets an outbox consumer keep the
    /// transactional-outbox design doc's "single ordered consumer per
    /// collection" invariant across 2+ replicas. Same `Option`-shaped
    /// "this driver never claims this capability" default every other
    /// accessor here uses: a driver with no mutual-exclusion primitive of
    /// its own simply never advertises one, and a deployment that never
    /// configures a lease (`config::IndexApplierConfig::lease`) never asks
    /// even a driver that does. Advertising it says nothing about which
    /// keys are in play — the key is the caller's
    /// (`crate::lease::LeaseKey`) — and nothing about who currently leads;
    /// a coordinator that cannot be reached is an error, never a silent
    /// grant (`crate::lease::Lease::try_acquire`'s own contract).
    fn lease(&self) -> Option<Arc<dyn Lease>> {
        None
    }

    /// The durable job ledger this backend can hold (`#182`) — see
    /// [`crate::job`]'s own module doc. Same `Option`-shaped "this driver
    /// never claims this capability" default every other accessor here uses,
    /// and the one that decides whether a deployment gets an OGC API —
    /// Processes root at all: with no storage advertising this there is
    /// nowhere durable to record a job, so `tellurion-server` serves no
    /// Processes root rather than a root whose submissions evaporate.
    ///
    /// Unlike every other capability here, this one is NOT reached through a
    /// collection: a job belongs to a `(tenant, catalog)` and to a process,
    /// never to a collection's lanes, so [`Router::resolve_job_store`] looks
    /// it up by the storage id `ServerConfig::processes` names. Advertising
    /// it says nothing about whether the ledger table has actually been
    /// provisioned — a driver's own [`JobStore`](crate::job::JobStore)
    /// methods refuse that case at request time, by name, exactly the way
    /// `write_sink`'s outbox table does.
    fn job_store(&self) -> Option<Arc<dyn JobStore>> {
        None
    }

    /// Called once per (collection, storage) pair the collection's lanes
    /// actually resolve to at `Router::build` time — a driver's chance to
    /// reject a physical target (table/column names, ...) that is
    /// syntactically invalid for its backend before the first request ever
    /// reaches it. A collection routed to more than one storage across its
    /// lanes (`#21`) is checked once against each distinct one. The default
    /// accepts everything; drivers with no backend-specific identifier
    /// syntax have nothing further to check beyond `AppConfig::validate`'s
    /// referential-integrity pass.
    fn validate_collection(&self, _decl: &CollectionDecl) -> Result<()> {
        Ok(())
    }

    /// How many requests this backend can sustain concurrently (e.g. a
    /// connection pool's size) — lets the driver-agnostic server layer keep
    /// its admission control coherent with what storage can actually take,
    /// instead of guessing from CPU count alone. `None` means "no opinion";
    /// `Router::total_capacity_hint` then falls back to a generic heuristic.
    fn capacity_hint(&self) -> Option<usize> {
        None
    }
}

pub trait DriverFactory: Send + Sync {
    /// Matches `StorageDecl::driver` in config.
    fn name(&self) -> &str;

    fn build(&self, decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>>;
}

/// The storage-driver seam's boot-time registry (`#112`): every driver crate
/// this binary was compiled with registers exactly one [`DriverFactory`]
/// here, once, in `main` — see that seam's own registration lines. Backed by
/// [`NamedRegistry`](crate::extension::NamedRegistry), the generic name ->
/// factory map every registry-shaped seam in this crate now shares, rather
/// than each hand-rolling its own map, its own "unknown name" wording, and
/// its own iteration order.
#[derive(Default)]
pub struct Registry {
    factories: crate::extension::NamedRegistry<dyn DriverFactory>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, factory: Arc<dyn DriverFactory>) {
        let name = factory.name().to_string();
        self.factories.register(name, factory);
    }

    /// Every registered driver name, alphabetically — what a boot log line
    /// enumerates as "the storage drivers this binary actually contains"
    /// (`#112`).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.factories.names()
    }

    fn build(&self, decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
        let factory = self.factories.get(decl.driver.as_str()).ok_or_else(|| {
            Error::Config(format!(
                "storage '{}' references unknown driver '{}'",
                decl.id, decl.driver
            ))
        })?;
        factory.build(decl)
    }
}

/// One lane's ordered, non-empty driver chain, resolved once at
/// `Router::build` time: index 0 is the primary and serves every request;
/// later entries are a read-only fallback tail consulted only when an
/// earlier entry's driver call errors (`#21`). Storage ids travel alongside
/// each driver for boot-validation error messages and descriptor-anchor
/// lookups.
struct RoutedLane {
    entries: Vec<(String, Arc<dyn StorageDriver>)>,
}

/// Which chain entry actually served a read (`#183`) — the router-side half
/// of the `X-Tellurion-Source` response header (`crate::hint::
/// READ_SOURCE_HEADER`). Returned by [`Router::resolve_features_read`]
/// alongside the resolved source; shared (cheaply clonable) with the
/// fallback wrapper, which records the storage id of whichever entry's call
/// returned `Ok`. For a single-entry lane the id is recorded at resolve
/// time instead — only that entry can ever serve, and recording eagerly is
/// what lets the single-entry path keep returning the bare driver source
/// with no per-call wrapper (the `#21` "zero added overhead" rule).
///
/// Meaningful only after a successful read: a request whose every entry
/// errored has no serving entry to name, and a caller must not emit the
/// header for it (a single-entry lane's eagerly-recorded id is harmless
/// under that rule, since the caller never reaches header-emission on an
/// error). Set-once by design (`OnceLock`): a handler that issues a
/// follow-up read on the same resolved source (e.g. `get_item`'s canonical
/// re-read) keeps the label of its principal read rather than the last one.
#[derive(Clone, Default)]
pub struct ServedSource(Arc<OnceLock<String>>);

impl ServedSource {
    fn record(&self, storage_id: &str) {
        let _ = self.0.set(storage_id.to_string());
    }

    /// The serving entry's storage id, once a read succeeded.
    pub fn storage_id(&self) -> Option<&str> {
        self.0.get().map(String::as_str)
    }
}

/// Applies a `prefer:` hint to `lane`'s resolved chain (`#183`): the named
/// entry moves to the front, every other entry keeps its configured
/// relative order behind it as the ordinary fallback tail — a reorder,
/// never an extension, so the entry *set* is always exactly the configured
/// one and a preferred entry that errors still falls through instead of
/// 404ing. Borrowed (no clone at all) whenever the hint changes nothing:
/// no `prefer:` token, a name matching no entry in this chain (unknown
/// names are harmless no-ops, same as unknown hint tokens), or a name
/// already at the front. Capability validation always runs against the
/// configured order before this is consulted, so a hinted request can never
/// dodge — or newly trip — a boot-shaped `Error::Config`.
fn preferred_entries<'a>(
    lane: &'a RoutedLane,
    prefer: Option<&str>,
) -> Cow<'a, [(String, Arc<dyn StorageDriver>)]> {
    let Some(prefer) = prefer else {
        return Cow::Borrowed(&lane.entries);
    };
    match lane.entries.iter().position(|(id, _)| id == prefer) {
        None | Some(0) => Cow::Borrowed(&lane.entries),
        Some(index) => {
            let mut entries = lane.entries.clone();
            let preferred = entries.remove(index);
            entries.insert(0, preferred);
            Cow::Owned(entries)
        }
    }
}

struct RoutedCollection {
    decl: CollectionDecl,
    /// This collection's owning tenant/catalog, both internal ids (`#39`).
    /// Denormalized here from `decl.catalog`'s own catalog decl so
    /// `Router::lookup` can verify a `(tenant, catalog, collection)` triple
    /// without a second map lookup — a defense against a resolver bug that
    /// maps an external id to an internal id from the wrong tenant/catalog,
    /// not something `collections`' key alone would catch (collection
    /// internal ids are globally unique, so the key lookup alone would
    /// already find the right entry; this check exists purely to fail loud
    /// on a scope mismatch instead of silently succeeding).
    tenant: String,
    catalog: String,
    features: RoutedLane,
    tiles: RoutedLane,
    /// The OGC API Maps `/collections/{cid}/map` lane (`#86`) — same
    /// "defaults to the single `storage`" shape as `tiles` (see
    /// `RoutingDecl::maps`'s own doc), resolved independently so a
    /// collection may point it at a different storage than `tiles`.
    maps: RoutedLane,
    /// The write lane (`#25`): unlike `features`/`tiles`, this is `None`
    /// whenever the collection declares no `routing.write` at all — there is
    /// no "defaults to the single `storage`" fallback for write, since a
    /// storage advertising `write_sink` is the exception, not the rule (every
    /// existing driver in this workspace advertises `feature_source`/
    /// `tile_source` by default; none advertise `write_sink` yet). A
    /// collection that never asked to be writable has nothing here to
    /// resolve, and `resolve_write` refuses it as a plain capability
    /// unsupported rather than trying a storage nobody named for writes.
    write: Option<RoutedLane>,
    /// The index lane (`#67`): same "`None` unless `routing.index` was
    /// explicitly declared, no fallback tail" shape as `write` — a derived
    /// index is opt-in per collection, and applying obligations has nowhere
    /// sensible to fall through to either. `resolve_index` refuses a
    /// collection with no `routing.index` as a plain capability unsupported.
    index: Option<RoutedLane>,
    /// The search lane (`#67`): `None` unless `routing.search` was explicitly
    /// declared, same as `write`/`index` — but, unlike those two, this MAY
    /// hold more than one entry (an ordered fallback tail the freshness gate
    /// walks, `RoutingDecl`'s own doc). `resolve_search` refuses a collection
    /// with no `routing.search` as a plain capability unsupported.
    search: Option<RoutedLane>,
    /// Whether `decl.routing.features` / `.tiles` was explicitly configured,
    /// as opposed to defaulting to the single `storage`. Boot-time capability
    /// validation (`validate_catalog`) only applies to a lane the operator
    /// actually named — an unrouted lane keeps the pre-`#21` request-time
    /// `CapabilityUnsupported` behavior, since not every collection needs
    /// every capability (a tiles-only PMTiles collection has no features
    /// lane to speak of, for instance).
    features_explicit: bool,
    tiles_explicit: bool,
    /// Whether `decl.routing.maps` was explicitly configured (`#86`) — same
    /// "boot-time capability validation only applies to a lane the operator
    /// actually named" rule `tiles_explicit`'s own doc gives.
    maps_explicit: bool,
}

impl RoutedCollection {
    /// The driver `CatalogSource` introspection anchors to (`#19`/`#27`):
    /// the features lane's primary driver. Design-fork decision (`#21`):
    /// when lanes diverge, the features lane is the collection's canonical
    /// storage, since a collection's identity (table/geometry/pk) is a
    /// features concept; the tiles lane may legitimately point at a
    /// derived or prebuilt store with no independent catalog of its own.
    /// This is also exactly the single `storage` driver whenever the
    /// features lane isn't explicitly routed, so unrouted collections see
    /// no behavior change.
    fn anchor(&self) -> &Arc<dyn StorageDriver> {
        &self.features.entries[0].1
    }

    fn anchor_storage_id(&self) -> &str {
        &self.features.entries[0].0
    }
}

/// Merges `physical`/`derived` into a `CollectionDescriptor` for `decl`,
/// then enforces `geometry`/`pk` are concrete only when `anchor`'s driver
/// actually implements `FeatureSource` (`#20`) — a tiles-only archive driver
/// (PMTiles) has no table-shaped concept of either and must never be
/// required to resolve them; a driver that does claim `FeatureSource` still
/// needs both, exactly as every collection did before `#20` relaxed
/// `merge_descriptor` itself to never error. Shared by `validate_catalog`
/// (the eager boot-time pass) and `resolved_descriptor` (the lazy
/// re-derivation on TTL expiry) so neither path can silently hand a
/// `FeatureSource`-backed driver a descriptor missing the columns its
/// query-building code assumes are present (see `CollectionDecl::
/// resolved_geometry`/`resolved_pk`).
///
/// Also reconciles `decl.schema` against the freshly merged descriptor when
/// one is declared (`#44`) — the same boot-or-first-touch seam, so a
/// declaration drifting from the backend fails exactly where `geometry`/`pk`
/// already do, never as a separate validation pass.
///
/// `tile_properties` is the collection's settings-resolved vector-tile
/// property allowlist (`#85`, `settings::resolve_effective_settings`'s own
/// `tile_properties` field) — not part of `decl` itself, since (unlike
/// `decl.schema`) it inherits down the platform -> tenant -> catalog ->
/// collection chain, so both call sites below resolve it from `Router`'s own
/// `effective_settings` before calling in. Reconciled the same
/// boot-or-first-touch way `decl.schema` is, via
/// `descriptor::reconcile_tile_properties`.
fn merge_and_enforce(
    anchor: &Arc<dyn StorageDriver>,
    decl: &CollectionDecl,
    physical: &crate::catalog::PhysicalCollection,
    derived: descriptor::DerivedFields,
    tile_properties: &[String],
) -> Result<CollectionDescriptor> {
    let resolved = descriptor::merge_descriptor(decl, physical, derived);
    if anchor.feature_source().is_some() {
        descriptor::require_feature_capable(&decl.id, &resolved, decl.kind)?;
    }
    if let Some(schema) = &decl.schema {
        descriptor::reconcile_schema(&decl.id, schema, &resolved)?;
    }
    if let Some(modified_column) = &decl.modified_column {
        descriptor::reconcile_modified_column(
            &decl.id,
            modified_column,
            decl.schema.as_ref(),
            &resolved,
        )?;
    }
    descriptor::reconcile_tile_properties(&decl.id, tile_properties, &resolved)?;
    Ok(resolved)
}

/// Gathers every backend-derived field `merge_descriptor` needs (`#19`): one
/// `CatalogSource` call each for extent, row estimate, attribute schema, and
/// temporal column. Shared by `validate_catalog` and `resolved_descriptor`
/// so the four-call sequence exists exactly once; each caller wraps errors
/// at its own grain (`validate_catalog` names the collection for a boot-time
/// message, `resolved_descriptor` propagates as-is).
async fn derived_fields(
    catalog: &Arc<dyn CatalogSource>,
    physical: &crate::catalog::PhysicalCollection,
) -> Result<descriptor::DerivedFields> {
    let extent = catalog.extent(physical).await?;
    let row_estimate = catalog.row_estimate(physical).await?;
    let attributes = catalog.attribute_schema(physical).await?;
    let temporal_column = catalog.temporal_column(physical).await?;
    let projection = catalog.projection(physical).await?;
    Ok(descriptor::DerivedFields {
        extent,
        row_estimate,
        attributes,
        temporal_column,
        projection,
    })
}

/// The table-lookup + `derived_fields` + `merge_and_enforce` sequence
/// `Router::resolved_descriptor` runs on a cache miss or a stale entry: one
/// `CatalogSource::collections()` query against `routed`'s anchor storage
/// (this single collection only, unlike `validate_catalog`'s
/// batched-per-storage query), then the same derive/merge/enforce sequence
/// that path uses. Pulled out so `resolved_descriptor` only has to decide
/// what to do with the `Result`, not how to compute it. `tile_properties` is
/// threaded straight through to `merge_and_enforce` — see that function's
/// own doc for why it travels alongside `routed` rather than living on it.
///
/// `#104`: PostGIS's `geometry_columns` view reports one row per (table,
/// geometry column), so a table with two spatial columns yields two rows
/// sharing `name` here. Silently taking whichever row `collections()`
/// happened to return first bound the collection to an arbitrary,
/// non-deterministic geometry column — refused instead via
/// [`refuse_ambiguous_geometry_column`] whenever `decl.geometry` is not
/// pinned; a pin already names a column unambiguously and skips the check.
/// Regardless of a pin, any declared `geometry_variants` are checked against
/// this same `matches` set via [`refuse_invalid_geometry_variants`].
async fn derive_one_descriptor(
    routed: &RoutedCollection,
    tile_properties: &[String],
) -> Result<CollectionDescriptor> {
    let decl = &routed.decl;
    let target = descriptor::target_table(decl);
    let catalog = routed.anchor().catalog_source();
    let physical = catalog.collections().await?;
    let matches: Vec<_> = physical.into_iter().filter(|p| p.name == target).collect();
    if matches.is_empty() {
        return Err(Error::Config(format!(
            "collection '{}': storage '{}' does not report a table named '{}' in its catalog",
            decl.id,
            routed.anchor_storage_id(),
            target
        )));
    }
    if decl.geometry.is_none() {
        refuse_ambiguous_geometry_column(&decl.id, routed.anchor_storage_id(), target, &matches)?;
    }
    refuse_invalid_geometry_variants(
        &decl.id,
        routed.anchor_storage_id(),
        target,
        matches.iter(),
        decl,
    )?;
    let physical =
        descriptor_physical_row(&decl.id, routed.anchor_storage_id(), target, &matches, decl)?;
    let derived = derived_fields(&catalog, physical).await?;
    merge_and_enforce(routed.anchor(), decl, physical, derived, tile_properties)
}

/// Refuses to auto-derive a geometry column when `matches` — every physical
/// row the backend reported for one target table — names more than one
/// (`#104`). PostGIS's `geometry_columns` view is one row per (table,
/// geometry column); a table with two spatial columns is invisible as
/// ambiguity to a plain by-name lookup, which just sees two candidate rows
/// and (before this fix) picked whichever came back first. Deterministic
/// startup failure beats silently binding to an arbitrary column: the error
/// names the table and every distinct candidate column so the operator can
/// pin one via this collection's `geometry:` config key.
///
/// Never called once `decl.geometry` is pinned — both call sites
/// ([`derive_one_descriptor`] and `Router::validate_catalog`) skip this
/// check in that case, since an override already names a column
/// unambiguously regardless of how many rows the backend reports.
fn refuse_ambiguous_geometry_column(
    collection_id: &str,
    storage_id: &str,
    target: &str,
    matches: &[crate::catalog::PhysicalCollection],
) -> Result<()> {
    let mut candidates: Vec<&str> = matches
        .iter()
        .filter_map(|p| p.geometry_column.as_deref())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if candidates.len() > 1 {
        candidates.sort_unstable();
        return Err(Error::Config(format!(
            "collection '{collection_id}': table '{target}' in storage '{storage_id}' reports {} geometry columns ({}) and none is pinned — set this collection's 'geometry' config key to one of them",
            candidates.len(),
            candidates.join(", ")
        )));
    }
    Ok(())
}

/// Selects the physical row that supplies a descriptor's backend-only
/// metadata. A geometry pin names that row; an unpinned collection reaches
/// this only after ambiguity was refused, so its sole candidate remains the
/// first row. A single physical row keeps the long-standing override behavior
/// even when a backend reports a different geometry name.
fn descriptor_physical_row<'a>(
    collection_id: &str,
    storage_id: &str,
    target: &str,
    matches: &'a [crate::catalog::PhysicalCollection],
    decl: &CollectionDecl,
) -> Result<&'a crate::catalog::PhysicalCollection> {
    if let Some(geometry) = decl.geometry.as_deref() {
        if let Some(physical) = matches
            .iter()
            .find(|physical| physical.geometry_column.as_deref() == Some(geometry))
        {
            return Ok(physical);
        }
        if matches.len() > 1 {
            return Err(Error::Config(format!(
                "collection '{collection_id}': geometry pin '{geometry}' does not match any geometry column reported for table '{target}' in storage '{storage_id}'"
            )));
        }
    }
    Ok(&matches[0])
}

/// Plain, human-readable SRID for an error message — `None` (the PostGIS
/// "unset" sentinel, filtered the same way `merge_descriptor` already
/// treats a literal SRID `0`) reads as "unset" rather than a bare `None`.
fn describe_srid(srid: Option<i32>) -> String {
    srid.map_or_else(|| "unset".to_string(), |value| value.to_string())
}

/// Plain, human-readable geometry type for an error message — `None` (the
/// backend couldn't determine one) reads as "unknown" rather than a bare
/// `None`.
fn describe_geometry_type(geometry_type: &Option<String>) -> &str {
    geometry_type.as_deref().unwrap_or("unknown")
}

/// Refuses a declared `geometry_variants` entry that doesn't exist on the
/// backend, or that exists but disagrees with the base geometry column's
/// SRID or geometry type (`#104`, design point 5). A no-op — zero I/O beyond
/// the `matches` the caller already fetched — for the ordinary case (no
/// variants declared), the same "additive, opt-in" shape `Places3dConf`/
/// `SchemaDecl` reconciliation already have.
///
/// `matches` is every physical row the backend reported for this collection's
/// target table (the same set [`refuse_ambiguous_geometry_column`] checks for
/// ambiguity) — reused rather than re-queried, and deliberately NOT the
/// single `physical` row a caller may have already picked out of it: that
/// row is chosen by position (`matches[0]`/`matches.into_iter().next()`),
/// which is only guaranteed to be the *base* geometry column's own row when
/// there is exactly one candidate. This function re-derives the base row
/// itself — the pinned `decl.geometry`, or (post-ambiguity-check) the sole
/// distinct geometry column `matches` reports — so a pinned collection whose
/// base column isn't physical row zero still gets a correct comparison.
///
/// Called from every boot-or-first-touch path that resolves this
/// collection's physical shape at all: [`derive_one_descriptor`],
/// `Router::validate_catalog`'s eager sweep, and [`probe_pinned_collection`]'s
/// fully-pinned fast path — a declared variant is checked regardless of
/// which of the three ever runs for a given collection.
fn refuse_invalid_geometry_variants<'a>(
    collection_id: &str,
    storage_id: &str,
    target: &str,
    matches: impl IntoIterator<Item = &'a crate::catalog::PhysicalCollection>,
    decl: &CollectionDecl,
) -> Result<()> {
    if decl.geometry_variants.is_empty() {
        return Ok(());
    }
    let matches: Vec<&crate::catalog::PhysicalCollection> = matches.into_iter().collect();

    let base_column = decl
        .geometry
        .as_deref()
        .or_else(|| matches.iter().find_map(|p| p.geometry_column.as_deref()));
    let Some(base_column) = base_column else {
        return Err(Error::Config(format!(
            "collection '{collection_id}': geometry_variants declared but table '{target}' in storage '{storage_id}' reports no geometry column to serve as the base"
        )));
    };
    let base = matches
        .iter()
        .find(|p| p.geometry_column.as_deref() == Some(base_column))
        .ok_or_else(|| {
            Error::Config(format!(
                "collection '{collection_id}': geometry_variants declared but table '{target}' in storage '{storage_id}' does not report base geometry column '{base_column}'"
            ))
        })?;
    let base_srid = base.srid.filter(|&srid| srid > 0);

    for variant in &decl.geometry_variants {
        let physical = matches
            .iter()
            .find(|p| p.geometry_column.as_deref() == Some(variant.column.as_str()))
            .ok_or_else(|| {
                Error::Config(format!(
                    "collection '{collection_id}': geometry_variants entry '{}' names a column table '{target}' in storage '{storage_id}' does not report",
                    variant.column
                ))
            })?;
        let variant_srid = physical.srid.filter(|&srid| srid > 0);
        if variant_srid != base_srid {
            return Err(Error::Config(format!(
                "collection '{collection_id}': geometry_variants entry '{}' has srid {} but base column '{base_column}' has srid {}",
                variant.column,
                describe_srid(variant_srid),
                describe_srid(base_srid)
            )));
        }
        if physical.geometry_type != base.geometry_type {
            return Err(Error::Config(format!(
                "collection '{collection_id}': geometry_variants entry '{}' has geometry type {} but base column '{base_column}' has geometry type {}",
                variant.column,
                describe_geometry_type(&physical.geometry_type),
                describe_geometry_type(&base.geometry_type)
            )));
        }
    }
    Ok(())
}

/// The existence/type probe `Router::verify_pinned_collection` runs once per
/// fully-pinned collection under lazy validation (`#61`): one
/// `CatalogSource::collections()` call — the same call [`derive_one_descriptor`]
/// makes — checked only for presence, never merged into a
/// `CollectionDescriptor`. Confirms `decl`'s target table exists, and that its
/// pinned `geometry`/`pk` column names each appear on some physical row
/// reported for that table exactly as the backend names them. That match is
/// itself the "plausible type" check, not a separate step: a name that shows
/// up as a physical row's `geometry_column` is provably a real geometry
/// column (PostGIS reports it from the `geometry_columns` view, which only
/// ever lists genuine geometry-typed columns — see
/// `tellurion-postgis::catalog::CATALOG_QUERY`), and a name that shows up as
/// `primary_key` is provably that table's actual primary-key column (from a
/// real `PRIMARY KEY` constraint), not merely some other column that happens
/// to share the pinned name.
///
/// `decl.geometry`/`decl.pk` are guaranteed `Some` — this is only ever called
/// from `effective_decl`'s fully-pinned fast path, which already checked
/// that.
async fn probe_pinned_collection(routed: &RoutedCollection) -> Result<()> {
    let decl = &routed.decl;
    let target = descriptor::target_table(decl);
    let storage_id = routed.anchor_storage_id();
    let physical = routed.anchor().catalog_source().collections().await?;
    let matches: Vec<_> = physical.iter().filter(|p| p.name == target).collect();
    if matches.is_empty() {
        return Err(Error::Config(format!(
            "collection '{}': storage '{}' does not report a table named '{}' in its catalog",
            decl.id, storage_id, target
        )));
    }

    let geometry = decl.geometry.as_deref().unwrap_or_default();
    if !matches
        .iter()
        .any(|p| p.geometry_column.as_deref() == Some(geometry))
    {
        return Err(Error::Config(format!(
            "collection '{}': pinned geometry column '{}' does not exist on table '{}' in storage '{}'",
            decl.id, geometry, target, storage_id
        )));
    }

    let pk = decl.pk.as_deref().unwrap_or_default();
    if !matches.iter().any(|p| p.primary_key.as_deref() == Some(pk)) {
        return Err(Error::Config(format!(
            "collection '{}': pinned pk column '{}' does not exist on table '{}' in storage '{}'",
            decl.id, pk, target, storage_id
        )));
    }

    refuse_invalid_geometry_variants(&decl.id, storage_id, target, matches.iter().copied(), decl)?;

    Ok(())
}

/// Builds every declared `object_stores` entry once, at `Router::build`
/// time — the object-store counterpart of the `drivers` loop right above,
/// kept as its own free function since it shares none of that loop's
/// `DriverFactory`/`Registry` machinery (an object store is never a
/// `StorageDriver`, see `config::ObjectStoreDecl`'s own doc). A `root` that
/// doesn't exist as a writable directory fails here, at boot — a named,
/// actionable startup error rather than a confusing first-upload I/O
/// failure (`objectstore::FsObjectStore::new`'s own doc).
fn build_object_stores(decls: &[ObjectStoreDecl]) -> Result<HashMap<String, Arc<dyn ObjectStore>>> {
    let mut stores = HashMap::with_capacity(decls.len());
    for decl in decls {
        let store = objectstore::build_object_store(decl).map_err(|err| {
            Error::Config(format!(
                "object_store '{}' failed to initialize: {err}",
                decl.id
            ))
        })?;
        stores.insert(decl.id.clone(), store);
    }
    Ok(stores)
}

/// Resolves `lane`'s ordered storage ids against `drivers` into a
/// `RoutedLane`: `routing`'s list when the lane was explicitly configured,
/// else the collection's single `storage` — the "unambiguous single
/// storage" default. Every driver lookup is defensive even though
/// `AppConfig::validate` should already have caught an unknown id, matching
/// the pre-`#21` `collection.storage` lookup this replaces.
fn build_lane(
    drivers: &HashMap<String, Arc<dyn StorageDriver>>,
    collection: &CollectionDecl,
    routing: Option<&LaneRouting>,
) -> Result<RoutedLane> {
    let storage_ids: &[String] = match routing {
        Some(lane) => &lane.0,
        None => std::slice::from_ref(&collection.storage),
    };
    let mut entries = Vec::with_capacity(storage_ids.len());
    for storage_id in storage_ids {
        let driver = drivers.get(storage_id).ok_or_else(|| {
            Error::Config(format!(
                "collection '{}' references unknown storage '{}'",
                collection.id, storage_id
            ))
        })?;
        entries.push((storage_id.clone(), Arc::clone(driver)));
    }
    Ok(RoutedLane { entries })
}

/// Capability check for one explicitly routed lane (`#21`, `#59`): every
/// entry in `lane`'s resolved chain must satisfy `has_capability`, not just
/// the primary — a fallback entry that can't actually serve the lane is a
/// misconfiguration worth catching before the first request reaches it,
/// same as the primary. Fails on the first violation with a message naming
/// the collection, the lane, and the offending storage id.
///
/// Cheap (no I/O — every check here is a trait-method call against an
/// already-built driver, never a backend round trip), so it costs nothing to
/// run twice: once from `validate_catalog`'s eager boot-time sweep, and once
/// more from `resolve_features`/`resolve_tiles` themselves, so
/// `registry.validation: lazy` (which skips the eager sweep entirely) still
/// catches a misconfigured explicit lane the first time a request actually
/// resolves it, rather than never. Before that second call site existed,
/// `features_source`/`tiles_source` silently dropped an incapable entry out
/// of a multi-entry lane's fallback chain instead of refusing, so the
/// misconfiguration never surfaced under `lazy` at all (`#59`).
fn validate_lane_capability(
    collection_id: &str,
    lane: &str,
    routed_lane: &RoutedLane,
    has_capability: impl Fn(&dyn StorageDriver) -> bool,
) -> Result<()> {
    for (storage_id, driver) in &routed_lane.entries {
        if !has_capability(driver.as_ref()) {
            return Err(Error::Config(format!(
                "collection '{collection_id}': routing lane '{lane}' names storage '{storage_id}', which does not implement the '{lane}' capability"
            )));
        }
    }
    Ok(())
}

/// Whether `routed`'s write lane resolves all the way to a `WriteSink`
/// (`#208`) — the single predicate both [`Router::resolve_write`] (which
/// then goes on to produce the sink, and names each failure) and
/// [`Router::write_lane_resolves`] (which only reports the verdict) agree
/// on.
///
/// Written as the same two checks `resolve_write` makes, in the same order
/// and through the same [`validate_lane_capability`] call, so "what a write
/// will do" and "what `Allow` says a write will do" cannot drift apart: a
/// collection with no `routing.write` at all, and one whose routed storage
/// does not advertise `write_sink`, both mean the same thing to a caller —
/// no write to this collection will succeed — even though `resolve_write`
/// distinguishes them in the error it raises.
fn write_lane_resolves(routed: &RoutedCollection) -> bool {
    routed.write.as_ref().is_some_and(|lane| {
        validate_lane_capability(&routed.decl.id, "write", lane, |driver| {
            driver.write_sink().is_some()
        })
        .is_ok()
    })
}

/// Capability check for a collection declaring `places3d` (`#15`, `#59`):
/// places3d rides the tiles lane's `TileSource` regardless of whether that
/// lane was explicitly routed, so every entry in the resolved tiles chain
/// (not just the primary) must support it. Same cheap, no-I/O shape as
/// [`validate_lane_capability`], and shared for the identical reason: called
/// from both `validate_catalog`'s eager sweep and `resolve_tiles` itself, so
/// a misconfigured places3d collection surfaces the first time a request
/// resolves its tiles lane under `registry.validation: lazy`, not never.
fn validate_places3d_capability(decl: &CollectionDecl, tiles: &RoutedLane) -> Result<()> {
    if decl.places3d.is_none() {
        return Ok(());
    }
    for (storage_id, driver) in &tiles.entries {
        if driver.tile_source().is_none() {
            return Err(Error::Config(format!(
                "collection '{}': declares places3d, which requires the tiles capability, but storage '{}' does not support tiles",
                decl.id, storage_id
            )));
        }
    }
    Ok(())
}

/// Capability check for an explicit `search` lane (`#67`, `#59`): unlike
/// [`validate_lane_capability`], not every entry needs the *same*
/// capability — only entry 0 (`routing.search`'s primary) is ever asked for
/// `SearchSource`; entries 1+ (the fallback tail) are always plain degraded
/// `FeatureSource` reads, never a second index attempt, even when their
/// driver also happens to advertise `SearchSource` for some other
/// collection's index — see `Router::resolve_search`'s own doc for why that
/// asymmetry is deliberate. So entry 0 may satisfy either capability;
/// entries 1+ must satisfy `FeatureSource` specifically. Same cheap, no-I/O
/// shape as `validate_lane_capability`, called from both `validate_catalog`'s
/// eager sweep and `resolve_search` itself for the same `#59` lazy-mode
/// reason.
fn validate_search_lane_capability(collection_id: &str, lane: &RoutedLane) -> Result<()> {
    let mut entries = lane.entries.iter();
    if let Some((storage_id, driver)) = entries.next() {
        if driver.search_source().is_none() && driver.feature_source().is_none() {
            return Err(Error::Config(format!(
                "collection '{collection_id}': routing lane 'search' names storage '{storage_id}' as its primary entry, which implements neither the derived-index search capability nor a feature-source fallback"
            )));
        }
    }
    for (storage_id, driver) in entries {
        if driver.feature_source().is_none() {
            return Err(Error::Config(format!(
                "collection '{collection_id}': routing lane 'search' names storage '{storage_id}' in its fallback tail, which does not implement the feature-source capability the degraded search path needs"
            )));
        }
    }
    Ok(())
}

/// The `#67` "grant principle" refusal for `search`: entry 0 of `lane`, when
/// it advertises `SearchSource`, is only entitled to serve this collection's
/// search reads when the *same* collection also provisions it as its own
/// derived index, via `routing.index` naming that exact storage — a search
/// lane cannot route to "the index" for free without the collection having
/// declared it derives one there (the applier is what actually keeps that
/// index converging; see the applier design doc). Scoped to entry 0 only,
/// matching `resolve_search`'s own "only entry 0 is ever asked for
/// `SearchSource`" rule — an entry 1+ tail is never treated as an index
/// target, so it has nothing to provision. Same cheap, no-I/O shape as
/// [`validate_search_lane_capability`], called from the same two sites.
fn validate_search_lane_provisioning(
    collection_id: &str,
    lane: &RoutedLane,
    index: Option<&RoutedLane>,
) -> Result<()> {
    let Some((storage_id, driver)) = lane.entries.first() else {
        return Ok(());
    };
    if driver.search_source().is_none() {
        return Ok(());
    }
    let provisioned =
        index.is_some_and(|index_lane| index_lane.entries.iter().any(|(id, _)| id == storage_id));
    if !provisioned {
        return Err(Error::Config(format!(
            "collection '{collection_id}': routing lane 'search' names storage '{storage_id}' as its derived-index search target, but this collection's routing.index does not provision it there"
        )));
    }
    Ok(())
}

/// A cached lazy-validation pin-verification verdict (`#61`): success, or a
/// named `Error::Config` failure, for one fully-pinned collection's
/// once-per-collection existence/type probe — see `Router::
/// verify_pinned_collection`. Same TTL-governed shape as `CachedDescriptor`
/// (success and failure alike are cached, both staleness-governed by the
/// same `descriptor_ttl`), but deliberately its own type rather than reusing
/// `CachedDescriptor`: a verified pin is never turned into a
/// `CollectionDescriptor` — nothing is derived — so there is no
/// `CollectionDescriptor` value to put in a `CachedDescriptor::outcome` here.
#[derive(Debug, Clone)]
struct CachedPinVerification {
    outcome: std::result::Result<(), String>,
    computed_at: Instant,
}

impl CachedPinVerification {
    fn is_stale(&self, ttl: Duration) -> bool {
        self.computed_at.elapsed() >= ttl
    }
}

/// A cached geometry-profile computation (`#101`), keyed by collection
/// internal id. `profile` is `None` both when the driver declined
/// (`CatalogSource::geometry_profile`'s default) and when it genuinely has
/// nothing to report — the cache doesn't distinguish the two, mirroring
/// `descriptor_cache`'s own "capability-declined and genuinely-absent look
/// identical" shape for `extent`/`row_estimate`. Unlike `CachedDescriptor`/
/// `CachedPinVerification`, there is no `Err` variant to cache: a transient
/// failure computing a profile is never worth calcifying into a standing
/// verdict, so [`Router::geometry_profile_cached`] simply never writes one
/// to this cache and the next call tries again.
#[derive(Debug, Clone, Copy)]
struct CachedGeometryProfile {
    profile: Option<crate::catalog::GeometryProfile>,
    computed_at: Instant,
}

impl CachedGeometryProfile {
    fn is_stale(&self, ttl: Duration) -> bool {
        self.computed_at.elapsed() >= ttl
    }
}

/// Everything below the HTTP boundary keys on internal ids only (`#39`):
/// `collections` is keyed by a collection's internal id alone — internal ids
/// are globally unique (`AppConfig::validate`), so that key is already
/// unambiguous; `RoutedCollection::tenant`/`catalog` exist for
/// [`Router::lookup`] to verify the resolved triple actually matches, not
/// because the key needs help disambiguating. There is no schema-per-tenant
/// or other tenant-shaped concrete storage here — tenancy and catalogs are
/// purely a routing/settings key.
pub struct Router {
    collections: HashMap<String, RoutedCollection>,
    drivers: HashMap<String, Arc<dyn StorageDriver>>,
    /// Built object stores (assets-and-object-storage proposal, first
    /// slice), keyed by `config.object_stores[].id` — a sibling index to
    /// `drivers` above, deliberately separate: an object store is never a
    /// `StorageDriver` (see `config::ObjectStoreDecl`'s own doc for why the
    /// two concepts never touch). `Router::resolve_object_store` looks a
    /// collection's own `object_store` id up here.
    object_stores: HashMap<String, Arc<dyn ObjectStore>>,
    descriptor_ttl: Duration,
    /// `config.registry.validation` (`#42`), carried onto `Router` itself so
    /// `effective_decl`'s fully-pinned fast path knows whether to run its
    /// lazy-only verification probe (`#61`) — see
    /// `verify_pinned_collection`'s own doc for why this is gated on the
    /// mode rather than unconditional: under `eager`, `validate_catalog`'s
    /// boot sweep already checked every collection, pinned or not, before
    /// the server ever started serving, so re-probing here would just be a
    /// second, redundant backend round trip for no new information.
    registry_validation: RegistryValidationMode,
    /// One materialized `EffectiveSettings` per collection (internal id),
    /// computed once at `build` time by walking the platform -> tenant ->
    /// catalog -> collection chain (`settings.rs`, `#39`).
    effective_settings: HashMap<String, EffectiveSettings>,
    /// `effective_settings`, field for field, tagged with where each value
    /// came from (`#110`) — computed in the same `build_from_snapshot` pass
    /// via `settings::resolve_effective_settings_with_provenance`, never a
    /// second re-derivation. Backs the control lane's effective-config
    /// view; nothing on the request path reads this map.
    effective_settings_provenance: HashMap<String, EffectiveSettingsProvenance>,
    /// One materialized `VisibilityDecl` per collection (internal id, `#34`
    /// policy layer): the collection's own declaration if it sets any
    /// non-default visibility, else its owning catalog's — see
    /// `VisibilityDecl`'s own doc for the two-level "nearest wins, whole
    /// value replaces" rule this computes once here rather than re-walking
    /// per request, the same reasoning `effective_settings` above already
    /// documents for the four-level settings chain.
    effective_visibility: HashMap<String, VisibilityDecl>,
    /// One materialized protocol exposure matrix per CATALOG (internal id,
    /// `#185`) — resolved once here through the same settings chain
    /// everything else in this struct rides, at the catalog node (platform
    /// -> tenant -> catalog, with an empty collection level standing in for
    /// the depth this key is never asked at).
    ///
    /// Per catalog, not per collection, because the gate that reads it runs
    /// before any collection is known: `/{tenant}/features/catalogs/{cat}/
    /// collections` names no collection at all, and the protocol root's own
    /// `/`, `/conformance`, and `/api` never will. Resolving inside the
    /// enforcement middleware instead would mean walking the chain on every
    /// request — exactly what `settings.rs`'s "resolve at load, not per
    /// request" rule exists to prevent. See
    /// [`Router::catalog_protocols`].
    catalog_protocols: HashMap<String, ProtocolsConf>,
    /// Shared, count-bounded derived-descriptor cache, keyed by collection
    /// internal id (`#42`, registry scale-out) — see this module's doc for
    /// why this lives here rather than per-`RoutedCollection`.
    descriptor_cache: moka::future::Cache<String, CachedDescriptor>,
    /// Shared, count-bounded cache of lazy-validation pin-verification
    /// verdicts (`#61`), keyed by collection internal id — the same bounded
    /// shape as `descriptor_cache`, but a distinct cache: a verified pin's
    /// entry is never a `CollectionDescriptor`, so blending the two into one
    /// keyspace would let a verification placeholder shadow (or be shadowed
    /// by) that collection's real derived descriptor.
    pin_verification_cache: moka::future::Cache<String, CachedPinVerification>,
    /// Cached geometry-statistics profiles (`#101`), keyed by collection
    /// internal id — a separate, count-bounded `moka` cache from
    /// `descriptor_cache`, sharing its capacity budget the same way
    /// `pin_verification_cache` does (see that field's own doc), but never
    /// folded into `CachedDescriptor` itself: unlike extent/row_estimate/
    /// attribute_schema/temporal_column (cheap, statistics-only lookups
    /// `derived_fields` bundles into every descriptor re-derivation), a
    /// geometry profile samples real table rows (`TABLESAMPLE`) — bounded
    /// and cheap relative to the table's size, but not free, and not
    /// something every feature/tile request touching `effective_decl`
    /// should risk paying for. See [`Router::geometry_profile_cached`].
    geometry_profile_cache: moka::future::Cache<String, CachedGeometryProfile>,
}

/// What a conformance fold answers when *nothing* in the deployment
/// participated in it at all — the one place the folds on [`Router`]
/// genuinely disagree, so it is a parameter of
/// [`fold_conformance_classes`] rather than something each fold re-decides
/// in its own tail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WhenNoneParticipate {
    /// The seed stands, unnarrowed. Appropriate only for classes the server
    /// itself honours without any storage-driver participant, such as its
    /// landing page, conformance document, API definition, and encodings.
    KeepsSeed,
    /// Nothing is claimed. Used where the seeded classes describe a
    /// driver-honoured capability — CQL2 evaluation, optimistic locking, Part
    /// 4 write behaviour, or Part 2/Part 3 feature behaviour. With no
    /// participant, the deployment has nowhere to honour such a claim.
    ClaimsNothing,
}

/// The one shape every `/conformance` fold on [`Router`] has (`#217`): start
/// from a seed of candidate classes, narrow it by intersecting with what each
/// routed entry honours, return what survives.
///
/// **The invariant, in one line: a class is advertised only if every routed
/// entry that participates really honours it.** A client reading
/// `/conformance` off a protocol root has not yet picked a collection — it
/// cannot know whether the request it is about to send lands on the strongest
/// or the weakest thing behind this deployment — so the honest claim is the
/// intersection, never the union. A per-collection surface
/// (`descriptor::canonical::CanonicalCapabilities`) still names the wider
/// truth once a collection is known.
///
/// `honoured` maps one entry — a configured driver, or a routed collection,
/// whichever unit the family is folded over — to the classes that entry
/// honours:
///
/// - `None`: the entry does not participate in this family at all (a
///   tile-only archive has nothing to say about CQL2; a read-only collection
///   nothing to say about Part 4). It neither contributes nor narrows, and it
///   does not count as participation for [`WhenNoneParticipate`].
/// - `Some(classes)`: the entry participates and honours exactly `classes`.
///   `Some(Vec::new())` is therefore how a fold is zeroed for good — an entry
///   that participates but honours nothing removes every candidate, and no
///   later entry can re-add a class an earlier one already refused.
///
/// Deliberately *not* abstracted here: which entries participate and what
/// each one must prove. Those asymmetries are real — `locking` needs a read
/// lane behind a write sink, `update` needs read and write to be the same
/// single storage — and they stay written out in each caller's own closure,
/// where a reader can see them, rather than being flattened into extra
/// parameters on this signature.
fn fold_conformance_classes<T>(
    seed: &[&'static str],
    entries: impl IntoIterator<Item = T>,
    when_none_participate: WhenNoneParticipate,
    honoured: impl Fn(T) -> Option<Vec<&'static str>>,
) -> Vec<&'static str> {
    let mut classes = seed.to_vec();
    let mut participated = false;
    for entry in entries {
        let Some(declared) = honoured(entry) else {
            continue;
        };
        participated = true;
        classes.retain(|class| declared.contains(class));
    }
    if participated || when_none_participate == WhenNoneParticipate::KeepsSeed {
        classes
    } else {
        Vec::new()
    }
}

/// The all-or-nothing form of [`fold_conformance_classes`]'s `honoured`
/// answer, for a family gated on a plain capability bool rather than on a set
/// the entry declares itself (Part 2's `crs_capable`, Part 3's
/// `filter_capable`): a driver that has the capability honours the whole
/// seed, one that lacks it honours none of it.
fn honoured_if(capable: bool, seed: &[&'static str]) -> Vec<&'static str> {
    if capable {
        seed.to_vec()
    } else {
        Vec::new()
    }
}

impl Router {
    /// Builds every storage declared in `config` once, then indexes every
    /// tenant/catalog/collection declared directly on `config` (`AppConfig.
    /// tenants`/`.catalogs`/`.collections`) — the file-backed default, and
    /// the only behavior this ever had before `#42`'s relational registry
    /// backend existed. Thin wrapper over [`build_from_snapshot`](Self::
    /// build_from_snapshot), the actual index-building implementation; see
    /// `context::build_router_and_resolver` for the entry point that
    /// dispatches on `config.registry.backend` instead of always reading
    /// `config.tenants`/`.catalogs`/`.collections` directly.
    pub fn build(config: &AppConfig, registry: &Registry) -> Result<Self> {
        Self::build_from_snapshot(
            config,
            &config.tenants,
            &config.catalogs,
            &config.collections,
            registry,
        )
    }

    /// Builds every storage declared in `config` once, then indexes each of
    /// `tenants`/`catalogs`/`collections` — the normalized routing input
    /// (`#42`, third slice; `#143` for `tenants` itself) — by internal id,
    /// resolving each collection's `features` and `tiles` lanes to their
    /// ordered driver chains (`#21`) and materializing its effective
    /// settings (`#39`, `settings.rs`). Each is either `config.tenants`/
    /// `.catalogs`/`.collections` themselves for the file-backed default
    /// ([`build`](Self::build)), or a walked snapshot for the relational
    /// backend (`context::build_router_and_resolver`) — `catalogs`/
    /// `collections` from a [`RoutingSnapshot`](crate::config::RoutingSnapshot)
    /// walked via a `RegistryReader`, `tenants` from `tenant::
    /// snapshot_tenants` walked via a `TenantReader`. This function cannot
    /// tell which source produced any of the three, which is exactly the
    /// point: one index-building implementation, never two. Everything else
    /// built here (drivers, settings inheritance) still reads `config`
    /// directly.
    ///
    /// Assumes `config` already passed `AppConfig::validate` and, when
    /// `tenants`/`catalogs`/`collections` came from a relational walk rather
    /// than `config` itself, that they already passed
    /// [`validate_tenant_snapshot`](crate::tenant::validate_tenant_snapshot)/
    /// [`validate_registry_snapshot`](crate::config::validate_registry_snapshot)
    /// too (ids unique at their scope, refs resolvable — including every
    /// routing lane's storage ids, and every collection's `catalog` /
    /// catalog's `tenant`).
    pub fn build_from_snapshot(
        config: &AppConfig,
        tenants: &[TenantDecl],
        catalogs: &[CatalogDecl],
        collections: &[CollectionDecl],
        registry: &Registry,
    ) -> Result<Self> {
        let mut drivers = HashMap::with_capacity(config.storages.len());
        for storage in &config.storages {
            drivers.insert(storage.id.clone(), registry.build(storage)?);
        }

        let object_stores = build_object_stores(&config.object_stores)?;

        let catalogs_by_id: HashMap<&str, &CatalogDecl> = catalogs
            .iter()
            .map(|catalog| (catalog.id.as_str(), catalog))
            .collect();
        let tenants_by_id: HashMap<&str, &TenantDecl> = tenants
            .iter()
            .map(|tenant| (tenant.id.as_str(), tenant))
            .collect();
        // Named profiles (`#111`) — looked up once here rather than per
        // collection; `resolve_effective_settings_with_provenance` takes
        // this same map for every node it resolves.
        let profiles_by_id: HashMap<&str, &SettingsDecl> = config
            .profiles
            .iter()
            .map(|profile| (profile.id.as_str(), &profile.settings))
            .collect();

        // `#185`: the per-catalog protocol exposure matrix, resolved at the
        // catalog node (empty collection level — this key is never asked at
        // a depth below a catalog; see the field's own doc for why the gate
        // cannot wait for a collection to be known). Built over `catalogs`
        // itself rather than inside the per-collection loop below, so a
        // catalog with no collections at all still gets its roots gated.
        let mut catalog_protocols = HashMap::with_capacity(catalogs.len());
        for catalog in catalogs {
            let Some(tenant) = tenants_by_id.get(catalog.tenant.as_str()) else {
                return Err(Error::Config(format!(
                    "catalog '{}' references unknown tenant '{}'",
                    catalog.id, catalog.tenant
                )));
            };
            let resolved = settings::resolve_effective_settings(
                &SettingsDecl::default(),
                &catalog.settings,
                &tenant.settings,
                &config.settings,
                &profiles_by_id,
            );
            catalog_protocols.insert(catalog.id.clone(), resolved.protocols_or_default());
        }

        let mut routed_collections = HashMap::with_capacity(collections.len());
        let mut effective_settings = HashMap::with_capacity(collections.len());
        let mut effective_settings_provenance = HashMap::with_capacity(collections.len());
        let mut effective_visibility = HashMap::with_capacity(collections.len());
        for collection in collections {
            let features = build_lane(&drivers, collection, collection.routing.features.as_ref())?;
            let tiles = build_lane(&drivers, collection, collection.routing.tiles.as_ref())?;
            // The maps lane defaults to the single `storage` exactly like
            // `tiles` (`#86`, `RoutingDecl::maps`'s own doc) — not the
            // opt-in-only shape `write`/`index`/`search` have below.
            let maps = build_lane(&drivers, collection, collection.routing.maps.as_ref())?;
            // Write has no "defaults to the single storage" fallback (see
            // `RoutedCollection::write`'s own doc) — only built when the
            // collection explicitly names a `routing.write`.
            let write = match &collection.routing.write {
                Some(routing) => Some(build_lane(&drivers, collection, Some(routing))?),
                None => None,
            };
            // Index has no "defaults to the single storage" fallback either
            // (`#67`, same reasoning as `write`) — only built when the
            // collection explicitly names a `routing.index`.
            let index = match &collection.routing.index {
                Some(routing) => Some(build_lane(&drivers, collection, Some(routing))?),
                None => None,
            };
            // Search has no "defaults to the single storage" fallback either
            // (`#67`, same reasoning as `write`/`index`) — only built when the
            // collection explicitly names a `routing.search`. Unlike `write`/
            // `index`, `build_lane` here may resolve more than one entry (an
            // ordered fallback tail); nothing about its construction needs to
            // know that.
            let search = match &collection.routing.search {
                Some(routing) => Some(build_lane(&drivers, collection, Some(routing))?),
                None => None,
            };

            // A driver's physical-target syntax check is per storage, not
            // per lane; validate once against each distinct storage this
            // collection's lanes actually touch.
            let mut checked = HashSet::new();
            for (storage_id, driver) in features
                .entries
                .iter()
                .chain(tiles.entries.iter())
                .chain(maps.entries.iter())
                .chain(write.iter().flat_map(|lane| lane.entries.iter()))
                .chain(index.iter().flat_map(|lane| lane.entries.iter()))
                .chain(search.iter().flat_map(|lane| lane.entries.iter()))
            {
                if checked.insert(storage_id.as_str()) {
                    driver.validate_collection(collection)?;
                }
            }

            let catalog = catalogs_by_id
                .get(collection.catalog.as_str())
                .ok_or_else(|| {
                    Error::Config(format!(
                        "collection '{}' references unknown catalog '{}'",
                        collection.id, collection.catalog
                    ))
                })?;
            let tenant = tenants_by_id.get(catalog.tenant.as_str()).ok_or_else(|| {
                Error::Config(format!(
                    "catalog '{}' references unknown tenant '{}'",
                    catalog.id, catalog.tenant
                ))
            })?;

            let (resolved_settings, resolved_provenance) =
                settings::resolve_effective_settings_with_provenance(
                    &collection.settings,
                    &catalog.settings,
                    &tenant.settings,
                    &config.settings,
                    &profiles_by_id,
                );
            // A collection's own explicit per-zoom caps still win outright
            // over anything inherited — inheritance only fills the gap when
            // the collection left `tiles.caps` empty. See `settings.rs`'s
            // module doc for why this lives here (materialized once, per
            // collection) rather than re-walked per request. Provenance
            // follows the same branch (`#110`): an explicit `tiles.caps`
            // resolves through a rule outside the settings chain entirely,
            // so it is tagged `Derived` rather than whichever level the
            // settings chain itself would have named.
            let (inherited_tile_caps, tile_caps_provenance) = if collection.tiles.caps.0.is_empty()
            {
                (
                    resolved_settings.tile_caps.clone(),
                    resolved_provenance.tile_caps,
                )
            } else {
                (collection.tiles.caps.clone(), SettingsProvenance::Derived)
            };
            effective_settings.insert(
                collection.id.clone(),
                EffectiveSettings {
                    tile_caps: inherited_tile_caps,
                    cache_ttl_s: resolved_settings.cache_ttl_s,
                    slow_request_ms: resolved_settings.slow_request_ms,
                    stac: resolved_settings.stac,
                    tile_properties: resolved_settings.tile_properties,
                    colormap: resolved_settings.colormap,
                    max_request_body_bytes: resolved_settings.max_request_body_bytes,
                    tile_vertex_budget: resolved_settings.tile_vertex_budget,
                    items_vertex_budget: resolved_settings.items_vertex_budget,
                    page_max_bytes: resolved_settings.page_max_bytes,
                    max_asset_bytes: resolved_settings.max_asset_bytes,
                    asset_media_types: resolved_settings.asset_media_types,
                    batch: resolved_settings.batch,
                    protocols: resolved_settings.protocols,
                },
            );
            effective_settings_provenance.insert(
                collection.id.clone(),
                EffectiveSettingsProvenance {
                    tile_caps: tile_caps_provenance,
                    ..resolved_provenance
                },
            );

            // `#34`: the collection's own visibility wins outright when it
            // sets any non-default value; otherwise it inherits the owning
            // catalog's — see `VisibilityDecl`'s own doc.
            let resolved_visibility = if collection.visibility.is_default() {
                catalog.visibility.clone()
            } else {
                collection.visibility.clone()
            };
            effective_visibility.insert(collection.id.clone(), resolved_visibility);

            routed_collections.insert(
                collection.id.clone(),
                RoutedCollection {
                    decl: collection.clone(),
                    tenant: tenant.id.clone(),
                    catalog: catalog.id.clone(),
                    features_explicit: collection.routing.features.is_some(),
                    tiles_explicit: collection.routing.tiles.is_some(),
                    maps_explicit: collection.routing.maps.is_some(),
                    features,
                    tiles,
                    maps,
                    write,
                    index,
                    search,
                },
            );
        }

        let descriptor_cache = moka::future::Cache::builder()
            .max_capacity(config.server.descriptor_cache_capacity)
            .build();
        // Reuses `descriptor_cache_capacity` rather than a dedicated knob:
        // this cache is bounded by the same registry-scale-out concern
        // (`#42`) and holds at most one entry per collection, same as
        // `descriptor_cache`.
        let pin_verification_cache = moka::future::Cache::builder()
            .max_capacity(config.server.descriptor_cache_capacity)
            .build();
        // `#101`: same "reuses `descriptor_cache_capacity` rather than a
        // dedicated knob" reasoning as `pin_verification_cache` above — one
        // entry per collection, same registry-scale-out concern.
        let geometry_profile_cache = moka::future::Cache::builder()
            .max_capacity(config.server.descriptor_cache_capacity)
            .build();

        Ok(Self {
            collections: routed_collections,
            drivers,
            object_stores,
            descriptor_ttl: Duration::from_secs(config.server.descriptor_ttl_s),
            registry_validation: config.registry.validation,
            effective_settings,
            effective_settings_provenance,
            effective_visibility,
            catalog_protocols,
            descriptor_cache,
            pin_verification_cache,
            geometry_profile_cache,
        })
    }

    /// The materialized effective settings for one collection (internal
    /// id) — `None` only for an id `Router` never indexed (an unresolvable
    /// collection has no settings to report, same as it has no descriptor).
    pub fn effective_settings(&self, collection_internal_id: &str) -> Option<&EffectiveSettings> {
        self.effective_settings.get(collection_internal_id)
    }

    /// `effective_settings`'s own provenance (`#110`) for the same
    /// collection — always present exactly when `effective_settings` is,
    /// since both maps are populated together in the same
    /// `build_from_snapshot` pass. Backs the control lane's effective-config
    /// view (`tellurion-server::config_view`); no request-lane code calls
    /// this.
    pub fn effective_settings_provenance(
        &self,
        collection_internal_id: &str,
    ) -> Option<&EffectiveSettingsProvenance> {
        self.effective_settings_provenance
            .get(collection_internal_id)
    }

    /// The materialized protocol exposure matrix for one catalog (internal
    /// id, `#185`) — `None` only for a catalog this `Router` never indexed,
    /// which is also a catalog no request can route to, so a caller that
    /// gates on this treats `None` as "nothing to gate" and lets the
    /// handler's own not-found answer stand.
    pub fn catalog_protocols(&self, catalog_internal_id: &str) -> Option<ProtocolsConf> {
        self.catalog_protocols.get(catalog_internal_id).copied()
    }

    /// How many collections this `Router` actually indexed (`#42`, `#59`) —
    /// the snapshot's own count, not whatever `AppConfig.collections` itself
    /// happens to hold. Identical to `config.collections.len()` for the
    /// file-backed default (`Router::build` indexes exactly that slice), but
    /// the only correct source under `registry.backend: relational`, where
    /// `AppConfig.collections` is always empty by the double-source rule and
    /// every real collection lives in `Router`'s own snapshot-built index
    /// instead. See `reload.rs`'s own reload-complete log line, the reason
    /// this exists.
    pub fn collection_count(&self) -> usize {
        self.collections.len()
    }

    /// The materialized effective visibility for one collection (internal
    /// id, `#34` policy layer) — `None` only for an id `Router` never
    /// indexed. See [`effective_settings`](Self::effective_settings)'s own
    /// doc for why this is precomputed once at build time rather than
    /// re-walked per request.
    pub fn effective_visibility(&self, collection_internal_id: &str) -> Option<&VisibilityDecl> {
        self.effective_visibility.get(collection_internal_id)
    }

    /// This collection's declared [`CollectionKind`] (`#192`) — `None` only
    /// for an id this `Router` never indexed, the same convention
    /// [`effective_visibility`](Self::effective_visibility) uses. Read
    /// straight off the indexed declaration rather than materialized into a
    /// map of its own: unlike settings or visibility, a kind inherits from
    /// nothing and resolves against nothing, so there is no chain to walk
    /// and nothing to precompute.
    pub fn collection_kind(&self, collection_internal_id: &str) -> Option<CollectionKind> {
        self.collections
            .get(collection_internal_id)
            .map(|routed| routed.decl.kind)
    }

    /// Whether this collection's write lane resolves to a usable
    /// [`WriteSink`] — the advertisement-side reading of exactly the
    /// predicate [`resolve_write`](Self::resolve_write) enforces on the
    /// request path (`#208`).
    ///
    /// `false` for an id this `Router` never indexed, which keeps the
    /// convention every other accessor here follows: a collection that does
    /// not resolve has no write capability to report, and the caller's own
    /// not-found answer stands.
    ///
    /// Exists so an `Allow` header can be *derived from* live write
    /// capability instead of asserted alongside it. OGC API — Features —
    /// Part 4 (OGC 20-002r1) Requirement 16 clause C
    /// (`/req/create-replace-delete/options-response`) requires the `Allow`
    /// value to be "the list of methods that are allowed for the resource
    /// at the time and within the context of the request", so the
    /// advertisement has to read the same routing snapshot the write itself
    /// will — the `#220` rule that an advertisement and a request must be
    /// incapable of disagreeing, applied to a header instead of a link.
    ///
    /// Deliberately synchronous and I/O-free: it stops at the point
    /// `resolve_write` stops caring about capability. `resolve_write` goes
    /// on to derive an effective decl, which can fail for reasons that are
    /// not capability questions at all (a stale descriptor, a storage
    /// outage) and which no `OPTIONS` response should pay for.
    pub fn write_lane_resolves(&self, collection_internal_id: &str) -> bool {
        self.collections
            .get(collection_internal_id)
            .is_some_and(write_lane_resolves)
    }

    /// Whether *any* collection this `Router` indexed declares
    /// `kind: record` (`#192`).
    ///
    /// Exists so the server's per-request kind gate
    /// (`app::enforce_collection_kind`) can answer "nothing to gate" with
    /// one `bool` read instead of resolving a collection id on every request
    /// that names one. A deployment that never declares a record collection
    /// — every deployment written before `#192` — therefore pays a single
    /// branch and reaches its handlers exactly as it did before, which is
    /// what keeps the tiles hot path and the "unconfigured deployments are
    /// byte-for-byte unchanged" rule intact at the same time.
    pub fn has_record_collections(&self) -> bool {
        self.collections
            .values()
            .any(|routed| routed.decl.kind.is_record())
    }

    /// Sums every registered storage's `capacity_hint`. `None` if there are
    /// no storages, or if any one of them declines to report a hint — a
    /// partial sum would understate capacity for the drivers that did
    /// answer, which is worse than admitting the caller has no coherent
    /// number to work with at all.
    pub fn total_capacity_hint(&self) -> Option<usize> {
        if self.drivers.is_empty() {
            return None;
        }
        self.drivers
            .values()
            .map(|driver| driver.capacity_hint())
            .sum()
    }

    /// Exercises the mandatory catalog capability of every configured
    /// storage. This is intentionally separate from catalog validation: a
    /// readiness poll only needs to prove the existing dependency seam is
    /// usable, not rebuild descriptors or revalidate configuration.
    pub async fn probe_storages(&self) -> Result<()> {
        let mut storages: Vec<_> = self.drivers.iter().collect();
        storages.sort_unstable_by_key(|(storage_id, _)| storage_id.as_str());

        let mut failed = Vec::new();
        for (storage_id, driver) in storages {
            if driver.catalog_source().collections().await.is_err() {
                failed.push(storage_id.as_str());
            }
        }

        if !failed.is_empty() {
            return Err(Error::Storage(Box::new(std::io::Error::other(format!(
                "readiness probe failed for storages: {}",
                failed.join(", ")
            )))));
        }
        Ok(())
    }

    /// Cross-checks every configured collection against physical reality,
    /// once, at boot — the config-load half of the driver contract (see the
    /// design doc's "Capability checks happen ... at config load ... never
    /// at request time" rule) — and eagerly derives + caches every
    /// collection's `CollectionDescriptor` (`#19`/`#27`) so the first request
    /// never pays the derivation cost. Six things fail this fast, with an
    /// actionable message: a collection's target table (its `table` override,
    /// or its `id` by convention — see `descriptor::target_table`) is absent
    /// from the features lane anchor storage's `CatalogSource` enumeration
    /// (see `RoutedCollection::anchor`); that table reports more than one
    /// geometry column and none is pinned (`#104`, see
    /// `refuse_ambiguous_geometry_column`); a declared `geometry_variants`
    /// entry names a column the backend doesn't report, or one whose SRID or
    /// geometry type disagrees with the base column's (`#104`, see
    /// `refuse_invalid_geometry_variants`); a collection declares `places3d`
    /// (built from MVT, same as the tiles lane) while any driver in its
    /// resolved tiles lane does not advertise `TileSource`; an explicitly
    /// routed lane (`#21`) names a storage whose driver does not implement
    /// that lane's capability trait; or a collection's `geometry`/`pk` is
    /// neither overridden nor derivable from the backend. An unrouted lane
    /// (the default single-`storage` case) is not eagerly capability-checked
    /// beyond the places3d rule — not every collection needs every
    /// capability, and a request-time `CapabilityUnsupported` still covers
    /// it, exactly as before lanes existed.
    ///
    /// Deliberately not run as part of `build`: `build` stays a fast,
    /// synchronous wiring step; this does I/O (one `CatalogSource` query per
    /// registered storage, even ones with no collections yet — a broken
    /// storage is worth surfacing at boot regardless) and so is async and
    /// explicit at the call site (`main.rs`, right after `build`).
    pub async fn validate_catalog(&self) -> Result<()> {
        // `#104`: grouped into a `Vec` per table name, not a single
        // `PhysicalCollection`, so a table with more than one geometry
        // column keeps every candidate row visible to the loop below,
        // instead of collapsing them into whichever one a plain `insert`
        // happened to overwrite last.
        let mut catalogs: HashMap<&str, HashMap<String, Vec<crate::catalog::PhysicalCollection>>> =
            HashMap::with_capacity(self.drivers.len());
        for (storage_id, driver) in &self.drivers {
            let physical = driver
                .catalog_source()
                .collections()
                .await
                .map_err(|source| {
                    Error::Config(format!(
                        "storage '{storage_id}': catalog introspection failed: {source}"
                    ))
                })?;
            let mut by_name: HashMap<String, Vec<crate::catalog::PhysicalCollection>> =
                HashMap::new();
            for p in physical {
                by_name.entry(p.name.clone()).or_default().push(p);
            }
            catalogs.insert(storage_id.as_str(), by_name);
        }

        for routed in self.collections.values() {
            let decl = &routed.decl;

            if routed.features_explicit {
                validate_lane_capability(&decl.id, "features", &routed.features, |driver| {
                    driver.feature_source().is_some()
                })?;
            }
            if routed.tiles_explicit {
                // `#37`: the tiles lane's capability is MVT (`tile_source`)
                // OR raster (`raster_source`) — a driver claims at most one,
                // but either satisfies an explicit `routing.tiles` entry.
                validate_lane_capability(&decl.id, "tiles", &routed.tiles, |driver| {
                    driver.tile_source().is_some() || driver.raster_source().is_some()
                })?;
            }
            if routed.maps_explicit {
                // `#86`, first slice: the maps lane only ever renders vector
                // collections from the existing MVT-first pipeline, so it
                // needs exactly `tile_source` — never `raster_source`, unlike
                // `tiles` above. A collection explicitly routing `maps` at a
                // raster-only storage is refused here, at boot, rather than
                // resolving to a capability this slice never implements.
                validate_lane_capability(&decl.id, "maps", &routed.maps, |driver| {
                    driver.tile_source().is_some()
                })?;
            }
            if let Some(write) = &routed.write {
                validate_lane_capability(&decl.id, "write", write, |driver| {
                    driver.write_sink().is_some()
                })?;
            }
            if let Some(index) = &routed.index {
                validate_lane_capability(&decl.id, "index", index, |driver| {
                    driver.index_sink().is_some()
                })?;
            }
            if let Some(search) = &routed.search {
                validate_search_lane_capability(&decl.id, search)?;
                validate_search_lane_provisioning(&decl.id, search, routed.index.as_ref())?;
            }
            validate_places3d_capability(decl, &routed.tiles)?;

            // `build` already resolved the features lane's primary to a
            // driver, so its storage's catalog was queried above
            // unconditionally.
            let by_name = &catalogs[routed.anchor_storage_id()];
            let target = descriptor::target_table(decl);
            let matches = by_name.get(target).ok_or_else(|| {
                Error::Config(format!(
                    "collection '{}': storage '{}' does not report a table named '{}' in its catalog",
                    decl.id, routed.anchor_storage_id(), target
                ))
            })?;
            if decl.geometry.is_none() {
                refuse_ambiguous_geometry_column(
                    &decl.id,
                    routed.anchor_storage_id(),
                    target,
                    matches,
                )?;
            }
            refuse_invalid_geometry_variants(
                &decl.id,
                routed.anchor_storage_id(),
                target,
                matches.iter(),
                decl,
            )?;
            let physical = descriptor_physical_row(
                &decl.id,
                routed.anchor_storage_id(),
                target,
                matches,
                decl,
            )?;

            let catalog = routed.anchor().catalog_source();
            let derived = derived_fields(&catalog, physical).await.map_err(|source| {
                Error::Config(format!(
                    "collection '{}': descriptor introspection failed: {source}",
                    decl.id
                ))
            })?;
            let tile_properties = self
                .effective_settings
                .get(&decl.id)
                .map(|effective| effective.tile_properties.as_slice())
                .unwrap_or(&[]);
            let resolved =
                merge_and_enforce(routed.anchor(), decl, physical, derived, tile_properties)?;
            self.descriptor_cache
                .insert(
                    decl.id.clone(),
                    CachedDescriptor {
                        outcome: Ok(resolved),
                        computed_at: Instant::now(),
                    },
                )
                .await;

            if routed.tiles_explicit
                && routed
                    .tiles
                    .entries
                    .iter()
                    .all(|(_, driver)| driver.tile_source().is_some())
            {
                // Raster-only tile lanes were already validated above via
                // `RasterSource`. Only MVT lanes have collection-dependent
                // `TileSource::tile_capable` metadata to validate here.
                let source = tiles_source(&decl.id, &routed.tiles)?;
                let effective = self.effective_decl(routed).await?;
                if !source.tile_capable(&effective) {
                    return Err(Error::Config(format!(
                        "collection '{}': storage '{}' does not support tiles for the resolved collection metadata",
                        decl.id,
                        routed.anchor_storage_id()
                    )));
                }
            }
        }

        Ok(())
    }

    /// Looks up `collection` (internal id) and verifies it actually belongs
    /// to `tenant`/`catalog` (both internal ids, `#39`) — see
    /// `RoutedCollection::tenant`/`catalog`'s doc for why this check exists
    /// even though the map key alone already disambiguates.
    fn lookup(&self, tenant: &str, catalog: &str, collection: &str) -> Result<&RoutedCollection> {
        let routed = self.collections.get(collection).ok_or(Error::NotFound)?;
        if routed.tenant != tenant || routed.catalog != catalog {
            return Err(Error::NotFound);
        }
        Ok(routed)
    }

    /// Get-or-derive `routed`'s descriptor: a cached value still within TTL
    /// is returned as-is (an `Ok` or a cached `Config` failure alike — see
    /// `CachedDescriptor`'s doc); a missing or stale one is re-derived via
    /// [`derive_one_descriptor`] and the outcome is cached before returning.
    /// Works whether or not `validate_catalog` ever ran — that call is an
    /// eager warm-up + fail-fast, not a prerequisite (`registry.validation:
    /// lazy`, `#42`, relies on exactly that: the first request to resolve a
    /// collection is what triggers derivation here).
    ///
    /// Only a `Error::Config` outcome is ever cached, success or failure
    /// alike — a transient error (storage down, timeout) is returned as-is
    /// and never written to `descriptor_cache`, so the next request against
    /// the same collection gets a fresh attempt rather than a calcified
    /// verdict from a passing outage.
    async fn resolved_descriptor(&self, routed: &RoutedCollection) -> Result<CollectionDescriptor> {
        let collection_id = routed.decl.id.as_str();
        if let Some(cached) = self.descriptor_cache.get(collection_id).await {
            if !cached.is_stale(self.descriptor_ttl) {
                return cached.outcome.clone().map_err(Error::Config);
            }
        }

        let tile_properties = self
            .effective_settings
            .get(collection_id)
            .map(|effective| effective.tile_properties.as_slice())
            .unwrap_or(&[]);
        match derive_one_descriptor(routed, tile_properties).await {
            Ok(resolved) => {
                self.descriptor_cache
                    .insert(
                        collection_id.to_string(),
                        CachedDescriptor {
                            outcome: Ok(resolved.clone()),
                            computed_at: Instant::now(),
                        },
                    )
                    .await;
                Ok(resolved)
            }
            Err(Error::Config(message)) => {
                self.descriptor_cache
                    .insert(
                        collection_id.to_string(),
                        CachedDescriptor {
                            outcome: Err(message.clone()),
                            computed_at: Instant::now(),
                        },
                    )
                    .await;
                Err(Error::Config(message))
            }
            Err(other) => Err(other),
        }
    }

    /// Recomputes `routed`'s geometry profile unconditionally (`#101`) —
    /// bypassing `geometry_profile_cache` regardless of its own staleness —
    /// and writes the fresh result back into the cache before returning it.
    /// The physical facts a profile samples against (table/geometry column/
    /// srid/geometry type) are read from `resolved_descriptor`, reusing its
    /// existing TTL-bounded cache rather than a second `CatalogSource::
    /// collections()` round trip: those facts change far less often than a
    /// profile itself needs recomputing, and `derive_one_descriptor` already
    /// resolved the ambiguous-geometry-column case (`#104`) by the time this
    /// runs. Errors from the sampling query itself are never cached here —
    /// see [`CachedGeometryProfile`]'s own doc for why a transient failure
    /// must never calcify into a standing verdict.
    async fn geometry_profile_uncached(
        &self,
        routed: &RoutedCollection,
    ) -> Result<Option<crate::catalog::GeometryProfile>> {
        let descriptor = self.resolved_descriptor(routed).await?;
        let catalog = routed.anchor().catalog_source();
        let physical = crate::catalog::PhysicalCollection {
            name: descriptor.table,
            geometry_column: descriptor.geometry,
            primary_key: descriptor.pk,
            srid: descriptor.srid,
            geometry_type: descriptor.geometry_type,
        };
        let profile = catalog.geometry_profile(&physical).await?;
        self.geometry_profile_cache
            .insert(
                routed.decl.id.clone(),
                CachedGeometryProfile {
                    profile,
                    computed_at: Instant::now(),
                },
            )
            .await;
        Ok(profile)
    }

    /// Get-or-compute `routed`'s geometry profile (`#101`): a cached value
    /// still within `descriptor_ttl` is returned as-is; a missing or stale
    /// one is recomputed via [`geometry_profile_uncached`](Self::
    /// geometry_profile_uncached). Deliberately its own cache
    /// (`geometry_profile_cache`), never folded into `descriptor_cache` —
    /// see that field's own doc on `Router` for why a sampled profile must
    /// not ride the same TTL cadence the hot feature/tile resolve path
    /// (`effective_decl`) depends on. The public [`geometry_profile`](Self::
    /// geometry_profile) and `canonical_descriptor` are this method's only
    /// callers, neither on that hot path.
    async fn geometry_profile_cached(
        &self,
        routed: &RoutedCollection,
    ) -> Result<Option<crate::catalog::GeometryProfile>> {
        let collection_id = routed.decl.id.as_str();
        if let Some(cached) = self.geometry_profile_cache.get(collection_id).await {
            if !cached.is_stale(self.descriptor_ttl) {
                return Ok(cached.profile);
            }
        }
        self.geometry_profile_uncached(routed).await
    }

    /// The current geometry-statistics profile for `(tenant, catalog,
    /// collection)` (`#101`), TTL-cached — see
    /// [`geometry_profile_cached`](Self::geometry_profile_cached)'s own doc
    /// for why this rides a separate cache from `descriptor_cache`. `Ok
    /// (None)` both when this collection's driver never overrides
    /// `CatalogSource::geometry_profile` (the default) and when it genuinely
    /// has nothing to report — the same "capability-declined and
    /// genuinely-absent look identical" shape `extent`/`row_estimate`
    /// already have. A collection nobody ever asked this about stays
    /// exactly as cheap as it was before `#101`: nothing here runs unless a
    /// caller reaches for it.
    pub async fn geometry_profile(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<Option<crate::catalog::GeometryProfile>> {
        let _phase = enter_phase(Phase::Routing);
        let routed = self.lookup(tenant, catalog, collection)?;
        self.geometry_profile_cached(routed).await
    }

    /// Forces a fresh geometry-profile computation for `(tenant, catalog,
    /// collection)`, bypassing `geometry_profile_cache` regardless of its
    /// TTL — the explicit refresh path design point 3 (`#101`) calls for:
    /// this profile is derived data about a mutable table, and nothing
    /// should be able to trust an old one indefinitely just because the
    /// cache TTL hasn't expired yet. The result also replaces whatever was
    /// cached, so a subsequent [`geometry_profile`](Self::geometry_profile)
    /// call sees the fresh value immediately rather than waiting out the
    /// TTL itself.
    pub async fn refresh_geometry_profile(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<Option<crate::catalog::GeometryProfile>> {
        let _phase = enter_phase(Phase::Routing);
        let routed = self.lookup(tenant, catalog, collection)?;
        self.geometry_profile_uncached(routed).await
    }

    /// `routed.decl` with `table`/`geometry`/`pk` guaranteed `Some`: unchanged
    /// when the config fully overrides all three (`#61`: still no descriptor
    /// derivation, though under lazy validation this now costs one cached
    /// verification probe — see below), else filled from `resolved_descriptor`
    /// — which also fills `datetime` (override > derived, same precedence as
    /// `geometry`/`pk`) and `row_estimate` (always backend-derived, no
    /// override concept).
    ///
    /// Design note (`#19`): a collection with `table`/`geometry`/`pk` all
    /// overridden takes the fast path and never derives `datetime`/
    /// `row_estimate` either — an operator who has fully pinned a
    /// collection's physical shape hasn't asked for anything else derived,
    /// and `row_estimate`-driven tile-cap heuristics only matter for a
    /// collection whose `tiles.caps` is *also* left unconfigured (see
    /// `descriptor::heuristics::effective_feature_cap`), which is the same
    /// "leave it to derive" collection shape this fast path already skips.
    /// This is still true after `#61`: the fast path never calls
    /// `resolved_descriptor`, so it never gains `datetime`/`row_estimate`/
    /// `extent`/`attributes` — only the pinned contract's *derivation* half
    /// changed. The pinned contract itself ("a fully-overridden collection
    /// never derives from the catalog") stays intact either way.
    ///
    /// `#61`: under `registry.validation: lazy`, this fast path now runs
    /// [`verify_pinned_collection`](Self::verify_pinned_collection) — one
    /// cached, once-per-collection existence/type probe closing the
    /// asymmetry with `eager` (whose boot sweep already validates every
    /// collection, pinned or not, before the fast path is ever reached; see
    /// that method's own doc for why it is skipped under `eager` rather than
    /// run redundantly). Before `#61`, a fully-pinned collection was never
    /// checked against the backend at all under `lazy`, at boot or at first
    /// touch, so a typo'd pin surfaced only as a raw storage error the first
    /// time a real query ran against it — never a named `Error::Config` the
    /// way every other collection shape gets. See `config/example.yaml`'s own
    /// `registry:` comment for the operator-facing version of this trade-off.
    async fn effective_decl(&self, routed: &RoutedCollection) -> Result<CollectionDecl> {
        let decl = &routed.decl;
        if decl.table.is_some() && decl.geometry.is_some() && decl.pk.is_some() {
            if self.registry_validation == RegistryValidationMode::Lazy {
                self.verify_pinned_collection(routed).await?;
            }
            return Ok(
                self.apply_inherited_tile_properties(self.apply_inherited_settings(decl.clone()))
            );
        }

        let descriptor = self.resolved_descriptor(routed).await?;
        let mut resolved = decl.clone();
        resolved.table = Some(descriptor.table);
        // Already `Option<String>` on both sides (`#20`): `None` here means
        // this collection's anchor driver has no table-shaped geometry/pk
        // concept (PMTiles) — a `FeatureSource` consumer would never reach
        // this decl with either field `None` (`merge_and_enforce` fails
        // boot/derivation first), so no `.expect()` here is ever exercised
        // by a real query-building driver.
        resolved.geometry = descriptor.geometry;
        resolved.pk = descriptor.pk;
        resolved.datetime = descriptor.datetime;
        resolved.row_estimate = descriptor.row_estimate;
        resolved.srid = descriptor.srid;
        // `#36`: the backend's own projection facts, carried exactly like
        // `row_estimate`/`srid` above (same fully-pinned fast path leaves it
        // `None`) — the STAC lane reads it off the decl to emit `proj:*`
        // fields it can genuinely stand behind.
        resolved.projection = descriptor.projection;
        // `#278`: the backend's own column list, carried so a driver can
        // project GeoJSON `properties` by naming columns instead of
        // rendering every column (geometry included) through `to_jsonb` and
        // then deleting the ones it didn't want. Same derived-carrier shape
        // as `row_estimate`/`srid` above, and the same fully-pinned fast
        // path leaves it `None` — see `CollectionDecl::attribute_columns`.
        resolved.attribute_columns = descriptor.attributes;
        Ok(self.apply_inherited_tile_properties(self.apply_inherited_settings(resolved)))
    }

    /// Overlays this collection's materialized `EffectiveSettings` values
    /// (`settings.rs`, `#39`) onto the decl a driver actually receives —
    /// the settings-inheritance chain's real consumers. Applied
    /// unconditionally, on both `effective_decl` paths (physically-overridden
    /// fast path included): settings inheritance is independent of whether
    /// this collection's table/geometry/pk needed backend derivation.
    /// `Router::build` already decided each winning value (the collection's
    /// own if set, else the nearest ancestor's) — this just carries that
    /// decision onto the decl callers/drivers see.
    ///
    /// `tile_caps` lands on `decl.tiles.caps` — a pre-existing, separately
    /// named field every zoom-cap consumer already reads (`#39`'s own
    /// bridge from the new inheritance input onto the old consumption
    /// site). `colormap` (`#92`) has no such separate site: it overlays
    /// `decl.settings.colormap` in place, so after this call that field no
    /// longer means "this collection's own declared override" but "the
    /// fully resolved, effective value" — the same reading every other
    /// field this method touches already gets once a decl has passed
    /// through `effective_decl`.
    fn apply_inherited_settings(&self, mut decl: CollectionDecl) -> CollectionDecl {
        if let Some(effective) = self.effective_settings.get(&decl.id) {
            decl.tiles.caps = effective.tile_caps.clone();
            decl.settings.colormap = effective.colormap.clone();
            // `#90`: same overlay shape as `colormap` above — a `TileSource`
            // driver only ever sees a `CollectionDecl`, never the `Router`
            // that resolved the settings chain, so the effective value has
            // to land somewhere on the decl itself for the driver to read.
            decl.settings.tile_vertex_budget = Some(effective.tile_vertex_budget);
            decl.settings.items_vertex_budget = Some(effective.items_vertex_budget);
            // `#184`: same overlay shape again, but the resolved value is
            // itself an `Option` (no built-in default — `None` means the
            // page byte budget is off), so it lands as-is rather than
            // wrapped in `Some` like the two budgets above.
            decl.settings.page_max_bytes = effective.page_max_bytes;
        }
        decl
    }

    /// Overlays this collection's materialized
    /// `EffectiveSettings.tile_properties` (`settings.rs`, `#85`) onto the
    /// decl a `TileSource` driver actually receives — same shape and same
    /// "applied unconditionally on both `effective_decl` paths" reasoning as
    /// [`apply_inherited_tile_caps`]. `decl.tile_properties` itself is never
    /// operator-configured (`#[serde(skip)]` — see that field's own doc);
    /// this is the only place it is ever written.
    fn apply_inherited_tile_properties(&self, mut decl: CollectionDecl) -> CollectionDecl {
        if let Some(effective) = self.effective_settings.get(&decl.id) {
            decl.tile_properties = effective.tile_properties.clone();
        }
        decl
    }

    /// [`effective_decl`](Self::effective_decl) plus this collection's
    /// geometry profile (`#101`/`#102`) attached onto
    /// `CollectionDecl::geometry_profile` — the one deliberate place a
    /// profile crosses from `Router`'s own TTL-cached `geometry_profile_
    /// cache` onto the decl a driver actually receives, so `TileSource::
    /// mvt_tile` (`descriptor::heuristics::
    /// simplify_tolerance_meters_for_profile`'s caller) can read
    /// `decl.geometry_profile` directly instead of needing a `Router`
    /// reference of its own.
    ///
    /// Called only by [`resolve_tiles`](Self::resolve_tiles) and
    /// [`resolve_maps`](Self::resolve_maps) — never `resolve_features`/
    /// `resolve_write`/`resolve_raster` — because a geometry profile only
    /// feeds simplification tolerance, and only those two lanes resolve to
    /// a `TileSource` that ever consults it (`resolve_maps` rasterizes the
    /// same MVT `resolve_tiles` itself vends — see `RoutingDecl::maps`'s own
    /// doc). `resolve_raster` stays on plain `effective_decl`: a raster-
    /// native backend (Cloud-Optimized GeoTIFF) has no `ST_SimplifyPreserveTopology`-
    /// shaped tolerance concept for a profile to feed in the first place.
    ///
    /// Cost: `geometry_profile_cached` is the same TTL-cached read
    /// `canonical_descriptor` already uses — once warm, a hit costs one
    /// in-memory `moka` lookup, no I/O. The first request for a collection
    /// within a TTL window (or the first ever) pays for one bounded
    /// `TABLESAMPLE` query (`CatalogSource::geometry_profile`'s own doc:
    /// cheap relative to table size, but not free) — accepted here the same
    /// way `effective_decl`'s own descriptor derivation already accepts a
    /// first-request cost for `row_estimate`/`srid`, deliberately in
    /// preference to an eager boot-time prefetch: sampling every
    /// collection's geometry at startup would slow boot in proportion to
    /// collection count and sample tables that may never serve a tile
    /// request at all (a features-only collection has no tiles lane to
    /// trigger this in the first place). A profile-computation failure
    /// never fails the resolve; the decl proceeds with `geometry_profile:
    /// None`, the same fallback as a collection whose driver never computed
    /// one at all — mirroring `canonical_descriptor`'s own never-fail-the-
    /// request handling of this same call.
    async fn effective_tile_decl(&self, routed: &RoutedCollection) -> Result<CollectionDecl> {
        let mut decl = self.effective_decl(routed).await?;
        decl.geometry_profile = match self.geometry_profile_cached(routed).await {
            Ok(profile) => profile,
            Err(error) => {
                tracing::warn!(
                    %error,
                    collection = %routed.decl.external_id(),
                    "failed to derive geometry profile; tile lane falls back to the zoom-only                      simplification tolerance"
                );
                None
            }
        };
        Ok(decl)
    }

    /// Lazy validation's once-per-collection existence/type check for a
    /// fully-pinned collection (`table`/`geometry`/`pk` all overridden,
    /// `#61`). `effective_decl`'s fast path never derives a descriptor for
    /// such a collection — the pinned contract stays intact — which under
    /// `registry.validation: lazy` used to mean a typo'd pin surfaced only as
    /// a raw storage error on the collection's first real query, never a
    /// named config error. This runs [`probe_pinned_collection`] — one
    /// `CatalogSource::collections()` call, the cheapest existence/type check
    /// available, never turned into a `CollectionDescriptor` — and caches the
    /// verdict (success or a named `Error::Config` failure) in
    /// `pin_verification_cache`, TTL-governed exactly like `resolved_descriptor`
    /// caches every other lazily validated collection's verdict (see
    /// `CachedDescriptor`'s own doc for why both outcomes are cached).
    ///
    /// Only ever called from `effective_decl`'s fast path when
    /// `registry_validation` is `Lazy` — under `Eager`, `validate_catalog`'s
    /// boot sweep already validated every collection, pinned or not, before
    /// the server ever started serving requests, so a second check here would
    /// just be a redundant backend round trip for no new information (see
    /// `effective_decl`'s own doc).
    async fn verify_pinned_collection(&self, routed: &RoutedCollection) -> Result<()> {
        let collection_id = routed.decl.id.as_str();
        if let Some(cached) = self.pin_verification_cache.get(collection_id).await {
            if !cached.is_stale(self.descriptor_ttl) {
                return cached.outcome.clone().map_err(Error::Config);
            }
        }

        match probe_pinned_collection(routed).await {
            Ok(()) => {
                self.pin_verification_cache
                    .insert(
                        collection_id.to_string(),
                        CachedPinVerification {
                            outcome: Ok(()),
                            computed_at: Instant::now(),
                        },
                    )
                    .await;
                Ok(())
            }
            Err(Error::Config(message)) => {
                self.pin_verification_cache
                    .insert(
                        collection_id.to_string(),
                        CachedPinVerification {
                            outcome: Err(message.clone()),
                            computed_at: Instant::now(),
                        },
                    )
                    .await;
                Err(Error::Config(message))
            }
            Err(other) => Err(other),
        }
    }

    /// The effective (post-precedence) `CollectionDescriptor` for
    /// `(tenant, collection)` — table/geometry/pk after override/derivation,
    /// plus the derived spatial extent. Unlike `resolve_features`/
    /// `resolve_tiles`, this always consults the descriptor cache (extent has
    /// no override, so it is never free to skip): callers that only need
    /// physical fields for query-building should use those instead.
    pub async fn collection_descriptor(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<CollectionDescriptor> {
        let _phase = enter_phase(Phase::Routing);
        let routed = self.lookup(tenant, catalog, collection)?;
        self.resolved_descriptor(routed).await
    }

    /// The one read-side merge of every metadata source this workspace has
    /// for `(tenant, catalog, collection)` (`#50`, first half) — see
    /// `descriptor::canonical`'s module doc for the four sources and the
    /// provenance rule. `Err` only for an unresolvable `(tenant, catalog,
    /// collection)` triple (`Error::NotFound`, from `lookup`); a descriptor-
    /// derivation failure never fails this call the way it can
    /// `collection_descriptor` — it is caught here, logged once, and
    /// surfaces as absent physical facts inside an otherwise-populated
    /// `CanonicalDescriptor` (the same never-fail-the-request-over-metadata
    /// philosophy `tellurion-stac`/`tellurion-features` already applied
    /// per-crate before this merge existed, now centralized in one place).
    ///
    /// Reuses the existing TTL-bounded `descriptor_cache` for the physical
    /// facts (via `resolved_descriptor`, the same call `collection_descriptor`
    /// makes) and the existing `resolve_features`/`resolve_tiles` capability
    /// probes for `capabilities` — no second cache concept, and identical
    /// has-capability semantics to what callers computed by hand before this
    /// existed (including the interaction with `effective_decl`'s fast path
    /// for a collection whose `table`/`geometry`/`pk` are all overridden).
    pub async fn canonical_descriptor(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<descriptor::canonical::CanonicalDescriptor> {
        let _phase = enter_phase(Phase::Routing);
        let routed = self.lookup(tenant, catalog, collection)?;

        let physical = match self.resolved_descriptor(routed).await {
            Ok(descriptor) => Some(descriptor),
            Err(error) => {
                tracing::warn!(
                    %error,
                    tenant,
                    catalog,
                    collection = %routed.decl.external_id(),
                    "failed to derive collection descriptor; canonical descriptor omits physical facts"
                );
                None
            }
        };

        let features_result = self.resolve_features(tenant, catalog, collection).await;
        let has_features = features_result.is_ok();
        // Capability metadata (Requirement 2, `/req/crs/fc-md-crs-list`):
        // whether the resolved `FeatureSource` can actually reproject —
        // `false` when the features lane doesn't resolve at all, same as
        // `has_features`. Read straight off the source `resolve_features`
        // already produced above rather than a second `Router` round trip.
        let crs_capable = features_result
            .as_ref()
            .map(|(_, source)| source.crs_capable())
            .unwrap_or(false);
        // `#105`: this collection's own declared CQL2 classes, read straight
        // off the same resolved `FeatureSource`. `#287`: `None` — never an
        // empty `Vec` — when the features lane doesn't resolve at all, so a
        // consumer can tell "does not participate in filtering" (no
        // `FeatureSource`, member omitted from the collection document)
        // from "participates and honours nothing" (`Some(vec![])`, the
        // honest empty list); see this field's own doc on
        // `CanonicalCapabilities`.
        let cql2_conformance_classes = features_result
            .as_ref()
            .ok()
            .map(|(_, source)| source.cql2_conformance_classes());
        // `#37`: the tiles lane's capability is MVT (`resolve_tiles`) OR
        // raster (`resolve_raster`) — a raster-only collection (Cloud-
        // Optimized GeoTIFF) must still report `tiles: true` here, or every
        // `#50` consumer of this canonical descriptor (starting with
        // `tellurion-features`' own `/collections` listing, which omits a
        // collection entirely when neither capability is set) would treat
        // it as capability-less and never advertise it at all. The vector
        // half also travels on its own (`tiles_vector`, `#287`) so a
        // consumer advertising the MVT lane specifically never gates on
        // this deliberately coarse merge — same probe order (and the same
        // cost for every collection that has a `TileSource`) as
        // `tellurion_tiles::handlers::tile` and `TilesLinkContributor`.
        let tiles_vector = self
            .resolve_tiles(tenant, catalog, collection)
            .await
            .is_ok();
        let has_tiles = tiles_vector
            || self
                .resolve_raster(tenant, catalog, collection)
                .await
                .is_ok();
        let stac = self
            .effective_settings(&routed.decl.id)
            .and_then(|settings| settings.stac.as_ref());
        let write_result = self.resolve_write(tenant, catalog, collection).await;
        let has_write = write_result.is_ok();
        // `#107`: this collection's own Optimistic Locking classes, gated
        // on BOTH the features and write lanes resolving — the guard reads
        // current state through `FeatureSource::item` before ever comparing
        // against an `If-Match`/`If-Unmodified-Since` precondition, so
        // neither class means anything for a collection missing either
        // lane. ETags rides the resolved `WriteSink`'s own declared set
        // (`WriteSink::locking_conformance_classes`); Timestamps is a
        // per-collection declaration (`CollectionDecl::modified_column`),
        // never driver-declared — see `locking`'s own module doc for why
        // the two classes are gathered differently here. `#287`: `None`
        // when the features lane doesn't resolve (no `FeatureSource`, so
        // this collection says nothing about locking and its metadata
        // carries no member); `Some(vec![])` when it resolves but the write
        // lane doesn't — the empty list every features-capable read-only
        // collection has always shown.
        let locking_conformance_classes: Option<Vec<&'static str>> = has_features.then(|| {
            if has_write {
                let mut classes: Vec<&'static str> = write_result
                    .as_ref()
                    .map(|(_, sink)| sink.locking_conformance_classes())
                    .unwrap_or_default();
                if routed.decl.modified_column.is_some() {
                    classes.push(crate::locking::OPTIMISTIC_LOCKING_TIMESTAMPS_CLASS);
                }
                classes
            } else {
                Vec::new()
            }
        });
        // `#101`: same never-fail-the-request-over-metadata handling
        // `physical` above already gets — a profile-computation failure is
        // logged and the canonical descriptor simply omits it, rather than
        // failing collection-describe metadata over a signal nothing else
        // in this workspace requires.
        let geometry_profile = match self.geometry_profile_cached(routed).await {
            Ok(profile) => profile,
            Err(error) => {
                tracing::warn!(
                    %error,
                    tenant,
                    catalog,
                    collection = %routed.decl.external_id(),
                    "failed to derive geometry profile; canonical descriptor omits it"
                );
                None
            }
        };

        Ok(descriptor::canonical::build(
            physical.as_ref(),
            &routed.decl,
            routed.decl.schema.as_ref(),
            stac,
            descriptor::canonical::CanonicalCapabilities {
                features: has_features,
                tiles: has_tiles,
                tiles_vector,
                places3d: routed.decl.places3d.is_some(),
                crs_capable,
                cql2_conformance_classes,
                write: has_write,
                locking_conformance_classes,
            },
            geometry_profile,
        ))
    }

    /// The CQL2 (1.0) conformance classes every currently-configured,
    /// features-capable driver in this deployment satisfies (`#105`) — the
    /// intersection the Features and STAC roots' `/conformance` responses
    /// expose (`tellurion-server::landing::conformance_classes`), computed
    /// from `self.drivers` (built once from `config.storages` at
    /// `Router::build` time) rather than one static, workspace-wide list.
    ///
    /// **"In use" means declared in `config.storages`**, not "referenced by
    /// at least one collection" — the same convention `landing::
    /// conformance_classes` already applies to `config.object_stores` for
    /// the STAC asset classes (presence of the declaration is what's
    /// checked there too, not whether any collection actually references
    /// it). A storage nothing routes to is already a configuration this
    /// workspace doesn't otherwise treat specially, so this method doesn't
    /// either.
    ///
    /// A driver with no `FeatureSource` at all (a tile/raster-only archive —
    /// PMTiles, COG, Zarr) never participates in CQL2 filtering to begin
    /// with, so it neither contributes classes nor narrows the
    /// intersection; it is simply skipped.
    ///
    /// The fold starts from [`filter::CQL2_CONFORMANCE_CLASSES`] (the full
    /// set this workspace's shared parser/compiler could ever satisfy —
    /// `case-insensitive-comparison` excepted, see that constant's own doc)
    /// and narrows by intersecting each in-use driver's own declared set. A
    /// deployment with zero features-capable drivers configured (an
    /// all-tiles/raster deployment) claims none: every class in this seed
    /// describes CQL2 evaluation that only a `FeatureSource` can honour.
    ///
    /// **Why the intersection, not the union**: a client reading this off
    /// the Features/STAC root's `/conformance` has not yet picked a
    /// collection — it cannot know whether the request it's about to send
    /// will land on the strongest or the weakest driver behind this
    /// deployment, so the honest claim is the class every driver it could
    /// possibly reach actually earns. A per-collection surface
    /// (`descriptor::canonical::CanonicalCapabilities::
    /// cql2_conformance_classes`, read off `canonical_descriptor` above)
    /// still names the true, wider set once a collection is known — a
    /// PostGIS-backed collection in an otherwise GeoPackage/Iceberg-mixed
    /// deployment still advertises its own full set there even when this
    /// method's workspace-wide answer is narrower.
    pub fn cql2_conformance_classes(&self) -> Vec<&'static str> {
        fold_conformance_classes(
            crate::filter::CQL2_CONFORMANCE_CLASSES,
            self.drivers.values(),
            WhenNoneParticipate::ClaimsNothing,
            |driver| Some(driver.feature_source()?.cql2_conformance_classes()),
        )
    }

    /// OGC API Features — Part 4's Create/Replace/Delete requirements class
    /// (OGC 20-002r1, Table 2), earned per deployment from the write lanes
    /// that actually resolve (`#263`) — the last family the fold had never
    /// been asked about, and until this method existed the one Part 4 class
    /// `tellurion_features::CONFORMANCE_CLASSES` still named statically,
    /// which is how a read-only deployment came to promise a requirements
    /// class whose every method it declines on the same URIs.
    ///
    /// **Which entries participate is the class's own quantifier.**
    /// Requirement 1 clause A reads: "A server SHALL implement one or more
    /// of the methods HTTP POST, PUT and/or DELETE for each mutable
    /// resource." (Its identifier in the published text is
    /// `/req/core/methods` — that document's own inconsistency, since every
    /// other requirement in clause 6 is `/req/create-replace-delete/…`; the
    /// prose is what is cited here.) The obligation is scoped to *mutable*
    /// resources, so a collection this deployment never offered as mutable
    /// says nothing about the class either way and answers `None` — the
    /// [`fold_conformance_classes`] "does not participate" case, spelled out
    /// there with this exact family as its example. `routing.write` is the
    /// only way a collection is offered as mutable here: there is no
    /// "defaults to the single storage" fallback for the write lane (see
    /// `RoutedCollection::write`'s own doc).
    ///
    /// **Whole-deployment or per-collection.** Both, and the fold's own
    /// shape is what reconciles them. A catalog with one writable collection
    /// and nine read-only ones declares the class: the nine are not mutable
    /// resources, so they do not narrow it, and the one that is honours it.
    /// A catalog with two collections offered as mutable, one of whose write
    /// lanes cannot actually write, declares nothing: that one participates
    /// and honours nothing, which zeroes the fold. That is neither "any" nor
    /// "every" — it is "every resource this deployment *offers as mutable*",
    /// which is precisely the set Requirement 1 clause A quantifies over.
    /// So the reading is not inherited from the sibling folds by accident;
    /// it is the one the requirement's own wording asks for, and it happens
    /// to be the reading [`fold_conformance_classes`] already implements.
    ///
    /// **Nothing mutable at all claims nothing**
    /// ([`WhenNoneParticipate::ClaimsNothing`]), rather than reading clause
    /// A as vacuously true over an empty set of mutable resources. The
    /// class's own overview is what settles it: "A server that implements
    /// this requirements class provides the ability to add, replace and/or
    /// remove individual resources from a collection." A deployment with no
    /// write lane anywhere provides no such ability, and every conditional
    /// requirement in clause 6 (Requirements 2, 7 and 13, each conditioned
    /// on "Server declares support for the … method … via the `Allow`
    /// header in the response to an OPTIONS request") is dead there too,
    /// because `#208` already narrowed that `Allow` to the methods the write
    /// lane can back.
    ///
    /// The honoured half asks [`write_lane_resolves`] and nothing else —
    /// the same predicate [`resolve_write`](Self::resolve_write) enforces on
    /// the request path and [`Router::write_lane_resolves`] reports to the
    /// `Allow` header (`#208`). One predicate for what a write does, what
    /// `Allow` says a write will do, and what `/conformance` claims a write
    /// can do, so no two of the three can drift apart.
    pub fn create_replace_delete_conformance_classes(&self) -> Vec<&'static str> {
        const SEED: &[&str] = &[crate::outbox::CREATE_REPLACE_DELETE_CONFORMANCE_CLASS];
        fold_conformance_classes(
            SEED,
            self.collections.values(),
            WhenNoneParticipate::ClaimsNothing,
            |routed| {
                // Offered as mutable, so this collection participates; a
                // collection with no write lane is not a mutable resource
                // and is skipped entirely.
                routed.write.as_ref()?;
                Some(honoured_if(write_lane_resolves(routed), SEED))
            },
        )
    }

    /// OGC API Features — Part 4's feature-body class when every writable
    /// collection in this deployment earns it. Unlike driver-wide CQL2
    /// capabilities, default-CRS write correctness can depend on the
    /// collection's storage SRID, so each routed write sink receives the
    /// collection declaration and the root advertises the conservative
    /// intersection. A deployment with no writable collection does not
    /// advertise a write-specific class.
    ///
    /// `#263`: this class's Dependency row names Requirements Class
    /// "Create/Replace/Delete", and clause 5.4 defines a direct dependency
    /// as one where "Every server implementing the requirements class has to
    /// conform to the referenced Standard or requirements class". So it may
    /// never survive where
    /// [`create_replace_delete_conformance_classes`](Self::create_replace_delete_conformance_classes)
    /// withholds that class, and the honoured half below therefore gates on
    /// the same [`write_lane_resolves`] predicate that fold uses rather than
    /// on this one lane entry's sink. Identical for every write lane
    /// `AppConfig::validate` accepts (write lanes are single-entry), and the
    /// stricter of the two only where a multi-entry lane reached a `Router`
    /// unvalidated — which `resolve_write` refuses anyway.
    pub fn features_write_conformance_classes(&self) -> Vec<&'static str> {
        fold_conformance_classes(
            &[crate::outbox::FEATURES_PART4_FEATURES_CLASS],
            self.collections.values(),
            WhenNoneParticipate::ClaimsNothing,
            |routed| {
                let lane = routed.write.as_ref()?;
                let (_, driver) = lane
                    .entries
                    .first()
                    .expect("build_lane never produces an empty RoutedLane");
                // A writable collection whose write lane cannot actually
                // write participates (it is writable per config) and honours
                // nothing, so it zeroes the fold rather than being skipped.
                Some(match driver.write_sink() {
                    Some(sink) if write_lane_resolves(routed) => {
                        sink.features_conformance_classes(&routed.decl)
                    }
                    _ => Vec::new(),
                })
            },
        )
    }

    /// The OGC API Features — Part 4 (20-002r1 draft) Optimistic Locking,
    /// ETags class every currently-configured, write-capable driver in this
    /// deployment satisfies (`#107`) — the workspace-wide intersection
    /// folded into a deployment's `/conformance` response
    /// (`tellurion-server::landing::conformance_classes`), the direct
    /// counterpart of [`cql2_conformance_classes`](Self::cql2_conformance_classes)
    /// for this different requirement-class family (see `locking`'s own
    /// module doc for why Part 4 locking needs a parallel mechanism rather
    /// than folding into the CQL2 one). Same "in use" convention as that
    /// method: a driver declared in `config.storages`, regardless of
    /// whether any collection currently routes to it.
    ///
    /// A write-capable driver with NO read lane at all (`feature_source()`
    /// is `None`) can never satisfy the guard — it reads current state
    /// through `FeatureSource::item` before ever comparing an `If-Match`,
    /// so a deployment where such a driver is configured for writes can
    /// never honestly declare this class workspace-wide, regardless of what
    /// any other driver declares; narrows the fold to empty rather than
    /// simply skipping that driver the way one with no write lane at all is
    /// skipped below (a driver that never writes at all has nothing to say
    /// about this class either way).
    ///
    /// Never includes the Timestamps class: unlike ETags, no driver ever
    /// declares or withholds it — it is a per-collection fact
    /// (`CollectionDecl::modified_column.is_some()`), so it only ever
    /// appears on a specific collection's own
    /// `CanonicalCapabilities::locking_conformance_classes`
    /// (`Router::canonical_descriptor`), never in this workspace-wide fold.
    ///
    /// `#150` changed nothing about this fold's shape — only what a driver
    /// must be able to do before it may participate positively. A driver
    /// that cannot re-verify the precondition inside its own write
    /// transaction now declares nothing (GeoPackage), and this intersection
    /// narrows accordingly. That is exactly the mechanism this fold exists
    /// to provide: honesty about a mixed deployment is arrived at by
    /// folding, never by any caller special-casing a driver.
    ///
    /// With no write-capable driver, the fold claims nothing: the ETags class
    /// describes precondition handling in a write transaction, so a
    /// raster-only or otherwise read-only deployment has nowhere to honour
    /// it.
    pub fn locking_conformance_classes(&self) -> Vec<&'static str> {
        fold_conformance_classes(
            crate::locking::LOCKING_CONFORMANCE_CLASSES,
            self.drivers.values(),
            WhenNoneParticipate::ClaimsNothing,
            |driver| {
                let sink = driver.write_sink()?;
                if driver.feature_source().is_none() {
                    // Write-capable but unreadable: the guard reads current
                    // state through `FeatureSource::item` before it can ever
                    // compare an `If-Match`, so this driver participates and
                    // honours nothing — see this method's own doc.
                    return Some(Vec::new());
                }
                Some(sink.locking_conformance_classes())
            },
        )
    }

    /// Part 4 Update classes every actually writable collection can honestly
    /// serve. A collection participates only when its features lane is one
    /// storage (no stale fallback tail) and that exact storage is also its
    /// write lane, because PATCH reads the target and the committed
    /// representation back. Configured-but-unused drivers do not earn a
    /// deployment claim; no writable collection means no Update claim.
    pub fn update_conformance_classes(&self) -> Vec<&'static str> {
        fold_conformance_classes(
            &[crate::outbox::UPDATE_CONFORMANCE_CLASS],
            self.collections.values(),
            WhenNoneParticipate::ClaimsNothing,
            |routed| {
                let write_lane = routed.write.as_ref()?;
                // Every remaining `Some(Vec::new())` below is a writable
                // collection whose read/write pair cannot round-trip a PATCH:
                // it participates (it is writable) and honours nothing.
                let ([(feature_storage, feature_driver)], [(write_storage, write_driver)]) = (
                    routed.features.entries.as_slice(),
                    write_lane.entries.as_slice(),
                ) else {
                    return Some(Vec::new());
                };
                if feature_storage != write_storage || feature_driver.feature_source().is_none() {
                    return Some(Vec::new());
                }
                let Some(sink) = write_driver.write_sink() else {
                    return Some(Vec::new());
                };
                Some(sink.update_conformance_classes())
            },
        )
    }

    /// OGC API — Features Part 2: CRS by Reference (18-058r1)'s single
    /// conformance class, folded per deployment (`#217`) exactly the way
    /// [`cql2_conformance_classes`](Self::cql2_conformance_classes) folds
    /// CQL2's — same "in use means declared in `config.storages`" convention,
    /// same intersection-not-union reasoning, same skipping of a driver with
    /// no `FeatureSource` at all.
    ///
    /// The class promises a client may name a CRS other than the one a
    /// collection is served in on `crs`/`bbox-crs` and get coordinates back
    /// in it. Only a driver whose [`FeatureSource::crs_capable`] is `true`
    /// can honour that; every other driver in this workspace advertises
    /// exactly one CRS per collection (`crate::crs::advertised_crs`) and
    /// refuses every other value with a 400 (the enforcement gate in
    /// `tellurion-features`' items handler, `crate::crs::can_serve`).
    /// Claiming Part 2 there would advertise a capability no request could
    /// exercise.
    ///
    /// `#227` changed *which* single CRS that is — a projected collection
    /// under such a driver advertises its own storage CRS instead of CRS84,
    /// because that is what its rows genuinely come out in — but not the
    /// count, so this fold's conclusion is unchanged: one CRS on offer is no
    /// CRS negotiation, whichever one it happens to be.
    ///
    /// Unlike CQL2's fold this claims nothing when no features-capable driver
    /// is configured at all ([`WhenNoneParticipate::ClaimsNothing`]): CQL2's
    /// classes grade a filtering capability the deployment either has or does
    /// not, while Part 2 is that capability, so "no driver to contradict it"
    /// is not a reason to claim it.
    pub fn crs_conformance_classes(&self) -> Vec<&'static str> {
        fold_conformance_classes(
            crate::crs::CRS_CONFORMANCE_CLASSES,
            self.drivers.values(),
            WhenNoneParticipate::ClaimsNothing,
            |driver| {
                let source = driver.feature_source()?;
                Some(honoured_if(
                    source.crs_capable(),
                    crate::crs::CRS_CONFORMANCE_CLASSES,
                ))
            },
        )
    }

    /// OGC API — Features Part 3: Filtering (19-079r2)'s query-parameter
    /// classes ([`crate::filter::FILTERING_CONFORMANCE_CLASSES`]), folded per
    /// deployment (`#217`) — the twin of
    /// [`crs_conformance_classes`](Self::crs_conformance_classes), gated on
    /// [`FeatureSource::filter_capable`] instead.
    ///
    /// Part 3 says a `filter` may be sent at all; the CQL2 classes
    /// [`cql2_conformance_classes`](Self::cql2_conformance_classes) folds say
    /// how much of the language behind it a driver understands. The two are
    /// declared separately because a driver can be honest about one and not
    /// the other, and three drivers in this workspace (FlatGeobuf,
    /// GeoParquet, memory) answer 400 to any `filter` at all — a deployment
    /// built on them can claim neither. `conf/queryables` is not folded here:
    /// the queryables document itself is served for every collection
    /// regardless of driver, so it stays in `tellurion-features`' static list
    /// (see [`crate::filter::FILTERING_CONFORMANCE_CLASSES`]'s own doc).
    ///
    /// ## The `filter-crs` condition (`#217`, the residual this issue kept)
    ///
    /// `filter_capable` alone is not enough to earn these classes. Part 3
    /// Requirement 8 (`/req/filter/filter-crs-param`) is a *conditional*
    /// requirement — its condition is "Server supports additional coordinate
    /// reference systems" — and it says that when a `filter-crs` is supplied
    /// "the server SHALL process all geometries in the filter expression
    /// using the CRS identified by the URI in `filter-crs`". A driver that
    /// answers [`FeatureSource::crs_capable`] `true` is exactly a server
    /// supporting additional CRSs for the resources it backs, so the
    /// condition fires and Requirement 8 becomes binding on it; one that
    /// answers `false` offers only the Part 1 default CRS, the condition
    /// never fires, and Requirement 7 (`/req/filter/filter-crs-wgs84`,
    /// "process all geometries in the filter expression using CRS84") is the
    /// whole of its obligation — which is what its compiler already does.
    ///
    /// So a driver honours these classes when it accepts a `filter` at all
    /// AND, only where the Requirement 8 condition fires, can genuinely
    /// honour the parameter ([`FeatureSource::filter_crs_capable`]). PostGIS
    /// is the one driver in this workspace on the interesting side of that:
    /// it is `crs_capable`, so before `#217` it made the condition fire
    /// while treating `filter-crs` as inert — the deployment advertised
    /// Part 3 without honouring Requirement 8. It now transforms a filter's
    /// spatial literals for real; a driver that could not would fold these
    /// classes away here instead, and `tellurion-features`' handler refuses
    /// the parameter by name on its behalf either way.
    ///
    /// ## Why Requirement 7 is NOT a second condition on this fold (`#247`)
    ///
    /// Requirement 7 (`/req/filter/filter-crs-wgs84`) is unconditional, and
    /// `#247` made it mean real work: with no `filter-crs` on the wire a
    /// filter's geometries are processed in CRS84, which against a projected
    /// storage is the same transform Requirement 8's CRS84 value asks for. A
    /// driver that cannot transform therefore cannot satisfy Requirement 7
    /// **for a collection whose storage is projected** — and only for such a
    /// collection.
    ///
    /// That "and only for" is why the condition cannot live here. This fold is
    /// deployment-wide and per-*driver*; whether Requirement 7 costs a driver
    /// anything is per-*collection*, and a collection's storage SRID is not
    /// knowable from a `StorageDriver` at all — for most collections it is
    /// derived from the backend at request time
    /// ([`effective_decl`](Self::effective_decl)), asynchronously, which a
    /// synchronous `/conformance` answer cannot await. Gating on
    /// `filter_crs_capable` alone would instead strip Part 3 from every
    /// GeoPackage deployment on earth, including the overwhelming majority
    /// whose collections are all CRS84 and for whom Requirement 7 has always
    /// been satisfied for free.
    ///
    /// So the truth about Requirement 7 is told per collection, by a named
    /// `400` from `tellurion-features`' items handler (and `tellurion-stac`'s
    /// `unservable_filter_reason` on the `/search` lane), keyed on
    /// [`crate::crs::crs84_literals_need_transform`] — never by serving the
    /// filter in the wrong CRS, and never by the `500` `#247` removed. That is
    /// the same division of labour
    /// [`item_search_filter_conformance_classes`](Self::item_search_filter_conformance_classes)
    /// already documents for `#248`: a per-collection storage-SRID gate is the
    /// protocol crate's, not this fold's.
    pub fn filtering_conformance_classes(&self) -> Vec<&'static str> {
        fold_conformance_classes(
            crate::filter::FILTERING_CONFORMANCE_CLASSES,
            self.drivers.values(),
            WhenNoneParticipate::ClaimsNothing,
            |driver| {
                let source = driver.feature_source()?;
                let honours_requirement_8 = !source.crs_capable() || source.filter_crs_capable();
                Some(honoured_if(
                    source.filter_capable() && honours_requirement_8,
                    crate::filter::FILTERING_CONFORMANCE_CLASSES,
                ))
            },
        )
    }

    /// STAC API — Item Search: Filter Extension's own class
    /// ([`crate::filter::ITEM_SEARCH_FILTER_CONFORMANCE_CLASSES`]), folded per
    /// deployment (`#248`) — the STAC `/search` twin of
    /// [`filtering_conformance_classes`](Self::filtering_conformance_classes),
    /// gated on [`FeatureSource::filter_capable`] alone.
    ///
    /// The class binds filtering to `/search`, and `/search` reaches a
    /// collection through the very same `FeatureSource::items` call `/items`
    /// does, so a driver that answers 400 to any `filter` (FlatGeobuf,
    /// GeoParquet, memory) cannot honour it there either. Before `#248` this
    /// class was declared unconditionally by `tellurion-stac`'s static list,
    /// which produced a self-contradicting document on such a deployment: the
    /// extension defines Item Search Filter as binding *Basic CQL2* to
    /// `/search`, and [`cql2_conformance_classes`](Self::cql2_conformance_classes)
    /// already (correctly) folds Basic CQL2 away when no driver declares it —
    /// so `/conformance` claimed the binding while withholding the thing bound.
    ///
    /// ## Why this does NOT carry Part 3's `filter-crs` condition
    ///
    /// [`filtering_conformance_classes`](Self::filtering_conformance_classes)
    /// additionally requires [`FeatureSource::filter_crs_capable`] of any
    /// `crs_capable` driver, because Part 3 Requirement 8
    /// (`/req/filter/filter-crs-param`) says a supplied `filter-crs` SHALL be
    /// the CRS the filter's geometries are processed in, and its condition
    /// ("Server supports additional coordinate reference systems") fires for
    /// exactly those drivers. The STAC Filter Extension pins the same parameter
    /// far more tightly: "filter-crs: recommended to not be passed, but server
    /// must only accept `http://www.opengis.net/def/crs/OGC/1.3/CRS84` as a
    /// valid value, may reject any others", and "The parameter `filter-crs`
    /// always defaults to `http://www.opengis.net/def/crs/OGC/1.3/CRS84` for a
    /// STAC API" (`stac-api-extensions/filter` `README.md`, `v1.0.0-rc.4`).
    ///
    /// So on the `/search` lane there is no client-nameable CRS a driver could
    /// fail to transform into: `tellurion-stac`'s own parser refuses every
    /// value but CRS84 by name, and CRS84 is honoured per collection — for
    /// free where the storage is already CRS84
    /// ([`crate::crs::crs84_literals_need_transform`]), and behind
    /// `filter_crs_capable` where it is not, with a named 400 otherwise. That
    /// per-collection gate is the protocol crate's, not this fold's: unlike
    /// Part 3's condition it depends on a *collection's* storage SRID, which a
    /// deployment-wide `/conformance` answer cannot see.
    pub fn item_search_filter_conformance_classes(&self) -> Vec<&'static str> {
        fold_conformance_classes(
            crate::filter::ITEM_SEARCH_FILTER_CONFORMANCE_CLASSES,
            self.drivers.values(),
            WhenNoneParticipate::ClaimsNothing,
            |driver| {
                let source = driver.feature_source()?;
                Some(honoured_if(
                    source.filter_capable(),
                    crate::filter::ITEM_SEARCH_FILTER_CONFORMANCE_CLASSES,
                ))
            },
        )
    }

    /// Resolves the collection's tenant/catalog-scoped features lane to a
    /// single `FeatureSource` (`#21`) plus its effective decl. `tenant`/
    /// `catalog` stay explicit, required parameters here (as on every
    /// resolve entry point) so a future credentials→tenant-claims check has
    /// one seam to sit in front of, not several. All three ids are internal
    /// (`#39`) — resolving a request's external ids to these is the caller's
    /// job (see `Resolver`), not this method's.
    ///
    /// Re-runs `validate_lane_capability` for an explicit lane before
    /// building the source (`#59`): `validate_catalog`'s eager sweep already
    /// caught this at boot when it ran, but under `registry.validation:
    /// lazy` (which skips that sweep) this is the collection's first touch —
    /// see that function's own doc for why re-checking here is cheap and why
    /// it closes a real gap (a misconfigured multi-entry lane silently
    /// dropping the bad entry instead of ever refusing).
    pub async fn resolve_features(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<(CollectionDecl, Arc<dyn FeatureSource>)> {
        let (decl, source, _served) = self
            .resolve_features_read(tenant, catalog, collection, &Hints::none())
            .await?;
        Ok((decl, source))
    }

    /// [`resolve_features`](Self::resolve_features) with the two `#183`
    /// read-lane additions — everything else (tenant/catalog rationale,
    /// the `#59` lazy-mode capability re-check) is identical, and the
    /// unhinted method now delegates here with [`Hints::none`], under which
    /// this resolves byte-for-byte like it always did:
    ///
    /// - `hints.prefer()` reorders the resolved chain (`preferred_entries`)
    ///   AFTER the capability validation ran against the configured order —
    ///   a reorder, never an extension, so the non-preferred entries stay
    ///   behind the preferred one as the ordinary fallback tail and a hint
    ///   can neither widen the entry set the ABAC checkpoint scoped nor
    ///   dodge a misconfiguration refusal. Read lanes only, by
    ///   construction: [`resolve_write`](Self::resolve_write) takes no
    ///   hints at all.
    /// - The returned [`ServedSource`] names the chain entry that actually
    ///   serves the read, so the handler can emit the `X-Tellurion-Source`
    ///   observability header (`crate::hint::READ_SOURCE_HEADER`) — see
    ///   `ServedSource`'s own doc for when it is meaningful.
    pub async fn resolve_features_read(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
        hints: &Hints,
    ) -> Result<(CollectionDecl, Arc<dyn FeatureSource>, ServedSource)> {
        let _phase = enter_phase(Phase::Routing);
        let routed = self.lookup(tenant, catalog, collection)?;
        if routed.features_explicit {
            validate_lane_capability(&routed.decl.id, "features", &routed.features, |driver| {
                driver.feature_source().is_some()
            })?;
        }
        let entries = preferred_entries(&routed.features, hints.prefer());
        let (source, served) = features_source(&routed.decl.id, &entries)?;
        let decl = self.effective_decl(routed).await?;
        Ok((decl, source, served))
    }

    /// Resolves the collection's tenant/catalog-scoped tiles lane to a
    /// single `TileSource` (`#21`) plus its effective decl — see
    /// [`resolve_features`](Self::resolve_features) for the tenant/catalog
    /// rationale and the lazy-mode capability re-check this makes for an
    /// explicit lane. `places3d` rides this same lane: it has no resolve
    /// entry point of its own, so its own capability check
    /// (`validate_places3d_capability`) also lands here, first-touch, for
    /// exactly the same `#59` reason. The effective decl comes from
    /// [`effective_tile_decl`](Self::effective_tile_decl), not plain
    /// `effective_decl` — see that method's own doc for why this lane (and
    /// [`resolve_maps`](Self::resolve_maps)) also attaches a geometry
    /// profile (`#101`/`#102`).
    pub async fn resolve_tiles(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<(CollectionDecl, Arc<dyn TileSource>)> {
        let _phase = enter_phase(Phase::Routing);
        let routed = self.lookup(tenant, catalog, collection)?;
        if routed.tiles_explicit {
            validate_lane_capability(&routed.decl.id, "tiles", &routed.tiles, |driver| {
                driver.tile_source().is_some()
            })?;
        }
        validate_places3d_capability(&routed.decl, &routed.tiles)?;
        let source = tiles_source(&routed.decl.id, &routed.tiles)?;
        let decl = self.effective_tile_decl(routed).await?;
        if !source.tile_capable(&decl) {
            return Err(Error::CapabilityUnsupported {
                collection: decl.id.clone(),
                capability: "tiles".to_string(),
            });
        }
        Ok((decl, source))
    }

    /// Resolves the collection's tenant/catalog-scoped maps lane (`#86`,
    /// OGC API Maps Part 1) to a single `TileSource` plus its effective decl
    /// — same shape and lazy-mode re-check as
    /// [`resolve_tiles`](Self::resolve_tiles), over `routed.maps` instead of
    /// `routed.tiles`. The VECTOR half of the maps lane: it rasterizes from
    /// the same MVT capability the tiles lane uses. The raster half is
    /// [`resolve_maps_raster`](Self::resolve_maps_raster) (`#37`), a
    /// separate entry point rather than a fallback tail inside this one, so
    /// a caller (and a link contributor) can ask which of the two a
    /// collection actually resolves to and get an answer per capability
    /// rather than a merged verdict. Also uses [`effective_tile_decl`](Self::
    /// effective_tile_decl) rather than plain `effective_decl`, for the
    /// same `#101`/`#102` geometry-profile reason `resolve_tiles` does — the
    /// PNG this handler rasterizes is simplified through the exact same
    /// `TileSource::mvt_tile` call, so it needs the same profile.
    pub async fn resolve_maps(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<(CollectionDecl, Arc<dyn TileSource>)> {
        let _phase = enter_phase(Phase::Routing);
        let routed = self.lookup(tenant, catalog, collection)?;
        if routed.maps_explicit {
            validate_lane_capability(&routed.decl.id, "maps", &routed.maps, |driver| {
                driver.tile_source().is_some()
            })?;
        }
        let source = maps_source(&routed.decl.id, &routed.maps)?;
        let decl = self.effective_tile_decl(routed).await?;
        if !source.tile_capable(&decl) {
            return Err(Error::CapabilityUnsupported {
                collection: decl.id.clone(),
                capability: "maps".to_string(),
            });
        }
        Ok((decl, source))
    }

    /// Resolves the collection's tenant/catalog-scoped MAPS lane to a single
    /// `RasterSource` plus its effective decl (`#37`) — the raster
    /// counterpart of [`resolve_maps`](Self::resolve_maps), riding the SAME
    /// `routing.maps` lane rather than a lane of its own, exactly the way
    /// [`resolve_raster`](Self::resolve_raster) rides `routing.tiles` for a
    /// raster collection's PNG tiles.
    ///
    /// Independent of the vector lane in both directions: it never consults
    /// `TileSource`, and a driver that advertises neither capability refuses
    /// here by name (`CapabilityUnsupported { capability: "maps" }`) rather
    /// than degrading to some whole-source read. `tellurion-tiles`' own
    /// `maps::map` calls this only after [`resolve_maps`](Self::resolve_maps)
    /// has already refused — the same resolution order (and the same cost
    /// for a collection that does have a `TileSource`)
    /// `tellurion_tiles::handlers::tile` pays between `resolve_tiles` and
    /// `resolve_raster`.
    ///
    /// Uses plain [`effective_decl`](Self::effective_decl), NOT
    /// `effective_tile_decl`: a geometry profile (`#101`/`#102`) describes
    /// how vector geometry is simplified on its way into an MVT, and this
    /// lane never produces one — the same choice `resolve_raster` already
    /// makes for the same reason. The effective decl still carries the
    /// collection's inherited `settings.colormap`, which is what the driver
    /// reads to classify samples.
    pub async fn resolve_maps_raster(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<(CollectionDecl, Arc<dyn RasterSource>)> {
        let _phase = enter_phase(Phase::Routing);
        let routed = self.lookup(tenant, catalog, collection)?;
        if routed.maps_explicit {
            validate_lane_capability(&routed.decl.id, "maps", &routed.maps, |driver| {
                driver.raster_source().is_some()
            })?;
        }
        let source = maps_raster_source(&routed.decl.id, &routed.maps)?;
        let decl = self.effective_decl(routed).await?;
        Ok((decl, source))
    }

    /// Resolves the collection's tenant/catalog-scoped tiles lane to a
    /// single `RasterSource` (`#37`) plus its effective decl — the raster
    /// counterpart of [`resolve_tiles`](Self::resolve_tiles), riding the
    /// same `tiles` lane rather than a lane of its own: a raster collection
    /// still declares `routing.tiles` (or leaves it to the single-`storage`
    /// default) exactly like a vector one, and `tellurion-tiles`' PNG
    /// handler tries this only after `resolve_tiles` itself has already
    /// refused the collection (no driver in its tiles lane implements
    /// `TileSource`) — see that handler's own doc for the resolution order.
    pub async fn resolve_raster(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<(CollectionDecl, Arc<dyn RasterSource>)> {
        let routed = self.lookup(tenant, catalog, collection)?;
        if routed.tiles_explicit {
            validate_lane_capability(&routed.decl.id, "tiles", &routed.tiles, |driver| {
                driver.raster_source().is_some()
            })?;
        }
        let source = raster_source_for_lane(&routed.decl.id, &routed.tiles)?;
        let decl = self.effective_decl(routed).await?;
        Ok((decl, source))
    }

    /// Resolves the collection's tenant/catalog-scoped write lane to a
    /// single `WriteSink` (`#25`) plus its effective decl — the write
    /// counterpart of [`resolve_features`](Self::resolve_features)/
    /// [`resolve_tiles`](Self::resolve_tiles), with one deliberate
    /// difference: there is no "defaults to the single storage" fallback
    /// (see `RoutedCollection::write`'s own doc) and no fallback tail even
    /// when `routing.write` names more than one storage — `Router::build`
    /// refuses that at build time (`build_lane`'s caller here only ever
    /// reads `entries[0]`, and `AppConfig::validate` catches a multi-entry
    /// write lane before a `Router` is even built). A collection with no
    /// `routing.write` at all, or whose named storage doesn't advertise
    /// `write_sink`, refuses with the same `CapabilityUnsupported` a
    /// features/tiles lane without the capability gives.
    ///
    /// The capability half of this — everything up to and including the
    /// [`validate_lane_capability`] call — is exactly
    /// [`write_lane_resolves`], which [`Router::write_lane_resolves`]
    /// exposes so an `Allow` header can advertise live write capability
    /// rather than URI shape (`#208`). Anything added to the capability
    /// checks below belongs in that predicate, not here, or the
    /// advertisement and the request start disagreeing.
    pub async fn resolve_write(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<(CollectionDecl, Arc<dyn WriteSink>)> {
        let routed = self.lookup(tenant, catalog, collection)?;
        let Some(lane) = &routed.write else {
            return Err(Error::CapabilityUnsupported {
                collection: routed.decl.id.clone(),
                capability: "write".to_string(),
            });
        };
        validate_lane_capability(&routed.decl.id, "write", lane, |driver| {
            driver.write_sink().is_some()
        })?;
        let (_, driver) = lane
            .entries
            .first()
            .expect("build_lane never produces an empty RoutedLane");
        let sink = driver
            .write_sink()
            .ok_or_else(|| Error::CapabilityUnsupported {
                collection: routed.decl.id.clone(),
                capability: "write".to_string(),
            })?;
        let decl = self.effective_decl(routed).await?;
        Ok((decl, sink))
    }

    /// Resolves the collection's tenant/catalog-scoped database-backed
    /// asset-record capability (assets-and-object-storage proposal, first
    /// slice) — the same "anchor driver" this collection's
    /// `CollectionDescriptor` introspection uses (`RoutedCollection::
    /// anchor`), since assets are metadata about the collection's own
    /// canonical storage, not a separate routable lane the way `write`/
    /// `index` are. Refuses with `CapabilityUnsupported("assets")` when the
    /// anchor driver doesn't advertise `asset_record_store` at all —
    /// whether a given collection's `"<table>_assets"` table has actually
    /// been provisioned is a request-time question the driver itself
    /// answers, by name (mirroring `resolve_write`'s own
    /// `OutboxTableMissing` precedent).
    pub async fn resolve_assets(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<(CollectionDecl, Arc<dyn AssetRecordStore>)> {
        let routed = self.lookup(tenant, catalog, collection)?;
        let store =
            routed
                .anchor()
                .asset_record_store()
                .ok_or_else(|| Error::CapabilityUnsupported {
                    collection: routed.decl.id.clone(),
                    capability: "assets".to_string(),
                })?;
        let decl = self.effective_decl(routed).await?;
        Ok((decl, store))
    }

    /// Resolves this collection's asset-record store *for the STAC Item
    /// projection* (`#221`), when the collection opted into it — the same
    /// `AssetRecordStore` capability [`resolve_assets`](Self::resolve_assets)
    /// hands the assets API, reached through the same anchor driver, with
    /// exactly one difference: the projection is opt-in per collection, so
    /// this answers `Ok(None)` instead of refusing when it is off.
    ///
    /// `Ok(None)` is the ORDINARY answer, the same shape
    /// [`resolve_stac_metadata`](Self::resolve_stac_metadata) uses: a
    /// collection that never declared `stac_item_assets: true` (every
    /// collection that predates `#221`) resolves to `None` without probing
    /// a driver, and its STAC Items keep carrying exactly the
    /// capability-derived asset map they carry today. That is what makes
    /// the whole slice invisible to an unconfigured deployment.
    ///
    /// A collection that DID opt in against an anchor driver advertising no
    /// `asset_record_store` is the same named `CapabilityUnsupported
    /// ("assets")` refusal `resolve_assets` already gives — the operator
    /// asked for per-item assets on storage that has no asset records at
    /// all, and serving Items without them would look exactly like the
    /// un-opted-in case. Whether the `"<table>_assets"` table was ever
    /// provisioned stays a request-time question the driver answers by name
    /// (PostGIS: `AssetsTableMissing`), never a capability check here.
    pub async fn resolve_item_assets(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<Option<Arc<dyn AssetRecordStore>>> {
        let _phase = enter_phase(Phase::Routing);
        let routed = self.lookup(tenant, catalog, collection)?;
        if !routed.decl.stac_item_assets {
            return Ok(None);
        }
        let store =
            routed
                .anchor()
                .asset_record_store()
                .ok_or_else(|| Error::CapabilityUnsupported {
                    collection: routed.decl.id.clone(),
                    capability: "assets".to_string(),
                })?;
        Ok(Some(store))
    }

    /// Resolves this collection's per-item STAC metadata sidecar (`#202`),
    /// when it has one — the capability `tellurion-stac`'s handlers batch
    /// one lookup per page against, and the ONLY lane that ever consults
    /// it: nothing here is reachable from the Features lane, whose
    /// responses this slice leaves untouched by construction.
    ///
    /// `Ok(None)` is the ORDINARY answer, not a refusal — the same shape
    /// [`resolve_volume`](Self::resolve_volume) uses: a collection that
    /// never declared `stac_metadata: true` (every collection that predates
    /// `#202`) resolves to `None` without probing a driver, and its STAC
    /// Items stay byte-for-byte what they are today.
    ///
    /// Anchored to the collection's canonical storage
    /// (`RoutedCollection::anchor`, the same driver `resolve_assets` uses)
    /// rather than to a routable lane of its own: `"<table>_stac"` is a
    /// sidecar of the collection's own physical table, so it lives wherever
    /// that table does — STAC keeps resolving `features`, and `RoutingDecl`
    /// grows nothing.
    ///
    /// A collection that DID declare the sidecar against an anchor driver
    /// advertising no `stac_metadata_source` is a named
    /// `CapabilityUnsupported("stac-metadata")` refusal rather than another
    /// `Ok(None)`: the operator asked for per-item STAC metadata and this
    /// storage cannot serve any, so silently serving Items without it would
    /// hide a genuine misconfiguration behind output that looks exactly
    /// like the un-opted-in case. Whether the table itself was ever
    /// provisioned is a separate, request-time question the driver answers
    /// by name (PostGIS: `StacTableMissing`), never a capability check
    /// here.
    pub async fn resolve_stac_metadata(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<Option<Arc<dyn StacMetadataSource>>> {
        let _phase = enter_phase(Phase::Routing);
        let routed = self.lookup(tenant, catalog, collection)?;
        if !routed.decl.stac_metadata {
            return Ok(None);
        }
        let source =
            routed
                .anchor()
                .stac_metadata_source()
                .ok_or_else(|| Error::CapabilityUnsupported {
                    collection: routed.decl.id.clone(),
                    capability: "stac-metadata".to_string(),
                })?;
        Ok(Some(source))
    }

    /// Resolves the collection's configured `object_store` (assets-and-
    /// object-storage proposal, first slice) — independent of every
    /// `StorageDriver`-shaped lane (see `config::ObjectStoreDecl`'s own doc
    /// for why the two concepts never touch). No `object_store` declared at
    /// all refuses with `CapabilityUnsupported("managed-storage")` — a
    /// collection with no managed-storage lane, the same remote-assets-only
    /// story the `core` conformance class alone already fully supports.
    pub fn resolve_object_store(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<Arc<dyn ObjectStore>> {
        let routed = self.lookup(tenant, catalog, collection)?;
        let object_store_id =
            routed
                .decl
                .object_store
                .as_deref()
                .ok_or_else(|| Error::CapabilityUnsupported {
                    collection: routed.decl.id.clone(),
                    capability: "managed-storage".to_string(),
                })?;
        self.object_stores
            .get(object_store_id)
            .cloned()
            .ok_or_else(|| {
                Error::Config(format!(
                    "collection '{}' references object_store '{object_store_id}', which is not built",
                    routed.decl.id
                ))
            })
    }

    /// Resolves the collection's tenant/catalog-scoped write lane to its
    /// primary driver's `OutboxSource` (`#67`) — the applier's read side of
    /// the same storage [`resolve_write`](Self::resolve_write) resolves the
    /// write side of, since a driver that advertises `write_sink` also
    /// advertises `outbox_source` by design (the design doc's own
    /// invariant). Refuses with `CapabilityUnsupported("outbox")` under the
    /// same conditions `resolve_write` refuses with `"write"`: no
    /// `routing.write` at all, or a named storage that doesn't advertise it.
    pub async fn resolve_outbox(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<(CollectionDecl, Arc<dyn OutboxSource>)> {
        let routed = self.lookup(tenant, catalog, collection)?;
        let Some(lane) = &routed.write else {
            return Err(Error::CapabilityUnsupported {
                collection: routed.decl.id.clone(),
                capability: "outbox".to_string(),
            });
        };
        let (_, driver) = lane
            .entries
            .first()
            .expect("build_lane never produces an empty RoutedLane");
        let source = driver
            .outbox_source()
            .ok_or_else(|| Error::CapabilityUnsupported {
                collection: routed.decl.id.clone(),
                capability: "outbox".to_string(),
            })?;
        let decl = self.effective_decl(routed).await?;
        Ok((decl, source))
    }

    /// Resolves the coordinator an outbox consumer for this collection
    /// competes for leadership on (`#193`): the `Lease` capability of the
    /// same write-lane primary [`resolve_outbox`](Self::resolve_outbox)
    /// reads obligations from. Deliberately that storage and no other —
    /// the database already holding a collection's obligations is the one
    /// component every replica of a write deployment demonstrably shares,
    /// which is what makes clustering cost no new mandatory dependency.
    ///
    /// Returns no `CollectionDecl`: a lease key is built from routing
    /// identity (tenant/catalog/collection), never from physical
    /// table/geometry/pk facts, so there is nothing here worth paying a
    /// descriptor derivation for. Refuses with
    /// `CapabilityUnsupported("lease")` under the same conditions
    /// `resolve_outbox` refuses with `"outbox"`: no `routing.write` at all,
    /// or a named storage that does not advertise the capability.
    pub fn resolve_lease(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<Arc<dyn Lease>> {
        let routed = self.lookup(tenant, catalog, collection)?;
        let Some(lane) = &routed.write else {
            return Err(Error::CapabilityUnsupported {
                collection: routed.decl.id.clone(),
                capability: "lease".to_string(),
            });
        };
        let (_, driver) = lane
            .entries
            .first()
            .expect("build_lane never produces an empty RoutedLane");
        driver.lease().ok_or_else(|| Error::CapabilityUnsupported {
            collection: routed.decl.id.clone(),
            capability: "lease".to_string(),
        })
    }

    /// Resolves the deployment-wide durable job ledger (`#182`): the
    /// [`JobStore`] advertised by the storage `ServerConfig::processes.storage`
    /// names.
    ///
    /// The one capability resolver here that takes a **storage id** rather
    /// than a `(tenant, catalog, collection)` triple, because a job is not a
    /// collection's: it belongs to a process and to a catalog, and `#182`'s
    /// design is deliberately one ledger the whole deployment shares so
    /// heterogeneous replicas can claim from it.
    ///
    /// Refuses with [`Error::Config`] rather than
    /// [`Error::CapabilityUnsupported`] for both failure modes — an unknown
    /// storage id, or one whose driver does not advertise a job store —
    /// because both are the operator having named the wrong storage in
    /// `server.processes`, and because `CapabilityUnsupported`'s own field is
    /// a *collection*: reporting a storage id there would render as
    /// "collection 'pg' does not support capability 'job_store'", which names
    /// a collection that does not exist. Same treatment every driver's
    /// `*TableMissing` already receives at the `tellurion_core::Error`
    /// boundary, and for the same reason: a misconfiguration, not a fault.
    ///
    /// Never consulted by a request path. `tellurion-server` calls this once
    /// at boot; on `Err` it logs the refusal by name and serves no Processes
    /// root at all, which is `#182`'s own "a deployment with no runner
    /// capability does not get a half-working Processes root" rule.
    pub fn resolve_job_store(&self, storage: &str) -> Result<Arc<dyn JobStore>> {
        let driver = self.drivers.get(storage).ok_or_else(|| {
            Error::Config(format!(
                "server.processes.storage names storage '{storage}', which is not declared"
            ))
        })?;
        driver.job_store().ok_or_else(|| {
            Error::Config(format!(
                "server.processes.storage names storage '{storage}', whose driver advertises no durable job ledger"
            ))
        })
    }

    /// Resolves the collection's tenant/catalog-scoped index lane to a
    /// single `IndexSink` (`#67`) plus its effective decl — the derived-
    /// index counterpart of [`resolve_write`](Self::resolve_write), same
    /// "exactly one driver, no fallback tail" shape (see
    /// `RoutedCollection::index`'s own doc): `AppConfig::validate` rejects
    /// an empty or multi-entry `routing.index` before a `Router` is ever
    /// built. A collection with no `routing.index` at all, or whose named
    /// storage doesn't advertise `index_sink`, refuses with
    /// `CapabilityUnsupported("index")`.
    pub async fn resolve_index(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<(CollectionDecl, Arc<dyn IndexSink>)> {
        let routed = self.lookup(tenant, catalog, collection)?;
        let Some(lane) = &routed.index else {
            return Err(Error::CapabilityUnsupported {
                collection: routed.decl.id.clone(),
                capability: "index".to_string(),
            });
        };
        validate_lane_capability(&routed.decl.id, "index", lane, |driver| {
            driver.index_sink().is_some()
        })?;
        let (_, driver) = lane
            .entries
            .first()
            .expect("build_lane never produces an empty RoutedLane");
        let sink = driver
            .index_sink()
            .ok_or_else(|| Error::CapabilityUnsupported {
                collection: routed.decl.id.clone(),
                capability: "index".to_string(),
            })?;
        let decl = self.effective_decl(routed).await?;
        Ok((decl, sink))
    }

    /// Resolves the collection's tenant/catalog-scoped search lane (`#67`,
    /// freshness-gated search routing, design doc section 4): only entry 0
    /// (`routing.search`'s primary) is ever freshness-gated — mirroring the
    /// "index 0 is the primary ... later entries are a read-only fallback
    /// tail" shape every other lane's `RoutedLane` already has (`RoutedLane`'s
    /// own doc) — and only entry 0 is ever asked for `SearchSource`; entries
    /// 1+ are always tried as plain degraded `FeatureSource` reads, never as
    /// another index attempt, even when their driver also happens to
    /// advertise `SearchSource` (the ordinary case for another PostGIS
    /// storage in this workspace, since that capability is driver-wide, not
    /// per-collection — see `StorageDriver::search_source`'s own doc). That
    /// is what makes `search: [index, main]` mean what the design doc says
    /// it means: "index" is the one entry ever measured for freshness,
    /// "main" is unconditionally the degraded fallback, never a second
    /// freshness attempt against whatever index table it happens to expose
    /// for some other collection.
    ///
    /// The gate: `lag = write lane's OutboxSource::primary_high_water(c) -
    /// SearchSource::applied_high_water(c)`; entry 0 serves only while
    /// `lag <= collection.search.freshness_bound`. Any failure to resolve
    /// either high-water mark (no `routing.write`, a missing outbox/index
    /// table, ...) makes the lag unknown, which this treats exactly like
    /// "exceeds the bound" — falls through to the tail — per the design
    /// doc's "staleness is surfaced, never faked" stance (section 6): an
    /// unmeasurable index is never trusted by default. A chain exhausted
    /// with nothing able to answer refuses with `CapabilityUnsupported`,
    /// same as a collection with no `routing.search` at all.
    pub async fn resolve_search(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<(CollectionDecl, SearchResolution)> {
        let (decl, resolution, _served) = self
            .resolve_search_read(tenant, catalog, collection, &Hints::none())
            .await?;
        Ok((decl, resolution))
    }

    /// [`resolve_search`](Self::resolve_search) with the `#183` read-lane
    /// additions — the unhinted method delegates here with [`Hints::none`],
    /// under which the walk below visits the configured order and resolves
    /// exactly as documented there. The extras:
    ///
    /// - `hints.prefer()` reorders the chain before the walk
    ///   (`preferred_entries`) but AFTER both search-lane validations ran
    ///   against the configured order, so a hint can neither dodge nor
    ///   newly trip a misconfiguration refusal. One `resolve_search` rule
    ///   survives reordering untouched: only the CONFIGURED entry 0 is
    ///   ever an index attempt. A preferred tail entry serves as a plain
    ///   degraded `FeatureSource` read — which is the whole point of
    ///   `prefer:main` when diagnosing index-vs-main divergence — and is
    ///   never freshness-gated, even when its driver happens to advertise
    ///   `SearchSource` for some other collection's index; conversely, the
    ///   configured primary keeps its freshness gate wherever it lands in
    ///   the walk, and its own `FeatureSource` still never substitutes for
    ///   its stale index. A preferred entry that cannot serve falls through
    ///   to the rest of the chain in configured order — reorder, never
    ///   extend.
    /// - The returned `String` is the storage id of the entry that this
    ///   resolution routed the read to — the search lane's counterpart of
    ///   [`resolve_features_read`](Self::resolve_features_read)'s
    ///   [`ServedSource`], known at resolve time here (no per-call recorder
    ///   needed) because this method always commits to exactly one entry.
    pub async fn resolve_search_read(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
        hints: &Hints,
    ) -> Result<(CollectionDecl, SearchResolution, String)> {
        let routed = self.lookup(tenant, catalog, collection)?;
        let Some(lane) = &routed.search else {
            return Err(Error::CapabilityUnsupported {
                collection: routed.decl.id.clone(),
                capability: "search".to_string(),
            });
        };
        validate_search_lane_capability(&routed.decl.id, lane)?;
        validate_search_lane_provisioning(&routed.decl.id, lane, routed.index.as_ref())?;

        // The walk order: configured order, or the preferred entry pulled to
        // the front with the rest keeping configured order behind it — the
        // index-based twin of `preferred_entries` (which reorders the pairs
        // themselves), used here because "is this the configured primary?"
        // must key on the entry's configured POSITION, not its storage id,
        // to stay exact even for a degenerate chain repeating an id.
        let preferred_index = hints
            .prefer()
            .and_then(|prefer| lane.entries.iter().position(|(id, _)| id == prefer));
        let walk = preferred_index
            .into_iter()
            .chain((0..lane.entries.len()).filter(|index| Some(*index) != preferred_index));

        for index in walk {
            let (storage_id, driver) = &lane.entries[index];
            if index == 0 {
                if let Some(search_source) = driver.search_source() {
                    let bound = routed.decl.search.freshness_bound;
                    let outbox = routed
                        .write
                        .as_ref()
                        .and_then(|write_lane| write_lane.entries.first())
                        .and_then(|(_, driver)| driver.outbox_source());
                    let fresh = match &outbox {
                        Some(outbox) => {
                            index_is_fresh(
                                outbox.as_ref(),
                                search_source.as_ref(),
                                &routed.decl,
                                bound,
                            )
                            .await
                        }
                        // No write lane to measure the primary's own
                        // high-water against — the lag is unknown, treated
                        // like "exceeds the bound" (`resolve_search`'s doc).
                        None => false,
                    };
                    if fresh {
                        let decl = self.effective_decl(routed).await?;
                        return Ok((
                            decl,
                            SearchResolution::Index(search_source),
                            storage_id.clone(),
                        ));
                    }
                    // Index-capable but not fresh enough (or unmeasurable):
                    // fall through to the rest of the walk. Its own
                    // `FeatureSource` deliberately never substitutes for
                    // its stale index (`resolve_search`'s doc).
                    continue;
                }
                // The configured primary has nothing index-shaped to gate
                // at all — serve its own degraded read directly, no
                // freshness question to ask.
            }
            // A tail entry — or the search-incapable configured primary
            // just above — serves as a plain degraded read, never a second
            // index attempt (`resolve_search`'s doc).
            if let Some(feature_source) = driver.feature_source() {
                let decl = self.effective_decl(routed).await?;
                return Ok((
                    decl,
                    SearchResolution::Fallback(feature_source),
                    storage_id.clone(),
                ));
            }
        }

        Err(Error::CapabilityUnsupported {
            collection: routed.decl.id.clone(),
            capability: "search".to_string(),
        })
    }

    /// Resolves the collection's tenant/catalog-scoped tiles lane to its
    /// primary driver's `VolumeSource`, if it advertises one (`#15`) —
    /// `places3d` rides the tiles lane for this too, same as
    /// [`resolve_tiles`](Self::resolve_tiles). Unlike `resolve_features`/
    /// `resolve_tiles`, `Ok(None)` is the ordinary answer, not a refusal:
    /// "this driver has no true solid geometry, run the places3d extrusion
    /// fallback instead" is an expected outcome for every driver in this
    /// workspace today, not a misconfiguration. Only an unresolvable
    /// `(tenant, catalog, collection)` errors here.
    ///
    /// `#70`: a driver-wide `VolumeSource` answer means "this backend CAN
    /// serve solid geometry," not "THIS collection's own geometry column
    /// IS solid" — a footprint+height `places3d` collection can share a
    /// storage entry with a genuinely solid one. Once the driver advertises
    /// the capability at all, this narrows that answer against the
    /// collection's own descriptor-derived `geometry_type` fact (the same
    /// TTL-cached `resolved_descriptor` every other physical fact goes
    /// through): a reported type that isn't one of
    /// `is_volume_capable_geometry_type`'s names means this particular
    /// collection has no true solid geometry, so this falls back to `None`
    /// regardless of the driver-wide signal. A descriptor that fails to
    /// derive, or reports no `geometry_type` at all, keeps the pre-`#70`
    /// behavior (trust the driver-wide signal) — this check only ever
    /// narrows an existing `Some` answer, never turns a `None` into a
    /// `Some`.
    pub async fn resolve_volume(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
    ) -> Result<Option<Arc<dyn VolumeSource>>> {
        let _phase = enter_phase(Phase::Routing);
        let routed = self.lookup(tenant, catalog, collection)?;
        let Some(source) = volume_source(&routed.tiles) else {
            return Ok(None);
        };
        match self.resolved_descriptor(routed).await {
            Ok(descriptor) => match descriptor.geometry_type.as_deref() {
                Some(geometry_type)
                    if !crate::storage::is_volume_capable_geometry_type(geometry_type) =>
                {
                    Ok(None)
                }
                _ => Ok(Some(source)),
            },
            Err(_) => Ok(Some(source)),
        }
    }

    /// Forces `descriptor_cache`'s async eviction pass to run and reports
    /// its current entry count, so a test can assert on post-eviction state
    /// deterministically — the same pattern `MokaTileCache`'s own test-only
    /// `run_pending_tasks`/`weighted_size` helpers use (`cache.rs`).
    #[cfg(test)]
    async fn descriptor_cache_entry_count(&self) -> u64 {
        self.descriptor_cache.run_pending_tasks().await;
        self.descriptor_cache.entry_count()
    }
}

/// Resolves a features lane's driver chain (`entries`, already in serving
/// order — `preferred_entries`' output, which is the configured order unless
/// a `#183` `prefer:` hint reordered it) to a single `FeatureSource`: the
/// bare primary when the lane has exactly one entry — identical cost to the
/// pre-`#21` single-driver path, per the design's "a single-entry lane must
/// have zero added overhead" rule — else a `FallbackFeatureSource` wrapping
/// every entry that implements the capability, tried in serving order.
/// `Err(CapabilityUnsupported)` when none of the lane's entries do, exactly
/// the pre-`#21` request-time refusal for an unrouted lane whose single
/// storage lacks the capability.
///
/// Also returns the [`ServedSource`] naming the entry that serves (`#183`):
/// recorded eagerly for the single-entry case (only that entry can ever
/// serve, and eager recording is what keeps that path wrapper-free), at
/// call time by the fallback wrapper otherwise.
fn features_source(
    collection_id: &str,
    entries: &[(String, Arc<dyn StorageDriver>)],
) -> Result<(Arc<dyn FeatureSource>, ServedSource)> {
    let served = ServedSource::default();
    if let [(storage_id, driver)] = entries {
        let source = driver
            .feature_source()
            .ok_or_else(|| Error::CapabilityUnsupported {
                collection: collection_id.to_string(),
                capability: "features".to_string(),
            })?;
        served.record(storage_id);
        return Ok((
            observe_feature_source(budget_feature_source(source)),
            served,
        ));
    }
    let sources: Vec<_> = entries
        .iter()
        .filter_map(|(storage_id, driver)| {
            driver
                .feature_source()
                .map(|source| (storage_id.clone(), source))
        })
        .collect();
    if sources.is_empty() {
        return Err(Error::CapabilityUnsupported {
            collection: collection_id.to_string(),
            capability: "features".to_string(),
        });
    }
    let source = observe_feature_source(budget_feature_source(Arc::new(FallbackFeatureSource {
        entries: sources,
        served: served.clone(),
    })));
    Ok((source, served))
}

/// Tiles-lane counterpart of [`features_source`]; see its docs for the
/// single-entry zero-overhead and empty-chain-refusal rules, which apply
/// identically here.
fn tiles_source(collection_id: &str, lane: &RoutedLane) -> Result<Arc<dyn TileSource>> {
    if let [(_, driver)] = lane.entries.as_slice() {
        let source = driver
            .tile_source()
            .ok_or_else(|| Error::CapabilityUnsupported {
                collection: collection_id.to_string(),
                capability: "tiles".to_string(),
            })?;
        return Ok(observe_tile_source(source));
    }
    let sources: Vec<_> = lane
        .entries
        .iter()
        .filter_map(|(_, driver)| driver.tile_source())
        .collect();
    if sources.is_empty() {
        return Err(Error::CapabilityUnsupported {
            collection: collection_id.to_string(),
            capability: "tiles".to_string(),
        });
    }
    Ok(observe_tile_source(Arc::new(FallbackTileSource {
        entries: sources,
    })))
}

/// Maps-lane counterpart of [`tiles_source`] (`#86`); identical shape and
/// same single-entry zero-overhead / empty-chain-refusal rules — kept as its
/// own function (rather than reusing `tiles_source` directly) only so a
/// resolution failure names the `"maps"` capability, not `"tiles"`, in its
/// `Error::CapabilityUnsupported`.
fn maps_source(collection_id: &str, lane: &RoutedLane) -> Result<Arc<dyn TileSource>> {
    if let [(_, driver)] = lane.entries.as_slice() {
        let source = driver
            .tile_source()
            .ok_or_else(|| Error::CapabilityUnsupported {
                collection: collection_id.to_string(),
                capability: "maps".to_string(),
            })?;
        return Ok(observe_tile_source(source));
    }
    let sources: Vec<_> = lane
        .entries
        .iter()
        .filter_map(|(_, driver)| driver.tile_source())
        .collect();
    if sources.is_empty() {
        return Err(Error::CapabilityUnsupported {
            collection: collection_id.to_string(),
            capability: "maps".to_string(),
        });
    }
    Ok(observe_tile_source(Arc::new(FallbackTileSource {
        entries: sources,
    })))
}

/// Maps-lane counterpart of [`raster_source_for_lane`] (`#37`); identical
/// shape and identical rules, kept as its own function for exactly the
/// reason [`maps_source`] is — so a resolution failure over `routing.maps`
/// names the `"maps"` capability, not `"tiles"`.
fn maps_raster_source(collection_id: &str, lane: &RoutedLane) -> Result<Arc<dyn RasterSource>> {
    if let [(_, driver)] = lane.entries.as_slice() {
        return driver
            .raster_source()
            .ok_or_else(|| Error::CapabilityUnsupported {
                collection: collection_id.to_string(),
                capability: "maps".to_string(),
            });
    }
    let sources: Vec<_> = lane
        .entries
        .iter()
        .filter_map(|(_, driver)| driver.raster_source())
        .collect();
    if sources.is_empty() {
        return Err(Error::CapabilityUnsupported {
            collection: collection_id.to_string(),
            capability: "maps".to_string(),
        });
    }
    Ok(Arc::new(FallbackRasterSource { entries: sources }))
}

/// Raster-lane counterpart of [`tiles_source`] (`#37`); see its docs for the
/// single-entry zero-overhead and empty-chain-refusal rules, which apply
/// identically here.
fn raster_source_for_lane(collection_id: &str, lane: &RoutedLane) -> Result<Arc<dyn RasterSource>> {
    if let [(_, driver)] = lane.entries.as_slice() {
        return driver
            .raster_source()
            .ok_or_else(|| Error::CapabilityUnsupported {
                collection: collection_id.to_string(),
                capability: "tiles".to_string(),
            });
    }
    let sources: Vec<_> = lane
        .entries
        .iter()
        .filter_map(|(_, driver)| driver.raster_source())
        .collect();
    if sources.is_empty() {
        return Err(Error::CapabilityUnsupported {
            collection: collection_id.to_string(),
            capability: "tiles".to_string(),
        });
    }
    Ok(Arc::new(FallbackRasterSource { entries: sources }))
}

/// Resolves a tiles lane's `VolumeSource`, when its primary driver
/// advertises one (`#15`) — the true-solid-geometry counterpart of
/// `tiles_source`'s MVT resolution, consulted by the places3d lane before
/// its MVT+extrusion path runs. Unlike `tiles_source`/`features_source`,
/// absence is not a refusal (there is no `Err` case), and only the primary
/// entry is ever consulted, never a fallback tail: a tail entry is a
/// different backend that may hold different — or no — solid geometry for
/// the same collection, not a retry of the same data, so falling through to
/// it here would silently swap which real-world geometry a tile shows
/// instead of behaving like the read-only retry every other fallback tail
/// is.
fn volume_source(lane: &RoutedLane) -> Option<Arc<dyn VolumeSource>> {
    lane.entries
        .first()?
        .1
        .volume_source()
        .map(observe_volume_source)
}

/// Wraps an ordered, non-empty chain of `FeatureSource`s bound to the same
/// collection's features lane (`#21`): the first (primary) entry serves
/// every call, and only a call that returns `Err` falls through to the next
/// entry — a fallback tail is a preference among drivers, not a merge of
/// their results. Built only when a lane resolves to more than one entry;
/// see `features_source`. Each source travels with its storage id so a
/// successful call can record which entry served into `served` (`#183`) —
/// recorded only on `Ok`, since an entry that errored served nothing.
struct FallbackFeatureSource {
    entries: Vec<(String, Arc<dyn FeatureSource>)>,
    served: ServedSource,
}

#[async_trait::async_trait]
impl FeatureSource for FallbackFeatureSource {
    async fn items(&self, collection: &CollectionDecl, query: &ItemsQuery) -> Result<FeaturePage> {
        let mut last_err = None;
        for (storage_id, source) in &self.entries {
            match source.items(collection, query).await {
                Ok(page) => {
                    self.served.record(storage_id);
                    return Ok(page);
                }
                Err(error @ Error::ItemsVertexBudgetExceeded { .. }) => return Err(error),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.expect("FallbackFeatureSource.entries is never empty"))
    }

    async fn item(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
    ) -> Result<Option<serde_json::Value>> {
        let mut last_err = None;
        for (storage_id, source) in &self.entries {
            match source.item(collection, id, filter).await {
                Ok(value) => {
                    self.served.record(storage_id);
                    return Ok(value);
                }
                Err(error @ Error::ItemsVertexBudgetExceeded { .. }) => return Err(error),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.expect("FallbackFeatureSource.entries is never empty"))
    }

    fn filter_capable(&self) -> bool {
        self.entries
            .iter()
            .all(|(_, source)| source.filter_capable())
    }

    /// The intersection of every entry's own declared set (`#105`) — the
    /// same "every entry must earn it" rule `filter_capable` above already
    /// applies to the coarser bool: a request may land on any entry in this
    /// chain (the primary, or a fallback tail after the primary errors), so
    /// a class this composite advertises must be one every entry actually
    /// satisfies, not just the primary. Starts from the first entry's own
    /// set rather than the full candidate universe — unlike
    /// `Router::cql2_conformance_classes`'s workspace-wide fold, a lane with
    /// zero entries never exists (`entries` is always non-empty by
    /// construction, see `features_source`), so there is no vacuous case to
    /// seed for.
    fn cql2_conformance_classes(&self) -> Vec<&'static str> {
        let mut classes = self.entries[0].1.cql2_conformance_classes();
        for (_, source) in &self.entries[1..] {
            let declared = source.cql2_conformance_classes();
            classes.retain(|class| declared.contains(class));
        }
        classes
    }

    fn crs_capable(&self) -> bool {
        self.entries.iter().all(|(_, source)| source.crs_capable())
    }

    /// `#217`: same all-entries rule as `crs_capable`/`filter_capable` above
    /// — any entry in this chain may end up answering a given request, so
    /// `filter-crs` is only honoured when every one of them can transform a
    /// filter's spatial literals. A chain whose tail cannot would otherwise
    /// evaluate the same filter in a different CRS after a primary error,
    /// which is exactly the silent wrong-CRS evaluation `#217` exists to
    /// close.
    fn filter_crs_capable(&self) -> bool {
        self.entries
            .iter()
            .all(|(_, source)| source.filter_crs_capable())
    }

    async fn item_with_crs(
        &self,
        collection: &CollectionDecl,
        id: &str,
        filter: Option<&Filter>,
        requested_crs: crate::crs::RequestedCrs,
    ) -> Result<Option<serde_json::Value>> {
        let mut last_err = None;
        for (storage_id, source) in &self.entries {
            match source
                .item_with_crs(collection, id, filter, requested_crs)
                .await
            {
                Ok(value) => {
                    self.served.record(storage_id);
                    return Ok(value);
                }
                Err(error @ Error::ItemsVertexBudgetExceeded { .. }) => return Err(error),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.expect("FallbackFeatureSource.entries is never empty"))
    }
}

/// Tiles-lane counterpart of `FallbackFeatureSource`. `Ok(None)` — an empty
/// tile — is a valid answer from any entry and is returned as-is, never
/// treated as a reason to fall through to the tail; only `Err` does that.
struct FallbackTileSource {
    entries: Vec<Arc<dyn TileSource>>,
}

#[async_trait::async_trait]
impl TileSource for FallbackTileSource {
    async fn mvt_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> Result<Option<Bytes>> {
        let mut last_err = None;
        for source in &self.entries {
            match source.mvt_tile(collection, coord, filter).await {
                Ok(tile) => return Ok(tile),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.expect("FallbackTileSource.entries is never empty"))
    }

    fn tile_capable(&self, collection: &CollectionDecl) -> bool {
        self.entries
            .iter()
            .all(|source| source.tile_capable(collection))
    }

    fn filter_capable(&self) -> bool {
        self.entries.iter().all(|source| source.filter_capable())
    }

    /// `#190`: same all-entries intersection rule as `filter_capable` above
    /// — a grid is only advertised when EVERY entry in the fallback chain
    /// can serve it, since any entry may end up answering a given tile.
    fn supports_tile_matrix_set(&self, tms: crate::tms::TileMatrixSet) -> bool {
        self.entries
            .iter()
            .all(|source| source.supports_tile_matrix_set(tms))
    }

    /// `#190`: same first-`Ok`-wins fall-through as `mvt_tile` above, over
    /// the grid-parameterized entry point.
    async fn mvt_tile_in(
        &self,
        collection: &CollectionDecl,
        tms: crate::tms::TileMatrixSet,
        coord: TileCoord,
        filter: Option<&Filter>,
    ) -> Result<Option<Bytes>> {
        let mut last_err = None;
        for source in &self.entries {
            match source.mvt_tile_in(collection, tms, coord, filter).await {
                Ok(tile) => return Ok(tile),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.expect("FallbackTileSource.entries is never empty"))
    }

    async fn vector_layers(&self, collection: &CollectionDecl) -> Result<Option<Vec<String>>> {
        let mut last_err = None;
        for source in &self.entries {
            match source.vector_layers(collection).await {
                Ok(layers) => return Ok(layers),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.expect("FallbackTileSource.entries is never empty"))
    }
}

/// Raster-lane counterpart of `FallbackTileSource` (`#37`). `Ok(None)` — an
/// empty tile — is a valid answer from any entry and is returned as-is,
/// never treated as a reason to fall through to the tail; only `Err` does.
struct FallbackRasterSource {
    entries: Vec<Arc<dyn RasterSource>>,
}

#[async_trait::async_trait]
impl RasterSource for FallbackRasterSource {
    async fn raster_tile(
        &self,
        collection: &CollectionDecl,
        coord: TileCoord,
    ) -> Result<Option<crate::storage::RasterWindow>> {
        let mut last_err = None;
        for source in &self.entries {
            match source.raster_tile(collection, coord).await {
                Ok(window) => return Ok(window),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.expect("FallbackRasterSource.entries is never empty"))
    }
}

/// What [`Router::resolve_search`] decided for one request (`#67`): the
/// routed index answered fresh enough (`Index`), or the freshness gate
/// (lag over bound, or unmeasurable) sent the request to a fallback tail
/// entry's degraded primary read (`Fallback`) — design doc section 4. A
/// caller cares which, since the two are genuinely different capabilities
/// (`SearchSource::search` vs. `FeatureSource::items`), not two
/// implementations of the same query shape.
pub enum SearchResolution {
    Index(Arc<dyn SearchSource>),
    Fallback(Arc<dyn FeatureSource>),
}

/// The design doc's freshness gate (section 4): `lag = outbox's
/// primary_high_water(c) - search's applied_high_water(c)`; fresh enough
/// when `lag <= bound`. Any error resolving either high-water mark makes
/// freshness unmeasurable, which this treats as "not fresh" — an inability
/// to confirm freshness is itself grounds to prefer the fallback tail, never
/// a reason to serve from an index nobody can vouch for (section 6:
/// staleness is surfaced, never faked).
async fn index_is_fresh(
    outbox: &dyn OutboxSource,
    search: &dyn SearchSource,
    collection: &CollectionDecl,
    bound: u64,
) -> bool {
    let primary = outbox.primary_high_water(collection).await;
    let applied = search.applied_high_water(collection).await;
    match (primary, applied) {
        (Ok(primary), Ok(applied)) => primary.0.saturating_sub(applied.0) <= bound,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{AttributeColumn, PhysicalCollection, SpatialExtent};
    use crate::config::{ColorRamp, ColormapConf, ProtocolExposure};
    use crate::filter;
    use crate::locking;
    use crate::storage::{FeaturePage, ItemsQuery, TileCoord, VolumeMesh};
    use bytes::Bytes;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A `CatalogSource` that reports exactly the physical collections it
    /// was constructed with — no I/O, no real backend.
    struct FakeCatalog(Vec<PhysicalCollection>);

    #[async_trait::async_trait]
    impl CatalogSource for FakeCatalog {
        async fn collections(&self) -> Result<Vec<PhysicalCollection>> {
            Ok(self.0.clone())
        }
    }

    struct ProbeCatalog {
        calls: Arc<AtomicUsize>,
        fails: bool,
    }

    #[async_trait::async_trait]
    impl CatalogSource for ProbeCatalog {
        async fn collections(&self) -> Result<Vec<PhysicalCollection>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fails {
                Err(Error::Timeout)
            } else {
                Ok(vec![])
            }
        }
    }

    struct ProbeDriver {
        catalog: Arc<ProbeCatalog>,
    }

    impl StorageDriver for ProbeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::clone(&self.catalog) as Arc<dyn CatalogSource>
        }
    }

    struct ProbeFactory {
        name: &'static str,
        catalog: Arc<ProbeCatalog>,
    }

    impl DriverFactory for ProbeFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(ProbeDriver {
                catalog: Arc::clone(&self.catalog),
            }))
        }
    }

    fn probe_config() -> AppConfig {
        serde_yaml::from_str(
            r#"
storages:
  - { id: primary, driver: probe-primary, url_env: DATABASE_URL }
  - { id: secondary, driver: probe-secondary, url_env: DATABASE_URL2 }
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn probe_storages_calls_every_mandatory_catalog_once() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let secondary_calls = Arc::new(AtomicUsize::new(0));
        let mut registry = Registry::new();
        registry.register(Arc::new(ProbeFactory {
            name: "probe-primary",
            catalog: Arc::new(ProbeCatalog {
                calls: Arc::clone(&primary_calls),
                fails: false,
            }),
        }));
        registry.register(Arc::new(ProbeFactory {
            name: "probe-secondary",
            catalog: Arc::new(ProbeCatalog {
                calls: Arc::clone(&secondary_calls),
                fails: false,
            }),
        }));
        let router = Router::build(&probe_config(), &registry).unwrap();

        router.probe_storages().await.unwrap();

        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(secondary_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn probe_storages_attempts_every_catalog_and_reports_failures_deterministically() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let secondary_calls = Arc::new(AtomicUsize::new(0));
        let mut registry = Registry::new();
        registry.register(Arc::new(ProbeFactory {
            name: "probe-primary",
            catalog: Arc::new(ProbeCatalog {
                calls: Arc::clone(&primary_calls),
                fails: true,
            }),
        }));
        registry.register(Arc::new(ProbeFactory {
            name: "probe-secondary",
            catalog: Arc::new(ProbeCatalog {
                calls: Arc::clone(&secondary_calls),
                fails: true,
            }),
        }));
        let router = Router::build(&probe_config(), &registry).unwrap();

        let error = router.probe_storages().await.unwrap_err();

        assert_eq!(primary_calls.load(Ordering::SeqCst), 1);
        assert_eq!(secondary_calls.load(Ordering::SeqCst), 1);
        assert!(matches!(error, Error::Storage(_)));
        assert_eq!(
            error.to_string(),
            "storage error: readiness probe failed for storages: primary, secondary"
        );
    }

    fn physical(name: &str) -> PhysicalCollection {
        PhysicalCollection {
            name: name.to_string(),
            geometry_column: None,
            primary_key: None,
            srid: None,
            geometry_type: None,
        }
    }

    /// In-memory driver that only ever supports features, never tiles —
    /// exercises the capability-refusal path without any real backend.
    struct FakeFeaturesOnlyDriver;

    #[async_trait::async_trait]
    impl FeatureSource for FakeFeaturesOnlyDriver {
        async fn items(
            &self,
            _collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> Result<FeaturePage> {
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
        ) -> Result<Option<serde_json::Value>> {
            Ok(None)
        }
    }

    struct FakeTilesOnlyDriver;

    #[async_trait::async_trait]
    impl TileSource for FakeTilesOnlyDriver {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&Filter>,
        ) -> Result<Option<Bytes>> {
            Ok(Some(Bytes::from_static(b"mvt")))
        }
    }

    struct FallbackContractFeature {
        fails: bool,
        label: &'static str,
    }

    #[async_trait::async_trait]
    impl FeatureSource for FallbackContractFeature {
        async fn items(&self, _: &CollectionDecl, _: &ItemsQuery) -> Result<FeaturePage> {
            if self.fails {
                Err(Error::Timeout)
            } else {
                Ok(FeaturePage {
                    features_geojson: vec![serde_json::json!({ "source": self.label })],
                    number_matched: Some(1),
                    next_token: None,
                })
            }
        }

        async fn item(
            &self,
            _: &CollectionDecl,
            _: &str,
            filter: Option<&Filter>,
        ) -> Result<Option<serde_json::Value>> {
            assert!(
                filter.is_some(),
                "the grant filter must reach every fallback entry"
            );
            if self.fails {
                Err(Error::Timeout)
            } else {
                Ok(Some(serde_json::json!({ "source": self.label })))
            }
        }

        fn filter_capable(&self) -> bool {
            true
        }

        fn crs_capable(&self) -> bool {
            true
        }

        async fn item_with_crs(
            &self,
            _: &CollectionDecl,
            _: &str,
            filter: Option<&Filter>,
            requested_crs: crate::crs::RequestedCrs,
        ) -> Result<Option<serde_json::Value>> {
            assert!(
                filter.is_some(),
                "the grant filter must reach every fallback entry"
            );
            if self.fails {
                Err(Error::Timeout)
            } else {
                Ok(Some(serde_json::json!({
                    "source": self.label,
                    "crs84": requested_crs == crate::crs::RequestedCrs::Crs84
                })))
            }
        }
    }

    struct FallbackContractTile {
        fails: bool,
        tile: Option<Bytes>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl TileSource for FallbackContractTile {
        async fn mvt_tile(
            &self,
            _: &CollectionDecl,
            _: TileCoord,
            filter: Option<&Filter>,
        ) -> Result<Option<Bytes>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(
                filter.is_some(),
                "the grant filter must reach every fallback entry"
            );
            if self.fails {
                Err(Error::Timeout)
            } else {
                Ok(self.tile.clone())
            }
        }

        fn filter_capable(&self) -> bool {
            true
        }

        async fn vector_layers(&self, _: &CollectionDecl) -> Result<Option<Vec<String>>> {
            if self.fails {
                Err(Error::Timeout)
            } else {
                Ok(Some(vec!["roads".to_string()]))
            }
        }
    }

    #[tokio::test]
    async fn feature_fallback_preserves_capabilities_crs_and_grant_filters() {
        let source = FallbackFeatureSource {
            entries: vec![
                (
                    "primary".to_string(),
                    Arc::new(FallbackContractFeature {
                        fails: true,
                        label: "primary",
                    }),
                ),
                (
                    "tail".to_string(),
                    Arc::new(FallbackContractFeature {
                        fails: false,
                        label: "tail",
                    }),
                ),
            ],
            served: ServedSource::default(),
        };
        let collection: CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main").unwrap();
        let filter = crate::filter::parse_text("id = 'visible'").unwrap();

        assert!(source.filter_capable());
        assert!(source.crs_capable());
        let item = source
            .item_with_crs(
                &collection,
                "one",
                Some(&filter),
                crate::crs::RequestedCrs::Crs84,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item["source"], "tail");
        assert_eq!(item["crs84"], true);
        assert_eq!(
            source.served.storage_id(),
            Some("tail"),
            "the served-source recorder must name the entry that actually answered (`#183`)"
        );
    }

    /// A `FeatureSource` whose only purpose is declaring a fixed CQL2 class
    /// set (`#105`) — `items`/`item` are never called by the tests below,
    /// so they stay unreachable stubs.
    struct FakeClassesFeature {
        classes: &'static [&'static str],
    }

    #[async_trait::async_trait]
    impl FeatureSource for FakeClassesFeature {
        async fn items(&self, _: &CollectionDecl, _: &ItemsQuery) -> Result<FeaturePage> {
            unreachable!("not exercised by the cql2_conformance_classes tests")
        }

        async fn item(
            &self,
            _: &CollectionDecl,
            _: &str,
            _: Option<&Filter>,
        ) -> Result<Option<serde_json::Value>> {
            unreachable!("not exercised by the cql2_conformance_classes tests")
        }

        fn cql2_conformance_classes(&self) -> Vec<&'static str> {
            self.classes.to_vec()
        }
    }

    /// `#105`: a request may land on any entry in a fallback chain (the
    /// primary, or a tail after the primary errors), so a class the
    /// composite advertises must be one every entry actually satisfies —
    /// the same "every entry must earn it" rule `filter_capable` already
    /// applies to the coarser bool, generalized to the richer set.
    #[test]
    fn fallback_feature_source_declares_the_intersection_of_its_entries() {
        let source = FallbackFeatureSource {
            entries: vec![
                (
                    "strong".to_string(),
                    Arc::new(FakeClassesFeature {
                        classes: &[
                            filter::CQL2_CLASS_BASIC,
                            filter::CQL2_CLASS_CQL2_TEXT,
                            filter::CQL2_CLASS_CQL2_JSON,
                            filter::CQL2_CLASS_BASIC_SPATIAL_FUNCTIONS,
                            filter::CQL2_CLASS_TEMPORAL_FUNCTIONS,
                        ],
                    }),
                ),
                (
                    "weak".to_string(),
                    Arc::new(FakeClassesFeature {
                        classes: &[
                            filter::CQL2_CLASS_BASIC,
                            filter::CQL2_CLASS_CQL2_TEXT,
                            filter::CQL2_CLASS_CQL2_JSON,
                        ],
                    }),
                ),
            ],
            served: ServedSource::default(),
        };
        let declared = source.cql2_conformance_classes();
        assert_eq!(
            declared,
            vec![
                filter::CQL2_CLASS_BASIC,
                filter::CQL2_CLASS_CQL2_TEXT,
                filter::CQL2_CLASS_CQL2_JSON,
            ]
        );
    }

    #[tokio::test]
    async fn tile_fallback_preserves_filter_layers_and_does_not_fall_through_on_null() {
        let tail_calls = Arc::new(AtomicUsize::new(0));
        let source = FallbackTileSource {
            entries: vec![
                Arc::new(FallbackContractTile {
                    fails: true,
                    tile: None,
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
                Arc::new(FallbackContractTile {
                    fails: false,
                    tile: Some(Bytes::from_static(b"tail")),
                    calls: Arc::clone(&tail_calls),
                }),
            ],
        };
        let collection: CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main").unwrap();
        let filter = crate::filter::parse_text("id = 'visible'").unwrap();

        assert!(source.filter_capable());
        assert_eq!(
            source.vector_layers(&collection).await.unwrap().unwrap(),
            vec!["roads"]
        );
        assert_eq!(
            source
                .mvt_tile(&collection, TileCoord { z: 0, x: 0, y: 0 }, Some(&filter))
                .await
                .unwrap()
                .unwrap(),
            Bytes::from_static(b"tail")
        );

        let tail_calls = Arc::new(AtomicUsize::new(0));
        let null_source = FallbackTileSource {
            entries: vec![
                Arc::new(FallbackContractTile {
                    fails: false,
                    tile: None,
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
                Arc::new(FallbackContractTile {
                    fails: false,
                    tile: Some(Bytes::from_static(b"must-not-run")),
                    calls: Arc::clone(&tail_calls),
                }),
            ],
        };
        assert_eq!(
            null_source
                .mvt_tile(&collection, TileCoord { z: 0, x: 0, y: 0 }, Some(&filter))
                .await
                .unwrap(),
            None
        );
        assert_eq!(tail_calls.load(Ordering::SeqCst), 0);
    }

    struct FakeDriver {
        features: bool,
        tiles: bool,
    }

    impl StorageDriver for FakeDriver {
        // `config_with` (below) always declares a single collection whose
        // table is "demo" — matching that here keeps this driver usable by
        // both the pre-existing resolve_* tests and `validate_catalog`.
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![physical("demo")]))
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            self.features
                .then(|| Arc::new(FakeFeaturesOnlyDriver) as Arc<dyn FeatureSource>)
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            self.tiles
                .then(|| Arc::new(FakeTilesOnlyDriver) as Arc<dyn TileSource>)
        }
    }

    struct FakeFactory {
        features: bool,
        tiles: bool,
    }

    impl DriverFactory for FakeFactory {
        fn name(&self) -> &str {
            "fake"
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FakeDriver {
                features: self.features,
                tiles: self.tiles,
            }))
        }
    }

    fn config_with(features: bool, tiles: bool) -> (AppConfig, Registry) {
        let config: AppConfig = serde_yaml::from_str(
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
"#,
        )
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory { features, tiles }));
        (config, registry)
    }

    #[tokio::test]
    async fn resolves_features_when_supported() {
        let (config, registry) = config_with(true, false);
        let router = Router::build(&config, &registry).unwrap();
        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.id, "demo");
    }

    #[tokio::test]
    async fn refuses_capability_the_driver_lacks() {
        let (config, registry) = config_with(true, false);
        let router = Router::build(&config, &registry).unwrap();
        match router.resolve_tiles("public", "default", "demo").await {
            Err(Error::CapabilityUnsupported { capability, .. }) => {
                assert_eq!(capability, "tiles");
            }
            other => panic!("expected CapabilityUnsupported, got {}", other.is_ok()),
        }
    }

    /// `#112`: a storage naming a driver this binary's `Registry` has no
    /// factory for fails boot with a config error naming both the storage
    /// and the driver — the same outcome whether that name was never valid
    /// or belongs to a driver crate this binary happened to be built
    /// without its cargo feature. `Registry::build` cannot tell those two
    /// cases apart, which is exactly the point (see `extension::
    /// NamedRegistry`'s own doc): compiled-out is absent, and absent fails
    /// by name, never silently.
    #[test]
    fn a_storage_naming_a_driver_absent_from_this_registry_fails_boot_by_name() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: not-compiled-in, url_env: DATABASE_URL } ]
"#,
        )
        .unwrap();
        config.validate().unwrap();

        // An empty registry stands in for a real binary built without the
        // driver crate that would otherwise register "not-compiled-in" —
        // from `Registry`'s point of view the two situations are identical.
        let registry = Registry::new();

        match Router::build(&config, &registry) {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("main"),
                    "error should name the storage: {message}"
                );
                assert!(
                    message.contains("not-compiled-in"),
                    "error should name the unresolved driver: {message}"
                );
            }
            other => panic!("expected a named Error::Config, got {}", other.is_ok()),
        }
    }

    // -- `Router::cql2_conformance_classes` (`#105`) -------------------------

    /// Mimics PostGIS's own declared set (`tellurion-postgis`'s driver.rs):
    /// the full candidate universe.
    const STRONG_DRIVER_CLASSES: &[&str] = &[
        filter::CQL2_CLASS_BASIC,
        filter::CQL2_CLASS_CQL2_TEXT,
        filter::CQL2_CLASS_CQL2_JSON,
        filter::CQL2_CLASS_BASIC_SPATIAL_FUNCTIONS,
        filter::CQL2_CLASS_ADVANCED_COMPARISON_OPERATORS,
        filter::CQL2_CLASS_SPATIAL_FUNCTIONS,
        filter::CQL2_CLASS_TEMPORAL_FUNCTIONS,
    ];

    /// Mimics the Iceberg driver's own declared set: Basic CQL2 plus both
    /// encodings only.
    const WEAK_DRIVER_CLASSES: &[&str] = &[
        filter::CQL2_CLASS_BASIC,
        filter::CQL2_CLASS_CQL2_TEXT,
        filter::CQL2_CLASS_CQL2_JSON,
    ];

    struct ClassesDriver {
        classes: &'static [&'static str],
    }

    impl StorageDriver for ClassesDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![physical("demo")]))
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(FakeClassesFeature {
                classes: self.classes,
            }) as Arc<dyn FeatureSource>)
        }
    }

    struct ClassesFactory {
        name: &'static str,
        classes: &'static [&'static str],
    }

    impl DriverFactory for ClassesFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(ClassesDriver {
                classes: self.classes,
            }))
        }
    }

    /// A driver with no `FeatureSource` at all (`feature_source` stays at
    /// the trait default `None`) — the tile/raster-only archive shape
    /// (`Router::cql2_conformance_classes`'s own doc) that must never
    /// narrow the intersection just by being configured.
    struct NoFeatureDriver;

    impl StorageDriver for NoFeatureDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![physical("demo")]))
        }
    }

    struct NoFeatureFactory;

    impl DriverFactory for NoFeatureFactory {
        fn name(&self) -> &str {
            "no-feature"
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(NoFeatureDriver))
        }
    }

    /// Builds a config declaring one storage per `(id, driver)` pair, no
    /// collections — `Router::cql2_conformance_classes` reads `self.drivers`
    /// directly (`#105`'s own "in use means declared" convention), so no
    /// collection needs to route to any of them for this method's own tests.
    fn config_with_storages(storages: &[(&str, &str)]) -> (AppConfig, Registry) {
        let mut yaml = "storages:\n".to_string();
        for (id, driver) in storages {
            yaml.push_str(&format!(
                "  - {{ id: {id}, driver: {driver}, url_env: DATABASE_URL }}\n"
            ));
        }
        yaml.push_str(
            "tenants: [ { id: public } ]\ncatalogs: [ { id: default, tenant: public } ]\n",
        );
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(ClassesFactory {
            name: "strong",
            classes: STRONG_DRIVER_CLASSES,
        }));
        registry.register(Arc::new(ClassesFactory {
            name: "weak",
            classes: WEAK_DRIVER_CLASSES,
        }));
        registry.register(Arc::new(NoFeatureFactory));
        (config, registry)
    }

    /// Zero features-capable drivers means no CQL2 evaluator exists anywhere
    /// in the deployment, so none of the driver-honoured seed may survive.
    #[test]
    fn cql2_conformance_classes_is_empty_with_no_features_capable_driver() {
        let (config, registry) = config_with_storages(&[("archive", "no-feature")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.cql2_conformance_classes().is_empty());
    }

    /// A PostGIS-only deployment re-earns the full set at the workspace
    /// level too — not just the per-collection surface — because the
    /// intersection over one driver is that driver's own set.
    #[test]
    fn cql2_conformance_classes_reearns_the_full_set_when_the_only_driver_is_strong() {
        let (config, registry) = config_with_storages(&[("main", "strong")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.cql2_conformance_classes(),
            STRONG_DRIVER_CLASSES.to_vec()
        );
    }

    /// A mixed deployment (a PostGIS-strength driver alongside an
    /// Iceberg-strength one) narrows to exactly what both earn — the three
    /// richer classes the strong driver alone would re-earn stay withheld
    /// workspace-wide, honoring the weakest configured driver.
    #[test]
    fn cql2_conformance_classes_narrows_to_the_intersection_across_mixed_drivers() {
        let (config, registry) = config_with_storages(&[("main", "strong"), ("archival", "weak")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.cql2_conformance_classes(),
            WEAK_DRIVER_CLASSES.to_vec()
        );
    }

    /// A tile/raster-only archive driver configured alongside a strong
    /// features driver never narrows the intersection — it has no
    /// `FeatureSource` to consult at all, so it is simply skipped.
    #[test]
    fn cql2_conformance_classes_ignores_a_driver_with_no_feature_source() {
        let (config, registry) =
            config_with_storages(&[("main", "strong"), ("archive", "no-feature")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.cql2_conformance_classes(),
            STRONG_DRIVER_CLASSES.to_vec()
        );
    }

    /// `case-insensitive-comparison` never appears regardless of driver mix
    /// — no driver's own `cql2_conformance_classes` ever declares it (see
    /// `filter::CQL2_CONFORMANCE_CLASSES`'s own doc), so it can never survive
    /// into the intersection either.
    #[test]
    fn cql2_conformance_classes_never_includes_case_insensitive_comparison() {
        let (config, registry) = config_with_storages(&[("main", "strong"), ("archival", "weak")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(!router
            .cql2_conformance_classes()
            .contains(&filter::CQL2_CLASS_CASE_INSENSITIVE_COMPARISON));
    }

    // -- `Router::locking_conformance_classes` (`#107`) ----------------------

    struct LockingWriteSink {
        classes: &'static [&'static str],
    }

    #[async_trait::async_trait]
    impl WriteSink for LockingWriteSink {
        async fn apply(
            &self,
            _collection: &CollectionDecl,
            _mutation: crate::outbox::Mutation,
        ) -> Result<crate::outbox::Sequence> {
            unreachable!("not exercised by the locking_conformance_classes fold tests")
        }

        fn locking_conformance_classes(&self) -> Vec<&'static str> {
            self.classes.to_vec()
        }

        fn update_conformance_classes(&self) -> Vec<&'static str> {
            vec![crate::outbox::UPDATE_CONFORMANCE_CLASS]
        }
    }

    /// A write-capable driver, with a `feature_source` (`has_feature_source:
    /// true`) or deliberately without one (`false`, the structural gap
    /// `Router::locking_conformance_classes`'s own doc says must zero the
    /// fold — a write-only driver can never satisfy the guard, which reads
    /// current state through `FeatureSource::item` first).
    struct LockingFoldDriver {
        write_classes: &'static [&'static str],
        has_feature_source: bool,
    }

    impl StorageDriver for LockingFoldDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![physical("demo")]))
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            self.has_feature_source
                .then(|| Arc::new(FakeClassesFeature { classes: &[] }) as Arc<dyn FeatureSource>)
        }

        fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
            Some(Arc::new(LockingWriteSink {
                classes: self.write_classes,
            }) as Arc<dyn WriteSink>)
        }
    }

    struct LockingFoldFactory {
        name: &'static str,
        write_classes: &'static [&'static str],
        has_feature_source: bool,
    }

    impl DriverFactory for LockingFoldFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(LockingFoldDriver {
                write_classes: self.write_classes,
                has_feature_source: self.has_feature_source,
            }))
        }
    }

    fn config_with_locking_storages(storages: &[(&str, &str)]) -> (AppConfig, Registry) {
        let mut yaml = "storages:\n".to_string();
        for (id, driver) in storages {
            yaml.push_str(&format!(
                "  - {{ id: {id}, driver: {driver}, url_env: DATABASE_URL }}\n"
            ));
        }
        yaml.push_str(
            "tenants: [ { id: public } ]\ncatalogs: [ { id: default, tenant: public } ]\n",
        );
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(LockingFoldFactory {
            name: "locking-strong",
            write_classes: &[locking::OPTIMISTIC_LOCKING_ETAGS_CLASS],
            has_feature_source: true,
        }));
        registry.register(Arc::new(LockingFoldFactory {
            name: "locking-weak",
            write_classes: &[],
            has_feature_source: true,
        }));
        registry.register(Arc::new(LockingFoldFactory {
            name: "locking-write-only",
            write_classes: &[locking::OPTIMISTIC_LOCKING_ETAGS_CLASS],
            has_feature_source: false,
        }));
        registry.register(Arc::new(NoFeatureFactory));
        (config, registry)
    }

    /// Zero write-capable drivers means no optimistic-locking precondition can
    /// be honoured anywhere in the deployment, so the seed must not survive.
    #[test]
    fn locking_conformance_classes_is_empty_with_no_write_capable_driver() {
        let (config, registry) = config_with_locking_storages(&[("archive", "no-feature")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.locking_conformance_classes().is_empty());
    }

    /// One write+feature-capable driver declaring the ETags class re-earns
    /// the full seed at the workspace level.
    #[test]
    fn locking_conformance_classes_reearns_the_full_set_when_the_only_driver_declares_it() {
        let (config, registry) = config_with_locking_storages(&[("main", "locking-strong")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.locking_conformance_classes(),
            vec![locking::OPTIMISTIC_LOCKING_ETAGS_CLASS]
        );
    }

    /// A mixed deployment (one driver declares the class, another
    /// write-capable driver doesn't) narrows to empty — the honest
    /// intersection, exactly like CQL2's own mixed-driver test.
    #[test]
    fn locking_conformance_classes_narrows_to_empty_when_a_write_capable_driver_declares_nothing() {
        let (config, registry) =
            config_with_locking_storages(&[("main", "locking-strong"), ("weak", "locking-weak")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.locking_conformance_classes().is_empty());
    }

    /// A write-capable driver with NO read lane at all can never satisfy the
    /// guard (it needs `FeatureSource::item` to read current state before
    /// comparing) — this must zero the fold workspace-wide even though this
    /// same driver's own declared set names the class, and even alongside
    /// another driver that both declares the class AND has a read lane.
    #[test]
    fn locking_conformance_classes_is_empty_when_a_write_capable_driver_has_no_feature_source() {
        let (config, registry) = config_with_locking_storages(&[
            ("main", "locking-strong"),
            ("archive", "locking-write-only"),
        ]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.locking_conformance_classes().is_empty());
    }

    /// A driver with no write lane at all (a tile/raster-only archive) is
    /// simply skipped — it has nothing to say about this class either way,
    /// so it must never narrow the fold just by being configured.
    #[test]
    fn locking_conformance_classes_ignores_a_driver_with_no_write_sink_at_all() {
        let (config, registry) =
            config_with_locking_storages(&[("main", "locking-strong"), ("archive", "no-feature")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.locking_conformance_classes(),
            vec![locking::OPTIMISTIC_LOCKING_ETAGS_CLASS]
        );
    }

    fn update_config(collection: Option<&str>) -> (AppConfig, Registry) {
        let collections = collection.unwrap_or("");
        let yaml = format!(
            "storages:\n  - {{ id: read, driver: locking-strong, url_env: DATABASE_URL }}\n  - {{ id: write, driver: locking-strong, url_env: DATABASE_URL }}\ntenants: [ {{ id: public }} ]\ncatalogs: [ {{ id: default, tenant: public }} ]\n{collections}"
        );
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        let (_, registry) = config_with_locking_storages(&[]);
        (config, registry)
    }

    #[test]
    fn update_conformance_is_withheld_when_no_collection_uses_a_write_driver() {
        let (config, registry) = update_config(None);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.update_conformance_classes().is_empty());
    }

    #[test]
    fn update_conformance_requires_the_same_single_storage_for_read_and_write() {
        let collection = "collections:\n  - id: demo\n    catalog: default\n    storage: read\n    table: demo\n    geometry: geom\n    pk: id\n    routing: { features: read, write: write }\n";
        let (config, registry) = update_config(Some(collection));
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.update_conformance_classes().is_empty());
    }

    #[test]
    fn update_conformance_is_declared_for_a_same_storage_read_write_pair() {
        let collection = "collections:\n  - id: demo\n    catalog: default\n    storage: write\n    table: demo\n    geometry: geom\n    pk: id\n    routing: { write: write }\n";
        let (config, registry) = update_config(Some(collection));
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.update_conformance_classes(),
            vec![crate::outbox::UPDATE_CONFORMANCE_CLASS]
        );
    }

    // -- `Router::crs_conformance_classes` / `filtering_conformance_classes`
    // (`#217`) -------------------------------------------------------------

    /// A `FeatureSource` whose only purpose is answering the two capability
    /// bools Part 2 and Part 3 fold over — `items`/`item` are never called by
    /// the tests below, so they stay unreachable stubs, exactly like
    /// `FakeClassesFeature` above.
    struct FakeCapableFeature {
        crs_capable: bool,
        filter_capable: bool,
        filter_crs_capable: bool,
    }

    #[async_trait::async_trait]
    impl FeatureSource for FakeCapableFeature {
        async fn items(&self, _: &CollectionDecl, _: &ItemsQuery) -> Result<FeaturePage> {
            unreachable!("not exercised by the crs/filtering conformance fold tests")
        }

        async fn item(
            &self,
            _: &CollectionDecl,
            _: &str,
            _: Option<&Filter>,
        ) -> Result<Option<serde_json::Value>> {
            unreachable!("not exercised by the crs/filtering conformance fold tests")
        }

        fn crs_capable(&self) -> bool {
            self.crs_capable
        }

        fn filter_capable(&self) -> bool {
            self.filter_capable
        }

        fn filter_crs_capable(&self) -> bool {
            self.filter_crs_capable
        }
    }

    struct CapableDriver {
        crs_capable: bool,
        filter_capable: bool,
        filter_crs_capable: bool,
    }

    impl StorageDriver for CapableDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![physical("demo")]))
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(FakeCapableFeature {
                crs_capable: self.crs_capable,
                filter_capable: self.filter_capable,
                filter_crs_capable: self.filter_crs_capable,
            }) as Arc<dyn FeatureSource>)
        }
    }

    struct CapableFactory {
        name: &'static str,
        crs_capable: bool,
        filter_capable: bool,
        filter_crs_capable: bool,
    }

    impl DriverFactory for CapableFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(CapableDriver {
                crs_capable: self.crs_capable,
                filter_capable: self.filter_capable,
                filter_crs_capable: self.filter_crs_capable,
            }))
        }
    }

    /// Same "no collection needs to route anywhere" shape as
    /// `config_with_storages`, with the three driver shapes this workspace
    /// really ships — PostGIS (reprojects, filters, honours `filter-crs`),
    /// GeoPackage (filters, never reprojects), and FlatGeobuf/GeoParquet/
    /// memory (neither) — plus `postgis-like-without-filter-crs`, which is
    /// PostGIS exactly as it stood before `#217`: reprojecting output, so
    /// Part 3 Requirement 8's "Server supports additional coordinate
    /// reference systems" condition fires, while `filter-crs` itself is
    /// inert. That shape is not a hypothetical — it is the overclaim this
    /// issue was opened for, kept here so the fold that closes it has
    /// something that can actually fail.
    fn config_with_capable_storages(storages: &[(&str, &str)]) -> (AppConfig, Registry) {
        let mut yaml = "storages:\n".to_string();
        for (id, driver) in storages {
            yaml.push_str(&format!(
                "  - {{ id: {id}, driver: {driver}, url_env: DATABASE_URL }}\n"
            ));
        }
        yaml.push_str(
            "tenants: [ { id: public } ]\ncatalogs: [ { id: default, tenant: public } ]\n",
        );
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(CapableFactory {
            name: "postgis-like",
            crs_capable: true,
            filter_capable: true,
            filter_crs_capable: true,
        }));
        registry.register(Arc::new(CapableFactory {
            name: "postgis-like-without-filter-crs",
            crs_capable: true,
            filter_capable: true,
            filter_crs_capable: false,
        }));
        registry.register(Arc::new(CapableFactory {
            name: "geopackage-like",
            crs_capable: false,
            filter_capable: true,
            filter_crs_capable: false,
        }));
        registry.register(Arc::new(CapableFactory {
            name: "flatgeobuf-like",
            crs_capable: false,
            filter_capable: false,
            filter_crs_capable: false,
        }));
        registry.register(Arc::new(NoFeatureFactory));
        (config, registry)
    }

    /// With no features-capable driver configured at all there is nothing
    /// that could honour Part 2, so the class is withheld, just as the CQL2
    /// fold withholds its driver-honoured classes in the same situation.
    #[test]
    fn crs_conformance_classes_is_empty_with_no_features_capable_driver() {
        let (config, registry) = config_with_capable_storages(&[("archive", "no-feature")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.crs_conformance_classes().is_empty());
    }

    #[test]
    fn crs_conformance_classes_declares_the_class_when_every_driver_can_reproject() {
        let (config, registry) = config_with_capable_storages(&[("main", "postgis-like")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.crs_conformance_classes(),
            vec![crate::crs::CRS_CONFORMANCE_CLASS]
        );
    }

    /// The overclaim `#217` names: a deployment where any routed features
    /// driver cannot reproject offers exactly one CRS on that collection —
    /// whichever one it is actually served in (`#227`) — so the
    /// deployment-wide Part 2 claim has to go.
    #[test]
    fn crs_conformance_classes_is_empty_when_any_driver_cannot_reproject() {
        let (config, registry) =
            config_with_capable_storages(&[("main", "postgis-like"), ("gpkg", "geopackage-like")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.crs_conformance_classes().is_empty());
    }

    /// A tile/raster-only archive has no `FeatureSource` to consult, so it
    /// never narrows this fold either — the same skip CQL2's own
    /// `cql2_conformance_classes_ignores_a_driver_with_no_feature_source`
    /// pins.
    #[test]
    fn crs_conformance_classes_ignores_a_driver_with_no_feature_source() {
        let (config, registry) =
            config_with_capable_storages(&[("main", "postgis-like"), ("archive", "no-feature")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.crs_conformance_classes(),
            vec![crate::crs::CRS_CONFORMANCE_CLASS]
        );
    }

    #[test]
    fn filtering_conformance_classes_is_empty_with_no_features_capable_driver() {
        let (config, registry) = config_with_capable_storages(&[("archive", "no-feature")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.filtering_conformance_classes().is_empty());
    }

    /// A driver that cannot reproject still filters perfectly well, and owes
    /// nothing to Requirement 8: not being `crs_capable` is exactly the case
    /// where that requirement's condition never fires, so Requirement 7's
    /// CRS84 default — which its compiler already implements — is the whole
    /// obligation. GeoPackage alongside PostGIS therefore keeps all three
    /// classes.
    #[test]
    fn filtering_conformance_classes_declares_every_class_when_every_driver_filters() {
        let (config, registry) =
            config_with_capable_storages(&[("main", "postgis-like"), ("gpkg", "geopackage-like")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.filtering_conformance_classes(),
            filter::FILTERING_CONFORMANCE_CLASSES.to_vec()
        );
    }

    /// The overclaim `#217` names for Part 3: one configured driver that
    /// answers 400 to any `filter` (FlatGeobuf/GeoParquet/memory) withdraws
    /// the deployment-wide claim, however capable the rest are.
    #[test]
    fn filtering_conformance_classes_is_empty_when_any_driver_refuses_filter() {
        let (config, registry) =
            config_with_capable_storages(&[("main", "postgis-like"), ("fgb", "flatgeobuf-like")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.filtering_conformance_classes().is_empty());
    }

    #[test]
    fn filtering_conformance_classes_ignores_a_driver_with_no_feature_source() {
        let (config, registry) =
            config_with_capable_storages(&[("main", "postgis-like"), ("archive", "no-feature")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.filtering_conformance_classes(),
            filter::FILTERING_CONFORMANCE_CLASSES.to_vec()
        );
    }

    /// `conf/queryables` is served for every collection regardless of driver,
    /// so it must never ride this fold — a deployment that folds Part 3 away
    /// still serves the queryables document, and still declares that class
    /// from `tellurion-features`' static list.
    #[test]
    fn filtering_conformance_classes_never_folds_the_always_served_queryables_class() {
        const QUERYABLES_CLASS: &str =
            "http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/queryables";
        assert!(!filter::FILTERING_CONFORMANCE_CLASSES.contains(&QUERYABLES_CLASS));
        let (config, registry) = config_with_capable_storages(&[("main", "postgis-like")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(!router
            .filtering_conformance_classes()
            .contains(&QUERYABLES_CLASS));
    }

    /// The residual overclaim `#217` was reopened for, pinned: a driver that
    /// reprojects output geometry makes Part 3 Requirement 8
    /// (`/req/filter/filter-crs-param`) binding on itself by satisfying its
    /// "Server supports additional coordinate reference systems" condition.
    /// If it then cannot process a filter's geometries in the CRS a
    /// `filter-crs` names, it does not conform, and the deployment must not
    /// advertise Part 3 — however happily it accepts a `filter`.
    #[test]
    fn filtering_conformance_classes_is_empty_when_a_reprojecting_driver_ignores_filter_crs() {
        let (config, registry) =
            config_with_capable_storages(&[("main", "postgis-like-without-filter-crs")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(
            router.filtering_conformance_classes().is_empty(),
            "a crs_capable driver that cannot honour filter-crs fails Part 3 Requirement 8,              so the deployment may not advertise the Filtering classes"
        );
    }

    /// The same driver's Part 2 claim is untouched: it really does reproject
    /// output geometry, which is all Part 2 asks. The two families are folded
    /// separately precisely so one can be honest while the other is not —
    /// the exact state `#217` found the workspace in.
    #[test]
    fn a_driver_that_ignores_filter_crs_still_earns_the_crs_conformance_class() {
        let (config, registry) =
            config_with_capable_storages(&[("main", "postgis-like-without-filter-crs")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.crs_conformance_classes(),
            vec![crate::crs::CRS_CONFORMANCE_CLASS]
        );
        assert!(router.filtering_conformance_classes().is_empty());
    }

    /// One driver failing Requirement 8 zeroes the fold for the whole
    /// deployment, exactly as one driver refusing `filter` already does — a
    /// client reading `/conformance` off the protocol root cannot know which
    /// collection its next request lands on.
    #[test]
    fn filtering_conformance_classes_is_empty_when_any_driver_ignores_filter_crs() {
        let (config, registry) = config_with_capable_storages(&[
            ("main", "postgis-like"),
            ("legacy", "postgis-like-without-filter-crs"),
        ]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.filtering_conformance_classes().is_empty());
    }

    // -- STAC Item Search Filter (`#248`) ----------------------------------

    /// The overclaim `#248` closes on the STAC root: the Item Search Filter
    /// class binds *Filter and Basic CQL2* to `/search`, and a deployment
    /// whose only driver answers 400 to every `filter` can honour neither. It
    /// was declared unconditionally by `tellurion-stac`'s static list until
    /// this fold existed.
    #[test]
    fn item_search_filter_conformance_classes_is_empty_when_any_driver_refuses_filter() {
        let (config, registry) =
            config_with_capable_storages(&[("main", "postgis-like"), ("fgb", "flatgeobuf-like")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.item_search_filter_conformance_classes().is_empty());
    }

    #[test]
    fn item_search_filter_conformance_classes_is_empty_with_no_features_capable_driver() {
        let (config, registry) = config_with_capable_storages(&[("archive", "no-feature")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.item_search_filter_conformance_classes().is_empty());
    }

    #[test]
    fn item_search_filter_conformance_classes_ignores_a_driver_with_no_feature_source() {
        let (config, registry) =
            config_with_capable_storages(&[("main", "postgis-like"), ("archive", "no-feature")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.item_search_filter_conformance_classes(),
            filter::ITEM_SEARCH_FILTER_CONFORMANCE_CLASSES.to_vec()
        );
    }

    /// The decisive difference from `filtering_conformance_classes` above,
    /// pinned: Part 3's Requirement 8 condition does NOT ride this fold. The
    /// STAC Filter Extension pins `filter-crs` to CRS84 — "server must only
    /// accept `http://www.opengis.net/def/crs/OGC/1.3/CRS84` as a valid value,
    /// may reject any others" — so there is no client-nameable CRS on
    /// `/search` a driver could fail to transform into, and a driver that
    /// reprojects output without honouring a Part 3 `filter-crs` still serves
    /// the STAC lane completely. That is the same fixture the Features fold
    /// folds away, kept here answering the opposite way on purpose.
    #[test]
    fn a_driver_that_ignores_part_3_filter_crs_still_earns_the_item_search_filter_class() {
        let (config, registry) =
            config_with_capable_storages(&[("main", "postgis-like-without-filter-crs")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.filtering_conformance_classes().is_empty());
        assert_eq!(
            router.item_search_filter_conformance_classes(),
            filter::ITEM_SEARCH_FILTER_CONFORMANCE_CLASSES.to_vec()
        );
    }

    /// A GeoPackage-shaped deployment — filters, never reprojects — keeps the
    /// class, which is what the live demos in `scripts/` are and what the
    /// `#248` contract smoke asserts over real HTTP.
    #[test]
    fn item_search_filter_conformance_classes_declares_the_class_for_a_filtering_deployment() {
        let (config, registry) =
            config_with_capable_storages(&[("main", "postgis-like"), ("gpkg", "geopackage-like")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.item_search_filter_conformance_classes(),
            filter::ITEM_SEARCH_FILTER_CONFORMANCE_CLASSES.to_vec()
        );
    }

    /// `#86`: the maps lane defaults to the single `storage` exactly like
    /// `tiles` when `routing.maps` is omitted.
    #[tokio::test]
    async fn resolve_maps_defaults_to_the_single_storage_when_unrouted() {
        let (config, registry) = config_with(true, true);
        let router = Router::build(&config, &registry).unwrap();
        let (decl, _source) = router
            .resolve_maps("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.id, "demo");
    }

    /// `#86`: a maps-lane resolution failure names the `"maps"` capability,
    /// not `"tiles"` — the reason `maps_source` exists as its own function
    /// rather than reusing `tiles_source` directly (see its own doc).
    #[tokio::test]
    async fn resolve_maps_refuses_capability_the_driver_lacks() {
        let (config, registry) = config_with(true, false);
        let router = Router::build(&config, &registry).unwrap();
        match router.resolve_maps("public", "default", "demo").await {
            Err(Error::CapabilityUnsupported { capability, .. }) => {
                assert_eq!(capability, "maps");
            }
            other => panic!("expected CapabilityUnsupported, got {}", other.is_ok()),
        }
    }

    #[tokio::test]
    async fn unknown_collection_is_not_found() {
        let (config, registry) = config_with(true, true);
        let router = Router::build(&config, &registry).unwrap();
        assert!(matches!(
            router
                .resolve_features("public", "default", "missing")
                .await,
            Err(Error::NotFound)
        ));
    }

    #[tokio::test]
    async fn unknown_tenant_is_not_found() {
        let (config, registry) = config_with(true, true);
        let router = Router::build(&config, &registry).unwrap();
        assert!(matches!(
            router
                .resolve_features("other-tenant", "default", "demo")
                .await,
            Err(Error::NotFound)
        ));
    }

    /// No-regression check for `#21`: a collection with no `routing` block
    /// resolves both lanes to the collection's single `storage`, exactly as
    /// it did before lanes existed — same decl, same driver, boot validation
    /// still passes.
    #[tokio::test]
    async fn a_collection_without_routing_behaves_exactly_like_before_lanes_existed() {
        let (config, registry) = config_with(true, true);
        let router = Router::build(&config, &registry).unwrap();
        router.validate_catalog().await.unwrap();

        let (features_decl, _features_source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        let (tiles_decl, _tiles_source) = router
            .resolve_tiles("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            features_decl, tiles_decl,
            "both lanes resolve to the same effective decl when routing is omitted"
        );
        assert_eq!(features_decl.storage, "main");
    }

    /// `#39`: a collection that leaves `tiles.caps` unset inherits its
    /// catalog's `settings.tile_caps` onto the decl `resolve_tiles` actually
    /// hands a driver — the settings chain's one real runtime consumer.
    #[tokio::test]
    async fn resolve_tiles_carries_the_catalogs_inherited_tile_caps_onto_the_served_decl() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs:
  - id: default
    tenant: public
    settings: { tile_caps: { z0: 500 } }
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
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            features: true,
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let (decl, _source) = router
            .resolve_tiles("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.tiles.caps.get(0), Some(500));

        let effective = router.effective_settings("demo").unwrap();
        assert_eq!(effective.tile_caps.get(0), Some(500));
        // `#110`: the effective-config view's provenance must agree with
        // the value request lanes actually received just above — inherited
        // from the catalog, not the collection.
        assert_eq!(
            router
                .effective_settings_provenance("demo")
                .unwrap()
                .tile_caps,
            settings::SettingsProvenance::Declared {
                level: settings::SettingsLevel::Catalog
            }
        );
    }

    /// `#185`: the exposure matrix is materialized per CATALOG, at load
    /// time, through the same nearest-level-wins chain — the catalog's own
    /// block wins over the tenant's, and a catalog that declares none
    /// inherits whatever its ancestors said.
    #[tokio::test]
    async fn catalog_protocols_materializes_the_exposure_matrix_per_catalog() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
settings: { protocols: { stac: disabled } }
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants:
  - id: public
    settings: { protocols: { tiles: disabled } }
catalogs:
  - id: default
    tenant: public
  - id: locked
    tenant: public
    settings: { protocols: { features_write: disabled } }
collections: []
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            features: true,
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        // Catalog with no block of its own: the tenant's whole block shows
        // through — including the platform's `stac: disabled` NOT showing
        // through, since a nearer level replaced the value outright.
        let default = router.catalog_protocols("default").unwrap();
        assert_eq!(default.tiles, ProtocolExposure::Disabled);
        assert_eq!(default.stac, ProtocolExposure::Enabled);

        // Catalog with its own block: it replaces the tenant's whole value,
        // so `tiles` is exposed again here.
        let locked = router.catalog_protocols("locked").unwrap();
        assert_eq!(locked.features_write, ProtocolExposure::Disabled);
        assert_eq!(locked.features, ProtocolExposure::Enabled);
        assert_eq!(locked.tiles, ProtocolExposure::Enabled);

        // Built per catalog, not per collection: neither catalog above has a
        // single collection, and both still have a matrix.
        assert_eq!(router.catalog_protocols("nope"), None);
    }

    /// A collection's own explicit `tiles.caps` still wins outright over
    /// anything its catalog would otherwise contribute — inheritance only
    /// ever fills a gap, never overrides an explicit local value.
    #[tokio::test]
    async fn resolve_tiles_prefers_the_collections_own_explicit_tile_caps_over_the_catalogs() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs:
  - id: default
    tenant: public
    settings: { tile_caps: { z0: 500 } }
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: { caps: { z0: 42 } }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            features: true,
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let (decl, _source) = router
            .resolve_tiles("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.tiles.caps.get(0), Some(42));
        // `#110`: the materialized `EffectiveSettings` map — the same one
        // `apply_inherited_settings` overlays onto every decl a driver
        // receives — carries the identical value, tagged `Derived` since a
        // collection's own physical `tiles.caps` bypasses the settings
        // chain entirely rather than winning it at the `Collection` level.
        assert_eq!(
            router.effective_settings("demo").unwrap().tile_caps.get(0),
            Some(42)
        );
        assert_eq!(
            router
                .effective_settings_provenance("demo")
                .unwrap()
                .tile_caps,
            settings::SettingsProvenance::Derived
        );
    }

    /// `#110` anti-drift: the effective-config view is meant to read
    /// straight off `Router::effective_settings`/`effective_settings_
    /// provenance` — the same two maps `apply_inherited_settings` overlays
    /// onto every decl a request-lane driver receives (`tile_caps`,
    /// `colormap`, `tile_vertex_budget`) or a handler reads directly
    /// (`max_request_body_bytes` in `tellurion-features::write_handlers`,
    /// `max_asset_bytes`/`asset_media_types` in
    /// `tellurion-stac::asset_handlers`). This test exercises all four
    /// provenance shapes the view reports (`built-in default`, `derived`,
    /// `inherited` naming two different ancestor levels, `local override`)
    /// against one collection, cross-checking each resolved value against
    /// the identical map a real request consults — so a future resolver
    /// change that shifts real behavior without shifting the view (or vice
    /// versa) fails here first, not in production.
    #[tokio::test]
    async fn effective_settings_provenance_agrees_with_the_request_lanes_materialized_values() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs:
  - id: default
    tenant: public
    settings: { tile_caps: { z0: 500 }, max_request_body_bytes: 555, items_vertex_budget: 12345, page_max_bytes: 65536 }
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
        registry.register(Arc::new(FakeFactory {
            features: true,
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let effective = router.effective_settings("demo").unwrap();
        let provenance = router.effective_settings_provenance("demo").unwrap();

        // Derived: the collection's own physical `tiles.caps` overrides
        // whatever the settings chain (here, the catalog's `settings.
        // tile_caps`) would otherwise contribute.
        assert_eq!(effective.tile_caps.get(0), Some(42));
        assert_eq!(provenance.tile_caps, settings::SettingsProvenance::Derived);

        // Local override: the collection's own `settings.
        // tile_vertex_budget` wins outright over anything an ancestor
        // could contribute.
        assert_eq!(effective.tile_vertex_budget, 4242);
        assert_eq!(
            provenance.tile_vertex_budget,
            settings::SettingsProvenance::Declared {
                level: settings::SettingsLevel::Collection
            }
        );

        // Inherited from the catalog: nothing on the collection sets
        // `max_request_body_bytes`.
        assert_eq!(effective.max_request_body_bytes, 555);
        assert_eq!(
            provenance.max_request_body_bytes,
            settings::SettingsProvenance::Declared {
                level: settings::SettingsLevel::Catalog
            }
        );
        assert_eq!(effective.items_vertex_budget, 12345);
        assert_eq!(
            provenance.items_vertex_budget,
            settings::SettingsProvenance::Declared {
                level: settings::SettingsLevel::Catalog
            }
        );
        // `#184`: inherited the same way, but the effective value stays an
        // `Option` — there is no built-in default to materialize.
        assert_eq!(effective.page_max_bytes, Some(65536));
        assert_eq!(
            provenance.page_max_bytes,
            settings::SettingsProvenance::Declared {
                level: settings::SettingsLevel::Catalog
            }
        );

        // Inherited from the platform: nothing below it sets
        // `slow_request_ms` — provenance must name the correct ancestor
        // level, not just "some ancestor," which is why this test also
        // exercises `max_request_body_bytes` above (a different level).
        assert_eq!(effective.slow_request_ms, 9000);
        assert_eq!(
            provenance.slow_request_ms,
            settings::SettingsProvenance::Declared {
                level: settings::SettingsLevel::Platform
            }
        );

        // Built-in default: nothing in the chain ever declares
        // `cache_ttl_s`.
        assert_eq!(
            effective.cache_ttl_s,
            settings::DEFAULT_SETTINGS_CACHE_TTL_S
        );
        assert_eq!(
            provenance.cache_ttl_s,
            settings::SettingsProvenance::BuiltInDefault
        );

        // The decl a real `TileSource` driver receives carries the exact
        // same derived `tile_caps` value the view above reports.
        let (decl, _source) = router
            .resolve_tiles("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.tiles.caps.get(0), effective.tile_caps.get(0));

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.settings.items_vertex_budget, Some(12345));
        // `#184`: the handler reads the byte budget off the decl the same
        // way the vertex budget's decorator does — prove the overlay
        // carries it.
        assert_eq!(decl.settings.page_max_bytes, Some(65536));
    }

    /// `#90`: a collection that leaves `settings.tile_vertex_budget` unset
    /// inherits its catalog's onto the decl `resolve_tiles` hands a
    /// driver — the same overlay `resolve_tiles_carries_the_catalogs_
    /// inherited_tile_caps_onto_the_served_decl` proves for `tile_caps`,
    /// since a `TileSource` driver has no other way to see a resolved
    /// settings-chain value than through the decl itself.
    #[tokio::test]
    async fn resolve_tiles_carries_the_catalogs_inherited_tile_vertex_budget_onto_the_served_decl()
    {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs:
  - id: default
    tenant: public
    settings: { tile_vertex_budget: 12345 }
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
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            features: true,
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let (decl, _source) = router
            .resolve_tiles("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.settings.tile_vertex_budget, Some(12345));

        let effective = router.effective_settings("demo").unwrap();
        assert_eq!(effective.tile_vertex_budget, 12345);
    }

    /// A collection with nothing anywhere in the chain declaring
    /// `tile_vertex_budget` still gets an overlaid, concrete value —
    /// `settings::DEFAULT_TILE_VERTEX_BUDGET` — not a bare `None` a driver
    /// would have to guess a fallback for itself.
    #[tokio::test]
    async fn resolve_tiles_overlays_the_default_tile_vertex_budget_when_nothing_declares_one() {
        let config: AppConfig = serde_yaml::from_str(
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
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            features: true,
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let (decl, _source) = router
            .resolve_tiles("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            decl.settings.tile_vertex_budget,
            Some(crate::settings::DEFAULT_TILE_VERTEX_BUDGET)
        );
    }

    #[test]
    fn build_fails_for_unregistered_driver_name() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: nonexistent, url_env: DATABASE_URL } ]
"#,
        )
        .unwrap();
        let registry = Registry::new();
        assert!(matches!(
            Router::build(&config, &registry),
            Err(Error::Config(_))
        ));
    }

    struct CapacityHintDriver {
        capacity: Option<usize>,
    }

    impl StorageDriver for CapacityHintDriver {
        // `two_storage_config` (below) declares no collections, so this is
        // never actually queried — present only to satisfy the trait.
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![]))
        }

        fn capacity_hint(&self) -> Option<usize> {
            self.capacity
        }
    }

    struct CapacityHintFactory {
        name: String,
        capacity: Option<usize>,
    }

    impl DriverFactory for CapacityHintFactory {
        fn name(&self) -> &str {
            &self.name
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(CapacityHintDriver {
                capacity: self.capacity,
            }))
        }
    }

    fn two_storage_config() -> AppConfig {
        serde_yaml::from_str(
            r#"
storages:
  - { id: main, driver: fake-a, url_env: DATABASE_URL }
  - { id: secondary, driver: fake-b, url_env: DATABASE_URL2 }
"#,
        )
        .unwrap()
    }

    #[test]
    fn total_capacity_hint_sums_across_storages_when_every_driver_reports_one() {
        let config = two_storage_config();
        let mut registry = Registry::new();
        registry.register(Arc::new(CapacityHintFactory {
            name: "fake-a".to_string(),
            capacity: Some(8),
        }));
        registry.register(Arc::new(CapacityHintFactory {
            name: "fake-b".to_string(),
            capacity: Some(16),
        }));

        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(router.total_capacity_hint(), Some(24));
    }

    #[test]
    fn total_capacity_hint_is_none_when_any_driver_has_no_opinion() {
        let config = two_storage_config();
        let mut registry = Registry::new();
        registry.register(Arc::new(CapacityHintFactory {
            name: "fake-a".to_string(),
            capacity: Some(8),
        }));
        registry.register(Arc::new(CapacityHintFactory {
            name: "fake-b".to_string(),
            capacity: None,
        }));

        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(router.total_capacity_hint(), None);
    }

    #[test]
    fn total_capacity_hint_is_none_with_no_storages() {
        let config: AppConfig = serde_yaml::from_str("storages: []").unwrap();
        let registry = Registry::new();
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(router.total_capacity_hint(), None);
    }

    /// A driver whose reported catalog and tile capability are both
    /// configurable, purpose-built for `validate_catalog` tests.
    struct CatalogDriver {
        tables: Vec<String>,
        tiles: bool,
    }

    impl StorageDriver for CatalogDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(
                self.tables.iter().map(|t| physical(t)).collect(),
            ))
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            self.tiles
                .then(|| Arc::new(FakeTilesOnlyDriver) as Arc<dyn TileSource>)
        }
    }

    struct CatalogFactory {
        tables: Vec<String>,
        tiles: bool,
    }

    impl DriverFactory for CatalogFactory {
        fn name(&self) -> &str {
            "catalog-fake"
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(CatalogDriver {
                tables: self.tables.clone(),
                tiles: self.tiles,
            }))
        }
    }

    fn catalog_config(places3d: bool) -> AppConfig {
        let places3d_yaml = if places3d {
            "\n    places3d: { height_property: height }"
        } else {
            ""
        };
        serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: catalog-fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id{places3d_yaml}
"#
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn validate_catalog_passes_when_the_table_is_reported() {
        let config = catalog_config(false);
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(CatalogFactory {
            tables: vec!["demo".to_string()],
            tiles: false,
        }));
        let router = Router::build(&config, &registry).unwrap();
        router.validate_catalog().await.unwrap();
    }

    #[tokio::test]
    async fn validate_catalog_fails_fast_when_the_table_is_absent() {
        let config = catalog_config(false);
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(CatalogFactory {
            tables: vec!["other_table".to_string()],
            tiles: false,
        }));
        let router = Router::build(&config, &registry).unwrap();
        match router.validate_catalog().await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_catalog_fails_fast_when_places3d_needs_tiles_the_driver_lacks() {
        let config = catalog_config(true);
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(CatalogFactory {
            tables: vec!["demo".to_string()],
            tiles: false,
        }));
        let router = Router::build(&config, &registry).unwrap();
        match router.validate_catalog().await {
            Err(Error::Config(message)) => {
                assert!(message.contains("places3d"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_catalog_passes_when_places3d_and_tiles_are_both_present() {
        let config = catalog_config(true);
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(CatalogFactory {
            tables: vec!["demo".to_string()],
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();
        router.validate_catalog().await.unwrap();
    }

    /// The `#20` proof at the contract level: a driver shaped like PMTiles —
    /// `CatalogSource` + `TileSource`, never `FeatureSource`, and physical
    /// rows with no geometry column or primary key at all (`CatalogDriver`
    /// reports `physical(name)`, which leaves both `None` — see above). A
    /// table-shaped driver (postgis) would fail `validate_catalog` here
    /// (`validate_catalog_fails_fast_when_a_physical_field_cannot_be_derived`
    /// covers that); a tiles-only one must boot clean, and the decl handed
    /// to its `TileSource` carries `geometry`/`pk` as `None` rather than a
    /// fabricated value.
    #[tokio::test]
    async fn validate_catalog_passes_for_a_tiles_only_driver_that_never_reports_geometry_or_pk() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: catalog-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(CatalogFactory {
            tables: vec!["demo".to_string()],
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();
        router.validate_catalog().await.unwrap();

        let (decl, _source) = router
            .resolve_tiles("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            decl.geometry, None,
            "this driver never reports a geometry column"
        );
        assert_eq!(decl.pk, None, "this driver never reports a primary key");

        match router.resolve_features("public", "default", "demo").await {
            Err(Error::CapabilityUnsupported { capability, .. }) => {
                assert_eq!(capability, "features");
            }
            other => panic!("expected Err(CapabilityUnsupported), got {}", other.is_ok()),
        }
    }

    /// Hands out a `TaggedFeatureSource`/`TaggedTileSource` carrying `label`,
    /// so a per-lane routing test can assert exactly which storage a
    /// request reached even when two drivers otherwise look identical
    /// (`#21`), and can make a driver's calls fail on demand (`error`) to
    /// exercise the fallback tail.
    struct TaggedFeatureSource {
        label: &'static str,
        error: bool,
    }

    #[async_trait::async_trait]
    impl FeatureSource for TaggedFeatureSource {
        async fn items(
            &self,
            _collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> Result<FeaturePage> {
            if self.error {
                return Err(Error::Timeout);
            }
            Ok(FeaturePage {
                features_geojson: vec![serde_json::json!({ "storage": self.label })],
                number_matched: Some(1),
                next_token: None,
            })
        }

        async fn item(
            &self,
            _collection: &CollectionDecl,
            _id: &str,
            _filter: Option<&Filter>,
        ) -> Result<Option<serde_json::Value>> {
            if self.error {
                return Err(Error::Timeout);
            }
            Ok(Some(serde_json::json!({ "storage": self.label })))
        }
    }

    struct TaggedTileSource {
        label: &'static str,
        error: bool,
    }

    #[async_trait::async_trait]
    impl TileSource for TaggedTileSource {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&Filter>,
        ) -> Result<Option<Bytes>> {
            if self.error {
                return Err(Error::Timeout);
            }
            Ok(Some(Bytes::from(self.label.as_bytes().to_vec())))
        }
    }

    struct LaneFakeDriver {
        label: &'static str,
        features: bool,
        tiles: bool,
        error: bool,
    }

    impl StorageDriver for LaneFakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![]))
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            self.features.then(|| {
                Arc::new(TaggedFeatureSource {
                    label: self.label,
                    error: self.error,
                }) as Arc<dyn FeatureSource>
            })
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            self.tiles.then(|| {
                Arc::new(TaggedTileSource {
                    label: self.label,
                    error: self.error,
                }) as Arc<dyn TileSource>
            })
        }
    }

    struct LaneFakeFactory {
        name: &'static str,
        label: &'static str,
        features: bool,
        tiles: bool,
        error: bool,
    }

    impl DriverFactory for LaneFakeFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(LaneFakeDriver {
                label: self.label,
                features: self.features,
                tiles: self.tiles,
                error: self.error,
            }))
        }
    }

    #[tokio::test]
    async fn validate_catalog_fails_fast_when_an_explicit_routing_lane_names_a_storage_lacking_the_capability(
    ) {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: lane-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { tiles: main }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-fake",
            label: "main",
            features: true,
            tiles: false,
            error: false,
        }));
        let router = Router::build(&config, &registry).unwrap();

        match router.validate_catalog().await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("tiles"), "message was: {message}");
                assert!(message.contains("main"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// `#86`: an explicit `routing.maps` naming a storage whose driver
    /// never advertises `TileSource` (a raster-only driver, or one with
    /// neither capability) fails boot the same way an incapable `tiles`
    /// lane does above — naming the `maps` capability specifically, not
    /// `tiles`, so an operator reading the boot error knows which lane to
    /// fix.
    #[tokio::test]
    async fn validate_catalog_fails_fast_when_an_explicit_maps_routing_lane_names_a_storage_lacking_the_capability(
    ) {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: lane-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { maps: main }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-fake",
            label: "main",
            features: true,
            tiles: false,
            error: false,
        }));
        let router = Router::build(&config, &registry).unwrap();

        match router.validate_catalog().await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("maps"), "message was: {message}");
                assert!(message.contains("main"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn features_and_tiles_lanes_bind_to_different_drivers_and_each_request_reaches_the_right_one(
    ) {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: feat-store, driver: lane-features, url_env: DATABASE_URL }
  - { id: tile-store, driver: lane-tiles, url_env: DATABASE_URL2 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: feat-store
    table: demo
    geometry: geom
    pk: id
    routing:
      features: feat-store
      tiles: tile-store
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-features",
            label: "feat-store",
            features: true,
            tiles: false,
            error: false,
        }));
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-tiles",
            label: "tile-store",
            features: false,
            tiles: true,
            error: false,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let (decl, features_source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        let page = features_source
            .items(&decl, &ItemsQuery::default())
            .await
            .unwrap();
        assert_eq!(
            page.features_geojson[0]["storage"], "feat-store",
            "the features lane must reach feat-store's driver, not tile-store's"
        );

        let (decl, tiles_source) = router
            .resolve_tiles("public", "default", "demo")
            .await
            .unwrap();
        let tile = tiles_source
            .mvt_tile(&decl, TileCoord { z: 0, x: 0, y: 0 }, None)
            .await
            .unwrap();
        assert_eq!(
            tile.unwrap(),
            Bytes::from_static(b"tile-store"),
            "the tiles lane must reach tile-store's driver, not feat-store's"
        );
    }

    #[tokio::test]
    async fn fallback_tail_serves_when_the_primary_entry_errors() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: broken, driver: lane-broken, url_env: DATABASE_URL }
  - { id: good, driver: lane-good, url_env: DATABASE_URL2 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: broken
    table: demo
    geometry: geom
    pk: id
    routing: { features: [broken, good] }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-broken",
            label: "broken",
            features: true,
            tiles: false,
            error: true,
        }));
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-good",
            label: "good",
            features: true,
            tiles: false,
            error: false,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let (decl, source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        let page = source.items(&decl, &ItemsQuery::default()).await.unwrap();
        assert_eq!(
            page.features_geojson[0]["storage"], "good",
            "the tail entry must serve once the primary errors"
        );
    }

    // -- read-lane hints and `prefer:` (`#183`) ------------------------------

    /// Two feature-capable storages on one features lane, `main` configured
    /// as the primary; `broken_alt` swaps `alt` for a driver whose calls
    /// always error, to prove a preferred entry that fails still falls
    /// through to the configured chain instead of 404ing.
    fn prefer_fixture(broken_alt: bool) -> Router {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: main, driver: lane-main, url_env: DATABASE_URL }
  - { id: alt, driver: lane-alt, url_env: DATABASE_URL2 }
  - { id: unrouted, driver: lane-unrouted, url_env: DATABASE_URL3 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: { features: [main, alt] }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-main",
            label: "main",
            features: true,
            tiles: false,
            error: false,
        }));
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-alt",
            label: "alt",
            features: true,
            tiles: false,
            error: broken_alt,
        }));
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-unrouted",
            label: "unrouted",
            features: true,
            tiles: false,
            error: false,
        }));
        Router::build(&config, &registry).unwrap()
    }

    async fn served_storage(router: &Router, hints: &Hints) -> (String, Option<String>) {
        let (decl, source, served) = router
            .resolve_features_read("public", "default", "demo", hints)
            .await
            .unwrap();
        let page = source.items(&decl, &ItemsQuery::default()).await.unwrap();
        (
            page.features_geojson[0]["storage"]
                .as_str()
                .unwrap()
                .to_string(),
            served.storage_id().map(str::to_string),
        )
    }

    #[tokio::test]
    async fn no_hints_resolves_the_configured_primary_and_names_it_as_served() {
        let router = prefer_fixture(false);
        let (storage, served) = served_storage(&router, &Hints::none()).await;
        assert_eq!(storage, "main");
        assert_eq!(served.as_deref(), Some("main"));
    }

    #[tokio::test]
    async fn prefer_reorders_the_features_chain_to_serve_the_named_entry() {
        let router = prefer_fixture(false);
        let hints = Hints::parse(Some("prefer:alt"));
        let (storage, served) = served_storage(&router, &hints).await;
        assert_eq!(
            storage, "alt",
            "prefer:alt must move the tail entry in front of the configured primary"
        );
        assert_eq!(served.as_deref(), Some("alt"));
    }

    #[tokio::test]
    async fn a_preferred_entry_that_errors_falls_through_to_the_configured_primary() {
        let router = prefer_fixture(true);
        let hints = Hints::parse(Some("prefer:alt"));
        let (storage, served) = served_storage(&router, &hints).await;
        assert_eq!(
            storage, "main",
            "prefer reorders — the non-preferred entries must remain as the fallback tail"
        );
        assert_eq!(served.as_deref(), Some("main"));
    }

    /// `prefer:` may only *reorder* the resolved chain, never extend it: a
    /// storage that exists in the config (and even in the boot registry)
    /// but was never routed into this lane is exactly as inert as a name
    /// that exists nowhere at all.
    #[tokio::test]
    async fn prefer_never_extends_the_chain_and_unknown_names_are_no_ops() {
        let router = prefer_fixture(false);
        for hinted in ["prefer:unrouted", "prefer:nope", "bogus-token"] {
            let hints = Hints::parse(Some(hinted));
            let (storage, served) = served_storage(&router, &hints).await;
            assert_eq!(storage, "main", "hint '{hinted}' must be a harmless no-op");
            assert_eq!(served.as_deref(), Some("main"));
        }
    }

    /// The single-entry path stays wrapper-free (`#21` zero-overhead rule),
    /// so its served-source label is recorded at resolve time — only that
    /// entry can ever serve.
    #[tokio::test]
    async fn a_single_entry_lane_names_its_only_entry_as_served() {
        let (config, registry) = config_with(true, false);
        let router = Router::build(&config, &registry).unwrap();
        let (_, _, served) = router
            .resolve_features_read("public", "default", "demo", &Hints::none())
            .await
            .unwrap();
        assert_eq!(served.storage_id(), Some("main"));
    }

    /// Search-lane fixture for the `#183` prefer tests: `idx` advertises
    /// `SearchSource` (applied high-water `applied`), `main` is the
    /// feature-capable degraded tail, and the write lane's outbox reports a
    /// primary high-water of 5 — so `applied: 5` makes the index fresh
    /// under the default `freshness_bound: 0` and `applied: 3` makes it
    /// stale.
    struct SearchFixtureSearchSource {
        applied: u64,
    }

    #[async_trait::async_trait]
    impl SearchSource for SearchFixtureSearchSource {
        async fn search(
            &self,
            _collection: &CollectionDecl,
            _query: &crate::outbox::SearchQuery,
        ) -> Result<crate::outbox::SearchPage> {
            unreachable!("the resolve tests never issue an actual search call")
        }

        async fn applied_high_water(
            &self,
            _collection: &CollectionDecl,
        ) -> Result<crate::outbox::Sequence> {
            Ok(crate::outbox::Sequence(self.applied))
        }
    }

    struct SearchFixtureOutbox;

    #[async_trait::async_trait]
    impl OutboxSource for SearchFixtureOutbox {
        async fn read_after(
            &self,
            _collection: &CollectionDecl,
            _after: crate::outbox::Sequence,
            _limit: u32,
        ) -> Result<Vec<crate::outbox::Obligation>> {
            unreachable!("the resolve tests never drain the outbox")
        }

        async fn primary_high_water(
            &self,
            _collection: &CollectionDecl,
        ) -> Result<crate::outbox::Sequence> {
            Ok(crate::outbox::Sequence(5))
        }
    }

    struct SearchFixtureDriver {
        label: &'static str,
        search_applied: Option<u64>,
        features: bool,
        outbox: bool,
    }

    impl StorageDriver for SearchFixtureDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![]))
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            self.features.then(|| {
                Arc::new(TaggedFeatureSource {
                    label: self.label,
                    error: false,
                }) as Arc<dyn FeatureSource>
            })
        }

        fn search_source(&self) -> Option<Arc<dyn SearchSource>> {
            self.search_applied.map(|applied| {
                Arc::new(SearchFixtureSearchSource { applied }) as Arc<dyn SearchSource>
            })
        }

        fn outbox_source(&self) -> Option<Arc<dyn OutboxSource>> {
            self.outbox
                .then(|| Arc::new(SearchFixtureOutbox) as Arc<dyn OutboxSource>)
        }
    }

    struct SearchFixtureFactory {
        name: &'static str,
        label: &'static str,
        search_applied: Option<u64>,
        features: bool,
        outbox: bool,
    }

    impl DriverFactory for SearchFixtureFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(SearchFixtureDriver {
                label: self.label,
                search_applied: self.search_applied,
                features: self.features,
                outbox: self.outbox,
            }))
        }
    }

    fn search_prefer_fixture(applied: u64) -> Router {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: idx, driver: search-idx, url_env: DATABASE_URL }
  - { id: main, driver: search-main, url_env: DATABASE_URL2 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing:
      write: main
      index: idx
      search: [idx, main]
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(SearchFixtureFactory {
            name: "search-idx",
            label: "idx",
            search_applied: Some(applied),
            features: false,
            outbox: false,
        }));
        registry.register(Arc::new(SearchFixtureFactory {
            name: "search-main",
            label: "main",
            search_applied: None,
            features: true,
            outbox: true,
        }));
        Router::build(&config, &registry).unwrap()
    }

    #[tokio::test]
    async fn search_serves_the_fresh_index_and_names_it_when_unhinted() {
        let router = search_prefer_fixture(5);
        let (_, resolution, served) = router
            .resolve_search_read("public", "default", "demo", &Hints::none())
            .await
            .unwrap();
        assert!(
            matches!(resolution, SearchResolution::Index(_)),
            "a caught-up index must serve the unhinted read"
        );
        assert_eq!(served, "idx");
    }

    /// The operator's chain-divergence diagnostic (`#183`): `prefer:main`
    /// routes the read to the degraded feature tail even though the index
    /// is perfectly fresh — without a config edit and reload.
    #[tokio::test]
    async fn search_prefer_routes_past_a_fresh_index_to_the_degraded_tail() {
        let router = search_prefer_fixture(5);
        let hints = Hints::parse(Some("prefer:main"));
        let (_, resolution, served) = router
            .resolve_search_read("public", "default", "demo", &hints)
            .await
            .unwrap();
        assert!(matches!(resolution, SearchResolution::Fallback(_)));
        assert_eq!(served, "main");
    }

    /// Preferring the configured primary is the identity, and the stale-
    /// index fall-through (the pre-`#183` behavior) still names the tail
    /// entry that actually serves.
    #[tokio::test]
    async fn search_prefer_of_the_primary_is_the_identity_and_stale_still_falls_through() {
        let router = search_prefer_fixture(5);
        let hints = Hints::parse(Some("prefer:idx"));
        let (_, resolution, served) = router
            .resolve_search_read("public", "default", "demo", &hints)
            .await
            .unwrap();
        assert!(matches!(resolution, SearchResolution::Index(_)));
        assert_eq!(served, "idx");

        let stale = search_prefer_fixture(3);
        let (_, resolution, served) = stale
            .resolve_search_read("public", "default", "demo", &Hints::none())
            .await
            .unwrap();
        assert!(
            matches!(resolution, SearchResolution::Fallback(_)),
            "a lagging index must fall through to the degraded tail"
        );
        assert_eq!(served, "main");
    }

    /// A preferred stale index is still freshness-gated wherever it lands
    /// in the walk — `prefer:` can express a preference, never overrule
    /// the gate (`resolve_search_read`'s "only the configured entry 0 is
    /// ever an index attempt" rule works both ways).
    #[tokio::test]
    async fn search_prefer_cannot_force_a_stale_index_to_serve() {
        let router = search_prefer_fixture(3);
        let hints = Hints::parse(Some("prefer:idx"));
        let (_, resolution, served) = router
            .resolve_search_read("public", "default", "demo", &hints)
            .await
            .unwrap();
        assert!(matches!(resolution, SearchResolution::Fallback(_)));
        assert_eq!(served, "main");
    }

    struct BudgetRefusalSource {
        refuse: bool,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl FeatureSource for BudgetRefusalSource {
        async fn items(
            &self,
            collection: &CollectionDecl,
            _query: &ItemsQuery,
        ) -> Result<FeaturePage> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.refuse {
                return Err(Error::ItemsVertexBudgetExceeded {
                    collection: collection.id.clone(),
                    feature_id: "large".to_string(),
                    cumulative_vertices: 2,
                    limit: 1,
                });
            }
            Ok(FeaturePage {
                features_geojson: Vec::new(),
                number_matched: Some(0),
                next_token: None,
            })
        }

        async fn item(
            &self,
            collection: &CollectionDecl,
            _id: &str,
            _filter: Option<&Filter>,
        ) -> Result<Option<serde_json::Value>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.refuse {
                return Err(Error::ItemsVertexBudgetExceeded {
                    collection: collection.id.clone(),
                    feature_id: "large".to_string(),
                    cumulative_vertices: 2,
                    limit: 1,
                });
            }
            Ok(None)
        }
    }

    #[tokio::test]
    async fn deterministic_items_budget_refusal_never_advances_to_a_fallback_tail() {
        let primary_calls = Arc::new(AtomicUsize::new(0));
        let tail_calls = Arc::new(AtomicUsize::new(0));
        let source = FallbackFeatureSource {
            entries: vec![
                (
                    "primary".to_string(),
                    Arc::new(BudgetRefusalSource {
                        refuse: true,
                        calls: Arc::clone(&primary_calls),
                    }),
                ),
                (
                    "tail".to_string(),
                    Arc::new(BudgetRefusalSource {
                        refuse: false,
                        calls: Arc::clone(&tail_calls),
                    }),
                ),
            ],
            served: ServedSource::default(),
        };
        let collection: CollectionDecl =
            serde_yaml::from_str("id: demo\ncatalog: default\nstorage: main").unwrap();

        assert!(matches!(
            source.items(&collection, &ItemsQuery::default()).await,
            Err(Error::ItemsVertexBudgetExceeded { .. })
        ));
        assert!(matches!(
            source.item(&collection, "large", None).await,
            Err(Error::ItemsVertexBudgetExceeded { .. })
        ));
        assert_eq!(primary_calls.load(Ordering::SeqCst), 2);
        assert_eq!(tail_calls.load(Ordering::SeqCst), 0);
    }

    // -- lazy-mode lane-capability checks (`#59`) ----------------------------
    //
    // None of the three tests below ever call `validate_catalog` — the same
    // "no eager sweep ran" condition `registry.validation: lazy` leaves a
    // collection's first real request in. Before `#59`, `resolve_features`/
    // `resolve_tiles` themselves ran no capability check of their own for an
    // explicit lane or a places3d declaration; `features_source`/
    // `tiles_source` silently dropped an incapable entry out of a
    // multi-entry lane's fallback chain instead of refusing, so the
    // misconfiguration these tests set up would previously have resolved
    // successfully (using only the capable entry) — never surfacing at all
    // outside an eager boot. Contrast `refuses_capability_the_driver_lacks`
    // above: an *unrouted* lane's single storage lacking the capability
    // already refused before and after `#59` (`Error::CapabilityUnsupported`,
    // unchanged) — that's an ordinary "this collection doesn't do X"
    // refusal, not a misconfigured explicit routing declaration.

    #[tokio::test]
    async fn resolve_features_fails_first_touch_when_an_explicit_multi_entry_lane_has_an_incapable_entry(
    ) {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: good, driver: lane-good, url_env: DATABASE_URL }
  - { id: bad, driver: lane-bad, url_env: DATABASE_URL2 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: good
    table: demo
    geometry: geom
    pk: id
    routing: { features: [good, bad] }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-good",
            label: "good",
            features: true,
            tiles: false,
            error: false,
        }));
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-bad",
            label: "bad",
            features: false,
            tiles: false,
            error: false,
        }));
        let router = Router::build(&config, &registry).unwrap();

        match router.resolve_features("public", "default", "demo").await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("features"), "message was: {message}");
                assert!(message.contains("bad"), "message was: {message}");
            }
            other => panic!(
                "expected Err(Config(_)) at first touch, matching what validate_catalog would \
                 have raised at boot; got is_ok = {}",
                other.is_ok()
            ),
        }
    }

    #[tokio::test]
    async fn resolve_tiles_fails_first_touch_when_an_explicit_multi_entry_lane_has_an_incapable_entry(
    ) {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: good, driver: lane-good, url_env: DATABASE_URL }
  - { id: bad, driver: lane-bad, url_env: DATABASE_URL2 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: good
    table: demo
    geometry: geom
    pk: id
    routing: { tiles: [good, bad] }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-good",
            label: "good",
            features: false,
            tiles: true,
            error: false,
        }));
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-bad",
            label: "bad",
            features: false,
            tiles: false,
            error: false,
        }));
        let router = Router::build(&config, &registry).unwrap();

        match router.resolve_tiles("public", "default", "demo").await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("tiles"), "message was: {message}");
                assert!(message.contains("bad"), "message was: {message}");
            }
            other => panic!(
                "expected Err(Config(_)) at first touch, matching what validate_catalog would \
                 have raised at boot; got is_ok = {}",
                other.is_ok()
            ),
        }
    }

    #[tokio::test]
    async fn resolve_tiles_fails_first_touch_when_a_places3d_collections_unrouted_lane_lacks_tiles()
    {
        // Deliberately no `routing:` block — an unrouted lane, whose single
        // storage `validate_lane_capability` never checks (see
        // `RoutedCollection::tiles_explicit`'s own doc: boot-time capability
        // validation only ever applied to a lane the operator explicitly
        // named). This isolates the places3d-specific check: it runs
        // "regardless of whether that lane was explicitly routed"
        // (`validate_places3d_capability`'s own doc), so it must be what
        // catches this, not `validate_lane_capability`.
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: lane-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    places3d: { height_property: height }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(LaneFakeFactory {
            name: "lane-fake",
            label: "main",
            features: true,
            tiles: false,
            error: false,
        }));
        let router = Router::build(&config, &registry).unwrap();

        match router.resolve_tiles("public", "default", "demo").await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("places3d"), "message was: {message}");
                assert!(message.contains("main"), "message was: {message}");
            }
            other => panic!(
                "expected Err(Config(_)) naming places3d at first touch, matching what \
                 validate_catalog would have raised at boot (not a generic \
                 CapabilityUnsupported); got is_ok = {}",
                other.is_ok()
            ),
        }
    }

    /// A `CatalogSource` purpose-built for descriptor-derivation tests: reports
    /// one fixed physical row and (optionally) an extent/row estimate/
    /// attribute schema/temporal column, and counts how many times
    /// `collections()` was called — the seam these tests use to assert on
    /// TTL-caching behavior.
    struct DescriptorFakeCatalog {
        physical: PhysicalCollection,
        /// `#104`: additional physical rows `with_additional_physical`
        /// appends, for the "backend reports more than one geometry column
        /// for this table" ambiguity tests — empty for every other test,
        /// which only ever needs the single fixed `physical` row above.
        extra_physical: Vec<PhysicalCollection>,
        extent: Option<SpatialExtent>,
        row_estimate: Option<u64>,
        attributes: Option<Vec<AttributeColumn>>,
        temporal_column: Option<String>,
        /// `#101`: geometry profile `geometry_profile` reports, when a test
        /// opts in via `with_geometry_profile` — `None` for every other test,
        /// exercising the same "driver never overrides the default" shape a
        /// real driver without this capability has.
        geometry_profile: Option<crate::catalog::GeometryProfile>,
        /// `#36`: projection facts `projection` reports, when a test opts in
        /// via `with_projection` — `None` for every other test, same
        /// "driver never overrides the default" shape as `geometry_profile`.
        projection: Option<crate::catalog::ProjectionFacts>,
        calls: std::sync::atomic::AtomicUsize,
        /// `#101`: separate counter from `calls` (which only counts
        /// `collections()`) — the geometry-profile caching tests need to
        /// distinguish "the cheap physical lookup ran again" from "the
        /// profile itself was recomputed."
        geometry_profile_calls: std::sync::atomic::AtomicUsize,
    }

    impl DescriptorFakeCatalog {
        fn new(physical: PhysicalCollection, extent: Option<SpatialExtent>) -> Self {
            Self {
                physical,
                extra_physical: Vec::new(),
                extent,
                row_estimate: None,
                attributes: None,
                temporal_column: None,
                geometry_profile: None,
                projection: None,
                calls: std::sync::atomic::AtomicUsize::new(0),
                geometry_profile_calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        /// Builder extension for the richer-descriptor tests (`#19`): attaches
        /// a row estimate, attribute schema, and temporal-column candidate
        /// alongside the fixed physical row every other test already relies
        /// on.
        fn with_richer_fields(
            mut self,
            row_estimate: u64,
            attributes: Vec<AttributeColumn>,
            temporal_column: &str,
        ) -> Self {
            self.row_estimate = Some(row_estimate);
            self.attributes = Some(attributes);
            self.temporal_column = Some(temporal_column.to_string());
            self
        }

        /// Builder extension for the ambiguous-geometry-column tests
        /// (`#104`): `collections()` reports this row alongside the fixed
        /// `physical` one, same table name, so a caller sees two candidate
        /// rows for one target table — exactly what PostGIS's
        /// `geometry_columns` view returns for a table with two spatial
        /// columns.
        fn with_additional_physical(mut self, extra: PhysicalCollection) -> Self {
            self.extra_physical.push(extra);
            self
        }

        /// `#101`: opts this fake into reporting `profile` from
        /// `geometry_profile` — the shape a real capability-supporting
        /// driver (PostGIS) has, as opposed to every other test's default
        /// (the trait's own `Ok(None)`).
        fn with_geometry_profile(mut self, profile: crate::catalog::GeometryProfile) -> Self {
            self.geometry_profile = Some(profile);
            self
        }

        /// `#36`: opts this fake into reporting `facts` from `projection` —
        /// the shape a raster driver that reads its own georeferencing
        /// (COG/Zarr) has, as opposed to every other test's default (the
        /// trait's own `Ok(None)`).
        fn with_projection(mut self, facts: crate::catalog::ProjectionFacts) -> Self {
            self.projection = Some(facts);
            self
        }

        fn collections_calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn geometry_profile_calls(&self) -> usize {
            self.geometry_profile_calls
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl CatalogSource for DescriptorFakeCatalog {
        async fn collections(&self) -> Result<Vec<PhysicalCollection>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut rows = vec![self.physical.clone()];
            rows.extend(self.extra_physical.iter().cloned());
            Ok(rows)
        }

        async fn extent(&self, _physical: &PhysicalCollection) -> Result<Option<SpatialExtent>> {
            Ok(self.extent)
        }

        async fn row_estimate(&self, _physical: &PhysicalCollection) -> Result<Option<u64>> {
            Ok(self.row_estimate)
        }

        async fn attribute_schema(
            &self,
            _physical: &PhysicalCollection,
        ) -> Result<Option<Vec<AttributeColumn>>> {
            Ok(self.attributes.clone())
        }

        async fn temporal_column(&self, _physical: &PhysicalCollection) -> Result<Option<String>> {
            Ok(self.temporal_column.clone())
        }

        async fn geometry_profile(
            &self,
            _physical: &PhysicalCollection,
        ) -> Result<Option<crate::catalog::GeometryProfile>> {
            self.geometry_profile_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.geometry_profile)
        }

        async fn projection(
            &self,
            _physical: &PhysicalCollection,
        ) -> Result<Option<crate::catalog::ProjectionFacts>> {
            Ok(self.projection)
        }
    }

    struct DescriptorFakeDriver {
        catalog: Arc<DescriptorFakeCatalog>,
        /// `#107`: `None` for every pre-existing test (this driver never
        /// resolves a write lane, exactly as before this field existed) —
        /// only `build_descriptor_router_with_write` sets it, for the
        /// `canonical_descriptor` Optimistic Locking tests below.
        write_sink: Option<Arc<dyn WriteSink>>,
    }

    impl StorageDriver for DescriptorFakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::clone(&self.catalog) as Arc<dyn CatalogSource>
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(FakeFeaturesOnlyDriver) as Arc<dyn FeatureSource>)
        }

        fn write_sink(&self) -> Option<Arc<dyn WriteSink>> {
            self.write_sink.clone()
        }
    }

    struct DescriptorFakeFactory {
        catalog: Arc<DescriptorFakeCatalog>,
        write_sink: Option<Arc<dyn WriteSink>>,
    }

    impl DriverFactory for DescriptorFakeFactory {
        fn name(&self) -> &str {
            "descriptor-fake"
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(DescriptorFakeDriver {
                catalog: Arc::clone(&self.catalog),
                write_sink: self.write_sink.clone(),
            }))
        }
    }

    fn descriptor_physical(
        geometry_column: Option<&str>,
        primary_key: Option<&str>,
    ) -> PhysicalCollection {
        PhysicalCollection {
            name: "demo".to_string(),
            geometry_column: geometry_column.map(str::to_string),
            primary_key: primary_key.map(str::to_string),
            srid: Some(4326),
            geometry_type: None,
        }
    }

    /// Builds a single-collection config against the `descriptor-fake` driver,
    /// omitting `table`/`geometry`/`pk` when `None` is passed, with a
    /// caller-chosen `descriptor_ttl_s`.
    fn descriptor_config(
        table: Option<&str>,
        geometry: Option<&str>,
        pk: Option<&str>,
        ttl_s: u64,
    ) -> AppConfig {
        let mut yaml = format!(
            "server: {{ descriptor_ttl_s: {ttl_s} }}\n\
             storages: [ {{ id: main, driver: descriptor-fake, url_env: DATABASE_URL }} ]\n\
             tenants: [ {{ id: public }} ]\n\
             catalogs: [ {{ id: default, tenant: public }} ]\n\
             collections:\n  - id: demo\n    catalog: default\n    storage: main\n"
        );
        if let Some(table) = table {
            yaml.push_str(&format!("    table: {table}\n"));
        }
        if let Some(geometry) = geometry {
            yaml.push_str(&format!("    geometry: {geometry}\n"));
        }
        if let Some(pk) = pk {
            yaml.push_str(&format!("    pk: {pk}\n"));
        }
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        config
    }

    /// `#61`: the same single-collection shape `descriptor_config` builds,
    /// but with `registry: { validation: lazy }` and a fully-pinned
    /// `table`/`geometry`/`pk` — the shape `effective_decl`'s lazy-only
    /// pin-verification tests need.
    fn lazy_descriptor_config(
        table: Option<&str>,
        geometry: Option<&str>,
        pk: Option<&str>,
    ) -> AppConfig {
        let mut yaml = "registry: { validation: lazy }\n\
             storages: [ { id: main, driver: descriptor-fake, url_env: DATABASE_URL } ]\n\
             tenants: [ { id: public } ]\n\
             catalogs: [ { id: default, tenant: public } ]\n\
             collections:\n  - id: demo\n    catalog: default\n    storage: main\n"
            .to_string();
        if let Some(table) = table {
            yaml.push_str(&format!("    table: {table}\n"));
        }
        if let Some(geometry) = geometry {
            yaml.push_str(&format!("    geometry: {geometry}\n"));
        }
        if let Some(pk) = pk {
            yaml.push_str(&format!("    pk: {pk}\n"));
        }
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        config
    }

    fn build_descriptor_router(catalog: Arc<DescriptorFakeCatalog>, config: &AppConfig) -> Router {
        let mut registry = Registry::new();
        registry.register(Arc::new(DescriptorFakeFactory {
            catalog,
            write_sink: None,
        }));
        Router::build(config, &registry).unwrap()
    }

    /// `#107`: same fixture as [`build_descriptor_router`], with `write_sink`
    /// wired so `Router::resolve_write`/`canonical_descriptor`'s `write`/
    /// `locking_conformance_classes` fields have something real to resolve —
    /// every pre-existing `descriptor_config`-based test still goes through
    /// the plain [`build_descriptor_router`] above, unaffected.
    fn build_descriptor_router_with_write(
        catalog: Arc<DescriptorFakeCatalog>,
        config: &AppConfig,
        write_sink: Arc<dyn WriteSink>,
    ) -> Router {
        let mut registry = Registry::new();
        registry.register(Arc::new(DescriptorFakeFactory {
            catalog,
            write_sink: Some(write_sink),
        }));
        Router::build(config, &registry).unwrap()
    }

    #[tokio::test]
    async fn resolve_features_derives_geometry_and_pk_from_the_catalog_when_omitted() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            decl.table.as_deref(),
            Some("demo"),
            "table derives from the collection id by convention when omitted"
        );
        assert_eq!(decl.geometry.as_deref(), Some("geom"));
        assert_eq!(decl.pk.as_deref(), Some("id"));
    }

    #[tokio::test]
    async fn resolve_features_honors_a_geometry_override_that_diverges_from_the_backend() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config(None, Some("the_geom"), None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            decl.geometry.as_deref(),
            Some("the_geom"),
            "an override must win even though it contradicts the backend"
        );
        assert_eq!(decl.pk.as_deref(), Some("id"), "pk still derives normally");
    }

    /// `#104`: `demo`'s backend reports two rows for the same table — the
    /// shape PostGIS's `geometry_columns` view returns for a table with two
    /// spatial columns — and no `geometry:` override picks one. Lazy
    /// derivation (`derive_one_descriptor`, reached here because `resolve_
    /// features` is called with no prior `validate_catalog` boot sweep) must
    /// refuse rather than silently binding to whichever row came back first,
    /// and the error must name both candidate columns so the operator knows
    /// what to pin.
    #[tokio::test]
    async fn resolve_features_fails_when_the_catalog_reports_two_geometry_columns_and_none_is_pinned(
    ) {
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom_a"), Some("id")), None)
                .with_additional_physical(descriptor_physical(Some("geom_b"), Some("id"))),
        );
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let error = match router.resolve_features("public", "default", "demo").await {
            Err(err) => err,
            Ok(_) => panic!("two geometry columns with no pin must refuse rather than guess"),
        };
        let message = error.to_string();
        assert!(
            message.contains("demo"),
            "message must name the table: {message}"
        );
        assert!(
            message.contains("geom_a"),
            "message must name the first candidate column: {message}"
        );
        assert!(
            message.contains("geom_b"),
            "message must name the second candidate column: {message}"
        );
        assert!(
            message.contains("geometry"),
            "message must point at the 'geometry' config key: {message}"
        );
    }

    /// `#104` counterpart: the same two-geometry-column backend, but
    /// `geometry: geom_a` is pinned — the ambiguity check must not even run,
    /// and resolution must succeed exactly as it would with a single
    /// geometry column.
    #[tokio::test]
    async fn resolve_features_honors_a_geometry_pin_even_when_the_catalog_reports_two_geometry_columns(
    ) {
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom_a"), Some("id")), None)
                .with_additional_physical(descriptor_physical(Some("geom_b"), Some("id"))),
        );
        let config = descriptor_config(None, Some("geom_a"), None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .expect("a pinned geometry column must resolve despite the backend ambiguity");
        assert_eq!(decl.geometry.as_deref(), Some("geom_a"));
        assert_eq!(decl.pk.as_deref(), Some("id"), "pk still derives normally");
    }

    /// A geometry pin selects the physical geometry row as well as the
    /// collection's effective geometry name.  In particular, the descriptor
    /// must not borrow SRID/type from the first row PostGIS returns for the
    /// same table.
    #[tokio::test]
    async fn lazy_descriptor_uses_the_pinned_geometry_physical_row() {
        let mut web_mercator = descriptor_physical(Some("geom_3857"), Some("id"));
        web_mercator.srid = Some(3857);
        web_mercator.geometry_type = Some("POLYGON".to_string());
        let mut wgs84 = descriptor_physical(Some("geom_4326"), Some("id"));
        wgs84.srid = Some(4326);
        wgs84.geometry_type = Some("POINT".to_string());
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(wgs84, None).with_additional_physical(web_mercator),
        );
        let config = descriptor_config(None, Some("geom_3857"), Some("id"), 300);
        let router = build_descriptor_router(catalog, &config);

        let descriptor = router
            .collection_descriptor("public", "default", "demo")
            .await
            .expect("a pin to the second physical geometry row must resolve");

        assert_eq!(descriptor.geometry.as_deref(), Some("geom_3857"));
        assert_eq!(descriptor.srid, Some(3857));
        assert_eq!(descriptor.geometry_type.as_deref(), Some("POLYGON"));
    }

    /// The eager validation sweep must select the same pinned physical row it
    /// caches for later requests; otherwise startup succeeds with metadata
    /// for a different column than the one the collection serves.
    #[tokio::test]
    async fn validate_catalog_uses_the_pinned_geometry_physical_row() {
        let mut web_mercator = descriptor_physical(Some("geom_3857"), Some("id"));
        web_mercator.srid = Some(3857);
        web_mercator.geometry_type = Some("POLYGON".to_string());
        let mut wgs84 = descriptor_physical(Some("geom_4326"), Some("id"));
        wgs84.srid = Some(4326);
        wgs84.geometry_type = Some("POINT".to_string());
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(wgs84, None).with_additional_physical(web_mercator),
        );
        let config = descriptor_config(None, Some("geom_3857"), Some("id"), 300);
        let router = build_descriptor_router(catalog, &config);

        router
            .validate_catalog()
            .await
            .expect("a pin to the second physical geometry row must validate");
        let descriptor = router
            .collection_descriptor("public", "default", "demo")
            .await
            .expect("boot validation must cache the pinned physical row");

        assert_eq!(descriptor.geometry.as_deref(), Some("geom_3857"));
        assert_eq!(descriptor.srid, Some(3857));
        assert_eq!(descriptor.geometry_type.as_deref(), Some("POLYGON"));
    }

    #[tokio::test]
    async fn resolved_descriptor_is_cached_within_ttl_and_avoids_a_second_catalog_query() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        router
            .collection_descriptor("public", "default", "demo")
            .await
            .unwrap();

        assert_eq!(
            catalog.collections_calls(),
            1,
            "a descriptor still within TTL must not be re-derived"
        );
    }

    #[tokio::test]
    async fn resolved_descriptor_is_rederived_after_the_ttl_expires() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        // A zero TTL is stale immediately after being computed, so every
        // access re-derives — the deterministic way to exercise "lazily
        // re-derive on expiry" without a fake clock.
        let config = descriptor_config(None, None, None, 0);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();

        assert_eq!(
            catalog.collections_calls(),
            2,
            "a zero TTL must force re-derivation on every access"
        );
    }

    // -- geometry profile (`#101`) -------------------------------------------

    fn geometry_profile_fixture() -> crate::catalog::GeometryProfile {
        crate::catalog::GeometryProfile {
            sample_size: 128,
            computed_at: std::time::SystemTime::now(),
            vertices: crate::catalog::VertexStats {
                mean: 5.0,
                median: 4.0,
                p95: 9.0,
                max: 20,
                total_estimated: Some(1_280),
            },
            vertex_density_per_area: Some(0.2),
            multi_part_fraction: 0.05,
            mean_ring_count: Some(1.1),
            feature_size: crate::catalog::FeatureSizeStats {
                p50: Some(2.0),
                p95: Some(8.0),
                max: Some(10.0),
            },
        }
    }

    /// Design point 4 (`#101`): a collection whose driver never overrides
    /// `CatalogSource::geometry_profile` — every existing driver/fixture in
    /// this workspace before `#101` — must see `geometry_profile: None` on
    /// its canonical descriptor, exactly the same as before this field
    /// existed. `DescriptorFakeCatalog` here doesn't call
    /// `with_geometry_profile`, so it exercises the trait's own default.
    #[tokio::test]
    async fn canonical_descriptor_geometry_profile_is_none_when_the_driver_never_computed_one() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let canonical = router
            .canonical_descriptor("public", "default", "demo")
            .await
            .unwrap();
        assert!(
            canonical.geometry_profile.is_none(),
            "no profile computed must mean no profile reported, byte-for-byte today's behavior"
        );
    }

    #[tokio::test]
    async fn canonical_descriptor_surfaces_a_computed_geometry_profile() {
        let profile = geometry_profile_fixture();
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_geometry_profile(profile),
        );
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let canonical = router
            .canonical_descriptor("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(canonical.geometry_profile, Some(profile));
    }

    // -- `canonical_descriptor`'s Optimistic Locking capabilities (`#107`) ---

    /// Same single-collection shape `descriptor_config` builds, with
    /// `routing: { write: main }` (so `Router::resolve_write` actually
    /// resolves) and, optionally, a declared `modified_column`.
    fn descriptor_config_with_write(modified_column: Option<&str>) -> AppConfig {
        let mut yaml = String::from(
            "storages: [ { id: main, driver: descriptor-fake, url_env: DATABASE_URL } ]\n\
             tenants: [ { id: public } ]\n\
             catalogs: [ { id: default, tenant: public } ]\n\
             collections:\n  - id: demo\n    catalog: default\n    storage: main\n    table: demo\n    geometry: geom\n    pk: id\n    routing: { write: main }\n",
        );
        if let Some(column) = modified_column {
            yaml.push_str(&format!("    modified_column: {column}\n"));
        }
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        config
    }

    fn locking_write_sink(classes: &'static [&'static str]) -> Arc<dyn WriteSink> {
        Arc::new(LockingWriteSink { classes }) as Arc<dyn WriteSink>
    }

    /// Both lanes resolving and the write sink declaring the ETags class:
    /// this collection's own `locking_conformance_classes` names exactly
    /// that class — no `modified_column` declared, so Timestamps never
    /// joins it.
    #[tokio::test]
    async fn canonical_descriptor_declares_etags_when_the_write_sink_earns_it() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config_with_write(None);
        let router = build_descriptor_router_with_write(
            Arc::clone(&catalog),
            &config,
            locking_write_sink(&[locking::OPTIMISTIC_LOCKING_ETAGS_CLASS]),
        );

        let canonical = router
            .canonical_descriptor("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            canonical.capabilities.locking_conformance_classes,
            Some(vec![locking::OPTIMISTIC_LOCKING_ETAGS_CLASS])
        );
    }

    /// A declared `modified_column` adds the Timestamps class alongside
    /// whatever the write sink itself declares — a per-collection fact, not
    /// something the driver has any say over.
    #[tokio::test]
    async fn canonical_descriptor_adds_timestamps_when_modified_column_is_declared() {
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(
                    0,
                    vec![AttributeColumn {
                        name: "updated_at".to_string(),
                        sql_type: "timestamptz".to_string(),
                    }],
                    "updated_at",
                ),
        );
        let config = descriptor_config_with_write(Some("updated_at"));
        let router = build_descriptor_router_with_write(
            Arc::clone(&catalog),
            &config,
            locking_write_sink(&[locking::OPTIMISTIC_LOCKING_ETAGS_CLASS]),
        );

        let canonical = router
            .canonical_descriptor("public", "default", "demo")
            .await
            .unwrap();
        let classes = canonical
            .capabilities
            .locking_conformance_classes
            .as_deref()
            .expect("a features-capable collection always participates in locking metadata");
        assert!(classes.contains(&locking::OPTIMISTIC_LOCKING_ETAGS_CLASS));
        assert!(classes.contains(&locking::OPTIMISTIC_LOCKING_TIMESTAMPS_CLASS));
    }

    /// No `modified_column` declared at all: Timestamps never appears,
    /// regardless of what the write sink itself declares — this collection's
    /// own honest "no Timestamps class" answer (`CollectionDecl::
    /// modified_column`'s own doc: absence is never fabricated into a
    /// claim).
    #[tokio::test]
    async fn canonical_descriptor_never_declares_timestamps_without_a_modified_column() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config_with_write(None);
        let router = build_descriptor_router_with_write(
            Arc::clone(&catalog),
            &config,
            locking_write_sink(&[locking::OPTIMISTIC_LOCKING_ETAGS_CLASS]),
        );

        let canonical = router
            .canonical_descriptor("public", "default", "demo")
            .await
            .unwrap();
        assert!(!canonical
            .capabilities
            .locking_conformance_classes
            .as_deref()
            .expect("a features-capable collection always participates in locking metadata")
            .contains(&locking::OPTIMISTIC_LOCKING_TIMESTAMPS_CLASS));
    }

    /// A write sink that declares nothing (the trait default) never earns
    /// ETags for this collection either, even though its write lane
    /// resolves — the per-collection surface stays honest to what the
    /// concrete resolved sink actually claims, exactly like
    /// `cql2_conformance_classes` already does for the read side.
    #[tokio::test]
    async fn canonical_descriptor_declares_nothing_when_the_write_sink_earns_nothing() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config_with_write(None);
        let router = build_descriptor_router_with_write(
            Arc::clone(&catalog),
            &config,
            locking_write_sink(&[]),
        );

        let canonical = router
            .canonical_descriptor("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            canonical.capabilities.locking_conformance_classes,
            Some(Vec::new()),
            "participates (the features lane resolves) and honours nothing — \
             `Some(vec![])`, never `None` (`#287`)"
        );
    }

    /// No write lane at all (the plain `build_descriptor_router` fixture,
    /// `routing.write` unset): neither class can ever apply — there is no
    /// write side for a guard to protect.
    #[tokio::test]
    async fn canonical_descriptor_declares_nothing_when_the_write_lane_never_resolves() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let canonical = router
            .canonical_descriptor("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            canonical.capabilities.locking_conformance_classes,
            Some(Vec::new()),
            "features lane resolves, write lane never does: participates and \
             honours nothing — `Some(vec![])`, never `None` (`#287`)"
        );
    }

    #[tokio::test]
    async fn geometry_profile_is_cached_within_ttl_and_avoids_a_second_sampling_query() {
        let profile = geometry_profile_fixture();
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_geometry_profile(profile),
        );
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        router
            .geometry_profile("public", "default", "demo")
            .await
            .unwrap();
        router
            .geometry_profile("public", "default", "demo")
            .await
            .unwrap();

        assert_eq!(
            catalog.geometry_profile_calls(),
            1,
            "a profile still within TTL must not be resampled"
        );
    }

    #[tokio::test]
    async fn refresh_geometry_profile_bypasses_the_cache_regardless_of_ttl() {
        let profile = geometry_profile_fixture();
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_geometry_profile(profile),
        );
        // A long TTL: the cache would otherwise happily serve the first
        // computation forever within this test's lifetime — proving
        // `refresh_geometry_profile` genuinely bypasses it, not merely that
        // the TTL happened to expire.
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        router
            .geometry_profile("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(catalog.geometry_profile_calls(), 1);

        let refreshed = router
            .refresh_geometry_profile("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            catalog.geometry_profile_calls(),
            2,
            "an explicit refresh must resample even though the cached entry is still within TTL"
        );
        assert_eq!(refreshed, Some(profile));

        // The refreshed value also replaces what's cached, so the next
        // ordinary lookup sees it without paying for a third sample.
        router
            .geometry_profile("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(catalog.geometry_profile_calls(), 2);
    }

    #[tokio::test]
    async fn geometry_profile_of_an_unknown_collection_is_not_found() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let error = router
            .geometry_profile("public", "default", "nope")
            .await
            .unwrap_err();
        assert!(matches!(error, Error::NotFound));
    }

    #[tokio::test]
    async fn collection_descriptor_exposes_the_derived_extent_even_when_fully_overridden() {
        let extent = SpatialExtent {
            bbox: [1.0, 2.0, 3.0, 4.0],
        };
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            Some(extent),
        ));
        // table/geometry/pk are all overridden — `resolve_features` would
        // skip the catalog entirely for this collection, but extent has no
        // override, so `collection_descriptor` must still derive it.
        let config = descriptor_config(Some("demo"), Some("geom"), Some("id"), 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let descriptor = router
            .collection_descriptor("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(descriptor.extent, Some(extent));
    }

    #[tokio::test]
    async fn validate_catalog_fails_fast_when_a_physical_field_cannot_be_derived() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(None, None),
            None,
        ));
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        match router.validate_catalog().await {
            Err(Error::Config(message)) => {
                assert!(message.contains("geometry"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_catalog_warms_the_descriptor_cache_so_the_first_resolve_pays_no_extra_io() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        router.validate_catalog().await.unwrap();
        assert_eq!(catalog.collections_calls(), 1);

        router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            catalog.collections_calls(),
            1,
            "boot already warmed the cache; the first request must not re-query"
        );
    }

    // -- registry scale-out: lazy validation + bounded cache (`#42`) ---------

    /// The "cached verdict" half of `registry.validation: lazy`: a
    /// derivation failure (here, an overridden `table` the fake catalog
    /// never reports) is cached exactly like a success, so a repeat request
    /// against the same permanently misconfigured collection costs no
    /// second backend round trip — only a clear, immediate `Error::Config`.
    #[tokio::test]
    async fn resolved_descriptor_caches_a_config_failure_and_avoids_a_second_catalog_query() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        // The fake catalog only ever reports a table named "demo"
        // (`descriptor_physical`); overriding `table` to something else
        // deterministically triggers the "table missing" derivation
        // failure without touching `geometry`/`pk`, so `effective_decl`'s
        // fully-overridden fast path never short-circuits the derive call.
        let config = descriptor_config(Some("nonexistent_table"), None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        match router.resolve_features("public", "default", "demo").await {
            Err(Error::Config(_)) => {}
            other => panic!("expected Err(Config(_)), got is_ok={}", other.is_ok()),
        }
        match router
            .collection_descriptor("public", "default", "demo")
            .await
        {
            Err(Error::Config(_)) => {}
            other => {
                panic!("expected the cached failure to replay as Err(Config(_)), got {other:?}")
            }
        }

        assert_eq!(
            catalog.collections_calls(),
            1,
            "a cached Config failure must not repeat the backend round trip"
        );
    }

    /// A `CatalogSource` whose `collections()` call always fails with a
    /// transient error (never `Error::Config`) — purpose-built to prove such
    /// an error is never cached as a standing verdict, unlike a `Config`
    /// misconfiguration (see the test above).
    struct AlwaysFailingCatalog {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl AlwaysFailingCatalog {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl CatalogSource for AlwaysFailingCatalog {
        async fn collections(&self) -> Result<Vec<PhysicalCollection>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(Error::Timeout)
        }
    }

    struct AlwaysFailingDriver {
        catalog: Arc<AlwaysFailingCatalog>,
    }

    impl StorageDriver for AlwaysFailingDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::clone(&self.catalog) as Arc<dyn CatalogSource>
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(FakeFeaturesOnlyDriver) as Arc<dyn FeatureSource>)
        }
    }

    struct AlwaysFailingFactory {
        catalog: Arc<AlwaysFailingCatalog>,
    }

    impl DriverFactory for AlwaysFailingFactory {
        fn name(&self) -> &str {
            "always-failing-fake"
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(AlwaysFailingDriver {
                catalog: Arc::clone(&self.catalog),
            }))
        }
    }

    #[tokio::test]
    async fn resolved_descriptor_never_caches_a_transient_error() {
        let catalog = AlwaysFailingCatalog::new();
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: always-failing-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(AlwaysFailingFactory {
            catalog: Arc::clone(&catalog),
        }));
        let router = Router::build(&config, &registry).unwrap();

        assert!(matches!(
            router.resolve_features("public", "default", "demo").await,
            Err(Error::Timeout)
        ));
        assert!(matches!(
            router.resolve_features("public", "default", "demo").await,
            Err(Error::Timeout)
        ));

        assert_eq!(
            catalog.calls(),
            2,
            "a transient error must never be cached; every request must retry the backend"
        );
    }

    /// A `CatalogSource` that reports several distinct physical tables from
    /// one storage — lets a single fake driver back several collections at
    /// once, purpose-built for the descriptor-cache capacity test below.
    struct MultiTableCatalog {
        physicals: Vec<PhysicalCollection>,
    }

    #[async_trait::async_trait]
    impl CatalogSource for MultiTableCatalog {
        async fn collections(&self) -> Result<Vec<PhysicalCollection>> {
            Ok(self.physicals.clone())
        }
    }

    struct MultiTableDriver {
        catalog: Arc<MultiTableCatalog>,
    }

    impl StorageDriver for MultiTableDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::clone(&self.catalog) as Arc<dyn CatalogSource>
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(FakeFeaturesOnlyDriver) as Arc<dyn FeatureSource>)
        }
    }

    struct MultiTableFactory {
        catalog: Arc<MultiTableCatalog>,
    }

    impl DriverFactory for MultiTableFactory {
        fn name(&self) -> &str {
            "multi-table-fake"
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(MultiTableDriver {
                catalog: Arc::clone(&self.catalog),
            }))
        }
    }

    /// The descriptor-cache-budget deliverable itself (`#42`): three
    /// collections, each genuinely derived (no `table` override, so
    /// `effective_decl`'s fully-overridden fast path never applies), against
    /// a cache configured to hold at most one entry — proves cold entries
    /// get evicted rather than pinning memory forever as the number of
    /// derived collections grows past the configured budget.
    #[tokio::test]
    async fn descriptor_cache_is_bounded_by_descriptor_cache_capacity() {
        let catalog = Arc::new(MultiTableCatalog {
            physicals: vec![physical("alpha"), physical("bravo"), physical("charlie")],
        });
        let config: AppConfig = serde_yaml::from_str(
            r#"
server: { descriptor_cache_capacity: 1 }
storages: [ { id: main, driver: multi-table-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: alpha
    catalog: default
    storage: main
    geometry: geom
    pk: id
  - id: bravo
    catalog: default
    storage: main
    geometry: geom
    pk: id
  - id: charlie
    catalog: default
    storage: main
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(MultiTableFactory { catalog }));
        let router = Router::build(&config, &registry).unwrap();

        router
            .resolve_features("public", "default", "alpha")
            .await
            .unwrap();
        router
            .resolve_features("public", "default", "bravo")
            .await
            .unwrap();
        router
            .resolve_features("public", "default", "charlie")
            .await
            .unwrap();

        let entries = router.descriptor_cache_entry_count().await;
        assert!(
            entries <= 1,
            "descriptor cache should stay within its configured capacity of 1, has {entries} entries"
        );
    }

    /// `#104`: the eager boot-time sweep (`validate_catalog`) groups
    /// physical rows by table name before this loop ever runs, so this
    /// exercises a second, independent piece of code from the lazy
    /// `derive_one_descriptor` path above. `demo`'s backend reports two rows
    /// — same table, two distinct geometry columns, the shape PostGIS's
    /// `geometry_columns` view returns for a table with two spatial columns
    /// — and no `geometry:` override picks one, so boot must refuse rather
    /// than the old by-name `HashMap` silently collapsing both rows into
    /// whichever one a plain `insert` happened to overwrite last.
    #[tokio::test]
    async fn validate_catalog_fails_fast_when_a_table_reports_two_geometry_columns_and_none_is_pinned(
    ) {
        let catalog = Arc::new(MultiTableCatalog {
            physicals: vec![
                PhysicalCollection {
                    name: "demo".to_string(),
                    geometry_column: Some("geom_a".to_string()),
                    primary_key: Some("id".to_string()),
                    srid: Some(4326),
                    geometry_type: None,
                },
                PhysicalCollection {
                    name: "demo".to_string(),
                    geometry_column: Some("geom_b".to_string()),
                    primary_key: Some("id".to_string()),
                    srid: Some(4326),
                    geometry_type: None,
                },
            ],
        });
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: multi-table-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(MultiTableFactory { catalog }));
        let router = Router::build(&config, &registry).unwrap();

        let error = router
            .validate_catalog()
            .await
            .expect_err("two geometry columns with no pin must refuse boot rather than guess");
        let message = error.to_string();
        assert!(
            message.contains("demo"),
            "message must name the table: {message}"
        );
        assert!(
            message.contains("geom_a"),
            "message must name the first candidate column: {message}"
        );
        assert!(
            message.contains("geom_b"),
            "message must name the second candidate column: {message}"
        );
        assert!(
            message.contains("geometry"),
            "message must point at the 'geometry' config key: {message}"
        );
    }

    /// `#104` counterpart: the same two-geometry-column backend, but
    /// `geometry: geom_a` is pinned — boot must succeed exactly as it would
    /// with a single geometry column, since a pin already names one
    /// unambiguously and the ambiguity check must not run at all.
    #[tokio::test]
    async fn validate_catalog_passes_when_geometry_is_pinned_despite_two_geometry_columns() {
        let catalog = Arc::new(MultiTableCatalog {
            physicals: vec![
                PhysicalCollection {
                    name: "demo".to_string(),
                    geometry_column: Some("geom_a".to_string()),
                    primary_key: Some("id".to_string()),
                    srid: Some(4326),
                    geometry_type: None,
                },
                PhysicalCollection {
                    name: "demo".to_string(),
                    geometry_column: Some("geom_b".to_string()),
                    primary_key: Some("id".to_string()),
                    srid: Some(4326),
                    geometry_type: None,
                },
            ],
        });
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: multi-table-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    geometry: geom_a
    pk: id
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(MultiTableFactory { catalog }));
        let router = Router::build(&config, &registry).unwrap();

        router
            .validate_catalog()
            .await
            .expect("a pinned geometry column must resolve despite the backend ambiguity");
    }

    // -- geometry_variants boot validation (`#104`, design point 5) ---------

    /// `#104`: `demo` pins `geometry: geom` (so the ambiguity check never
    /// runs) and declares a `geom_z6` variant, but the backend's `collections()`
    /// only ever reports the base `geom` row — the shape a typo'd variant
    /// column name produces. Boot must refuse, naming the collection and the
    /// missing column, rather than let the tiles lane discover the typo as an
    /// always-empty tile the first time a low-zoom request reaches it.
    #[tokio::test]
    async fn validate_catalog_fails_fast_when_a_declared_geometry_variant_column_does_not_exist() {
        let catalog = Arc::new(MultiTableCatalog {
            physicals: vec![PhysicalCollection {
                name: "demo".to_string(),
                geometry_column: Some("geom".to_string()),
                primary_key: Some("id".to_string()),
                srid: Some(4326),
                geometry_type: Some("POINT".to_string()),
            }],
        });
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: multi-table-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 14 }
    geometry_variants:
      - column: geom_z6
        minzoom: 0
        maxzoom: 6
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(MultiTableFactory { catalog }));
        let router = Router::build(&config, &registry).unwrap();

        let error = router
            .validate_catalog()
            .await
            .expect_err("a variant naming a column the backend never reports must refuse boot");
        let message = error.to_string();
        assert!(
            message.contains("demo"),
            "message must name the collection: {message}"
        );
        assert!(
            message.contains("geom_z6"),
            "message must name the missing variant column: {message}"
        );
    }

    /// `#104`: `geom_z6` exists on the backend, but at a different SRID than
    /// the pinned base column `geom` — a typo-free but physically
    /// inconsistent pre-generalized column. Boot must refuse rather than let
    /// the tiles lane silently mix two coordinate systems across zooms.
    #[tokio::test]
    async fn validate_catalog_fails_fast_when_a_declared_geometry_variant_srid_disagrees_with_the_base_column(
    ) {
        let catalog = Arc::new(MultiTableCatalog {
            physicals: vec![
                PhysicalCollection {
                    name: "demo".to_string(),
                    geometry_column: Some("geom".to_string()),
                    primary_key: Some("id".to_string()),
                    srid: Some(4326),
                    geometry_type: Some("POINT".to_string()),
                },
                PhysicalCollection {
                    name: "demo".to_string(),
                    geometry_column: Some("geom_z6".to_string()),
                    primary_key: Some("id".to_string()),
                    srid: Some(3857),
                    geometry_type: Some("POINT".to_string()),
                },
            ],
        });
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: multi-table-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 14 }
    geometry_variants:
      - column: geom_z6
        minzoom: 0
        maxzoom: 6
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(MultiTableFactory { catalog }));
        let router = Router::build(&config, &registry).unwrap();

        let error = router
            .validate_catalog()
            .await
            .expect_err("a variant column at a different SRID than the base must refuse boot");
        let message = error.to_string();
        assert!(
            message.contains("demo"),
            "message must name the collection: {message}"
        );
        assert!(
            message.contains("geom_z6"),
            "message must name the mismatched variant column: {message}"
        );
        assert!(
            message.contains("srid"),
            "message must call out the srid mismatch: {message}"
        );
    }

    /// `#104` counterpart: same shape as the SRID mismatch above, but the
    /// backend reports `geom_z6` with a different geometry type than the
    /// pinned base column `geom` (a polygon variant column paired against a
    /// point base column, say).
    #[tokio::test]
    async fn validate_catalog_fails_fast_when_a_declared_geometry_variant_type_disagrees_with_the_base_column(
    ) {
        let catalog = Arc::new(MultiTableCatalog {
            physicals: vec![
                PhysicalCollection {
                    name: "demo".to_string(),
                    geometry_column: Some("geom".to_string()),
                    primary_key: Some("id".to_string()),
                    srid: Some(4326),
                    geometry_type: Some("POINT".to_string()),
                },
                PhysicalCollection {
                    name: "demo".to_string(),
                    geometry_column: Some("geom_z6".to_string()),
                    primary_key: Some("id".to_string()),
                    srid: Some(4326),
                    geometry_type: Some("POLYGON".to_string()),
                },
            ],
        });
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: multi-table-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 14 }
    geometry_variants:
      - column: geom_z6
        minzoom: 0
        maxzoom: 6
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(MultiTableFactory { catalog }));
        let router = Router::build(&config, &registry).unwrap();

        let error = router.validate_catalog().await.expect_err(
            "a variant column with a different geometry type than the base must refuse boot",
        );
        let message = error.to_string();
        assert!(
            message.contains("demo"),
            "message must name the collection: {message}"
        );
        assert!(
            message.contains("geom_z6"),
            "message must name the mismatched variant column: {message}"
        );
        assert!(
            message.contains("geometry type"),
            "message must call out the geometry-type mismatch: {message}"
        );
    }

    /// `#104`: `geom_z6` exists, shares the pinned base column's SRID and
    /// geometry type — boot must succeed exactly as it would with no
    /// `geometry_variants` declared at all.
    #[tokio::test]
    async fn validate_catalog_passes_when_a_declared_geometry_variant_matches_the_base_columns_srid_and_type(
    ) {
        let catalog = Arc::new(MultiTableCatalog {
            physicals: vec![
                PhysicalCollection {
                    name: "demo".to_string(),
                    geometry_column: Some("geom".to_string()),
                    primary_key: Some("id".to_string()),
                    srid: Some(4326),
                    geometry_type: Some("POINT".to_string()),
                },
                PhysicalCollection {
                    name: "demo".to_string(),
                    geometry_column: Some("geom_z6".to_string()),
                    primary_key: Some("id".to_string()),
                    srid: Some(4326),
                    geometry_type: Some("POINT".to_string()),
                },
            ],
        });
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: multi-table-fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
    geometry: geom
    pk: id
    tiles: { minzoom: 0, maxzoom: 14 }
    geometry_variants:
      - column: geom_z6
        minzoom: 0
        maxzoom: 6
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(MultiTableFactory { catalog }));
        let router = Router::build(&config, &registry).unwrap();

        router.validate_catalog().await.expect(
            "a variant matching the base column's srid and geometry type must boot cleanly",
        );
    }

    // -- declared schema reconciliation (`#44`) ------------------------------

    /// Builds a `descriptor-fake`-backed single-collection config exactly
    /// like `descriptor_config`, but with an inline `schema:` block appended
    /// under the collection — kept separate rather than growing
    /// `descriptor_config`'s own signature, since only these reconciliation
    /// tests need a declared schema.
    fn descriptor_config_with_schema(schema_yaml: &str) -> AppConfig {
        let yaml = format!(
            "storages: [ {{ id: main, driver: descriptor-fake, url_env: DATABASE_URL }} ]\n\
             tenants: [ {{ id: public }} ]\n\
             catalogs: [ {{ id: default, tenant: public }} ]\n\
             collections:\n  - id: demo\n    catalog: default\n    storage: main\n\
             {schema_yaml}"
        );
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        config
    }

    #[tokio::test]
    async fn validate_catalog_passes_when_a_declared_schema_matches_the_backend() {
        let attributes = vec![AttributeColumn {
            name: "population".to_string(),
            sql_type: "integer".to_string(),
        }];
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(100, attributes, "observed_at"),
        );
        let config = descriptor_config_with_schema(
            "    schema:\n      properties: [ { name: population, type: integer } ]\n",
        );
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        router.validate_catalog().await.unwrap();
    }

    /// The reconciliation half of `#44`: a declared property missing from
    /// the backend's attribute schema fails boot, naming the collection and
    /// the property — the same fail-fast discipline
    /// `validate_catalog_fails_fast_when_a_physical_field_cannot_be_derived`
    /// applies to `geometry`/`pk`.
    #[tokio::test]
    async fn validate_catalog_fails_fast_when_a_declared_property_is_missing_from_the_backend() {
        let attributes = vec![AttributeColumn {
            name: "name".to_string(),
            sql_type: "text".to_string(),
        }];
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(100, attributes, "observed_at"),
        );
        let config = descriptor_config_with_schema(
            "    schema:\n      properties: [ { name: population, type: integer } ]\n",
        );
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        match router.validate_catalog().await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("population"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// Type mismatch: the backend reports `population` as `text`, but the
    /// schema declares it `integer` — the error names the property plus
    /// both the declared and the actual (backend) type.
    #[tokio::test]
    async fn validate_catalog_fails_fast_when_a_declared_property_type_mismatches_the_backend() {
        let attributes = vec![AttributeColumn {
            name: "population".to_string(),
            sql_type: "text".to_string(),
        }];
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(100, attributes, "observed_at"),
        );
        let config = descriptor_config_with_schema(
            "    schema:\n      properties: [ { name: population, type: integer } ]\n",
        );
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        match router.validate_catalog().await {
            Err(Error::Config(message)) => {
                assert!(message.contains("population"), "message was: {message}");
                assert!(message.contains("integer"), "message was: {message}");
                assert!(message.contains("text"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// A declared property that names the collection's own geometry column
    /// is itself a mismatch — geometry has no `PropertyType`, so it can
    /// never appear in a declared schema's flat property model.
    #[tokio::test]
    async fn validate_catalog_fails_fast_when_a_declared_property_names_the_geometry_column() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config_with_schema(
            "    schema:\n      properties: [ { name: geom, type: string } ]\n",
        );
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        match router.validate_catalog().await {
            Err(Error::Config(message)) => {
                assert!(message.contains("geom"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// No-regression guard (`#44`): a collection with no `schema:` key at
    /// all boots exactly as before this feature existed — `validate_catalog`
    /// never even looks at reconciliation.
    #[tokio::test]
    async fn validate_catalog_passes_unchanged_when_no_schema_is_declared() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        router.validate_catalog().await.unwrap();
    }

    // -- vector-tile property allowlist reconciliation (`#85`) ---------------

    /// Builds a single-collection config against the `descriptor-fake` driver
    /// exactly like `descriptor_config`, but with an inline `settings:` block
    /// appended under the collection — kept separate rather than growing
    /// `descriptor_config`'s own signature, mirroring
    /// `descriptor_config_with_schema`.
    fn descriptor_config_with_settings(settings_yaml: &str) -> AppConfig {
        let yaml = format!(
            "storages: [ {{ id: main, driver: descriptor-fake, url_env: DATABASE_URL }} ]\n\
             tenants: [ {{ id: public }} ]\n\
             catalogs: [ {{ id: default, tenant: public }} ]\n\
             collections:\n  - id: demo\n    catalog: default\n    storage: main\n\
             {settings_yaml}"
        );
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        config
    }

    #[tokio::test]
    async fn validate_catalog_passes_when_tile_properties_matches_the_backend() {
        let attributes = vec![AttributeColumn {
            name: "name".to_string(),
            sql_type: "text".to_string(),
        }];
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(100, attributes, "observed_at"),
        );
        let config =
            descriptor_config_with_settings("    settings:\n      tile_properties: [name]\n");
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        router.validate_catalog().await.unwrap();
    }

    /// The reconciliation half of `#85`: an allowlisted column missing from
    /// the backend's attribute schema fails boot, naming the collection and
    /// the property — the same fail-fast discipline
    /// `validate_catalog_fails_fast_when_a_declared_property_is_missing_from_the_backend`
    /// applies to a declared `SchemaDecl` property.
    #[tokio::test]
    async fn validate_catalog_fails_fast_when_tile_properties_names_an_unknown_column() {
        let attributes = vec![AttributeColumn {
            name: "name".to_string(),
            sql_type: "text".to_string(),
        }];
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(100, attributes, "observed_at"),
        );
        let config =
            descriptor_config_with_settings("    settings:\n      tile_properties: [pop]\n");
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        match router.validate_catalog().await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("pop"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// An allowlisted column naming the collection's own geometry column is
    /// itself a mismatch — same rule `SchemaDecl` reconciliation already
    /// applies.
    #[tokio::test]
    async fn validate_catalog_fails_fast_when_tile_properties_names_the_geometry_column() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config =
            descriptor_config_with_settings("    settings:\n      tile_properties: [geom]\n");
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        match router.validate_catalog().await {
            Err(Error::Config(message)) => {
                assert!(message.contains("geom"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {other:?}"),
        }
    }

    /// `Router::effective_decl` carries the resolved allowlist onto the decl
    /// a driver actually receives — the settings-chain overlay
    /// `apply_inherited_tile_properties` applies, mirroring
    /// `apply_inherited_tile_caps`'s own coverage.
    #[tokio::test]
    async fn resolve_features_carries_the_resolved_tile_properties_onto_the_decl() {
        let attributes = vec![AttributeColumn {
            name: "name".to_string(),
            sql_type: "text".to_string(),
        }];
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(100, attributes, "observed_at"),
        );
        let config =
            descriptor_config_with_settings("    settings:\n      tile_properties: [name]\n");
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.tile_properties, vec!["name".to_string()]);
    }

    /// No-regression guard (`#85`): a collection with no `tile_properties`
    /// declared anywhere in the settings chain resolves to an empty
    /// allowlist — pk-only, exactly the behavior every collection had
    /// before this feature existed.
    #[tokio::test]
    async fn resolve_features_leaves_tile_properties_empty_when_none_is_declared() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert!(decl.tile_properties.is_empty());
    }

    /// `#19`: `collection_descriptor` (the always-derives inspection path —
    /// see `collection_descriptor_exposes_the_derived_extent_even_when_fully_overridden`
    /// above) exposes the richer descriptor fields, not just extent.
    #[tokio::test]
    async fn collection_descriptor_exposes_row_estimate_attributes_and_datetime() {
        let attributes = vec![AttributeColumn {
            name: "name".to_string(),
            sql_type: "text".to_string(),
        }];
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(12_345, attributes.clone(), "observed_at"),
        );
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let descriptor = router
            .collection_descriptor("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(descriptor.row_estimate, Some(12_345));
        assert_eq!(descriptor.attributes, Some(attributes));
        assert_eq!(descriptor.datetime.as_deref(), Some("observed_at"));
    }

    /// `#19`: a collection that leaves `datetime` unset derives it from the
    /// backend's single temporal-column candidate, exactly like `geometry`/
    /// `pk`.
    #[tokio::test]
    async fn resolve_features_derives_datetime_from_a_single_temporal_column_when_not_overridden() {
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(500, vec![], "observed_at"),
        );
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.datetime.as_deref(), Some("observed_at"));
    }

    /// `#19`: an explicit `datetime` override wins even when it diverges from
    /// the backend's derived temporal column — same precedence rule as
    /// `geometry`/`pk`.
    #[tokio::test]
    async fn resolve_features_honors_a_datetime_override_that_diverges_from_the_backend() {
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(500, vec![], "observed_at"),
        );
        let mut yaml = "server: { descriptor_ttl_s: 300 }\n\
             storages: [ { id: main, driver: descriptor-fake, url_env: DATABASE_URL } ]\n\
             tenants: [ { id: public } ]\n\
             catalogs: [ { id: default, tenant: public } ]\n\
             collections:\n  - id: demo\n    catalog: default\n    storage: main\n"
            .to_string();
        yaml.push_str("    datetime: captured_at\n");
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            decl.datetime.as_deref(),
            Some("captured_at"),
            "an override must win even though it contradicts the derived temporal column"
        );
    }

    /// `#19`: `row_estimate` flows onto the effective decl (not just the
    /// descriptor) for a collection that goes through derivation — it's what
    /// `descriptor::heuristics::effective_feature_cap` reads at tile-serving
    /// time.
    #[tokio::test]
    async fn resolve_features_carries_the_row_estimate_onto_the_effective_decl() {
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(98_765, vec![], "observed_at"),
        );
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.row_estimate, Some(98_765));
    }

    /// `#36`: a driver's `CatalogSource::projection` answer rides the
    /// derived descriptor onto both the effective decl (`decl.projection`,
    /// the STAC Items lane's carrier) and the canonical descriptor
    /// (`CanonicalDescriptor::projection`, the STAC Collection document's) —
    /// and a driver that never overrides the accessor leaves both `None`,
    /// exactly the pre-`#36` shape.
    #[tokio::test]
    async fn resolve_features_and_canonical_descriptor_carry_the_projection_facts() {
        let facts = crate::catalog::ProjectionFacts {
            epsg: Some(4326),
            transform: Some([0.01, 0.0, -1.28, 0.0, -0.01, 1.28]),
            shape: Some([256, 256]),
        };
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_projection(facts),
        );
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.projection, Some(facts));

        let canonical = router
            .canonical_descriptor("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(canonical.projection, Some(facts));
    }

    /// `#36`'s no-knowledge half: a driver that never overrides
    /// `CatalogSource::projection` resolves with `projection: None`
    /// everywhere — nothing is invented on the way through the descriptor.
    #[tokio::test]
    async fn resolve_features_leaves_projection_none_when_the_driver_never_reports_it() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = descriptor_config(None, None, None, 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.projection, None);

        let canonical = router
            .canonical_descriptor("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(canonical.projection, None);
    }

    /// `#19` design decision: a collection with `table`/`geometry`/`pk` all
    /// overridden takes `effective_decl`'s fast path and never derives
    /// `datetime`/`row_estimate` either, even though the backend has answers
    /// for both — `collection_descriptor` (previous test) still reports them
    /// for inspection, but the decl handed to a driver does not carry them.
    /// See `effective_decl`'s doc comment for the rationale.
    ///
    /// `#61`: under `eager` (the default `descriptor_config` uses here, and
    /// what this test specifically exercises), the fast path also still
    /// costs zero catalog calls of its own — `verify_pinned_collection` is
    /// lazy-only, since `eager`'s boot sweep (`validate_catalog`) already
    /// checked every collection, pinned or not, before a request could ever
    /// reach this path. Contrast
    /// `effective_decl_fast_path_never_derives_but_runs_one_cached_verification_probe_under_lazy`
    /// below, where the same never-derives guarantee holds but the catalog
    /// call count is 1, not 0.
    #[tokio::test]
    async fn effective_decl_fast_path_leaves_datetime_and_row_estimate_unset_when_physical_fields_are_all_overridden_under_eager(
    ) {
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(98_765, vec![], "observed_at"),
        );
        let config = descriptor_config(Some("demo"), Some("geom"), Some("id"), 300);
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.datetime, None);
        assert_eq!(decl.row_estimate, None);
        assert_eq!(
            catalog.collections_calls(),
            0,
            "under eager, a fully-overridden collection's fast path must still never touch \
             the catalog on its own — validate_catalog's boot sweep already covered it"
        );
    }

    /// `#61`: the lazy-mode counterpart of the `eager` test above — same
    /// never-derives guarantee (`datetime`/`row_estimate` stay `None`, the
    /// pinned contract is intact), but under `lazy` the fast path now owes
    /// this collection exactly one cached verification probe on first touch,
    /// and a second touch must not repeat it.
    #[tokio::test]
    async fn effective_decl_fast_path_never_derives_but_runs_one_cached_verification_probe_under_lazy(
    ) {
        let catalog = Arc::new(
            DescriptorFakeCatalog::new(descriptor_physical(Some("geom"), Some("id")), None)
                .with_richer_fields(98_765, vec![], "observed_at"),
        );
        let config = lazy_descriptor_config(Some("demo"), Some("geom"), Some("id"));
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        let (decl, _source) = router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            decl.datetime, None,
            "the pin still never derives datetime, lazy or not"
        );
        assert_eq!(
            decl.row_estimate, None,
            "the pin still never derives row_estimate, lazy or not"
        );
        assert_eq!(
            catalog.collections_calls(),
            1,
            "lazy owes a fully-pinned collection exactly one verification probe on first touch"
        );

        router
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            catalog.collections_calls(),
            1,
            "the verification verdict is cached — a second touch must not repeat the probe"
        );
    }

    /// `#61`: a typo'd `table` pin under `lazy` fails at first touch with a
    /// named `Error::Config` naming the collection and the missing table —
    /// not a raw query-time error, the gap this feature closes.
    #[tokio::test]
    async fn lazy_fully_pinned_typoed_table_fails_first_touch_with_a_named_error() {
        // The fake catalog only ever reports a table named "demo"
        // (`descriptor_physical`); pinning `table` to anything else
        // deterministically exercises the typo'd-pin case.
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = lazy_descriptor_config(Some("nonexistent_table"), Some("geom"), Some("id"));
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        match router.resolve_features("public", "default", "demo").await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(
                    message.contains("nonexistent_table"),
                    "message was: {message}"
                );
            }
            other => panic!(
                "expected a named Error::Config at first touch, not a raw query error; \
                 got is_ok = {}",
                other.is_ok()
            ),
        }
    }

    /// `#61`: a typo'd `geometry` pin under `lazy` fails at first touch
    /// naming the collection and the missing column — the table itself
    /// exists, only the declared geometry column does not.
    #[tokio::test]
    async fn lazy_fully_pinned_typoed_geometry_column_fails_first_touch_with_a_named_error() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = lazy_descriptor_config(Some("demo"), Some("the_geom"), Some("id"));
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        match router.resolve_features("public", "default", "demo").await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("the_geom"), "message was: {message}");
            }
            other => panic!(
                "expected a named Error::Config naming the missing geometry column; \
                 got is_ok = {}",
                other.is_ok()
            ),
        }
    }

    /// `#61`: a typo'd `pk` pin under `lazy` fails first touch the same way,
    /// naming the collection and the missing column.
    #[tokio::test]
    async fn lazy_fully_pinned_typoed_pk_column_fails_first_touch_with_a_named_error() {
        let catalog = Arc::new(DescriptorFakeCatalog::new(
            descriptor_physical(Some("geom"), Some("id")),
            None,
        ));
        let config = lazy_descriptor_config(Some("demo"), Some("geom"), Some("gid"));
        let router = build_descriptor_router(Arc::clone(&catalog), &config);

        match router.resolve_features("public", "default", "demo").await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("gid"), "message was: {message}");
            }
            other => panic!(
                "expected a named Error::Config naming the missing pk column; got is_ok = {}",
                other.is_ok()
            ),
        }
    }

    /// A `VolumeSource` that reports a fixed one-triangle mesh for every
    /// coordinate — enough to prove `resolve_volume` hands back a real
    /// driver capability, not to exercise any particular geometry shape
    /// (the places lane's own tests, in `tellurion-places`, cover
    /// consumption end to end).
    struct FakeVolumeSource;

    #[async_trait::async_trait]
    impl VolumeSource for FakeVolumeSource {
        async fn volume_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&Filter>,
        ) -> Result<Option<VolumeMesh>> {
            Ok(Some(VolumeMesh {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 5.0]],
                indices: vec![0, 1, 2],
            }))
        }
    }

    /// A driver whose `VolumeSource`/`TileSource` advertisement is each
    /// independently configurable, purpose-built for `resolve_volume` tests
    /// (`#15`).
    struct VolumeFakeDriver {
        volume: bool,
        tiles: bool,
    }

    impl StorageDriver for VolumeFakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![]))
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            self.tiles
                .then(|| Arc::new(FakeTilesOnlyDriver) as Arc<dyn TileSource>)
        }

        fn volume_source(&self) -> Option<Arc<dyn VolumeSource>> {
            self.volume
                .then(|| Arc::new(FakeVolumeSource) as Arc<dyn VolumeSource>)
        }
    }

    struct VolumeFakeFactory {
        name: &'static str,
        volume: bool,
        tiles: bool,
    }

    impl DriverFactory for VolumeFakeFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(VolumeFakeDriver {
                volume: self.volume,
                tiles: self.tiles,
            }))
        }
    }

    /// A single, unrouted collection against `driver` — the "tiles lane
    /// defaults to the single `storage`" case `resolve_volume` relies on for
    /// its zero-added-lookup fast path.
    fn single_storage_config(driver: &str) -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: {driver}, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
"#
        ))
        .unwrap();
        config.validate().unwrap();
        config
    }

    #[tokio::test]
    async fn resolve_volume_detects_a_driver_that_advertises_it() {
        let config = single_storage_config("volume-fake-a");
        let mut registry = Registry::new();
        registry.register(Arc::new(VolumeFakeFactory {
            name: "volume-fake-a",
            volume: true,
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let source = router
            .resolve_volume("public", "default", "demo")
            .await
            .unwrap();
        assert!(
            source.is_some(),
            "a driver that implements volume_source must resolve to Some"
        );
    }

    #[tokio::test]
    async fn resolve_volume_is_none_when_the_driver_never_advertises_it() {
        let config = single_storage_config("volume-fake-b");
        let mut registry = Registry::new();
        registry.register(Arc::new(VolumeFakeFactory {
            name: "volume-fake-b",
            volume: false,
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let source = router
            .resolve_volume("public", "default", "demo")
            .await
            .unwrap();
        assert!(
            source.is_none(),
            "the default StorageDriver::volume_source must resolve to None, the signal the \
             places3d lane uses to fall back to extrusion"
        );
    }

    #[tokio::test]
    async fn resolve_volume_unknown_collection_is_not_found() {
        let config = single_storage_config("volume-fake-c");
        let mut registry = Registry::new();
        registry.register(Arc::new(VolumeFakeFactory {
            name: "volume-fake-c",
            volume: true,
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        assert!(matches!(
            router.resolve_volume("public", "default", "missing").await,
            Err(Error::NotFound)
        ));
    }

    /// `#15`: only the tiles lane's *primary* entry's `VolumeSource` is ever
    /// consulted — a fallback tail entry (a different, independently
    /// configured backend) must never be silently substituted in for real
    /// solid geometry the way `tiles_source` substitutes a tail entry's MVT
    /// bytes when the primary's call errors; a missing `VolumeSource` is not
    /// an error to recover from, it's the ordinary trigger for the places3d
    /// extrusion fallback.
    #[tokio::test]
    async fn resolve_volume_never_falls_through_to_a_tail_entry_that_has_it() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: primary, driver: volume-fake-primary, url_env: DATABASE_URL }
  - { id: tail, driver: volume-fake-tail, url_env: DATABASE_URL2 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: primary
    routing: { tiles: [primary, tail] }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(VolumeFakeFactory {
            name: "volume-fake-primary",
            volume: false,
            tiles: true,
        }));
        registry.register(Arc::new(VolumeFakeFactory {
            name: "volume-fake-tail",
            volume: true,
            tiles: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let source = router
            .resolve_volume("public", "default", "demo")
            .await
            .unwrap();
        assert!(
            source.is_none(),
            "a tail entry's VolumeSource must never be substituted for the primary's absence"
        );
    }

    /// `#70`: a `CatalogSource` double that reports one real physical row for
    /// "demo" — table/geometry/pk match `single_storage_config`'s unrouted
    /// collection by convention, only `geometry_type` varies per test — so
    /// `resolve_volume`'s own `resolved_descriptor` call actually succeeds
    /// instead of hitting the "derivation failed, trust the driver-wide
    /// signal" fallback every other `resolve_volume` test in this module
    /// relies on.
    fn geometry_typed_catalog(geometry_type: &str) -> Arc<dyn CatalogSource> {
        Arc::new(FakeCatalog(vec![PhysicalCollection {
            name: "demo".to_string(),
            geometry_column: Some("geom".to_string()),
            primary_key: Some("id".to_string()),
            srid: Some(4326),
            geometry_type: Some(geometry_type.to_string()),
        }]))
    }

    struct GeometryTypedVolumeDriver {
        catalog: Arc<dyn CatalogSource>,
    }

    impl StorageDriver for GeometryTypedVolumeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::clone(&self.catalog)
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            Some(Arc::new(FakeTilesOnlyDriver) as Arc<dyn TileSource>)
        }

        fn volume_source(&self) -> Option<Arc<dyn VolumeSource>> {
            Some(Arc::new(FakeVolumeSource) as Arc<dyn VolumeSource>)
        }
    }

    struct GeometryTypedVolumeFactory {
        name: &'static str,
        geometry_type: &'static str,
    }

    impl DriverFactory for GeometryTypedVolumeFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(GeometryTypedVolumeDriver {
                catalog: geometry_typed_catalog(self.geometry_type),
            }))
        }
    }

    /// `#70`: a driver-wide `VolumeSource` answer is not enough on its own —
    /// a collection whose own descriptor-derived `geometry_type` isn't one
    /// of `is_volume_capable_geometry_type`'s names (a flat footprint column,
    /// here) must fall back to `None`, the extrusion trigger, even though
    /// `StorageDriver::volume_source` returns `Some`.
    #[tokio::test]
    async fn resolve_volume_declines_a_collection_whose_own_geometry_type_is_not_solid() {
        let config = single_storage_config("volume-fake-footprint-type");
        let mut registry = Registry::new();
        registry.register(Arc::new(GeometryTypedVolumeFactory {
            name: "volume-fake-footprint-type",
            geometry_type: "POLYGON",
        }));
        let router = Router::build(&config, &registry).unwrap();

        let source = router
            .resolve_volume("public", "default", "demo")
            .await
            .unwrap();
        assert!(
            source.is_none(),
            "a flat POLYGON geometry_type must decline the driver-wide VolumeSource answer"
        );
    }

    /// Counterpart of the above: a collection whose own `geometry_type` IS
    /// one of the solid names keeps the driver-wide `Some` answer.
    #[tokio::test]
    async fn resolve_volume_keeps_a_collection_whose_own_geometry_type_is_solid() {
        let config = single_storage_config("volume-fake-solid-type");
        let mut registry = Registry::new();
        registry.register(Arc::new(GeometryTypedVolumeFactory {
            name: "volume-fake-solid-type",
            geometry_type: "POLYHEDRALSURFACE",
        }));
        let router = Router::build(&config, &registry).unwrap();

        let source = router
            .resolve_volume("public", "default", "demo")
            .await
            .unwrap();
        assert!(
            source.is_some(),
            "a POLYHEDRALSURFACE geometry_type must keep the driver-wide VolumeSource answer"
        );
    }

    // -- `resolve_stac_metadata` (`#202`) -----------------------------------

    /// A `StacMetadataSource` that answers a fixed doc for every id asked
    /// for — enough to prove `resolve_stac_metadata` hands back a real
    /// driver capability; the merge itself is `tellurion-stac`'s own test
    /// surface.
    struct FakeStacMetadataSource;

    #[async_trait::async_trait]
    impl StacMetadataSource for FakeStacMetadataSource {
        async fn stac_metadata(
            &self,
            _collection: &CollectionDecl,
            feature_ids: &[String],
        ) -> Result<HashMap<String, serde_json::Value>> {
            Ok(feature_ids
                .iter()
                .map(|id| (id.clone(), serde_json::json!({"properties": {"x": 1}})))
                .collect())
        }
    }

    /// A driver whose `stac_metadata_source` advertisement is configurable,
    /// purpose-built for `resolve_stac_metadata`'s three answers.
    struct StacSidecarFakeDriver {
        sidecar: bool,
    }

    impl StorageDriver for StacSidecarFakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![]))
        }

        fn stac_metadata_source(&self) -> Option<Arc<dyn StacMetadataSource>> {
            self.sidecar
                .then(|| Arc::new(FakeStacMetadataSource) as Arc<dyn StacMetadataSource>)
        }
    }

    struct StacSidecarFakeFactory {
        name: &'static str,
        sidecar: bool,
    }

    impl DriverFactory for StacSidecarFakeFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(StacSidecarFakeDriver {
                sidecar: self.sidecar,
            }))
        }
    }

    fn stac_sidecar_config(driver: &str, stac_metadata: bool) -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: {driver}, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    stac_metadata: {stac_metadata}
"#
        ))
        .unwrap();
        config.validate().unwrap();
        config
    }

    /// The ordinary answer for every collection that predates `#202`:
    /// `Ok(None)`, never a refusal — and reached without ever asking the
    /// driver, so a collection that never opted in cannot be affected by
    /// what its storage does or does not advertise.
    #[tokio::test]
    async fn resolve_stac_metadata_is_none_when_the_collection_never_opted_in() {
        let config = stac_sidecar_config("stac-fake-a", false);
        let mut registry = Registry::new();
        registry.register(Arc::new(StacSidecarFakeFactory {
            name: "stac-fake-a",
            sidecar: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let source = router
            .resolve_stac_metadata("public", "default", "demo")
            .await
            .unwrap();
        assert!(
            source.is_none(),
            "a collection with no stac_metadata opt-in must resolve to None even against a \
             driver that advertises the capability"
        );
    }

    #[tokio::test]
    async fn resolve_stac_metadata_detects_a_driver_that_advertises_it() {
        let config = stac_sidecar_config("stac-fake-b", true);
        let mut registry = Registry::new();
        registry.register(Arc::new(StacSidecarFakeFactory {
            name: "stac-fake-b",
            sidecar: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let source = router
            .resolve_stac_metadata("public", "default", "demo")
            .await
            .unwrap();
        assert!(
            source.is_some(),
            "an opted-in collection on a capable driver must resolve to Some"
        );
    }

    /// An opted-in collection whose anchor driver advertises nothing is a
    /// NAMED refusal, not another `Ok(None)`: silently serving Items with no
    /// sidecar would be indistinguishable from the un-opted-in case above.
    #[tokio::test]
    async fn resolve_stac_metadata_refuses_an_opted_in_collection_on_an_incapable_driver() {
        let config = stac_sidecar_config("stac-fake-c", true);
        let mut registry = Registry::new();
        registry.register(Arc::new(StacSidecarFakeFactory {
            name: "stac-fake-c",
            sidecar: false,
        }));
        let router = Router::build(&config, &registry).unwrap();

        match router
            .resolve_stac_metadata("public", "default", "demo")
            .await
        {
            Err(Error::CapabilityUnsupported {
                collection,
                capability,
            }) => {
                assert_eq!(collection, "demo");
                assert_eq!(capability, "stac-metadata");
            }
            Err(other) => panic!("expected a named CapabilityUnsupported refusal, got {other:?}"),
            Ok(_) => panic!(
                "an opted-in collection on a driver advertising no stac_metadata_source must \
                 refuse by name, never resolve"
            ),
        }
    }

    // -- `resolve_item_assets` (`#221`) -------------------------------------

    /// An `AssetRecordStore` that only ever answers the batched read —
    /// enough to prove `resolve_item_assets` hands back a real driver
    /// capability. Every other method is unreachable through that resolver
    /// (the assets API resolves its own store via `resolve_assets`), so
    /// they refuse rather than pretend.
    struct FakeAssetRecordStore;

    #[async_trait::async_trait]
    impl AssetRecordStore for FakeAssetRecordStore {
        async fn register(
            &self,
            _collection: &CollectionDecl,
            _item_id: Option<&str>,
            _key: &str,
            _new_record: crate::asset::NewAssetRecord,
        ) -> Result<crate::asset::AssetRecord> {
            Err(Error::NotFound)
        }

        async fn get(
            &self,
            _collection: &CollectionDecl,
            _item_id: Option<&str>,
            _key: &str,
        ) -> Result<Option<crate::asset::AssetRecord>> {
            Ok(None)
        }

        async fn finalize(
            &self,
            _collection: &CollectionDecl,
            _item_id: Option<&str>,
            _key: &str,
            _outcome: crate::asset::FinalizeOutcome,
        ) -> Result<crate::asset::AssetRecord> {
            Err(Error::NotFound)
        }

        async fn delete(
            &self,
            _collection: &CollectionDecl,
            _item_id: Option<&str>,
            _key: &str,
        ) -> Result<Option<crate::asset::AssetRecord>> {
            Ok(None)
        }

        async fn list(
            &self,
            _collection: &CollectionDecl,
        ) -> Result<Vec<crate::asset::AssetRecordEntry>> {
            Ok(Vec::new())
        }

        async fn item_assets(
            &self,
            _collection: &CollectionDecl,
            _item_ids: &[String],
        ) -> Result<Vec<crate::asset::AssetRecordEntry>> {
            Ok(Vec::new())
        }
    }

    /// A driver whose `asset_record_store` advertisement is configurable,
    /// purpose-built for `resolve_item_assets`'s three answers.
    struct ItemAssetsFakeDriver {
        assets: bool,
    }

    impl StorageDriver for ItemAssetsFakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![]))
        }

        fn asset_record_store(&self) -> Option<Arc<dyn AssetRecordStore>> {
            self.assets
                .then(|| Arc::new(FakeAssetRecordStore) as Arc<dyn AssetRecordStore>)
        }
    }

    struct ItemAssetsFakeFactory {
        name: &'static str,
        assets: bool,
    }

    impl DriverFactory for ItemAssetsFakeFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(ItemAssetsFakeDriver {
                assets: self.assets,
            }))
        }
    }

    fn item_assets_config(driver: &str, stac_item_assets: bool) -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: {driver}, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    stac_item_assets: {stac_item_assets}
"#
        ))
        .unwrap();
        config.validate().unwrap();
        config
    }

    /// The ordinary answer for every collection that predates `#221`:
    /// `Ok(None)`, never a refusal — and reached without ever asking the
    /// driver, so a collection that never opted in cannot be affected by
    /// what its storage does or does not advertise. This is the entire
    /// "unconfigured deployments serve byte-identical Items" guarantee at
    /// the routing layer.
    #[tokio::test]
    async fn resolve_item_assets_is_none_when_the_collection_never_opted_in() {
        let config = item_assets_config("item-assets-fake-a", false);
        let mut registry = Registry::new();
        registry.register(Arc::new(ItemAssetsFakeFactory {
            name: "item-assets-fake-a",
            assets: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let store = router
            .resolve_item_assets("public", "default", "demo")
            .await
            .unwrap();
        assert!(
            store.is_none(),
            "a collection with no stac_item_assets opt-in must resolve to None even against a \
             driver that advertises the capability"
        );
    }

    #[tokio::test]
    async fn resolve_item_assets_detects_a_driver_that_advertises_it() {
        let config = item_assets_config("item-assets-fake-b", true);
        let mut registry = Registry::new();
        registry.register(Arc::new(ItemAssetsFakeFactory {
            name: "item-assets-fake-b",
            assets: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        let store = router
            .resolve_item_assets("public", "default", "demo")
            .await
            .unwrap();
        assert!(
            store.is_some(),
            "an opted-in collection on a capable driver must resolve to Some"
        );
    }

    /// An opted-in collection whose anchor driver advertises nothing is a
    /// NAMED refusal, not another `Ok(None)`: silently serving Items with
    /// no per-item assets would be indistinguishable from the un-opted-in
    /// case above. Same `"assets"` capability name `resolve_assets` already
    /// refuses under — one capability, one name.
    #[tokio::test]
    async fn resolve_item_assets_refuses_an_opted_in_collection_on_an_incapable_driver() {
        let config = item_assets_config("item-assets-fake-c", true);
        let mut registry = Registry::new();
        registry.register(Arc::new(ItemAssetsFakeFactory {
            name: "item-assets-fake-c",
            assets: false,
        }));
        let router = Router::build(&config, &registry).unwrap();

        match router
            .resolve_item_assets("public", "default", "demo")
            .await
        {
            Err(Error::CapabilityUnsupported {
                collection,
                capability,
            }) => {
                assert_eq!(collection, "demo");
                assert_eq!(capability, "assets");
            }
            Err(other) => panic!("expected a named CapabilityUnsupported refusal, got {other:?}"),
            Ok(_) => panic!(
                "an opted-in collection on a driver advertising no asset_record_store must \
                 refuse by name, never resolve"
            ),
        }
    }

    #[tokio::test]
    async fn resolve_item_assets_unknown_collection_is_not_found() {
        let config = item_assets_config("item-assets-fake-d", true);
        let mut registry = Registry::new();
        registry.register(Arc::new(ItemAssetsFakeFactory {
            name: "item-assets-fake-d",
            assets: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        assert!(matches!(
            router
                .resolve_item_assets("public", "default", "missing")
                .await,
            Err(Error::NotFound)
        ));
    }

    #[tokio::test]
    async fn resolve_stac_metadata_unknown_collection_is_not_found() {
        let config = stac_sidecar_config("stac-fake-d", true);
        let mut registry = Registry::new();
        registry.register(Arc::new(StacSidecarFakeFactory {
            name: "stac-fake-d",
            sidecar: true,
        }));
        let router = Router::build(&config, &registry).unwrap();

        assert!(matches!(
            router
                .resolve_stac_metadata("public", "default", "missing")
                .await,
            Err(Error::NotFound)
        ));
    }

    // -- `build_from_snapshot` (`#42`, third slice) -------------------------
    //
    // See `context::tests` for `build_router_and_resolver`'s own tests
    // (dispatch on `registry.backend`, a collection sourced entirely from a
    // registry reader, snapshot validation failures) — that function needs
    // both `Router` and `StaticResolver`, so its tests live where both are
    // already in scope rather than duplicating a `Resolver` story here.

    /// Builds a fully-specified single-tenant/catalog/collection `AppConfig`
    /// — `table`/`geometry`/`pk` all overridden, so `effective_decl` takes
    /// its fast path and `resolve_features` needs no backend I/O.
    fn db_side_config(collection_external_id: &str) -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    external_id: {collection_external_id}
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
"#
        ))
        .unwrap();
        config.validate().unwrap();
        config
    }

    fn fake_registry() -> Registry {
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            features: true,
            tiles: false,
        }));
        registry
    }

    /// `Router::build_from_snapshot`, fed the exact same `catalogs`/
    /// `collections` `Router::build` would have read off `config.catalogs`/
    /// `.collections` itself, resolves a collection identically either way —
    /// the "YAML path stays byte-for-byte identical" guarantee (`#42`, third
    /// slice), proven by comparing the two build paths' resolved decls
    /// directly rather than trusting they merely "look similar."
    #[tokio::test]
    async fn build_from_snapshot_is_equivalent_to_build_for_the_same_declarations() {
        let db_config = db_side_config("demo-ext");
        let registry = fake_registry();

        let via_build = Router::build(&db_config, &registry).unwrap();
        let (via_build_decl, _) = via_build
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();

        // A separate, otherwise-empty `AppConfig` supplies everything
        // `build_from_snapshot` still reads off `config` directly (storages,
        // tenants) — `catalogs`/`collections` come only from the explicit
        // slices, never from this config's own (empty) fields, which is
        // exactly what proves the snapshot is the real routing input.
        let empty_config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
"#,
        )
        .unwrap();
        let via_snapshot = Router::build_from_snapshot(
            &empty_config,
            &empty_config.tenants,
            &db_config.catalogs,
            &db_config.collections,
            &registry,
        )
        .unwrap();
        let (via_snapshot_decl, _) = via_snapshot
            .resolve_features("public", "default", "demo")
            .await
            .unwrap();

        assert_eq!(via_build_decl, via_snapshot_decl);
    }

    /// `#59`: `collection_count` reports the snapshot's own count — one
    /// here — even though `empty_config.collections` (the relational-
    /// backend double-source rule's empty section) is zero. This is the
    /// exact gap the reload-log fix relies on: `config.collections.len()`
    /// alone would have reported `0` for a relational-backend reload no
    /// matter how many collections the registry actually indexed.
    #[test]
    fn collection_count_reflects_the_snapshot_not_configs_own_collections_field() {
        let db_config = db_side_config("demo-ext");
        let registry = fake_registry();
        let empty_config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
"#,
        )
        .unwrap();
        assert!(empty_config.collections.is_empty());

        let via_snapshot = Router::build_from_snapshot(
            &empty_config,
            &empty_config.tenants,
            &db_config.catalogs,
            &db_config.collections,
            &registry,
        )
        .unwrap();
        assert_eq!(via_snapshot.collection_count(), 1);
    }

    // -- collection kind (`#192`) ---------------------------------------------

    fn kind_config(kinds: &[(&str, &str)]) -> (AppConfig, Registry) {
        let collections: String = kinds
            .iter()
            .map(|(id, kind)| {
                let kind_line = if kind.is_empty() {
                    String::new()
                } else {
                    format!("    kind: {kind}\n")
                };
                format!("  - id: {id}\n    catalog: default\n    storage: main\n{kind_line}")
            })
            .collect();
        let yaml = format!(
            "storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]\n\
             tenants: [ {{ id: public }} ]\n\
             catalogs: [ {{ id: default, tenant: public }} ]\n\
             collections:\n{collections}"
        );
        let config: AppConfig = serde_yaml::from_str(&yaml).unwrap();
        config.validate().unwrap();
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            features: true,
            tiles: true,
        }));
        (config, registry)
    }

    /// The fast path the server's per-request kind gate leans on: a
    /// deployment with no record collection answers `false` here, and that
    /// single `bool` is why such a deployment pays nothing at all for
    /// `#192` — no path parsing, no resolver round trip, on any request.
    #[test]
    fn has_record_collections_is_false_for_a_deployment_that_declares_no_kind() {
        let (config, registry) = kind_config(&[("alpha", ""), ("bravo", "vector")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(!router.has_record_collections());
    }

    /// A raster collection is not a record collection — the gate must stay
    /// off for a deployment that only labels coverages.
    #[test]
    fn has_record_collections_is_false_for_a_raster_only_deployment() {
        let (config, registry) = kind_config(&[("dem", "raster")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(!router.has_record_collections());
    }

    #[test]
    fn has_record_collections_is_true_as_soon_as_one_collection_declares_it() {
        let (config, registry) = kind_config(&[("alpha", ""), ("thesaurus", "record")]);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router.has_record_collections());
    }

    #[test]
    fn collection_kind_reports_each_collections_own_declared_kind() {
        let (config, registry) =
            kind_config(&[("alpha", ""), ("dem", "raster"), ("thesaurus", "record")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.collection_kind("alpha"),
            Some(CollectionKind::Vector)
        );
        assert_eq!(router.collection_kind("dem"), Some(CollectionKind::Raster));
        assert_eq!(
            router.collection_kind("thesaurus"),
            Some(CollectionKind::Record)
        );
    }

    /// Same "`None` only for an id this `Router` never indexed" convention
    /// `effective_visibility`/`effective_settings` follow — a caller that
    /// gates on this must treat `None` as "nothing to enforce", never as a
    /// kind of its own.
    #[test]
    fn collection_kind_is_none_for_an_id_the_router_never_indexed() {
        let (config, registry) = kind_config(&[("alpha", "")]);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(router.collection_kind("no-such-collection"), None);
    }

    // -- `resolve_write` (`#25`) ----------------------------------------------

    /// A collection with no `routing.write` at all refuses the same clean
    /// capability-unsupported way an unrouted features/tiles lane would if
    /// its driver lacked the capability — even though this same `FakeDriver`
    /// DOES advertise features/tiles, write has no "defaults to the single
    /// storage" fallback (see `RoutingDecl`'s own doc), so there is nothing
    /// here to resolve.
    #[tokio::test]
    async fn resolve_write_refuses_a_collection_with_no_write_routing_at_all() {
        let (config, registry) = config_with(true, true);
        let router = Router::build(&config, &registry).unwrap();
        match router.resolve_write("public", "default", "demo").await {
            Err(Error::CapabilityUnsupported { capability, .. }) => {
                assert_eq!(capability, "write");
            }
            other => panic!("expected CapabilityUnsupported, got {}", other.is_ok()),
        }
    }

    /// In-memory `WriteSink`/`OutboxSource` fixture used only by the tests
    /// below — no real backend, just enough to prove `Router::resolve_write`
    /// hands back a working sink.
    struct FakeWriteSink;

    #[async_trait::async_trait]
    impl crate::outbox::WriteSink for FakeWriteSink {
        async fn apply(
            &self,
            _collection: &CollectionDecl,
            _mutation: crate::outbox::Mutation,
        ) -> Result<crate::outbox::Sequence> {
            Ok(crate::outbox::Sequence(1))
        }

        fn features_conformance_classes(&self, collection: &CollectionDecl) -> Vec<&'static str> {
            if collection.srid == Some(4326) {
                vec![crate::outbox::FEATURES_PART4_FEATURES_CLASS]
            } else {
                Vec::new()
            }
        }
    }

    struct FakeWriteDriver;

    #[async_trait::async_trait]
    impl CatalogSource for FakeWriteDriver {
        async fn collections(&self) -> Result<Vec<PhysicalCollection>> {
            Ok(vec![physical("demo")])
        }
    }

    impl StorageDriver for FakeWriteDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeWriteDriver)
        }

        fn write_sink(&self) -> Option<Arc<dyn crate::outbox::WriteSink>> {
            Some(Arc::new(FakeWriteSink))
        }
    }

    struct FakeWriteFactory;

    impl DriverFactory for FakeWriteFactory {
        fn name(&self) -> &str {
            "fake-write"
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FakeWriteDriver))
        }
    }

    fn config_with_write_routing(write_storage: &str) -> (AppConfig, Registry) {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages:
  - {{ id: main, driver: fake, url_env: DATABASE_URL }}
  - {{ id: writable, driver: fake-write, url_env: DATABASE_URL }}
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    routing: {{ write: {write_storage} }}
"#
        ))
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            features: true,
            tiles: true,
        }));
        registry.register(Arc::new(FakeWriteFactory));
        (config, registry)
    }

    // ---- the clustered applier lease (`#193`) ----

    /// A driver that advertises the lease capability. Nothing is ever
    /// acquired here: `resolve_lease` is a pure capability lookup, and
    /// keeping it that way is what lets a boot-time wiring layer decide
    /// whether a collection can be leased without touching the database.
    struct FakeLeaseDriver;

    struct NeverGrantedLease;

    #[async_trait::async_trait]
    impl crate::lease::Lease for NeverGrantedLease {
        async fn try_acquire(
            &self,
            _key: &crate::lease::LeaseKey,
        ) -> Result<Option<crate::lease::LeaseGuard>> {
            Ok(None)
        }
    }

    #[async_trait::async_trait]
    impl CatalogSource for FakeLeaseDriver {
        async fn collections(&self) -> Result<Vec<PhysicalCollection>> {
            Ok(vec![physical("demo")])
        }
    }

    impl StorageDriver for FakeLeaseDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeLeaseDriver)
        }

        fn write_sink(&self) -> Option<Arc<dyn crate::outbox::WriteSink>> {
            Some(Arc::new(FakeWriteSink))
        }

        fn lease(&self) -> Option<Arc<dyn crate::lease::Lease>> {
            Some(Arc::new(NeverGrantedLease))
        }
    }

    struct FakeLeaseFactory;

    impl DriverFactory for FakeLeaseFactory {
        fn name(&self) -> &str {
            "fake-lease"
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FakeLeaseDriver))
        }
    }

    /// The lease is resolved from the write lane — the storage that holds
    /// the obligations — so leadership is coordinated by the same database
    /// the consumer is draining, never a second one.
    #[test]
    fn resolve_lease_answers_from_the_write_lanes_own_storage() {
        let (mut config, mut registry) = config_with_write_routing("writable");
        config.storages.push(
            serde_yaml::from_str("{ id: leasing, driver: fake-lease, url_env: DATABASE_URL }")
                .unwrap(),
        );
        config.collections[0].routing.write = Some(LaneRouting(vec!["leasing".to_string()]));
        config.validate().unwrap();
        registry.register(Arc::new(FakeLeaseFactory));

        let router = Router::build(&config, &registry).unwrap();
        assert!(router.resolve_lease("public", "default", "demo").is_ok());
    }

    /// A write-lane driver with no mutual-exclusion primitive refuses by
    /// name. It must never fall back to "no lease, drain anyway": the
    /// caller uses this refusal to decline to start a second drainer
    /// (`tellurion-server::applier`'s own fail-closed note).
    #[test]
    fn resolve_lease_refuses_a_driver_that_advertises_no_lease() {
        let (config, registry) = config_with_write_routing("writable");
        let router = Router::build(&config, &registry).unwrap();
        assert!(matches!(
            router.resolve_lease("public", "default", "demo"),
            Err(Error::CapabilityUnsupported { capability, .. }) if capability == "lease"
        ));
    }

    /// No write lane at all means no obligations, hence nothing to lead —
    /// the same condition `resolve_outbox` refuses under, named the same
    /// way.
    #[test]
    fn resolve_lease_refuses_a_collection_with_no_write_lane() {
        let (config, registry) = config_with(true, true);
        let router = Router::build(&config, &registry).unwrap();
        assert!(matches!(
            router.resolve_lease("public", "default", "demo"),
            Err(Error::CapabilityUnsupported { capability, .. }) if capability == "lease"
        ));
    }

    #[test]
    fn features_write_conformance_is_the_intersection_across_writable_collections() {
        let (mut supported, registry) = config_with_write_routing("writable");
        supported.collections[0].srid = Some(4326);
        let router = Router::build(&supported, &registry).unwrap();
        assert_eq!(
            router.features_write_conformance_classes(),
            vec![crate::outbox::FEATURES_PART4_FEATURES_CLASS]
        );

        let mut unsupported = supported.clone();
        unsupported.collections[0].srid = Some(2154);
        let router = Router::build(&unsupported, &registry).unwrap();
        assert!(router.features_write_conformance_classes().is_empty());

        let (unrouted, registry) = config_with(true, true);
        let router = Router::build(&unrouted, &registry).unwrap();
        assert!(router.features_write_conformance_classes().is_empty());
    }

    // -- `Router::create_replace_delete_conformance_classes` (`#263`) --------

    /// `#263`: two collections in ONE catalog, on the same two storages, so
    /// the "whole deployment or per collection" question can be asked
    /// directly rather than inferred from two separate deployments. `main`'s
    /// driver advertises no `write_sink`; `writable`'s does. `None` means
    /// the collection declares no `routing.write` at all — the only way a
    /// collection is offered as mutable here.
    fn config_with_two_collections(
        first_write: Option<&str>,
        second_write: Option<&str>,
    ) -> (AppConfig, Registry) {
        let routing = |lane: Option<&str>| match lane {
            Some(storage) => format!("    routing: {{ write: {storage} }}\n"),
            None => String::new(),
        };
        let mut config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages:
  - {{ id: main, driver: fake, url_env: DATABASE_URL }}
  - {{ id: writable, driver: fake-write, url_env: DATABASE_URL }}
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
{}  - id: other
    catalog: default
    storage: main
    table: other
    geometry: geom
    pk: id
{}"#,
            routing(first_write),
            routing(second_write),
        ))
        .unwrap();
        // `FakeWriteSink::features_conformance_classes` earns the Part 4
        // Features class only at 4326, and the dependency test below needs
        // both collections able to earn it so the only thing that can
        // withhold it is the dependency itself.
        for collection in &mut config.collections {
            collection.srid = Some(4326);
        }
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory {
            features: true,
            tiles: true,
        }));
        registry.register(Arc::new(FakeWriteFactory));
        (config, registry)
    }

    /// The Italy demo's shape, reduced: nothing in this deployment is
    /// offered as a mutable resource, so Requirement 1 clause A ("A server
    /// SHALL implement one or more of the methods HTTP POST, PUT and/or
    /// DELETE for each mutable resource") has nothing to be true of, and the
    /// class's own overview ("provides the ability to add, replace and/or
    /// remove individual resources from a collection") is false. Claim
    /// nothing rather than read clause A as vacuously satisfied.
    #[test]
    fn create_replace_delete_is_withheld_where_nothing_is_offered_as_mutable() {
        let (config, registry) = config_with(true, true);
        let router = Router::build(&config, &registry).unwrap();
        assert!(router
            .create_replace_delete_conformance_classes()
            .is_empty());
    }

    /// The other direction, and rule 1 of this campaign: a deployment that
    /// genuinely writes keeps the class exactly as it had it.
    #[test]
    fn create_replace_delete_is_declared_where_a_write_lane_really_resolves() {
        let (config, registry) = config_with_write_routing("writable");
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.create_replace_delete_conformance_classes(),
            vec![crate::outbox::CREATE_REPLACE_DELETE_CONFORMANCE_CLASS]
        );
    }

    /// Offered as mutable by config, but routed at a storage whose driver
    /// advertises no `write_sink`: every write to it is refused by name, so
    /// it participates and honours nothing — `Some(Vec::new())`, which
    /// zeroes the fold rather than being skipped the way a collection with
    /// no write lane at all is.
    #[test]
    fn create_replace_delete_is_withheld_where_a_mutable_collection_cannot_write() {
        let (config, registry) = config_with_write_routing("main");
        let router = Router::build(&config, &registry).unwrap();
        assert!(router
            .create_replace_delete_conformance_classes()
            .is_empty());
    }

    /// Question 1 of `#263`, answered explicitly rather than inherited: a
    /// catalog with one writable collection and one read-only one DOES
    /// honour the class somewhere, and declares it. The read-only collection
    /// is not a mutable resource, so Requirement 1 clause A does not
    /// quantify over it and it must not narrow the claim.
    #[test]
    fn create_replace_delete_survives_a_read_only_collection_beside_a_writable_one() {
        let (config, registry) = config_with_two_collections(Some("writable"), None);
        let router = Router::build(&config, &registry).unwrap();
        assert_eq!(
            router.create_replace_delete_conformance_classes(),
            vec![crate::outbox::CREATE_REPLACE_DELETE_CONFORMANCE_CLASS]
        );
    }

    /// ...and the other half of the same answer, which is what makes the
    /// rule "every resource offered as mutable" rather than "any": a SECOND
    /// collection offered as mutable whose lane cannot write zeroes the
    /// claim, even though the first one can. A client reading `/conformance`
    /// has not yet picked a collection.
    #[test]
    fn create_replace_delete_is_zeroed_by_a_second_mutable_collection_that_cannot_write() {
        let (config, registry) = config_with_two_collections(Some("writable"), Some("main"));
        let router = Router::build(&config, &registry).unwrap();
        assert!(router
            .create_replace_delete_conformance_classes()
            .is_empty());
    }

    /// The assertion that makes the four above worth having: the declaration
    /// is a *function of* the behaviour, not a list checked against another
    /// list. For every lane shape, the class is declared exactly when a
    /// write against that deployment would actually resolve — so a slice
    /// that re-declares the class without making writes work fails here, and
    /// so does one that makes writes work without re-declaring it.
    #[tokio::test]
    async fn create_replace_delete_declaration_agrees_with_what_a_write_does() {
        for (label, (config, registry)) in [
            ("no write lane at all", config_with(true, true)),
            (
                "a write lane at an incapable storage",
                config_with_write_routing("main"),
            ),
            (
                "a write lane at a capable storage",
                config_with_write_routing("writable"),
            ),
        ] {
            let router = Router::build(&config, &registry).unwrap();
            let a_write_resolves = router
                .resolve_write("public", "default", "demo")
                .await
                .is_ok();
            assert_eq!(
                router
                    .create_replace_delete_conformance_classes()
                    .contains(&crate::outbox::CREATE_REPLACE_DELETE_CONFORMANCE_CLASS),
                a_write_resolves,
                "the declaration disagreed with what a write does for {label}"
            );
        }
    }

    /// Part 4 clause 9.1 gives the Features requirements class a Dependency
    /// on Requirements Class "Create/Replace/Delete", and clause 5.4 defines
    /// a direct dependency as one where "Every server implementing the
    /// requirements class has to conform to the referenced Standard or
    /// requirements class". So `conf/features` may never be declared where
    /// `conf/create-replace-delete` is withheld, across every lane shape
    /// these fixtures can build.
    #[tokio::test]
    async fn part_4_features_never_outlives_its_create_replace_delete_dependency() {
        let shapes = [
            ("nothing mutable", config_with_two_collections(None, None)),
            (
                "one writable, one read-only",
                config_with_two_collections(Some("writable"), None),
            ),
            (
                "one writable, one mutable-but-incapable",
                config_with_two_collections(Some("writable"), Some("main")),
            ),
            (
                "both writable",
                config_with_two_collections(Some("writable"), Some("writable")),
            ),
        ];
        for (label, (config, registry)) in shapes {
            let router = Router::build(&config, &registry).unwrap();
            let create_replace_delete = router.create_replace_delete_conformance_classes();
            let features = router.features_write_conformance_classes();
            assert!(
                features.is_empty() || !create_replace_delete.is_empty(),
                "declared conf/features without its conf/create-replace-delete \
                 dependency for {label}"
            );
        }
    }

    #[tokio::test]
    async fn resolve_write_succeeds_when_the_lane_names_a_write_capable_driver() {
        let (config, registry) = config_with_write_routing("writable");
        let router = Router::build(&config, &registry).unwrap();
        let (decl, sink) = router
            .resolve_write("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.id, "demo");
        let sequence = sink
            .apply(
                &decl,
                crate::outbox::Mutation {
                    feature_id: "1".to_string(),
                    kind: crate::outbox::MutationKind::Delete,
                },
            )
            .await
            .unwrap();
        assert_eq!(sequence, crate::outbox::Sequence(1));
    }

    /// The explicit write lane names a real storage, but one whose driver
    /// doesn't advertise `write_sink` — refused the same first-touch way an
    /// incapable explicit features/tiles lane entry is (`#59`'s reasoning
    /// applied to the write lane): `Error::Config`, naming the collection,
    /// the lane, and the offending storage, matching exactly what
    /// `validate_catalog`'s eager boot sweep would have raised.
    #[tokio::test]
    async fn resolve_write_fails_when_the_explicit_lane_names_an_incapable_driver() {
        let (config, registry) = config_with_write_routing("main");
        let router = Router::build(&config, &registry).unwrap();
        match router.resolve_write("public", "default", "demo").await {
            Err(Error::Config(message)) => {
                assert!(message.contains("demo"), "message was: {message}");
                assert!(message.contains("write"), "message was: {message}");
                assert!(message.contains("main"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    /// `#208`: `write_lane_resolves` answers exactly what `resolve_write`
    /// goes on to do, across all three configurations the tests above
    /// distinguish — no lane at all, a lane at an incapable storage, and a
    /// lane at a capable one. An `Allow` header derived from the first must
    /// never promise what the second refuses, and the only way to keep that
    /// true as the capability checks evolve is to assert the two agree
    /// rather than to assert each separately.
    #[tokio::test]
    async fn write_lane_resolves_agrees_with_resolve_write_on_every_lane_shape() {
        for (label, (config, registry)) in [
            ("no write lane at all", config_with(true, true)),
            (
                "a write lane at an incapable storage",
                config_with_write_routing("main"),
            ),
            (
                "a write lane at a capable storage",
                config_with_write_routing("writable"),
            ),
        ] {
            let router = Router::build(&config, &registry).unwrap();
            assert_eq!(
                router.write_lane_resolves("demo"),
                router
                    .resolve_write("public", "default", "demo")
                    .await
                    .is_ok(),
                "write_lane_resolves disagreed with resolve_write for {label}"
            );
        }

        // And an id this router never indexed reports no capability rather
        // than panicking or defaulting to "writable".
        let (config, registry) = config_with_write_routing("writable");
        let router = Router::build(&config, &registry).unwrap();
        assert!(!router.write_lane_resolves("never-indexed"));
    }

    /// `validate_catalog`'s eager boot sweep catches the same misconfigured
    /// write lane before any request ever resolves it.
    #[tokio::test]
    async fn validate_catalog_catches_a_write_lane_naming_an_incapable_driver() {
        let (config, registry) = config_with_write_routing("main");
        let router = Router::build(&config, &registry).unwrap();
        match router.validate_catalog().await {
            Err(Error::Config(message)) => {
                assert!(message.contains("write"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    // -- `resolve_raster` (`#37`, raster-tile serving) -----------------------

    /// A `RasterSource` that always answers with a 1x1 opaque window — no
    /// real decode, only enough to prove `resolve_raster` wires the trait
    /// object through.
    struct FakeRasterSource;

    #[async_trait::async_trait]
    impl RasterSource for FakeRasterSource {
        async fn raster_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
        ) -> Result<Option<crate::storage::RasterWindow>> {
            Ok(Some(crate::storage::RasterWindow {
                width: 1,
                height: 1,
                rgba: vec![255, 0, 0, 255],
            }))
        }
    }

    /// A driver that advertises `raster_source` only, never `tile_source` —
    /// the shape a Cloud-Optimized GeoTIFF driver takes: MVT is an
    /// unsupported capability, not a driver that happens to answer empty.
    struct FakeRasterOnlyDriver;

    impl StorageDriver for FakeRasterOnlyDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeCatalog(vec![physical("demo")]))
        }

        fn raster_source(&self) -> Option<Arc<dyn RasterSource>> {
            Some(Arc::new(FakeRasterSource))
        }
    }

    struct FakeRasterOnlyFactory;

    impl DriverFactory for FakeRasterOnlyFactory {
        fn name(&self) -> &str {
            "fake-raster"
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FakeRasterOnlyDriver))
        }
    }

    fn config_with_raster_driver() -> (AppConfig, Registry) {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake-raster, url_env: DATABASE_URL } ]
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

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeRasterOnlyFactory));
        (config, registry)
    }

    /// `#37`: the MAPS lane resolves through `RasterSource` for a driver
    /// that advertises no `TileSource` at all — the capability the OGC API
    /// Maps `/collections/{cid}/map` route needs for a COG- or Zarr-backed
    /// collection, resolved over `routing.maps` rather than `routing.tiles`.
    #[tokio::test]
    async fn resolve_maps_raster_succeeds_for_a_driver_with_no_tile_source() {
        let (config, registry) = config_with_raster_driver();
        let router = Router::build(&config, &registry).unwrap();
        // The vector half genuinely refuses first — otherwise this proves
        // nothing about the raster half being reached at all.
        assert!(matches!(
            router.resolve_maps("public", "default", "demo").await,
            Err(Error::CapabilityUnsupported { .. })
        ));
        let (decl, source) = router
            .resolve_maps_raster("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.id, "demo");
        let window = source
            .raster_tile(&decl, TileCoord { z: 0, x: 0, y: 0 })
            .await
            .unwrap()
            .unwrap();
        assert_eq!((window.width, window.height), (1, 1));
    }

    /// `#37`: a maps-lane raster resolution failure names the `"maps"`
    /// capability, not `"tiles"` — the reason `maps_raster_source` exists as
    /// its own function rather than reusing `raster_source_for_lane`, and
    /// the same distinction `resolve_maps_refuses_capability_the_driver_lacks`
    /// already draws for the vector half.
    #[tokio::test]
    async fn resolve_maps_raster_refuses_capability_the_driver_lacks() {
        let (config, registry) = config_with(true, true);
        let router = Router::build(&config, &registry).unwrap();
        match router
            .resolve_maps_raster("public", "default", "demo")
            .await
        {
            Err(Error::CapabilityUnsupported { capability, .. }) => {
                assert_eq!(capability, "maps");
            }
            other => panic!("expected CapabilityUnsupported, got {}", other.is_ok()),
        }
    }

    #[tokio::test]
    async fn resolve_raster_succeeds_when_the_driver_advertises_it() {
        let (config, registry) = config_with_raster_driver();
        let router = Router::build(&config, &registry).unwrap();
        let (decl, source) = router
            .resolve_raster("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(decl.id, "demo");
        let window = source
            .raster_tile(&decl, TileCoord { z: 0, x: 0, y: 0 })
            .await
            .unwrap()
            .unwrap();
        assert_eq!((window.width, window.height), (1, 1));
    }

    /// `#92`: a collection that leaves `settings.colormap` unset inherits
    /// its catalog's declared colormap onto the decl `resolve_raster`
    /// actually hands a driver — the same `apply_inherited_settings`
    /// overlay `resolve_tiles_carries_the_catalogs_inherited_tile_caps_onto_the_served_decl`
    /// proves for `tile_caps`, exercised for the raster lane instead.
    #[tokio::test]
    async fn resolve_raster_carries_the_catalogs_inherited_colormap_onto_the_served_decl() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake-raster, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs:
  - id: default
    tenant: public
    settings:
      colormap: { kind: ramp, ramp: grayscale, min: 0.0, max: 255.0 }
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

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeRasterOnlyFactory));
        let router = Router::build(&config, &registry).unwrap();

        let (decl, _source) = router
            .resolve_raster("public", "default", "demo")
            .await
            .unwrap();
        assert_eq!(
            decl.settings.colormap,
            Some(ColormapConf::Ramp {
                ramp: ColorRamp::Grayscale,
                min: 0.0,
                max: 255.0,
            })
        );
    }

    /// The tiles lane's MVT capability is a distinct refusal from its raster
    /// one — a raster-only driver never satisfies `resolve_tiles`, exactly
    /// the same `CapabilityUnsupported` an MVT-only collection gets when a
    /// caller asks for a capability it never advertised.
    #[tokio::test]
    async fn resolve_tiles_refuses_a_raster_only_driver() {
        let (config, registry) = config_with_raster_driver();
        let router = Router::build(&config, &registry).unwrap();
        match router.resolve_tiles("public", "default", "demo").await {
            Err(Error::CapabilityUnsupported { capability, .. }) => {
                assert_eq!(capability, "tiles");
            }
            other => panic!("expected CapabilityUnsupported, got {}", other.is_ok()),
        }
    }

    /// `validate_catalog`'s eager boot sweep never requires a `TileSource`
    /// (or a `FeatureSource`) from an unrouted collection — a raster-only
    /// driver passes boot the same way a features-only or tiles-only one
    /// already does, since `routing.tiles` was never set explicitly here.
    #[tokio::test]
    async fn validate_catalog_accepts_an_unrouted_raster_only_collection() {
        let (config, registry) = config_with_raster_driver();
        let router = Router::build(&config, &registry).unwrap();
        router.validate_catalog().await.unwrap();
    }

    /// An explicit `routing.tiles` lane accepts a raster-only driver just as
    /// the request path does through `resolve_raster`; boot validation must
    /// not require the unrelated MVT `TileSource` capability.
    #[tokio::test]
    async fn validate_catalog_accepts_explicit_raster_only_tiles_routing() {
        let (mut config, registry) = config_with_raster_driver();
        config.collections[0].routing.tiles = Some(LaneRouting(vec!["main".to_string()]));
        config.validate().unwrap();

        let router = Router::build(&config, &registry).unwrap();
        router.validate_catalog().await.unwrap();
    }
}
