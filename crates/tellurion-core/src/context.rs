//! Shared state protocol crates receive: resolved config, the built
//! `Router`, the `Resolver` that turns a URL's external ids into the
//! internal ids everything else works in (`#39`), the `RegistryReader` seam
//! for reading catalog/collection declarations (`#42`), and the tenant
//! `TenantAuthorizer` (`#17`), when one is configured. Constructed once at
//! startup by the `tellurion` server binary; `config`/`router`/`resolver`/
//! `registry`/`authorizer` live behind one atomically-swapped [`ArcSwap`] so
//! a config reload replaces all of them together — a request never resolves
//! against a `Resolver` built from one config and a `Router` (or
//! `RegistryReader`/`TenantAuthorizer`) built from another. The tile cache
//! and style store are NOT part of the swapped state: the cache is keyed on
//! internal ids (`#39`'s rename-survives-a-cache-hit guarantee depends on it
//! staying the same object across a reload), and styles are a separate,
//! tenant/catalog-independent registry.
//!
//! `registry` is built here, from the same `config` every other piece of
//! swapped state is built from, by default: `new`/`reload` still call
//! `FileRegistryReader::build` internally, byte-for-byte the only behavior
//! either had before `#42`'s relational backend existed, so no existing
//! caller of these two needs to change. A relational backend (`#42`, second
//! slice) needs a real, fallible connection attempt, which must complete and
//! be judged good or bad *before* the atomic swap, never inside it —
//! `AppContext` itself never does that I/O or chooses a backend, so
//! [`new_with_registry`](AppContext::new_with_registry)/
//! [`reload_with_registry`](AppContext::reload_with_registry) let a caller
//! that already resolved `AppConfig.registry.backend` via
//! `registry::build_registry_reader` (the `tellurion` binary's wiring layer)
//! hand in the result instead — the same "caller builds and validates,
//! `AppContext` only ever holds" treatment `router`/`resolver`/`authorizer`
//! already get.

use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use bytes::Bytes;

use crate::audit::ConfigAuditLog;
use crate::auth::TenantAuthorizer;
use crate::cache::{Encoding, PopulateFuture, TileCache, TileKey};
use crate::config::{AppConfig, CollectionDecl, RegistryBackend, TenantDecl};
use crate::config_store::{ConfigStore, ConfigVersion};
use crate::control_model::ControlRevision;
use crate::control_policy::ControlPolicySet;
use crate::control_runtime::ControlRuntimeStatus;
use crate::control_store::ControlStore;
use crate::error::Error;
use crate::error::Result as CoreResult;
use crate::filter::Filter;
use crate::invalidation::GenerationStore;
use crate::links::LinkContributors;
use crate::observability::{
    in_phase, observe_registry, observe_resolver, observe_style_store, Phase,
};
use crate::rate_limit::RateCounter;
use crate::registry::{snapshot_from_registry, RegistryReader};
use crate::resolver::{Resolver, StaticResolver};
use crate::router::{Registry, Router};
use crate::storage::{TileCoord, TileSource};
use crate::style_store::StyleStore;
use crate::tenant::{snapshot_tenants, validate_tenant_snapshot, TenantReader};
use crate::tms::TileMatrixSet;

/// Derives a [`ConfigVersion`] from `config` by re-serializing it — the
/// fallback [`AppContext::new_with_registry`]/[`AppContext::
/// reload_with_registry`] use when a caller doesn't have the original
/// on-disk byte source at hand (in practice, every test in this workspace,
/// which builds an `AppConfig` straight from a YAML literal without ever
/// routing it through a `ConfigStore`). Content-deterministic — the same
/// parsed `AppConfig` always re-serializes to the same bytes — so it still
/// gives a real, comparable version for "did the config change between two
/// snapshots," even though it will not equal `FileConfigStore::
/// load_versioned`'s own token for the SAME document (which hashes the raw
/// file bytes, never a re-serialization). `main`/`reload.rs` — the two
/// callers that hold a real versioned read — use
/// `new_with_registry_and_version`/`reload_with_registry_and_version`
/// instead, so the config-version gauge and audit trail a real running
/// server exposes agree with the version a real `ConfigStore::write`
/// reports, not a value only ever comparable against itself.
fn derive_config_version(config: &AppConfig) -> ConfigVersion {
    let bytes = serde_yaml::to_string(config)
        .unwrap_or_default()
        .into_bytes();
    ConfigVersion::from_bytes(&bytes)
}

/// Builds `Router` and `Resolver` together from `config`'s registry backend
/// (`#42`, third slice; `#143` for the tenant half) — the wiring layer's
/// (the `tellurion` binary's `main.rs`/`reload.rs`) single entry point for
/// both, so a relational backend's registry/tenant walks feed BOTH indexes
/// rather than two separate, potentially inconsistent ones. A `Router` with
/// no matching `Resolver` entry is unreachable from any URL regardless of
/// what it can serve once resolved — a collection published to the registry
/// needs to resolve its external id here just as much as it needs to route
/// through `Router` — so building them from independent walks would risk
/// each observing a different snapshot of a registry mutated in between;
/// this function walks once and feeds both from that one result.
///
/// `RegistryBackend::File` (the default) builds each the way it always has —
/// `Router::build`/`StaticResolver::build`, no I/O, `registry_reader`/
/// `tenant_reader` never consulted (the caller still had to build both for
/// `AppContext`/this function regardless — they are simply unused here).
///
/// `RegistryBackend::Relational` instead walks `tenant_reader`
/// (`tenant::snapshot_tenants`) and `registry_reader`
/// (`registry::snapshot_from_registry`) to exhaustion, validates each result
/// (`tenant::validate_tenant_snapshot`, then `config::
/// validate_registry_snapshot` with exactly the same referential-integrity
/// rules a YAML-declared `catalogs`/`collections` is held to), and only then
/// builds `Router::build_from_snapshot` and `StaticResolver::
/// build_from_snapshot` from that one pair of walks — any walk or validation
/// failure is returned as `Err` before either is built, never partially
/// applied.
///
/// `config` must already have passed `AppConfig::validate` (same
/// precondition `Router::build` documents) — in particular, `registry.
/// backend: relational` with a config that ALSO declares `catalogs`/
/// `collections` is refused there, before this ever runs, so the two
/// sources never need reconciling here.
pub async fn build_router_and_resolver(
    config: &AppConfig,
    driver_registry: &Registry,
    registry_reader: &dyn RegistryReader,
    tenant_reader: &dyn TenantReader,
) -> CoreResult<(Router, Arc<dyn Resolver>, Vec<TenantDecl>)> {
    match config.registry.backend {
        RegistryBackend::File => Ok((
            Router::build(config, driver_registry)?,
            Arc::new(StaticResolver::build(config)),
            config.tenants.clone(),
        )),
        RegistryBackend::Relational => {
            let tenants = snapshot_tenants(tenant_reader).await?;
            validate_tenant_snapshot(config, &tenants)?;
            let snapshot = snapshot_from_registry(&tenants, registry_reader).await?;
            config.validate_with_registry(&tenants, &snapshot)?;
            let router = Router::build_from_snapshot(
                config,
                &tenants,
                &snapshot.catalogs,
                &snapshot.collections,
                driver_registry,
            )?;
            let resolver = StaticResolver::build_from_snapshot(
                &tenants,
                &snapshot.catalogs,
                &snapshot.collections,
            );
            Ok((router, Arc::new(resolver), tenants))
        }
    }
}

/// One atomically-swappable snapshot: a config, its normalized tenant
/// declarations, the `Router` built from them, the `Resolver` built from
/// them, the `RegistryReader` built from the same source (`#42`), and the
/// `TenantAuthorizer` built from the config and, for dynamic control-store
/// snapshots, the same revision's role bindings (`None` when `config.auth`
/// is absent/unconfigured, `!config.auth.is_configured()`, `#17`/`#34`) —
/// always replaced as a unit.
pub struct ContextState {
    pub config: AppConfig,
    /// The authoritative tenant declarations used to build this state's
    /// router and resolver. Equal to `config.tenants` for the file backend;
    /// sourced from `TenantReader` for a relational backend.
    pub tenants: Vec<TenantDecl>,
    pub router: Router,
    pub resolver: Arc<dyn Resolver>,
    pub authorizer: Option<Arc<dyn TenantAuthorizer>>,
    pub registry: Arc<dyn RegistryReader>,
    /// `#110`: the `ConfigVersion` this snapshot's own `config` was read
    /// from (or a re-serialized fallback — see `derive_config_version`)
    /// swapped in atomically with everything else here, so a request that
    /// reads it alongside `config` can never see one from a different
    /// generation. Backs the per-instance config-version gauge
    /// (`tellurion-server::metrics`) and the config-mutation audit trail's
    /// `expected_version`/`new_version` fields.
    pub config_version: ConfigVersion,
    /// Durable control revision that produced this active generation.
    /// `None` for legacy file-backed and directly constructed contexts.
    /// Stored beside the config so effective settings and their revision
    /// label are captured by one atomic [`AppContext::current`] read.
    pub control_revision: Option<ControlRevision>,
    /// `#215`: the compiled hierarchical administration policy this
    /// generation serves — bindings and path statements, already parsed.
    ///
    /// Lives inside the swapped `ContextState` rather than beside it so a
    /// request can never read a policy set from one generation against a
    /// `resolver` from another: the ids a decision is made about and the
    /// statements it is made with always come from the same activation.
    ///
    /// [`ControlPolicySet::default`] — no bindings, no statements — for
    /// every constructor that does not name one, which is every constructor
    /// that existed before `#215`. That value has exactly one possible
    /// answer (`ControlDecision::NotEngaged`), which is what makes an
    /// existing deployment provably unchanged rather than merely intended
    /// to be.
    pub control_policy: Arc<ControlPolicySet>,
}

pub struct AppContext {
    state: ArcSwap<ContextState>,
    pub cache: Arc<dyn TileCache>,
    pub style_store: Arc<dyn StyleStore>,
    /// Write-reactive tile-cache invalidation state (`#113`) —
    /// `GenerationStore::empty()` (every lookup answers generation `0`,
    /// byte-for-byte today's TTL-only behavior) until
    /// [`set_generations`](Self::set_generations) wires a real one in. Kept
    /// out of `new`/`new_with_registry`'s own parameter list deliberately:
    /// unlike `cache`/`style_store` (required for the server to do
    /// anything at all), this has an always-safe empty default, and dozens
    /// of existing call sites across this workspace's own test suites
    /// construct an `AppContext` positionally — adding a required
    /// constructor parameter for a feature that is off everywhere except
    /// one real wiring call site (`tellurion-server`'s `main.rs`) would
    /// touch all of them for no behavioral reason. `ArcSwap`, not a plain
    /// `Arc`, so it can be wired in after construction even through an
    /// already-`Arc`-wrapped `AppContext` — same reasoning as `state`
    /// above, though (unlike `state`) this is never swapped again after
    /// boot.
    generations: ArcSwap<GenerationStore>,
    /// `#110`: the writable seam the config-mutation control lane
    /// (`tellurion-server::config_mutation`) persists a validated new
    /// document through — `None` for any `AppContext` built without one
    /// (every constructor in this module except through
    /// [`with_config_store`](Self::with_config_store)), which is exactly
    /// how that control lane decides its own mutation routes don't exist
    /// at all for such an instance. Stable across a reload, the same
    /// "identity doesn't change on reload" treatment `cache`/`style_store`
    /// already get — only the swapped `ContextState.config` content
    /// changes, never which store manages it.
    pub config_store: Option<Arc<dyn ConfigStore>>,
    /// Durable control state. This is opt-in at construction time and
    /// deliberately remains stable across configuration reloads.
    pub control_store: Option<Arc<dyn ControlStore>>,
    /// Process-local observations of durable control convergence. This starts
    /// at revision zero for existing constructors and stays stable across
    /// configuration reloads.
    pub control_runtime_status: Arc<ControlRuntimeStatus>,
    /// `#110`: the bounded audit trail of applied configuration mutations.
    /// Stable across a reload (same reasoning as `config_store` above) —
    /// history should span the process's life, not any one config
    /// generation.
    pub audit_log: ConfigAuditLog,
    /// `#186`: the cross-protocol link-contributor seam. Empty for any
    /// `AppContext` built without [`with_link_contributors`](Self::
    /// with_link_contributors) — the constructors deliberately never take
    /// it, for the same "always-safe default, dozens of positional call
    /// sites" reason `generations` documents above — and an empty registry
    /// contributes no links at zero cost, so every existing caller's
    /// responses stay byte-for-byte unchanged. Stable across a reload like
    /// `cache`/`style_store`: registration is a boot-time fact about what
    /// this binary contains (`#112`), while every contribution consults the
    /// CURRENT snapshot's `Router` passed per call, so a reload's
    /// capability changes flow through without re-registering anything.
    pub link_contributors: LinkContributors,
    /// `#188`: the backend policy rate conditions charge their counters
    /// against — [`InProcessRateCounter`](crate::rate_limit::
    /// InProcessRateCounter) for every context any constructor here builds.
    ///
    /// Unlike `cache`/`style_store` this is not a constructor parameter, and
    /// this slice ships no builder to replace it: the in-process counter
    /// needs no configuration, no external process and no feature flag, and
    /// it costs nothing at all until a grant actually declares a ceiling
    /// (`policy::enforce_rate_limits` returns before ever reaching it
    /// otherwise) — so every existing call site stays untouched and every
    /// existing deployment stays byte-for-byte unchanged. The field is typed
    /// as the trait object, not the concrete counter, because a fleet-atomic
    /// backend arrives by being swapped in right here.
    ///
    /// Stable across a reload, the same treatment `cache` gets — and
    /// load-bearing here rather than merely tidy: rebuilding the counter on
    /// each config reload would hand every principal a fresh window, turning
    /// a reload into a way to reset a ceiling.
    pub rate_counter: Arc<dyn RateCounter>,
}

impl AppContext {
    /// Builds a fresh `AppContext` whose registry is
    /// `FileRegistryReader::build(&config)` — byte-for-byte the only
    /// behavior every caller of this constructor had before `#42`'s
    /// relational backend existed. See
    /// [`new_with_registry`](Self::new_with_registry) for the wiring layer
    /// (the `tellurion` binary) that resolves `AppConfig.registry.backend`
    /// itself and hands in the result instead.
    pub fn new(
        config: AppConfig,
        router: Router,
        resolver: Arc<dyn Resolver>,
        authorizer: Option<Arc<dyn TenantAuthorizer>>,
        cache: Arc<dyn TileCache>,
        style_store: Arc<dyn StyleStore>,
    ) -> Self {
        let registry: Arc<dyn RegistryReader> =
            Arc::new(crate::registry::FileRegistryReader::build(&config));
        let tenants = config.tenants.clone();
        Self::new_with_registry(
            config,
            tenants,
            router,
            resolver,
            authorizer,
            registry,
            cache,
            style_store,
        )
    }

    /// Same as [`new`](Self::new), but the caller supplies `registry`
    /// directly instead of this constructor implicitly building
    /// `FileRegistryReader` — the seam `main` uses once it has resolved
    /// `AppConfig.registry.backend` via `registry::build_registry_reader`
    /// (`#42`, second slice), the same "caller builds and validates,
    /// `AppContext` only ever holds" treatment `router`/`resolver`/
    /// `authorizer` already get. `tenants` must be the same authoritative
    /// snapshot used to build `router` and `resolver`; relational callers
    /// must never substitute the (necessarily empty) `config.tenants`.
    /// See the module doc.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_registry(
        config: AppConfig,
        tenants: Vec<TenantDecl>,
        router: Router,
        resolver: Arc<dyn Resolver>,
        authorizer: Option<Arc<dyn TenantAuthorizer>>,
        registry: Arc<dyn RegistryReader>,
        cache: Arc<dyn TileCache>,
        style_store: Arc<dyn StyleStore>,
    ) -> Self {
        let config_version = derive_config_version(&config);
        Self::new_with_registry_and_version(
            config,
            tenants,
            router,
            resolver,
            authorizer,
            registry,
            cache,
            style_store,
            config_version,
        )
    }

    /// Same as [`new_with_registry`](Self::new_with_registry), but the
    /// caller supplies the real [`ConfigVersion`] a versioned read already
    /// produced (`#110`) instead of this constructor deriving a fallback
    /// one from `config` alone — the seam `main` uses, having loaded via
    /// `ConfigStore::load_versioned` rather than plain `load`.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_registry_and_version(
        config: AppConfig,
        tenants: Vec<TenantDecl>,
        router: Router,
        resolver: Arc<dyn Resolver>,
        authorizer: Option<Arc<dyn TenantAuthorizer>>,
        registry: Arc<dyn RegistryReader>,
        cache: Arc<dyn TileCache>,
        style_store: Arc<dyn StyleStore>,
        config_version: ConfigVersion,
    ) -> Self {
        Self::new_with_registry_version_and_policy(
            config,
            tenants,
            router,
            resolver,
            authorizer,
            registry,
            cache,
            style_store,
            config_version,
            Arc::new(ControlPolicySet::default()),
            None,
        )
    }

    /// Same as [`new_with_registry_and_version`](Self::
    /// new_with_registry_and_version), but the caller supplies the compiled
    /// administration policy set (`#215`) instead of this constructor
    /// defaulting to the empty one.
    ///
    /// A separate constructor rather than a tenth parameter on the existing
    /// one, deliberately: every call site that existed before `#215` keeps
    /// compiling and keeps getting the empty set, which is the value that
    /// makes those deployments provably unchanged. Only the boot path that
    /// actually read a control snapshot calls this.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_registry_version_and_policy(
        config: AppConfig,
        tenants: Vec<TenantDecl>,
        router: Router,
        resolver: Arc<dyn Resolver>,
        authorizer: Option<Arc<dyn TenantAuthorizer>>,
        registry: Arc<dyn RegistryReader>,
        cache: Arc<dyn TileCache>,
        style_store: Arc<dyn StyleStore>,
        config_version: ConfigVersion,
        control_policy: Arc<ControlPolicySet>,
        control_revision: Option<ControlRevision>,
    ) -> Self {
        let resolver = observe_resolver(resolver);
        let registry = observe_registry(registry);
        Self {
            state: ArcSwap::from_pointee(ContextState {
                config,
                tenants,
                router,
                resolver,
                authorizer,
                registry,
                config_version,
                control_revision,
                control_policy,
            }),
            cache,
            style_store: observe_style_store(style_store),
            generations: ArcSwap::from_pointee(GenerationStore::empty()),
            config_store: None,
            control_store: None,
            control_runtime_status: Arc::new(ControlRuntimeStatus::new(0)),
            audit_log: ConfigAuditLog::default(),
            link_contributors: LinkContributors::default(),
            rate_counter: Arc::new(crate::rate_limit::InProcessRateCounter::new()),
        }
    }

    /// Builder: attaches the writable `ConfigStore` seam (`#110`) the
    /// config-mutation control lane persists through — see
    /// `AppContext::config_store`'s own doc for why this is the ONLY way
    /// to get one attached (every plain constructor leaves it `None`).
    /// Consumes and returns `self` so it reads naturally chained onto a
    /// constructor call, before the result is wrapped in an `Arc` for
    /// sharing.
    pub fn with_config_store(mut self, config_store: Arc<dyn ConfigStore>) -> Self {
        self.config_store = Some(config_store);
        self
    }

    /// Builder: attaches the durable control store.
    pub fn with_control_store(mut self, control_store: Arc<dyn ControlStore>) -> Self {
        self.control_store = Some(control_store);
        self
    }

    /// Builder: replaces the default local control-runtime tracker with the
    /// one created during durable control boot.
    pub fn with_control_runtime_status(
        mut self,
        control_runtime_status: Arc<ControlRuntimeStatus>,
    ) -> Self {
        self.control_runtime_status = control_runtime_status;
        self
    }

    /// Builder: attaches the boot-time link-contributor registry (`#186`) —
    /// see the `link_contributors` field's own doc for why this is the only
    /// way to get a non-empty one (every plain constructor leaves it empty,
    /// which means "contribute no links anywhere"). Same consume-and-return
    /// shape as [`with_config_store`](Self::with_config_store), chained
    /// before the result is `Arc`-wrapped for sharing.
    pub fn with_link_contributors(mut self, link_contributors: LinkContributors) -> Self {
        self.link_contributors = link_contributors;
        self
    }

    /// The current `(config, router, resolver)` snapshot. Cheap (one atomic
    /// load plus an `Arc` clone) and safe to hold across an `.await` point —
    /// unlike `ArcSwap::load`'s guard, this owns its own `Arc` rather than
    /// borrowing from the swap itself, so it can never delay a concurrent
    /// `reload`.
    pub fn current(&self) -> Arc<ContextState> {
        self.state.load_full()
    }

    /// Wires the real `GenerationStore` the write-reactive tile-cache
    /// invalidation consumer (`#113`) maintains, once its background tasks
    /// are spawned — see the `generations` field's own doc for why this is
    /// a post-construction setter rather than a `new`/`new_with_registry`
    /// parameter. Called at most once in practice, by `tellurion-server`'s
    /// `main.rs`, right after `generation_consumer::spawn_all` returns the
    /// store it built.
    pub fn set_generations(&self, store: Arc<GenerationStore>) {
        self.generations.store(store);
    }

    /// The generation to fold into a pyramid-coordinate tile's cache key —
    /// `0` whenever no real store was ever wired in (the consumer is off
    /// server-wide) or this collection never opted in, both of which keep
    /// the built key byte-for-byte identical to before `#113` existed.
    ///
    /// `tms` (`#190`) picks WHICH generation answers, because the store's
    /// bucket grid is WebMercator-indexed (`#142`'s write-bbox mapping
    /// included): a `WebMercatorQuad` coordinate resolves its own bucket(s)
    /// exactly as before, while a `WorldCRS84Quad` coordinate — whose
    /// `z`/`x`/`y` index a different grid entirely — falls back to the
    /// whole-collection generation instead of resolving the wrong bucket
    /// and risking a MISSED invalidation. See
    /// `GenerationStore::generation_for_collection`'s own doc for the
    /// deliberate over-invalidation this trades for correctness.
    pub fn tile_generation(
        &self,
        collection_internal_id: &str,
        tms: TileMatrixSet,
        coord: TileCoord,
    ) -> u64 {
        match tms {
            TileMatrixSet::WebMercatorQuad => self.generations.load().generation_for_tile(
                collection_internal_id,
                coord.z,
                coord.x,
                coord.y,
            ),
            TileMatrixSet::WorldCrs84Quad => self
                .generations
                .load()
                .generation_for_collection(collection_internal_id),
        }
    }

    /// [`tile_generation`](Self::tile_generation)'s counterpart for the OGC
    /// API Maps `Encoding::Map` lane (`#86`), which has no pyramid
    /// coordinate — see `GenerationStore::generation_for_bbox_mercator`'s
    /// own doc.
    pub fn tile_generation_for_bbox_mercator(
        &self,
        collection_internal_id: &str,
        bbox_mercator: [f64; 4],
    ) -> u64 {
        self.generations
            .load()
            .generation_for_bbox_mercator(collection_internal_id, bbox_mercator)
    }

    /// Atomically replaces the config/router/resolver/authorizer snapshot —
    /// the config reload seam (`#39`, and `#17` for `authorizer`). Rebuilds
    /// the registry the same way [`new`](Self::new) does
    /// (`FileRegistryReader::build(&config)`) — see
    /// [`reload_with_registry`](Self::reload_with_registry) for a caller
    /// that resolved `AppConfig.registry.backend` itself. Every request
    /// already in flight keeps whatever snapshot it captured via
    /// [`current`](Self::current); every request that calls `current` after
    /// this returns sees the new one — including the enforcement
    /// middleware's own `authorizer` lookup, so an `auth:` edit takes effect
    /// on the very next request with no restart. The tile cache is untouched
    /// (see the module doc), which is exactly what lets a
    /// tenant/catalog/collection rename serve the same cached tile under its
    /// new external id: the internal ids the cache keys on never change.
    pub fn reload(
        &self,
        config: AppConfig,
        router: Router,
        resolver: Arc<dyn Resolver>,
        authorizer: Option<Arc<dyn TenantAuthorizer>>,
    ) {
        let registry: Arc<dyn RegistryReader> =
            Arc::new(crate::registry::FileRegistryReader::build(&config));
        let tenants = config.tenants.clone();
        self.reload_with_registry(config, tenants, router, resolver, authorizer, registry);
    }

    /// Same as [`reload`](Self::reload), but the caller supplies `registry`
    /// directly — built (and, for a relational backend, already connected)
    /// by the caller *before* this runs, via `registry::build_registry_reader`
    /// (`#42`, second slice). This function does no I/O and cannot fail: a
    /// caller that fails to build a `registry` must not call this at all,
    /// which is exactly how a bad-config reload already keeps the previous
    /// state (see `reload.rs::attempt_reload`) — the whole `ContextState` is
    /// still only ever replaced as a unit, never partially. `tenants` must
    /// be the authoritative snapshot used to build `router` and `resolver`,
    /// including the database snapshot for a relational backend.
    pub fn reload_with_registry(
        &self,
        config: AppConfig,
        tenants: Vec<TenantDecl>,
        router: Router,
        resolver: Arc<dyn Resolver>,
        authorizer: Option<Arc<dyn TenantAuthorizer>>,
        registry: Arc<dyn RegistryReader>,
    ) {
        let config_version = derive_config_version(&config);
        self.reload_with_registry_and_version(
            config,
            tenants,
            router,
            resolver,
            authorizer,
            registry,
            config_version,
        );
    }

    /// Same as [`reload_with_registry`](Self::reload_with_registry), but
    /// the caller supplies the real [`ConfigVersion`] a versioned read
    /// already produced (`#110`) instead of a derived fallback — the seam
    /// `reload.rs`'s `attempt_reload` uses, having loaded via
    /// `ConfigStore::load_versioned` rather than plain `load`, so the
    /// config-version gauge and audit trail agree with what a real
    /// `ConfigStore::write` reports across every reload, not just the
    /// first boot.
    #[allow(clippy::too_many_arguments)]
    pub fn reload_with_registry_and_version(
        &self,
        config: AppConfig,
        tenants: Vec<TenantDecl>,
        router: Router,
        resolver: Arc<dyn Resolver>,
        authorizer: Option<Arc<dyn TenantAuthorizer>>,
        registry: Arc<dyn RegistryReader>,
        config_version: ConfigVersion,
    ) {
        self.reload_with_registry_version_and_policy(
            config,
            tenants,
            router,
            resolver,
            authorizer,
            registry,
            config_version,
            Arc::new(ControlPolicySet::default()),
            None,
        );
    }

    /// Same as [`reload_with_registry_and_version`](Self::
    /// reload_with_registry_and_version), plus the compiled administration
    /// policy set (`#215`) — swapped in the SAME `ContextState` as the
    /// router, resolver and authorizer it must agree with, so no request
    /// ever evaluates one generation's statements against another
    /// generation's resolved ids. Same "existing callers keep the empty set"
    /// reasoning as its constructor twin.
    #[allow(clippy::too_many_arguments)]
    pub fn reload_with_registry_version_and_policy(
        &self,
        config: AppConfig,
        tenants: Vec<TenantDecl>,
        router: Router,
        resolver: Arc<dyn Resolver>,
        authorizer: Option<Arc<dyn TenantAuthorizer>>,
        registry: Arc<dyn RegistryReader>,
        config_version: ConfigVersion,
        control_policy: Arc<ControlPolicySet>,
        control_revision: Option<ControlRevision>,
    ) {
        let resolver = observe_resolver(resolver);
        let registry = observe_registry(registry);
        self.state.store(Arc::new(ContextState {
            config,
            tenants,
            router,
            resolver,
            authorizer,
            registry,
            config_version,
            control_revision,
            control_policy,
        }));
    }

    /// The collection's resolved TTL for
    /// [`get_or_populate`](Self::get_or_populate) (`#51`) — exactly the
    /// value that method derives internally, exposed so a caller whose own
    /// `populate` closure needs to route a nested sub-fetch through the
    /// cache's TTL-aware entry point (a second cache key, populated from
    /// inside the first key's `populate` future) can pass this down instead
    /// of resolving `Router::effective_settings` a second time — which,
    /// besides being redundant, could return a different answer than the
    /// outer call if a config reload lands between the two lookups.
    pub fn cache_ttl(&self, collection_internal_id: &str) -> Option<Duration> {
        self.current()
            .router
            .effective_settings(collection_internal_id)
            .map(|settings| Duration::from_secs(settings.cache_ttl_s))
    }

    /// Runs a cache population with a caller-resolved TTL while preserving
    /// the same exclusive cache/encode phase accounting as
    /// [`get_or_populate`](Self::get_or_populate).
    pub async fn get_or_populate_with_ttl(
        &self,
        key: TileKey,
        populate: PopulateFuture,
        ttl: Duration,
    ) -> Result<Bytes, Arc<Error>> {
        in_phase(Phase::Cache, async {
            let populate = Box::pin(in_phase(Phase::Encode, populate));
            self.cache
                .get_or_populate_with_ttl(key, populate, ttl)
                .await
        })
        .await
    }

    /// TTL-aware `TileCache::get_or_populate` for one collection (`#46`):
    /// resolves the collection's materialized `EffectiveSettings::cache_ttl_s`
    /// (`settings.rs`) from the current router snapshot and threads it
    /// through `TileCache::get_or_populate_with_ttl`, so a collection's
    /// inherited `cache_ttl_s` actually reaches the L2 write path instead of
    /// silently falling back to whatever fixed TTL the L2 backend was itself
    /// configured with. Falls back to the plain, non-TTL
    /// `TileCache::get_or_populate` — today's behavior, unchanged — for the
    /// one case `Router::effective_settings` can return `None`: a collection
    /// internal id `Router` never indexed.
    pub async fn get_or_populate(
        &self,
        collection_internal_id: &str,
        key: TileKey,
        populate: PopulateFuture,
    ) -> Result<Bytes, Arc<Error>> {
        in_phase(Phase::Cache, async {
            let populate = Box::pin(in_phase(Phase::Encode, populate));
            match self.cache_ttl(collection_internal_id) {
                Some(ttl) => {
                    self.cache
                        .get_or_populate_with_ttl(key, populate, ttl)
                        .await
                }
                None => self.cache.get_or_populate(key, populate).await,
            }
        })
        .await
    }

    /// Fetches (or serves from cache) the MVT tile at `coord` for
    /// `tenant`/`catalog`/`collection`, on the `#34` policy-fingerprint cache
    /// lane — the fetch `tellurion-tiles` and `tellurion-places` both build
    /// their own MVT-first output (raster, GLB) on top of, previously two
    /// private copies of this same function. `filter` is a `#34` ABAC grant
    /// filter, `None` for unrestricted access — pushed both into
    /// [`mvt_key`]'s cache key (via its fingerprint) and into
    /// `source.mvt_tile` itself, so the driver applies exactly the filter
    /// the cache is partitioned by. Concurrent misses on the same key are
    /// coalesced by the cache (single-flight ahead of the driver hit), so N
    /// simultaneous requests for one missing tile trigger exactly one fetch.
    ///
    /// `ttl` is `None` when the caller has no TTL of its own to coordinate
    /// with: this method then resolves `collection`'s effective TTL itself,
    /// via [`get_or_populate`](Self::get_or_populate) — the plain, most
    /// common case. It's `Some` when the caller already resolved a TTL for
    /// a sibling cache entry this MVT fetch nests under (places' Glb tile,
    /// which needs its own Glb-keyed entry and this Mvt-keyed one to agree
    /// on the exact TTL one router snapshot produced) and passes it straight
    /// through, so this fetch never re-resolves it against a router snapshot
    /// that may have moved on in between — a real race under concurrent
    /// config reload, not a merely cosmetic one, which is why this stays a
    /// parameter instead of being resolved unconditionally in here.
    /// `tms` (`#190`) names the tile matrix set `coord`'s `z`/`x`/`y` index
    /// into — folded into the cache key (so the two grids' tiles never
    /// collide) and passed through to `source.mvt_tile_in` (so the driver
    /// builds the matching envelope). Callers have already refused a grid
    /// the resolved source can't serve (`TileSource::
    /// supports_tile_matrix_set`) before calling in here.
    #[allow(clippy::too_many_arguments)]
    pub async fn fetch_mvt(
        &self,
        tenant: &str,
        catalog: &str,
        collection: &str,
        tms: TileMatrixSet,
        coord: TileCoord,
        decl: &CollectionDecl,
        source: &Arc<dyn TileSource>,
        filter: Option<&Filter>,
        ttl: Option<Duration>,
    ) -> MvtFetch {
        let key = TileKey {
            // `#190`: the grid identity — see `TileKey::tms`'s own doc.
            tms,
            // `#85`: the collection's resolved vector-tile property
            // allowlist, so the Mvt-encoded cache entry always matches the
            // attribute shape `decl.tile_properties` actually configures —
            // see `TileKey::properties`'s own doc.
            properties: decl.tile_properties.clone(),
            // `#113`: this tile's bucket generation, so a write-reactive
            // bump forces a fresh render here — the one entry every other
            // encoding (Png/PngStyled/Glb) renders from, see
            // `TileKey::generation`'s own doc for why busting this one
            // entry is enough.
            generation: self.tile_generation(collection, tms, coord),
            ..mvt_key(
                tenant,
                catalog,
                collection,
                coord,
                filter.map(Filter::fingerprint),
            )
        };
        let source = Arc::clone(source);
        let decl = decl.clone();
        let filter = filter.cloned();
        let populate: PopulateFuture = Box::pin(async move {
            match source
                .mvt_tile_in(&decl, tms, coord, filter.as_ref())
                .await?
            {
                Some(bytes) => Ok(bytes),
                // Empty sentinel: a genuinely empty tile is cached so repeat
                // requests never re-hit the driver.
                None => Ok(Bytes::new()),
            }
        });

        let result = match ttl {
            Some(ttl) => self.get_or_populate_with_ttl(key, populate, ttl).await,
            None => self.get_or_populate(collection, key, populate).await,
        };

        match result {
            Ok(bytes) if bytes.is_empty() => MvtFetch::Empty,
            Ok(bytes) => MvtFetch::Hit(bytes),
            Err(error) => {
                tracing::error!(%error, tenant, catalog, collection, z = coord.z, x = coord.x, y = coord.y, "tile source failed to produce an MVT tile");
                MvtFetch::Failed
            }
        }
    }
}

/// `tenant`/`catalog`/`collection` are all INTERNAL ids (`#39`) — this is
/// what makes a tenant/catalog/collection rename (a config reload changing
/// only its `external_id`) serve the same cache entry under the new name:
/// none of these three ever changes for a given real-world collection.
///
/// `policy_fingerprint` (`#34`) is `None` for unrestricted access (the
/// unfiltered, pre-`#34` case — every caller with no matched filter passes
/// this through unchanged) or `Some(Filter::fingerprint())` when the
/// requesting subject's grant carries one; see [`TileKey`]'s own doc for the
/// full composition and sharing rules this partitions the cache by. Shared
/// by [`AppContext::fetch_mvt`] and by `tellurion-tiles`' own `png_key`/
/// `styled_png_key` (which build on this same `Mvt`-keyed identity via
/// struct-update syntax, only overriding `encoding`).
pub fn mvt_key(
    tenant: &str,
    catalog: &str,
    collection: &str,
    coord: TileCoord,
    policy_fingerprint: Option<u64>,
) -> TileKey {
    TileKey {
        tenant: tenant.to_string(),
        catalog: catalog.to_string(),
        collection: collection.to_string(),
        // `#190`: `WebMercatorQuad` here, exactly like `generation`/
        // `properties` below — the pre-`#190` grid every existing caller
        // and test means by a bare `(z, x, y)`; the one caller serving a
        // second grid (`fetch_mvt`, and the tiles handlers' own `*_key`
        // helpers building on it) overrides this via struct-update syntax.
        tms: TileMatrixSet::WebMercatorQuad,
        z: coord.z,
        x: coord.x,
        y: coord.y,
        encoding: Encoding::Mvt,
        policy_fingerprint,
        // `#113`: `0` here, exactly like `properties` below — a caller with
        // a real `GenerationStore` to consult (`AppContext::fetch_mvt`, and
        // the tiles/places/maps handlers' own `*_key` helpers) overrides
        // this via struct-update syntax after calling in here. Every other
        // caller (this crate's own tests) leaves it at `0`, which is
        // exactly the pre-`#113` key.
        generation: 0,
        // `#85`: the real, resolved allowlist is only known once a caller
        // has a `CollectionDecl` in hand — `fetch_mvt` overrides this right
        // after calling in here, from `decl.tile_properties`. Every other
        // caller of this function (png_key/styled_png_key/glb_key, and this
        // crate's own tests) leaves it at the pk-only default, which is
        // exactly the pre-`#85` key for a collection that never sets
        // `tile_properties` — see `TileKey::properties`'s own doc.
        properties: Vec::new(),
    }
}

/// Outcome of [`AppContext::fetch_mvt`]: `Hit` is real MVT content, `Empty`
/// is a genuinely empty tile (cached as a zero-length sentinel so repeat
/// requests never re-hit the driver), `Failed` is a driver error (already
/// logged by `fetch_mvt` itself).
pub enum MvtFetch {
    Hit(Bytes),
    Empty,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use crate::catalog::{CatalogSource, PhysicalCollection};
    use crate::config::{AppConfig, StorageDecl};
    use crate::control_runtime::ControlRuntimeStatus;
    use crate::control_store::{ControlStore, InMemoryControlStore};
    use crate::error::Result as CoreResult;
    use crate::observability::{active_phase_depth, in_phase, scope_request, Phase};
    use crate::resolver::StaticResolver;
    use crate::router::{DriverFactory, Registry, Router, StorageDriver};
    use crate::style_store::FileStyleStore;

    /// Minimal `StorageDriver`: declares a "demo" physical collection and
    /// nothing else — `AppContext::get_or_populate` only ever consults
    /// `Router::effective_settings`, never a real capability, so no
    /// feature/tile source is needed to exercise it.
    struct FakeDriver;

    #[async_trait::async_trait]
    impl CatalogSource for FakeDriver {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![PhysicalCollection {
                name: "demo".to_string(),
                geometry_column: None,
                primary_key: None,
                srid: None,
                geometry_type: None,
            }])
        }
    }

    impl StorageDriver for FakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FakeDriver)
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

    /// Records every `ttl` argument that actually reached this cache, and
    /// separately counts calls through the plain (non-TTL) entry point —
    /// enough to prove `AppContext::get_or_populate`'s settings-resolution
    /// logic without a real L2 backend (`cache.rs`'s own tests already cover
    /// a passed TTL reaching an `L2CacheAdapter`'s write).
    struct RecordingCache {
        last_ttl: Mutex<Option<Duration>>,
        ttl_calls: AtomicUsize,
        plain_calls: AtomicUsize,
    }

    impl RecordingCache {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                last_ttl: Mutex::new(None),
                ttl_calls: AtomicUsize::new(0),
                plain_calls: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait::async_trait]
    impl TileCache for RecordingCache {
        async fn get(&self, _key: &TileKey) -> Option<Bytes> {
            None
        }

        async fn insert(&self, _key: TileKey, _value: Bytes) {}

        async fn get_or_populate(
            &self,
            _key: TileKey,
            populate: PopulateFuture,
        ) -> Result<Bytes, Arc<Error>> {
            self.plain_calls.fetch_add(1, Ordering::SeqCst);
            populate.await.map_err(Arc::new)
        }

        async fn get_or_populate_with_ttl(
            &self,
            _key: TileKey,
            populate: PopulateFuture,
            ttl: Duration,
        ) -> Result<Bytes, Arc<Error>> {
            self.ttl_calls.fetch_add(1, Ordering::SeqCst);
            *self.last_ttl.lock().unwrap() = Some(ttl);
            populate.await.map_err(Arc::new)
        }
    }

    struct TimedCache;

    #[async_trait::async_trait]
    impl TileCache for TimedCache {
        async fn get(&self, _key: &TileKey) -> Option<Bytes> {
            None
        }

        async fn insert(&self, _key: TileKey, _value: Bytes) {}

        async fn get_or_populate(
            &self,
            _key: TileKey,
            populate: PopulateFuture,
        ) -> Result<Bytes, Arc<Error>> {
            tokio::time::advance(Duration::from_millis(2)).await;
            let value = populate.await.map_err(Arc::new)?;
            tokio::time::advance(Duration::from_millis(5)).await;
            Ok(value)
        }

        async fn get_or_populate_with_ttl(
            &self,
            key: TileKey,
            populate: PopulateFuture,
            _ttl: Duration,
        ) -> Result<Bytes, Arc<Error>> {
            self.get_or_populate(key, populate).await
        }
    }

    struct TimedTileSource;

    #[async_trait::async_trait]
    impl TileSource for TimedTileSource {
        async fn mvt_tile(
            &self,
            collection: &CollectionDecl,
            coord: TileCoord,
            filter: Option<&Filter>,
        ) -> CoreResult<Option<Bytes>> {
            assert_eq!(collection.id, "demo");
            assert_eq!(coord, TileCoord { z: 2, x: 1, y: 3 });
            assert!(filter.is_none());
            tokio::time::advance(Duration::from_millis(4)).await;
            Ok(Some(Bytes::from_static(b"mvt")))
        }
    }

    fn test_key() -> TileKey {
        TileKey {
            tenant: "public".to_string(),
            catalog: "default".to_string(),
            collection: "demo".to_string(),
            tms: TileMatrixSet::WebMercatorQuad,
            z: 0,
            x: 0,
            y: 0,
            encoding: crate::cache::Encoding::Mvt,
            policy_fingerprint: None,
            properties: Vec::new(),
            generation: 0,
        }
    }

    fn build_context(cache: Arc<dyn TileCache>, settings_yaml: &str) -> AppContext {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: fake, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: demo
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
{settings_yaml}
"#
        ))
        .unwrap();
        config.validate().unwrap();

        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = Router::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        AppContext::new(config, router, resolver, None, cache, style_store)
    }

    #[test]
    fn durable_control_store_is_opt_in_and_stable_across_reload() {
        let cache: Arc<dyn TileCache> = RecordingCache::new();
        let mut ctx = build_context(cache, "");
        assert!(ctx.control_store.is_none());

        let store: Arc<dyn ControlStore> = Arc::new(InMemoryControlStore::new());
        ctx = ctx.with_control_store(Arc::clone(&store));
        assert!(Arc::ptr_eq(ctx.control_store.as_ref().unwrap(), &store));

        let current = ctx.current();
        let config = current.config.clone();
        drop(current);
        let mut registry = Registry::new();
        registry.register(Arc::new(FakeFactory));
        let router = Router::build(&config, &registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        ctx.reload(config, router, resolver, None);

        assert!(Arc::ptr_eq(ctx.control_store.as_ref().unwrap(), &store));
    }

    #[test]
    fn control_runtime_status_defaults_to_zero_and_accepts_the_boot_tracker() {
        let cache: Arc<dyn TileCache> = RecordingCache::new();
        let ctx = build_context(cache, "");

        assert_eq!(ctx.current().control_revision, None);
        assert_eq!(ctx.control_runtime_status.snapshot().store_revision, 0);
        assert_eq!(ctx.control_runtime_status.snapshot().applied_revision, 0);

        let status = Arc::new(ControlRuntimeStatus::new(7));
        let ctx = ctx.with_control_runtime_status(Arc::clone(&status));

        assert!(Arc::ptr_eq(&ctx.control_runtime_status, &status));
    }

    #[test]
    fn control_revision_is_published_in_the_same_context_generation() {
        let cache: Arc<dyn TileCache> = RecordingCache::new();
        let ctx = build_context(cache, "");
        let current = ctx.current();
        let config = current.config.clone();
        let tenants = current.tenants.clone();
        let authorizer = current.authorizer.clone();
        let registry_reader = Arc::clone(&current.registry);
        let config_version = current.config_version.clone();
        let control_policy = Arc::clone(&current.control_policy);
        drop(current);
        let mut driver_registry = Registry::new();
        driver_registry.register(Arc::new(FakeFactory));
        let router = Router::build(&config, &driver_registry).unwrap();
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));

        ctx.reload_with_registry_version_and_policy(
            config,
            tenants,
            router,
            resolver,
            authorizer,
            registry_reader,
            config_version,
            control_policy,
            Some(7),
        );

        assert_eq!(ctx.current().control_revision, Some(7));
    }

    fn populate_ok() -> PopulateFuture {
        Box::pin(async { Ok(Bytes::from_static(b"tile")) })
    }

    /// `#46`: a collection with an explicit `cache_ttl_s` gets that exact
    /// value threaded through `get_or_populate_with_ttl`.
    #[tokio::test]
    async fn passes_the_collections_explicit_cache_ttl_s() {
        let cache = RecordingCache::new();
        let ctx = build_context(
            Arc::clone(&cache) as Arc<dyn TileCache>,
            "    settings: { cache_ttl_s: 45 }",
        );

        let result = ctx.get_or_populate("demo", test_key(), populate_ok()).await;
        assert_eq!(result.unwrap(), Bytes::from_static(b"tile"));
        assert_eq!(cache.ttl_calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.plain_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            *cache.last_ttl.lock().unwrap(),
            Some(Duration::from_secs(45))
        );
    }

    /// `#46`: a collection with no `cache_ttl_s` anywhere in its chain still
    /// resolves to the settings module's own default (`DEFAULT_SETTINGS_CACHE_TTL_S`,
    /// which is kept equal to the L2 adapter's own default TTL magnitude —
    /// see `settings.rs` — so this is the "byte-for-byte unchanged" case for
    /// any deployment that leaves both at their defaults) rather than
    /// skipping the TTL-aware path entirely.
    #[tokio::test]
    async fn falls_back_to_the_settings_default_when_the_chain_sets_nothing() {
        let cache = RecordingCache::new();
        let ctx = build_context(Arc::clone(&cache) as Arc<dyn TileCache>, "");

        let result = ctx.get_or_populate("demo", test_key(), populate_ok()).await;
        assert_eq!(result.unwrap(), Bytes::from_static(b"tile"));
        assert_eq!(cache.ttl_calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.plain_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            *cache.last_ttl.lock().unwrap(),
            Some(Duration::from_secs(
                crate::settings::DEFAULT_SETTINGS_CACHE_TTL_S
            ))
        );
    }

    /// `#46`: a collection internal id `Router` never indexed — the one
    /// case `effective_settings` returns `None` for — must fall back to the
    /// plain, non-TTL entry point untouched: today's behavior.
    #[tokio::test]
    async fn falls_back_to_the_plain_entry_point_for_an_unindexed_collection() {
        let cache = RecordingCache::new();
        let ctx = build_context(Arc::clone(&cache) as Arc<dyn TileCache>, "");

        let result = ctx
            .get_or_populate("not-a-real-collection", test_key(), populate_ok())
            .await;
        assert_eq!(result.unwrap(), Bytes::from_static(b"tile"));
        assert_eq!(cache.plain_calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.ttl_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn cache_accounting_excludes_population_while_encode_excludes_nested_query() {
        let ctx = build_context(Arc::new(TimedCache), "");
        let populate: PopulateFuture = Box::pin(async {
            in_phase(Phase::Query, tokio::time::advance(Duration::from_millis(3))).await;
            tokio::time::advance(Duration::from_millis(4)).await;
            Ok(Bytes::from_static(b"tile"))
        });

        let (result, snapshot) =
            scope_request(ctx.get_or_populate("demo", test_key(), populate)).await;

        assert_eq!(result.unwrap(), Bytes::from_static(b"tile"));
        assert_eq!(snapshot.cache(), Duration::from_millis(7));
        assert_eq!(snapshot.query(), Duration::from_millis(3));
        assert_eq!(
            snapshot.encode(Duration::from_millis(14)),
            Duration::from_millis(4)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn explicit_ttl_mvt_cache_work_is_exclusive_from_population_time() {
        let ctx = build_context(Arc::new(TimedCache), "");
        let collection: CollectionDecl = serde_yaml::from_str(
            "id: demo\ncatalog: default\nstorage: main\ntable: demo\ngeometry: geom\npk: id",
        )
        .unwrap();
        let source: Arc<dyn TileSource> = Arc::new(TimedTileSource);
        let coord = TileCoord { z: 2, x: 1, y: 3 };

        let (result, snapshot) = scope_request(ctx.fetch_mvt(
            "public",
            "default",
            "demo",
            TileMatrixSet::WebMercatorQuad,
            coord,
            &collection,
            &source,
            None,
            Some(Duration::from_secs(45)),
        ))
        .await;

        assert!(matches!(result, MvtFetch::Hit(bytes) if bytes == Bytes::from_static(b"mvt")));
        assert_eq!(snapshot.cache(), Duration::from_millis(7));
        assert_eq!(snapshot.query(), Duration::ZERO);
        assert_eq!(
            snapshot.encode(Duration::from_millis(11)),
            Duration::from_millis(4)
        );
    }

    /// A do-nothing `TileSource` whose only job is letting `fetch_mvt` reach
    /// `KeyRecordingCache` — the fixed `Bytes` it returns is never asserted
    /// on, only the key `fetch_mvt` builds around it.
    struct StubTileSource;

    #[async_trait::async_trait]
    impl TileSource for StubTileSource {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<Bytes>> {
            Ok(Some(Bytes::from_static(b"mvt")))
        }
    }

    /// Records the `TileKey` `get_or_populate`/`get_or_populate_with_ttl` was
    /// last called with — the seam `#85`'s cache-key test below needs, that
    /// `RecordingCache` (ttl-focused, `_key` discarded) doesn't provide.
    struct KeyRecordingCache {
        last_key: Mutex<Option<TileKey>>,
    }

    impl KeyRecordingCache {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                last_key: Mutex::new(None),
            })
        }
    }

    #[async_trait::async_trait]
    impl TileCache for KeyRecordingCache {
        async fn get(&self, _key: &TileKey) -> Option<Bytes> {
            None
        }

        async fn insert(&self, _key: TileKey, _value: Bytes) {}

        async fn get_or_populate(
            &self,
            key: TileKey,
            populate: PopulateFuture,
        ) -> Result<Bytes, Arc<Error>> {
            *self.last_key.lock().unwrap() = Some(key);
            populate.await.map_err(Arc::new)
        }

        async fn get_or_populate_with_ttl(
            &self,
            key: TileKey,
            populate: PopulateFuture,
            _ttl: Duration,
        ) -> Result<Bytes, Arc<Error>> {
            self.get_or_populate(key, populate).await
        }
    }

    /// `#85`: `fetch_mvt` folds the collection's resolved `tile_properties`
    /// into the Mvt-encoded cache key it builds — a config change to the
    /// allowlist changes the key, so it can never serve a tile cached under
    /// the old attribute shape. See `TileKey::properties`'s own doc.
    #[tokio::test]
    async fn fetch_mvt_folds_the_collections_tile_properties_into_the_cache_key() {
        let cache = KeyRecordingCache::new();
        let ctx = build_context(Arc::clone(&cache) as Arc<dyn TileCache>, "");
        let mut collection: CollectionDecl = serde_yaml::from_str(
            "id: demo\ncatalog: default\nstorage: main\ntable: demo\ngeometry: geom\npk: id",
        )
        .unwrap();
        collection.tile_properties = vec!["name".to_string(), "pop".to_string()];
        let source: Arc<dyn TileSource> = Arc::new(StubTileSource);
        let coord = TileCoord { z: 2, x: 1, y: 3 };

        ctx.fetch_mvt(
            "public",
            "default",
            "demo",
            TileMatrixSet::WebMercatorQuad,
            coord,
            &collection,
            &source,
            None,
            None,
        )
        .await;

        let key = cache.last_key.lock().unwrap().clone().unwrap();
        assert_eq!(key.properties, vec!["name".to_string(), "pop".to_string()]);
    }

    struct SnapshotSeams {
        label: &'static str,
    }

    #[async_trait::async_trait]
    impl Resolver for SnapshotSeams {
        async fn resolve_tenant(&self, external: &str) -> CoreResult<String> {
            assert_eq!(external, "public");
            assert_eq!(active_phase_depth(), 1, "resolver must have one wrapper");
            tokio::time::advance(Duration::from_millis(1)).await;
            Ok(self.label.to_string())
        }

        async fn resolve_catalog(&self, _: &str, _: &str) -> CoreResult<String> {
            unreachable!("not needed by this snapshot test")
        }

        async fn resolve_collection(&self, _: &str, _: &str) -> CoreResult<String> {
            unreachable!("not needed by this snapshot test")
        }

        fn tenant_external_id(&self, _: &str) -> Option<&str> {
            None
        }

        fn catalog_external_id(&self, _: &str) -> Option<&str> {
            None
        }

        fn collection_external_id(&self, _: &str) -> Option<&str> {
            None
        }

        fn catalogs_for_tenant(&self, _: &str) -> Vec<(&str, &str)> {
            vec![]
        }

        fn catalog_count(&self) -> usize {
            0
        }
    }

    #[async_trait::async_trait]
    impl RegistryReader for SnapshotSeams {
        async fn catalog(&self, tenant: &str, external: &str) -> CoreResult<Option<CatalogDecl>> {
            assert_eq!(tenant, "tenant-internal");
            assert_eq!(external, "default");
            assert_eq!(active_phase_depth(), 1, "registry must have one wrapper");
            tokio::time::advance(Duration::from_millis(2)).await;
            Ok(Some(CatalogDecl {
                id: self.label.to_string(),
                tenant: "public".to_string(),
                ..serde_yaml::from_str("id: placeholder\ntenant: public").unwrap()
            }))
        }

        async fn collection(&self, _: &str, _: &str) -> CoreResult<Option<CollectionDecl>> {
            unreachable!("not needed by this snapshot test")
        }

        async fn list_catalogs(&self, _: &str, _: PageRequest) -> CoreResult<Page<CatalogDecl>> {
            unreachable!("not needed by this snapshot test")
        }

        async fn list_collections(
            &self,
            _: &str,
            _: PageRequest,
        ) -> CoreResult<Page<CollectionDecl>> {
            unreachable!("not needed by this snapshot test")
        }
    }

    fn snapshot_parts(
        slow_request_ms: u64,
        label: &'static str,
    ) -> (
        AppConfig,
        Router,
        Arc<dyn Resolver>,
        Arc<dyn RegistryReader>,
    ) {
        let config: AppConfig = serde_yaml::from_str(&format!(
            "tenants: [{{ id: public }}]\nsettings: {{ slow_request_ms: {slow_request_ms} }}"
        ))
        .unwrap();
        config.validate().unwrap();
        let router = Router::build(&config, &Registry::new()).unwrap();
        let seams = Arc::new(SnapshotSeams { label });
        let resolver: Arc<dyn Resolver> = seams.clone();
        let registry: Arc<dyn RegistryReader> = seams;
        (config, router, resolver, registry)
    }

    async fn assert_observed_snapshot(state: &ContextState, label: &str) {
        let (_, phases) = scope_request(async {
            assert_eq!(
                state.resolver.resolve_tenant("public").await.unwrap(),
                label
            );
            assert_eq!(
                state
                    .registry
                    .catalog("tenant-internal", "default")
                    .await
                    .unwrap()
                    .unwrap()
                    .id,
                label
            );
        })
        .await;
        assert_eq!(phases.routing(), Duration::from_millis(1));
        assert_eq!(phases.query(), Duration::from_millis(2));
    }

    #[tokio::test(start_paused = true)]
    async fn construction_and_reload_keep_observed_resolver_and_registry_in_one_snapshot() {
        let (before_config, before_router, before_resolver, before_registry) =
            snapshot_parts(111, "before");
        let before_tenants = before_config.tenants.clone();
        let ctx = AppContext::new_with_registry(
            before_config,
            before_tenants,
            before_router,
            before_resolver,
            None,
            before_registry,
            RecordingCache::new(),
            Arc::new(FileStyleStore::new(&[])),
        );

        let before = ctx.current();
        assert_observed_snapshot(&before, "before").await;

        let (after_config, after_router, after_resolver, after_registry) =
            snapshot_parts(222, "after");
        let after_tenants = after_config.tenants.clone();
        ctx.reload_with_registry(
            after_config,
            after_tenants,
            after_router,
            after_resolver,
            None,
            after_registry,
        );

        let after = ctx.current();
        assert_eq!(before.config.settings.slow_request_ms, Some(111));
        assert_eq!(after.config.settings.slow_request_ms, Some(222));
        assert_observed_snapshot(&after, "after").await;
        assert_observed_snapshot(&before, "before").await;
    }

    // -- `build_router_and_resolver` (`#42`, third slice) --------------------

    use crate::config::{CatalogDecl, CollectionDecl};
    use crate::registry::{FileRegistryReader, Page, PageRequest};
    use crate::storage::{FeaturePage, FeatureSource, ItemsQuery};

    /// Same shape as the outer `FakeDriver`, plus a `FeatureSource` — needed
    /// here (unlike the cache tests above) because `resolve_features` refuses
    /// a driver that doesn't advertise the capability.
    struct RoutingFakeDriver;

    #[async_trait::async_trait]
    impl CatalogSource for RoutingFakeDriver {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![])
        }
    }

    #[async_trait::async_trait]
    impl FeatureSource for RoutingFakeDriver {
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
            _filter: Option<&crate::filter::Filter>,
        ) -> CoreResult<Option<serde_json::Value>> {
            Ok(None)
        }
    }

    impl StorageDriver for RoutingFakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(RoutingFakeDriver)
        }

        fn feature_source(&self) -> Option<Arc<dyn FeatureSource>> {
            Some(Arc::new(RoutingFakeDriver))
        }
    }

    struct RoutingFakeFactory;

    impl DriverFactory for RoutingFakeFactory {
        fn name(&self) -> &str {
            "routing-fake"
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(RoutingFakeDriver))
        }
    }

    fn routing_fake_registry() -> Registry {
        let mut registry = Registry::new();
        registry.register(Arc::new(RoutingFakeFactory));
        registry
    }

    /// A `RegistryReader` that fails every call — proves the `File` arm
    /// never consults it, and separately exercises the "the walk itself
    /// fails" branch of the relational arm.
    struct AlwaysErrorsRegistryReader;

    #[async_trait::async_trait]
    impl crate::registry::RegistryReader for AlwaysErrorsRegistryReader {
        async fn catalog(&self, _: &str, _: &str) -> CoreResult<Option<CatalogDecl>> {
            unreachable!("not exercised by these tests")
        }
        async fn collection(&self, _: &str, _: &str) -> CoreResult<Option<CollectionDecl>> {
            unreachable!("not exercised by these tests")
        }
        async fn list_catalogs(&self, _: &str, _: PageRequest) -> CoreResult<Page<CatalogDecl>> {
            Err(Error::Storage("registry unreachable".into()))
        }
        async fn list_collections(
            &self,
            _: &str,
            _: PageRequest,
        ) -> CoreResult<Page<CollectionDecl>> {
            Err(Error::Storage("registry unreachable".into()))
        }
    }

    fn routing_fake_config(collection_external_id: &str) -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: routing-fake, url_env: DATABASE_URL }} ]
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

    fn routing_relational_operator_config() -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: routing-fake, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        config
    }

    /// `RegistryBackend::File` (the default) never consults `registry_reader`
    /// at all, and resolves/routes exactly as `Router::build`/`StaticResolver::
    /// build` would have on their own.
    #[tokio::test]
    async fn file_backend_never_consults_the_registry_reader_and_resolves_and_routes() {
        let config = routing_fake_config("demo-ext");
        let driver_registry = routing_fake_registry();

        let tenant_reader = crate::tenant::FileTenantReader::build(&config);
        let (router, resolver, _tenants) = build_router_and_resolver(
            &config,
            &driver_registry,
            &AlwaysErrorsRegistryReader,
            &tenant_reader,
        )
        .await
        .expect("the file backend must not need the registry reader to succeed");

        let tenant = resolver.resolve_tenant("public").await.unwrap();
        let catalog = resolver.resolve_catalog(&tenant, "default").await.unwrap();
        let collection = resolver
            .resolve_collection(&catalog, "demo-ext")
            .await
            .unwrap();
        router
            .resolve_features(&tenant, &catalog, &collection)
            .await
            .expect("routes exactly like Router::build would have");
    }

    /// `RegistryBackend::Relational`'s whole point (`#42`, third slice): a
    /// collection this operator's own config never declares (`catalogs:`/
    /// `collections:` are empty here, per the double-source rule) still
    /// resolves its external id AND routes/serves through the RESOLVED
    /// internal id — proving the fix for the gap where `Router` alone had
    /// the collection indexed but `Resolver` had no way to turn its external
    /// id into the internal one `Router` needs, making it unreachable from
    /// any URL despite being "routable" in isolation.
    #[tokio::test]
    async fn relational_backend_resolves_and_routes_a_collection_yaml_never_declared() {
        let operator_config = routing_relational_operator_config();
        assert!(
            operator_config.catalogs.is_empty() && operator_config.collections.is_empty(),
            "this test's whole premise is that YAML declares neither"
        );

        let db_config = routing_fake_config("db-only-demo");
        let reader = FileRegistryReader::build(&db_config);
        let driver_registry = routing_fake_registry();
        let tenant_reader = crate::tenant::FileTenantReader::build(&db_config);

        let (router, resolver, _tenants) =
            build_router_and_resolver(&operator_config, &driver_registry, &reader, &tenant_reader)
                .await
                .expect("walks the reader and builds successfully");

        let tenant = resolver
            .resolve_tenant("public")
            .await
            .expect("the tenant sourced from the tenant reader must resolve");
        let catalog = resolver
            .resolve_catalog(&tenant, "default")
            .await
            .expect("the catalog sourced from the registry reader must resolve");
        let collection = resolver
            .resolve_collection(&catalog, "db-only-demo")
            .await
            .expect("the collection sourced from the registry reader must resolve");

        let (decl, _source) = router
            .resolve_features(&tenant, &catalog, &collection)
            .await
            .expect("the resolved internal ids must be routable");
        assert_eq!(decl.external_id(), "db-only-demo");
    }

    /// The relational tenant snapshot is authoritative for routing too: the
    /// operator document declares no tenants under the double-source rule;
    /// `Router` and `Resolver` must index the tenant read from the relational
    /// backend alongside its catalogs and collections.
    #[tokio::test]
    async fn relational_backend_routes_the_tenant_reader_snapshot_not_the_file_tenants() {
        let operator_config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: routing-fake, url_env: DATABASE_URL } ]
registry: { backend: relational, storage: main }
"#,
        )
        .unwrap();
        operator_config.validate().unwrap();

        let mut published = routing_fake_config("published-demo");
        published.tenants = vec![crate::config::TenantDecl {
            id: "database-tenant".to_string(),
            external_id: Some("database-tenant".to_string()),
            settings: Default::default(),
        }];
        published.catalogs[0].tenant = "database-tenant".to_string();
        published.validate().unwrap();

        let tenant_reader = crate::tenant::FileTenantReader::build(&published);
        let registry_reader = FileRegistryReader::build(&published);
        let driver_registry = routing_fake_registry();
        let (router, resolver, tenants) = build_router_and_resolver(
            &operator_config,
            &driver_registry,
            &registry_reader,
            &tenant_reader,
        )
        .await
        .expect("published tenant, catalog, and collection form one routing snapshot");

        assert_eq!(tenants, published.tenants);

        let tenant = resolver
            .resolve_tenant("database-tenant")
            .await
            .expect("tenant comes from the tenant reader snapshot");
        assert_eq!(tenant, "database-tenant");
        assert!(operator_config.tenants.is_empty());

        let catalog = resolver
            .resolve_catalog(&tenant, "default")
            .await
            .expect("catalog stays scoped to the published tenant");
        let collection = resolver
            .resolve_collection(&catalog, "published-demo")
            .await
            .expect("collection is reachable through the published tenant");
        router
            .resolve_features(&tenant, &catalog, &collection)
            .await
            .expect("router indexes the same published tenant snapshot");
    }

    /// A registry reader that fails mid-walk fails `build_router_and_resolver`
    /// outright — no partial `Router`/`Resolver` pair is ever built or
    /// returned.
    #[tokio::test]
    async fn relational_backend_propagates_a_reader_failure() {
        let operator_config = routing_relational_operator_config();
        let driver_registry = routing_fake_registry();
        let tenant_config = routing_fake_config("unused");
        let tenant_reader = crate::tenant::FileTenantReader::build(&tenant_config);

        let result = build_router_and_resolver(
            &operator_config,
            &driver_registry,
            &AlwaysErrorsRegistryReader,
            &tenant_reader,
        )
        .await;
        assert!(result.is_err(), "a reader failure must surface as Err");
    }

    /// A relational snapshot that fails the exact same referential-integrity
    /// bar a YAML `catalogs`/`collections` document is held to (here: a
    /// collection whose `storage` isn't one this operator's own config
    /// declares) fails `build_router_and_resolver` before either index is
    /// built — the database's rows get no less scrutiny than a hand-written
    /// config file.
    #[tokio::test]
    async fn relational_backend_rejects_a_snapshot_that_fails_validation() {
        let operator_config = routing_relational_operator_config();
        let db_config: AppConfig = serde_yaml::from_str(
            r#"
storages:
  - { id: main, driver: routing-fake, url_env: DATABASE_URL }
  - { id: other, driver: routing-fake, url_env: DATABASE_URL2 }
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: other
    table: demo
    geometry: geom
    pk: id
"#,
        )
        .unwrap();
        db_config.validate().unwrap();
        let reader = FileRegistryReader::build(&db_config);
        let driver_registry = routing_fake_registry();
        let tenant_reader = crate::tenant::FileTenantReader::build(&db_config);

        match build_router_and_resolver(&operator_config, &driver_registry, &reader, &tenant_reader)
            .await
        {
            Err(Error::Config(message)) => {
                assert!(message.contains("other"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }
}
