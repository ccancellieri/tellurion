//! `RegistryReader` — the read seam between the routing layer and wherever
//! catalog/collection *declarations* themselves live (`#42`, registry
//! scale-out). Distinct from two seams that already exist for adjacent
//! concerns: `ConfigStore` loads a whole `AppConfig` once, at boot, and
//! never again; `Resolver` (`#39`) only ever answers "what is this external
//! id's internal id," never hands back the declaration itself. This trait is
//! how a caller gets *the declaration* for one collection/catalog, or a
//! keyset-paginated slice of many, without touching every other declaration
//! in the registry to answer either question — the property that keeps a
//! request for one collection at O(1) registry state as the registry grows,
//! and the seam a future routed-storage registry driver (relational first,
//! per the design note) implements against instead of every protocol
//! handler walking `AppConfig.collections`/`.catalogs` directly.
//!
//! `FileRegistryReader`, the first implementor, is an in-memory index built
//! once from the same loaded `AppConfig` that `Router` and `StaticResolver`
//! already index — a file-backed deployment pays exactly today's cost,
//! byte-for-byte the behavior small deployments already have.
//!
//! A relational registry backend (`#42`, second slice) is a second
//! `RegistryReader` implementor, connected over the network rather than
//! built in memory — `tellurion-postgis`'s `PostgisRegistryReader`. Since
//! `tellurion-core` never depends on a concrete database client crate (see
//! this crate's own top-level doc), it cannot construct one directly; it
//! only defines [`RelationalRegistryFactory`], the same "trait here, driver
//! elsewhere" boundary `DriverFactory`/`Registry` (`router.rs`) already draw
//! for storage drivers. The wiring layer (the `tellurion` binary) registers
//! one concrete factory per compiled-in driver crate into a
//! [`RelationalRegistryFactories`], each under its own declared name
//! (`#162`), and calls [`build_registry_reader`] to dispatch on
//! `AppConfig.registry` — `backend` picking file vs relational and
//! `implementation` picking WHICH relational — before handing the result to
//! `AppContext::new`/`AppContext::reload` —
//! `AppContext` itself never chooses a backend or does I/O; it only holds
//! whichever `RegistryReader` the caller already built and validated, the
//! same treatment `Router`/`Resolver`/the tenant authorizer already get (see
//! `context.rs`'s own doc for why).
//!
//! Listing is keyset-paginated, never OFFSET (the same paging discipline
//! `ItemsQuery::token` already applies to feature items): entries are
//! ordered by external id, and a page's cursor is the external id of its
//! last entry. Callers must treat it as opaque — even though this
//! implementation's cursor happens to be a plain value, the same "opaque in
//! practice, plain in shape" convention `tellurion-postgis`'s own keyset
//! item tokens already use.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::sync::Arc;

use crate::config::{
    AppConfig, CatalogDecl, CollectionDecl, RegistryBackend, RoutingSnapshot, TenantDecl,
};
use crate::error::{Error, Result};

/// One page request: at most `limit` entries, resuming just after `after`
/// (the previous page's returned cursor) when present. `after: None` starts
/// from the beginning of the ordering.
#[derive(Debug, Clone, Default)]
pub struct PageRequest {
    pub limit: u32,
    pub after: Option<String>,
}

/// One page of `T`, plus the cursor for the next page — `None` once the
/// ordering is exhausted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

#[async_trait::async_trait]
pub trait RegistryReader: Send + Sync {
    /// `catalog_external_id` -> its declaration, scoped to
    /// `tenant_internal_id` (catalog external ids are unique per tenant, not
    /// globally — see `AppConfig::validate`). `Ok(None)` for an unknown
    /// external id under this tenant.
    async fn catalog(
        &self,
        tenant_internal_id: &str,
        catalog_external_id: &str,
    ) -> Result<Option<CatalogDecl>>;

    /// `collection_external_id` -> its declaration, scoped to
    /// `catalog_internal_id`. `Ok(None)` for an unknown external id under
    /// this catalog — a single-declaration lookup that touches only this
    /// collection's own index entry, never every other collection in the
    /// registry.
    async fn collection(
        &self,
        catalog_internal_id: &str,
        collection_external_id: &str,
    ) -> Result<Option<CollectionDecl>>;

    /// Every catalog owned by `tenant_internal_id`, keyset-paginated by
    /// external id.
    async fn list_catalogs(
        &self,
        tenant_internal_id: &str,
        page: PageRequest,
    ) -> Result<Page<CatalogDecl>>;

    /// Every collection owned by `catalog_internal_id`, keyset-paginated by
    /// external id — the seam `GET /collections` reads through
    /// (`tellurion-features::handlers::list_collections`) instead of
    /// scanning every collection in the registry on every request.
    async fn list_collections(
        &self,
        catalog_internal_id: &str,
        page: PageRequest,
    ) -> Result<Page<CollectionDecl>>;
}

/// Connects a relational `RegistryReader` (`#42`, second slice) — implemented
/// by a driver crate (`tellurion-postgis`'s `PostgisRegistryFactory`) and
/// registered by the wiring layer into a [`RelationalRegistryFactories`], the
/// same "driver stays out of core" boundary `DriverFactory` already draws for
/// storage drivers. `connect` actually attempts a connection rather than
/// deferring it, so an unreachable database is a hard error the caller sees
/// immediately — never a `RegistryReader` that silently fails its first real
/// query.
///
/// [`name`](Self::name) is the implementation's stable, config-facing name
/// (`#162`): the string `registry.implementation` selects it by, and the
/// string a boot log enumerates. It belongs to the implementation rather than
/// to the registration call site so a registration can never disagree with
/// the name an operator writes in config — the same discipline
/// `DriverFactory::name` and `ProcessRunner::description().id` already apply
/// to their own seams.
#[async_trait::async_trait]
pub trait RelationalRegistryFactory: Send + Sync {
    fn name(&self) -> &str;

    async fn connect(&self, database_url: &str) -> Result<Arc<dyn RegistryReader>>;
}

/// The relational registry backend seam's boot-time registry (`#112`,
/// `#162`): every driver crate this binary was compiled with registers
/// exactly one [`RelationalRegistryFactory`] here, once, in `main`.
///
/// A thin wrapper over [`NamedRegistry`](crate::extension::NamedRegistry) —
/// same "named, not discovered", "refuse by name", "deterministic iteration"
/// properties every other seam gets, keyed by each factory's own
/// [`name`](RelationalRegistryFactory::name) so a registration cannot
/// disagree with the name it registers under.
///
/// Not called `*Registry` like `router::Registry`/`process::ProcessRegistry`
/// only because "registry" already names this seam's *subject* — the
/// catalog/collection registry — and `RegistryRegistry` would read as a
/// stutter rather than as a type.
///
/// An empty one is the ordinary state of a binary built without any driver
/// crate providing a relational registry: absent and compiled-out are
/// deliberately the same thing here (see `NamedRegistry`'s own doc), and both
/// make a `registry.backend: relational` config fail boot by name.
#[derive(Default)]
pub struct RelationalRegistryFactories {
    factories: crate::extension::NamedRegistry<dyn RelationalRegistryFactory>,
}

impl RelationalRegistryFactories {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `factory` under its own declared name, replacing whatever
    /// was registered under that name before — the same last-write-wins
    /// behaviour [`NamedRegistry`](crate::extension::NamedRegistry) gives
    /// every other seam.
    pub fn register(&mut self, factory: Arc<dyn RelationalRegistryFactory>) {
        let name = factory.name().to_string();
        self.factories.register(name, factory);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn RelationalRegistryFactory>> {
        self.factories.get(name)
    }

    /// Every registered implementation name, alphabetically — what a boot log
    /// line enumerates as "the relational registry implementations this
    /// binary actually contains" (`#162`), in the same order run to run
    /// regardless of the order `register` happened to be called in.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.factories.names()
    }

    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }

    pub fn len(&self) -> usize {
        self.factories.len()
    }
}

/// Builds the `RegistryReader` a boot or reload should use for `config`:
/// `RegistryBackend::File` (the default) is always `FileRegistryReader`, no
/// I/O, and never consults `factories` at all — `file` stays the direct
/// built-in backend, not a registry entry, because there is no driver crate
/// it could be compiled out with. `RegistryBackend::Relational` selects one
/// registered factory (see [`select_relational_implementation`] for how
/// `registry.implementation` picks it and what an unknown name produces),
/// resolves `config.registry.storage`'s `url_env` (reusing the same
/// storage-connection shape `StorageDecl` already provides — never a second
/// one) and hands the URL to that factory's `connect`, so a database that's
/// unreachable right now surfaces as an `Err` here rather than a
/// `RegistryReader` that only fails once something actually calls it.
///
/// This function does no swapping of its own — it only builds and validates
/// one `RegistryReader`. The caller (`main`/`reload.rs`) decides what a
/// failure means: at boot, propagating it fails the whole process the same
/// way an unresolvable `Router::build` reference does; on reload, logging it
/// and returning early leaves `AppContext`'s previous, still-connected state
/// serving — see `context.rs`'s "always replaced as a unit" doc.
pub async fn build_registry_reader(
    config: &AppConfig,
    factories: &RelationalRegistryFactories,
) -> Result<Arc<dyn RegistryReader>> {
    match config.registry.backend {
        RegistryBackend::File => Ok(Arc::new(FileRegistryReader::build(config))),
        RegistryBackend::Relational => {
            let database_url = resolve_relational_database_url(config)?;
            let factory = select_relational_implementation(
                config.registry.implementation.as_deref(),
                &factories.factories,
                "registry",
            )?;
            factory.connect(&database_url).await
        }
    }
}

/// Picks which registered relational implementation a
/// `registry.backend: relational` config means, for either half of that one
/// knob — the catalog/collection [`RegistryReader`] seam here and the
/// [`TenantReader`](crate::tenant::TenantReader) seam in `tenant.rs`, which
/// share the SAME `registry.backend`/`.storage`/`.implementation` triple
/// (see `RegistryConfig`'s own doc for why a deployment does not get a second
/// knob). `seam` is the word that distinguishes them in an error message
/// (`"registry"` / `"tenant"`); everything else about the selection rule is
/// identical, so there is exactly one place that implements it.
///
/// Four cases, three of which are refusals that name what went wrong:
///
/// - `implementation` names something registered: that factory, chosen by
///   name rather than by "the only one there happened to be."
/// - `implementation` names something absent: `Error::Config` naming the
///   config key, the unknown name, and every name that IS registered.
///   Indistinguishable, on purpose, from a name whose driver crate was
///   compiled out — both are "this binary does not contain that."
/// - `implementation` unset, exactly one factory registered: that one. This
///   is the backwards-compatibility case and the reason the field is
///   `Option` (`#162`): before this existed, a `relational` backend meant
///   "the sole compiled-in relational factory," and a config written then
///   describes exactly that arrangement. Absence therefore means what this
///   deployment already did, never "unconfigured" — the same standard
///   `bootstrap::ControlStoreLocator::LegacyFile` sets.
/// - `implementation` unset, zero or several registered: `Error::Config`.
///   Zero keeps the pre-`#162` wording byte-for-byte, so a binary built
///   without any relational driver crate fails exactly as it always has.
///   Several is refused rather than resolved by alphabetical order or
///   registration order — picking one silently is the "silent degradation"
///   this codebase refuses everywhere else, and it is unreachable today
///   because exactly one relational implementation exists.
pub(crate) fn select_relational_implementation<'a, T: ?Sized>(
    implementation: Option<&str>,
    factories: &'a crate::extension::NamedRegistry<T>,
    seam: &str,
) -> Result<&'a Arc<T>> {
    let registered: Vec<&str> = factories.names().collect();
    match implementation {
        Some(name) => factories.get(name).ok_or_else(|| {
            Error::Config(format!(
                "registry.implementation '{name}' is not a relational {seam} implementation this \
                 binary contains; registered: [{}]",
                registered.join(", ")
            ))
        }),
        None if registered.len() == 1 => Ok(factories
            .get(registered[0])
            .expect("a name just enumerated from this registry always resolves")),
        None if registered.is_empty() => Err(Error::Config(format!(
            "registry.backend is 'relational' but this binary was built with no driver providing \
             a relational {seam} factory"
        ))),
        None => Err(Error::Config(format!(
            "registry.backend is 'relational' and this binary contains several relational {seam} \
             implementations, so registry.implementation must name one; registered: [{}]",
            registered.join(", ")
        ))),
    }
}

/// Resolves the database URL a `registry.backend: relational` config
/// connects through: `registry.storage` names a `storages` entry (reusing
/// that storage's own `url_env` rather than a second connection-config
/// shape), and this reads the environment variable it names. Shared by
/// [`build_registry_reader`] and, via `tenant::build_tenant_reader`, the
/// tenant reader's own relational dispatch (`#143`) — both readers connect
/// through the SAME `registry.backend`/`.storage` knob, so there is exactly
/// one place that resolves it to a URL.
///
/// `AppConfig::validate` already refuses a `relational` backend with no
/// `storage` set or one referencing an unknown storage — by the time this
/// runs against a config that passed validation, both lookups below always
/// succeed. A config that reaches here unvalidated (only ever a test's own
/// doing) gets a named `Error::Config` instead of a panic either way.
pub(crate) fn resolve_relational_database_url(config: &AppConfig) -> Result<String> {
    let storage_id = config.registry.storage.as_deref().ok_or_else(|| {
        Error::Config(
            "registry.backend is 'relational' but registry.storage is not set".to_string(),
        )
    })?;
    let storage = config
        .storages
        .iter()
        .find(|s| s.id == storage_id)
        .ok_or_else(|| {
            Error::Config(format!(
                "registry.storage '{storage_id}' does not reference a declared storage"
            ))
        })?;
    std::env::var(&storage.url_env).map_err(|_| {
        Error::Config(format!(
            "registry.storage '{storage_id}': environment variable '{}' is not set",
            storage.url_env
        ))
    })
}

/// Page size [`snapshot_from_registry`] requests per `list_catalogs`/
/// `list_collections` call while walking a relational registry to
/// exhaustion (`#42`, third slice) — large enough that a realistically
/// sized registry finishes in a small, fixed number of round trips per
/// tenant/catalog; small enough that no single page holds an unbounded
/// amount of memory. Internal only, not operator-configurable: it bounds a
/// boot/reload-time walk's own memory use, not anything a request-time
/// caller ever sees (contrast `ItemsQuery`'s client-facing page size). See
/// [`snapshot_from_registry_with_page_size`] for the parameterized version
/// tests use to exercise the multi-page walk without seeding a
/// four-figure row count.
const SNAPSHOT_PAGE_SIZE: u32 = 1000;

/// [`snapshot_from_registry`] with `SNAPSHOT_PAGE_SIZE` fixed to whatever a
/// caller passes, rather than the production default — the seam a test uses
/// to exercise the "more than one page" branch of the walk below by shrinking
/// `page_size` instead of seeding thousands of rows. Production code never
/// calls this directly; see `snapshot_from_registry`, which is the one
/// `context::build_router_and_resolver` actually uses.
pub async fn snapshot_from_registry_with_page_size(
    tenants: &[TenantDecl],
    reader: &dyn RegistryReader,
    page_size: u32,
) -> Result<RoutingSnapshot> {
    let mut catalogs = Vec::new();
    for tenant in tenants {
        let mut after = None;
        loop {
            let page = reader
                .list_catalogs(
                    &tenant.id,
                    PageRequest {
                        limit: page_size,
                        after,
                    },
                )
                .await?;
            let next = page.next;
            catalogs.extend(page.items);
            match next {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }
    }

    let mut collections = Vec::new();
    for catalog in &catalogs {
        let mut after = None;
        loop {
            let page = reader
                .list_collections(
                    &catalog.id,
                    PageRequest {
                        limit: page_size,
                        after,
                    },
                )
                .await?;
            let next = page.next;
            collections.extend(page.items);
            match next {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }
    }

    Ok(RoutingSnapshot {
        catalogs,
        collections,
    })
}

/// Walks `reader` to exhaustion and returns the same `(catalogs,
/// collections)` shape `Router::build`'s file-backed path already reads
/// straight off `AppConfig` — the relational half of `#42`'s third slice,
/// consumed by `context::build_router_and_resolver`. Tenant rows come from
/// the separate `TenantReader`: `RegistryReader` only answers "catalogs for
/// this tenant," never "every tenant" (see this module's own doc for why
/// that split is inherent to the trait). `tenants` is the caller's own
/// already-walked authoritative snapshot (`#143`,
/// `tenant::snapshot_tenants`) — for the file-backed default that walk is
/// exactly `config.tenants`, so this is a normalization rather than a
/// behavior change: the same "pass the normalized input in, never reach
/// back into `config` for it" treatment `Router::build_from_snapshot`
/// already gives `catalogs`/`collections`. For each tenant, `list_catalogs`
/// is paged to exhaustion (following each page's `next` cursor until `None`
/// — the same "walk one entry past the limit to detect more" convention
/// `FileRegistryReader::paginate` and `PostgisRegistryReader` both already
/// use, just driven from the caller's side here); for each catalog just
/// collected, `list_collections` is paged the same way. The result is not
/// yet validated — see `config::validate_registry_snapshot`, which
/// `context::build_router_and_resolver` calls before ever handing this to
/// `Router::build_from_snapshot`.
///
/// A consequence of walking tenant-first: a row in `registry_catalogs`
/// scoped to a `tenant_internal_id` not present in `tenants` is simply never
/// fetched — invisible, not a validation failure, since there is no tenant
/// to walk it under in the first place. That is a straightforward
/// orphaned-row situation (the publisher used a `tenant_internal_id` this
/// deployment's tenant snapshot doesn't currently have), not something this
/// walk can or should surface as an error on every boot/reload for every
/// deployment whose registry happens to have historical rows under a
/// retired tenant.
pub async fn snapshot_from_registry(
    tenants: &[TenantDecl],
    reader: &dyn RegistryReader,
) -> Result<RoutingSnapshot> {
    snapshot_from_registry_with_page_size(tenants, reader, SNAPSHOT_PAGE_SIZE).await
}

/// The first `RegistryReader`: two flat in-memory indexes (tenant ->
/// catalog-external-id -> `CatalogDecl`, catalog -> collection-external-id
/// -> `CollectionDecl`) built once from `AppConfig` at boot or reload — no
/// I/O, ever, the same convention `StaticResolver` already follows. Ordered
/// by `BTreeMap` so a keyset range scan (`list_catalogs`/`list_collections`)
/// is a plain `range` call, never a sort-then-slice on every request.
pub struct FileRegistryReader {
    catalogs_by_tenant: HashMap<String, BTreeMap<String, CatalogDecl>>,
    collections_by_catalog: HashMap<String, BTreeMap<String, CollectionDecl>>,
}

impl FileRegistryReader {
    pub fn build(config: &AppConfig) -> Self {
        let mut catalogs_by_tenant: HashMap<String, BTreeMap<String, CatalogDecl>> = HashMap::new();
        for catalog in &config.catalogs {
            catalogs_by_tenant
                .entry(catalog.tenant.clone())
                .or_default()
                .insert(catalog.external_id().to_string(), catalog.clone());
        }

        let mut collections_by_catalog: HashMap<String, BTreeMap<String, CollectionDecl>> =
            HashMap::new();
        for collection in &config.collections {
            collections_by_catalog
                .entry(collection.catalog.clone())
                .or_default()
                .insert(collection.external_id().to_string(), collection.clone());
        }

        Self {
            catalogs_by_tenant,
            collections_by_catalog,
        }
    }
}

/// Shared keyset-pagination walk for `list_catalogs`/`list_collections`
/// (and, via `pub(crate)` visibility, `tenant::FileTenantReader::
/// list_tenants` too — `#143`):
/// takes up to `page.limit` entries starting just after `page.after` (or
/// from the start when `None`), and reports the last entry actually
/// returned as `next` only when the ordering continues beyond that — the
/// same "walk one entry past the limit to detect more" shape a keyset SQL
/// query uses (`LIMIT n+1`), without ever needing an `OFFSET`. `index:
/// None` (an unknown tenant/catalog internal id) is an empty page, not an
/// error — the caller (`Router`/a handler) already treats "no such scope"
/// as its own concern.
pub(crate) fn paginate<T: Clone>(
    index: Option<&BTreeMap<String, T>>,
    page: &PageRequest,
) -> Page<T> {
    let Some(index) = index else {
        return Page {
            items: Vec::new(),
            next: None,
        };
    };
    let limit = page.limit.max(1) as usize;
    let range: Box<dyn Iterator<Item = (&String, &T)>> = match &page.after {
        Some(cursor) => Box::new(index.range((Bound::Excluded(cursor.clone()), Bound::Unbounded))),
        None => Box::new(index.iter()),
    };

    let mut items = Vec::with_capacity(limit.min(index.len()));
    let mut last_external_id: Option<String> = None;
    let mut has_more = false;
    for (external_id, value) in range {
        if items.len() == limit {
            has_more = true;
            break;
        }
        items.push(value.clone());
        last_external_id = Some(external_id.clone());
    }

    Page {
        items,
        next: if has_more { last_external_id } else { None },
    }
}

#[async_trait::async_trait]
impl RegistryReader for FileRegistryReader {
    async fn catalog(
        &self,
        tenant_internal_id: &str,
        catalog_external_id: &str,
    ) -> Result<Option<CatalogDecl>> {
        Ok(self
            .catalogs_by_tenant
            .get(tenant_internal_id)
            .and_then(|by_external| by_external.get(catalog_external_id))
            .cloned())
    }

    async fn collection(
        &self,
        catalog_internal_id: &str,
        collection_external_id: &str,
    ) -> Result<Option<CollectionDecl>> {
        Ok(self
            .collections_by_catalog
            .get(catalog_internal_id)
            .and_then(|by_external| by_external.get(collection_external_id))
            .cloned())
    }

    async fn list_catalogs(
        &self,
        tenant_internal_id: &str,
        page: PageRequest,
    ) -> Result<Page<CatalogDecl>> {
        Ok(paginate(
            self.catalogs_by_tenant.get(tenant_internal_id),
            &page,
        ))
    }

    async fn list_collections(
        &self,
        catalog_internal_id: &str,
        page: PageRequest,
    ) -> Result<Page<CollectionDecl>> {
        Ok(paginate(
            self.collections_by_catalog.get(catalog_internal_id),
            &page,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MULTI_CONFIG: &str = r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: public } ]
catalogs:
  - { id: default, tenant: public }
  - { id: secondary, tenant: public }
collections:
  - id: alpha
    catalog: default
    storage: main
  - id: bravo
    catalog: default
    storage: main
  - id: charlie
    catalog: default
    storage: main
"#;

    fn multi_config() -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(MULTI_CONFIG).unwrap();
        config.validate().unwrap();
        config
    }

    #[tokio::test]
    async fn collection_lookup_is_a_direct_hit_not_a_scan() {
        let reader = FileRegistryReader::build(&multi_config());
        let decl = reader
            .collection("default", "bravo")
            .await
            .unwrap()
            .expect("bravo is declared under the default catalog");
        assert_eq!(decl.id, "bravo");
    }

    #[tokio::test]
    async fn collection_lookup_is_none_for_an_unknown_external_id() {
        let reader = FileRegistryReader::build(&multi_config());
        assert!(reader
            .collection("default", "nonexistent")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn collection_lookup_is_none_for_an_unknown_catalog() {
        let reader = FileRegistryReader::build(&multi_config());
        assert!(reader
            .collection("nonexistent-catalog", "alpha")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn catalog_lookup_is_a_direct_hit() {
        let reader = FileRegistryReader::build(&multi_config());
        let decl = reader
            .catalog("public", "secondary")
            .await
            .unwrap()
            .expect("secondary is declared under the public tenant");
        assert_eq!(decl.id, "secondary");
    }

    #[tokio::test]
    async fn list_collections_returns_everything_in_one_page_when_the_limit_is_not_exceeded() {
        let reader = FileRegistryReader::build(&multi_config());
        let page = reader
            .list_collections(
                "default",
                PageRequest {
                    limit: 10,
                    after: None,
                },
            )
            .await
            .unwrap();
        let ids: Vec<&str> = page.items.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "bravo", "charlie"]);
        assert_eq!(page.next, None);
    }

    #[tokio::test]
    async fn list_collections_paginates_with_a_keyset_cursor_never_an_offset() {
        let reader = FileRegistryReader::build(&multi_config());

        let first = reader
            .list_collections(
                "default",
                PageRequest {
                    limit: 2,
                    after: None,
                },
            )
            .await
            .unwrap();
        let first_ids: Vec<&str> = first.items.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(first_ids, vec!["alpha", "bravo"]);
        assert_eq!(first.next.as_deref(), Some("bravo"));

        let second = reader
            .list_collections(
                "default",
                PageRequest {
                    limit: 2,
                    after: first.next.clone(),
                },
            )
            .await
            .unwrap();
        let second_ids: Vec<&str> = second.items.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(second_ids, vec!["charlie"]);
        assert_eq!(second.next, None);
    }

    #[tokio::test]
    async fn list_collections_is_an_empty_page_for_an_unknown_catalog() {
        let reader = FileRegistryReader::build(&multi_config());
        let page = reader
            .list_collections(
                "nonexistent",
                PageRequest {
                    limit: 10,
                    after: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.items, vec![]);
        assert_eq!(page.next, None);
    }

    #[tokio::test]
    async fn list_collections_for_a_catalog_with_no_collections_is_an_empty_page() {
        let reader = FileRegistryReader::build(&multi_config());
        let page = reader
            .list_collections(
                "secondary",
                PageRequest {
                    limit: 10,
                    after: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.items, vec![]);
        assert_eq!(page.next, None);
    }

    #[tokio::test]
    async fn list_catalogs_paginates_by_external_id() {
        let reader = FileRegistryReader::build(&multi_config());
        let page = reader
            .list_catalogs(
                "public",
                PageRequest {
                    limit: 1,
                    after: None,
                },
            )
            .await
            .unwrap();
        let ids: Vec<&str> = page.items.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["default"]);
        assert_eq!(page.next.as_deref(), Some("default"));
    }

    /// A `limit` of zero would make no forward progress at all — clamped up
    /// to one entry per page instead of looping forever on an empty page.
    #[tokio::test]
    async fn a_zero_limit_still_makes_forward_progress() {
        let reader = FileRegistryReader::build(&multi_config());
        let page = reader
            .list_collections(
                "default",
                PageRequest {
                    limit: 0,
                    after: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.next.as_deref(), Some("alpha"));
    }

    // -- `build_registry_reader` (`#42`, relational registry backend) -------

    /// Records the exact `database_url` `connect` was called with, and
    /// answers with an empty `FileRegistryReader` — enough to prove
    /// dispatch/URL-resolution without a real database or a second
    /// `RegistryReader` implementation living in this crate (which would
    /// need a database client this crate deliberately never depends on).
    struct RecordingFactory {
        name: &'static str,
        seen_url: std::sync::Mutex<Option<String>>,
    }

    impl RecordingFactory {
        fn new() -> Self {
            Self::named("recording")
        }

        fn named(name: &'static str) -> Self {
            Self {
                name,
                seen_url: std::sync::Mutex::new(None),
            }
        }
    }

    /// One registry holding exactly `entries` — the shape `main` builds at
    /// boot, spelled once so a test reads as "this binary contains these
    /// implementations."
    fn factories_of(
        entries: Vec<Arc<dyn RelationalRegistryFactory>>,
    ) -> RelationalRegistryFactories {
        let mut registry = RelationalRegistryFactories::new();
        for entry in entries {
            registry.register(entry);
        }
        registry
    }

    #[async_trait::async_trait]
    impl RelationalRegistryFactory for RecordingFactory {
        fn name(&self) -> &str {
            self.name
        }

        async fn connect(&self, database_url: &str) -> Result<Arc<dyn RegistryReader>> {
            *self.seen_url.lock().unwrap() = Some(database_url.to_string());
            Ok(Arc::new(FileRegistryReader::build(&AppConfig::default())))
        }
    }

    struct AlwaysFailsFactory;

    #[async_trait::async_trait]
    impl RelationalRegistryFactory for AlwaysFailsFactory {
        fn name(&self) -> &str {
            "always-fails"
        }

        async fn connect(&self, _database_url: &str) -> Result<Arc<dyn RegistryReader>> {
            Err(Error::Storage("connection refused".into()))
        }
    }

    fn file_backend_config() -> AppConfig {
        serde_yaml::from_str("storages: []").unwrap()
    }

    fn relational_backend_config(storage_id: &str, url_env: &str) -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: {storage_id}, driver: postgis, url_env: {url_env} }} ]
registry: {{ backend: relational, storage: {storage_id} }}
"#
        ))
        .unwrap();
        config.validate().unwrap();
        config
    }

    #[tokio::test]
    async fn file_backend_never_consults_the_factory() {
        let reader =
            build_registry_reader(&file_backend_config(), &RelationalRegistryFactories::new())
                .await
                .expect("file backend needs no factory");
        // A `FileRegistryReader` built from an empty config has nothing
        // indexed — enough to prove this is the file reader, not something
        // that reached the relational branch.
        assert!(reader
            .list_catalogs("public", PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }

    #[tokio::test]
    async fn relational_backend_resolves_the_referenced_storages_url_env_and_connects() {
        let env_var = "TELLURION_CORE_REGISTRY_TEST_URL_A";
        // Safety: this test's env var name is unique to it; no other test in
        // this crate reads or writes it concurrently.
        unsafe {
            std::env::set_var(env_var, "postgres://example/registry-test");
        }
        let config = relational_backend_config("main", env_var);
        let factory = Arc::new(RecordingFactory::new());

        build_registry_reader(&config, &factories_of(vec![factory.clone()]))
            .await
            .expect("connects via the recording factory");

        assert_eq!(
            factory.seen_url.lock().unwrap().as_deref(),
            Some("postgres://example/registry-test")
        );
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    #[tokio::test]
    async fn relational_backend_with_no_factory_is_a_config_error() {
        let env_var = "TELLURION_CORE_REGISTRY_TEST_URL_B";
        unsafe {
            std::env::set_var(env_var, "postgres://example/registry-test");
        }
        let config = relational_backend_config("main", env_var);

        let result = build_registry_reader(&config, &RelationalRegistryFactories::new()).await;
        assert!(
            matches!(result, Err(Error::Config(_))),
            "no factory means no way to build a relational reader"
        );
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    #[tokio::test]
    async fn relational_backend_with_an_unset_url_env_is_a_config_error() {
        let env_var = "TELLURION_CORE_REGISTRY_TEST_URL_UNSET";
        unsafe {
            std::env::remove_var(env_var);
        }
        let config = relational_backend_config("main", env_var);
        let factory = Arc::new(RecordingFactory::new());

        let result = build_registry_reader(&config, &factories_of(vec![factory.clone()])).await;
        assert!(
            matches!(result, Err(Error::Config(_))),
            "an unset url_env must fail before ever calling connect"
        );
        assert!(
            factory.seen_url.lock().unwrap().is_none(),
            "connect must never be called when the URL can't be resolved"
        );
    }

    #[tokio::test]
    async fn a_connect_failure_propagates_as_an_error() {
        let env_var = "TELLURION_CORE_REGISTRY_TEST_URL_C";
        unsafe {
            std::env::set_var(env_var, "postgres://example/registry-test");
        }
        let config = relational_backend_config("main", env_var);

        let result =
            build_registry_reader(&config, &factories_of(vec![Arc::new(AlwaysFailsFactory)])).await;
        assert!(
            matches!(result, Err(Error::Storage(_))),
            "a factory that fails to connect must surface as an error"
        );
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    // -- named registry backends (`#162`) ------------------------------------

    /// `relational_backend_config`, plus an explicit `registry.implementation`.
    fn relational_backend_config_naming(
        storage_id: &str,
        url_env: &str,
        implementation: &str,
    ) -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: {storage_id}, driver: postgis, url_env: {url_env} }} ]
registry: {{ backend: relational, storage: {storage_id}, implementation: {implementation} }}
"#
        ))
        .unwrap();
        config.validate().unwrap();
        config
    }

    /// The property `#162` exists for: with more than one relational
    /// implementation registered, the config *name* decides which one
    /// connects — not registration order, not alphabetical order, not "the
    /// only one there happened to be."
    #[tokio::test]
    async fn a_named_implementation_selects_that_factory_and_no_other() {
        let env_var = "TELLURION_CORE_REGISTRY_TEST_URL_NAMED";
        unsafe {
            std::env::set_var(env_var, "postgres://example/named");
        }
        // "alpha" sorts first, so a selection that quietly fell back to the
        // registry's own deterministic order would pick it instead.
        let alpha = Arc::new(RecordingFactory::named("alpha"));
        let zulu = Arc::new(RecordingFactory::named("zulu"));
        let config = relational_backend_config_naming("main", env_var, "zulu");

        build_registry_reader(&config, &factories_of(vec![alpha.clone(), zulu.clone()]))
            .await
            .expect("the named implementation connects");

        assert_eq!(
            zulu.seen_url.lock().unwrap().as_deref(),
            Some("postgres://example/named"),
            "the named implementation is the one that connected"
        );
        assert!(
            alpha.seen_url.lock().unwrap().is_none(),
            "no other registered implementation may be consulted"
        );
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    /// A name nothing registered is refused by name and told what IS
    /// registered — never silently downgraded to the file backend, and never
    /// resolved to some other entry. Indistinguishable, on purpose, from a
    /// name whose driver crate was compiled out.
    #[tokio::test]
    async fn an_unregistered_implementation_name_is_refused_by_name() {
        let env_var = "TELLURION_CORE_REGISTRY_TEST_URL_UNKNOWN";
        unsafe {
            std::env::set_var(env_var, "postgres://example/unknown");
        }
        let config = relational_backend_config_naming("main", env_var, "sqlite");
        let registered = factories_of(vec![
            Arc::new(RecordingFactory::named("alpha")),
            Arc::new(RecordingFactory::named("zulu")),
        ]);

        match build_registry_reader(&config, &registered).await {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("sqlite"),
                    "the refusal must name the unknown implementation: {message}"
                );
                assert!(
                    message.contains("registry.implementation"),
                    "the refusal must name the config key: {message}"
                );
                assert!(
                    message.contains("alpha") && message.contains("zulu"),
                    "the refusal must list what IS registered: {message}"
                );
            }
            other => panic!("expected a named Error::Config, got ok={}", other.is_ok()),
        }
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    /// Registration order must never leak into the refusal's wording — the
    /// listed names are the registry's own deterministic order, which is what
    /// makes the message (and the boot log built from the same iterator)
    /// stable run to run.
    #[tokio::test]
    async fn the_refusal_lists_registered_names_alphabetically_whatever_the_registration_order() {
        let env_var = "TELLURION_CORE_REGISTRY_TEST_URL_ORDER";
        unsafe {
            std::env::set_var(env_var, "postgres://example/order");
        }
        let config = relational_backend_config_naming("main", env_var, "nope");
        let reversed = factories_of(vec![
            Arc::new(RecordingFactory::named("zulu")),
            Arc::new(RecordingFactory::named("mike")),
            Arc::new(RecordingFactory::named("alpha")),
        ]);

        match build_registry_reader(&config, &reversed).await {
            Err(Error::Config(message)) => assert!(
                message.contains("[alpha, mike, zulu]"),
                "registered names must be listed alphabetically: {message}"
            ),
            other => panic!("expected a named Error::Config, got ok={}", other.is_ok()),
        }
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    /// The backwards-compatibility case, pinned: a config that never heard of
    /// `registry.implementation` — every config written before `#162` — still
    /// resolves to the sole compiled-in relational implementation, with no
    /// name written anywhere. This is what makes the new field additive
    /// rather than required.
    #[tokio::test]
    async fn an_unset_implementation_selects_the_sole_registered_one() {
        let env_var = "TELLURION_CORE_REGISTRY_TEST_URL_SOLE";
        unsafe {
            std::env::set_var(env_var, "postgres://example/sole");
        }
        let config = relational_backend_config("main", env_var);
        assert!(
            config.registry.implementation.is_none(),
            "a pre-`#162` config names no implementation"
        );
        let only = Arc::new(RecordingFactory::named("postgis"));

        build_registry_reader(&config, &factories_of(vec![only.clone()]))
            .await
            .expect("the sole registered implementation is selected without being named");

        assert_eq!(
            only.seen_url.lock().unwrap().as_deref(),
            Some("postgres://example/sole")
        );
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    /// The moment a second relational implementation exists, "the sole one"
    /// stops being an answer — so it becomes a refusal that names the choice,
    /// never a silent pick. The case `#162` was filed to make expressible.
    #[tokio::test]
    async fn an_unset_implementation_with_several_registered_is_refused_not_guessed() {
        let env_var = "TELLURION_CORE_REGISTRY_TEST_URL_AMBIGUOUS";
        unsafe {
            std::env::set_var(env_var, "postgres://example/ambiguous");
        }
        let config = relational_backend_config("main", env_var);
        let alpha = Arc::new(RecordingFactory::named("alpha"));
        let zulu = Arc::new(RecordingFactory::named("zulu"));

        match build_registry_reader(&config, &factories_of(vec![alpha.clone(), zulu.clone()])).await
        {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("registry.implementation"),
                    "the refusal must name the key that resolves the ambiguity: {message}"
                );
                assert!(
                    message.contains("alpha") && message.contains("zulu"),
                    "the refusal must list the candidates: {message}"
                );
            }
            other => panic!("expected a named Error::Config, got ok={}", other.is_ok()),
        }
        assert!(
            alpha.seen_url.lock().unwrap().is_none() && zulu.seen_url.lock().unwrap().is_none(),
            "an ambiguous selection must connect to nothing at all"
        );
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    /// A binary with no relational driver crate keeps failing with the exact
    /// pre-`#162` sentence, whether or not the config names an
    /// implementation — the "compiled out" message an operator may already
    /// have runbooks and alerts matching on.
    #[tokio::test]
    async fn an_empty_registry_keeps_the_pre_162_compiled_out_wording() {
        let env_var = "TELLURION_CORE_REGISTRY_TEST_URL_EMPTY";
        unsafe {
            std::env::set_var(env_var, "postgres://example/empty");
        }
        let config = relational_backend_config("main", env_var);

        match build_registry_reader(&config, &RelationalRegistryFactories::new()).await {
            Err(Error::Config(message)) => assert_eq!(
                message,
                "registry.backend is 'relational' but this binary was built with no driver \
                 providing a relational registry factory"
            ),
            other => panic!("expected a named Error::Config, got ok={}", other.is_ok()),
        }
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    /// `file` stays the direct built-in backend: it has no factory to name,
    /// so a leftover `implementation` cannot change what it builds and cannot
    /// make it fail. An unconfigured deployment reaches none of `#162`'s new
    /// code at all.
    #[tokio::test]
    async fn the_file_backend_ignores_implementation_and_never_touches_the_registry() {
        let config: AppConfig = serde_yaml::from_str(
            "storages: []\nregistry: { backend: file, implementation: sqlite }\n",
        )
        .unwrap();
        config.validate().unwrap();

        let reader = build_registry_reader(&config, &RelationalRegistryFactories::new())
            .await
            .expect("the file backend never consults the relational registry");
        assert!(reader
            .list_catalogs("public", PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }

    /// `RelationalRegistryFactories` keys entries by each factory's own
    /// declared name, so a registration can never disagree with the name an
    /// operator writes in config, and enumeration is alphabetical — the order
    /// `main`'s boot log line depends on.
    #[test]
    fn registration_keys_on_the_factorys_own_name_and_enumerates_deterministically() {
        let registry = factories_of(vec![
            Arc::new(RecordingFactory::named("zulu")),
            Arc::new(RecordingFactory::named("alpha")),
            Arc::new(RecordingFactory::named("mike")),
        ]);

        assert_eq!(
            registry.names().collect::<Vec<_>>(),
            vec!["alpha", "mike", "zulu"]
        );
        assert_eq!(registry.len(), 3);
        assert!(!registry.is_empty());
        assert_eq!(registry.get("mike").map(|f| f.name()), Some("mike"));
        assert!(registry.get("sqlite").is_none());
        assert!(RelationalRegistryFactories::new().is_empty());
    }

    // -- `snapshot_from_registry` (`#42`, third slice) -----------------------

    /// A "database" config with two tenants, several catalogs under one of
    /// them, and several collections under one of those catalogs — indexed
    /// into a `FileRegistryReader` standing in for a relational registry's
    /// rows (it implements `RegistryReader`'s keyset pagination the same way
    /// `PostgisRegistryReader` does — see `paginate`'s own doc), so the walk
    /// below exercises real `Page`/cursor semantics, not a hand-rolled mock.
    fn multi_tenant_db_config() -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-a }, { id: tenant-b } ]
catalogs:
  - { id: a-cat-0, external_id: a-cat-ext-0, tenant: tenant-a }
  - { id: a-cat-1, external_id: a-cat-ext-1, tenant: tenant-a }
  - { id: a-cat-2, external_id: a-cat-ext-2, tenant: tenant-a }
  - { id: b-cat-0, external_id: b-cat-ext-0, tenant: tenant-b }
collections:
  - { id: a0-col-0, external_id: a0-col-ext-0, catalog: a-cat-0, storage: main }
  - { id: a0-col-1, external_id: a0-col-ext-1, catalog: a-cat-0, storage: main }
  - { id: a0-col-2, external_id: a0-col-ext-2, catalog: a-cat-0, storage: main }
  - { id: a1-col-0, external_id: a1-col-ext-0, catalog: a-cat-1, storage: main }
  - { id: b0-col-0, external_id: b0-col-ext-0, catalog: b-cat-0, storage: main }
"#,
        )
        .unwrap();
        config.validate().unwrap();
        config
    }

    /// The "operator" config `snapshot_from_registry` actually walks — same
    /// tenants as [`multi_tenant_db_config`], no catalogs/collections of its
    /// own (irrelevant to a relational-backend walk either way).
    fn multi_tenant_operator_config() -> AppConfig {
        serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-a }, { id: tenant-b } ]
"#,
        )
        .unwrap()
    }

    /// The pagination requirement (`#42`, third slice): a `page_size` small
    /// enough that every tenant's catalog listing (3, then 1) and every
    /// catalog's collection listing (3, then 1, then 1) crosses more than
    /// one page at least once — proven by shrinking the internal page size
    /// via `snapshot_from_registry_with_page_size` rather than seeding a
    /// four-figure row count to force it out of the production default.
    #[tokio::test]
    async fn snapshot_walk_crosses_multiple_pages_and_still_collects_everything_exactly_once() {
        let db_config = multi_tenant_db_config();
        let reader = FileRegistryReader::build(&db_config);
        let operator_config = multi_tenant_operator_config();

        let snapshot = snapshot_from_registry_with_page_size(&operator_config.tenants, &reader, 2)
            .await
            .expect("the walk succeeds across multiple pages per tenant/catalog");

        let mut catalog_ids: Vec<&str> = snapshot.catalogs.iter().map(|c| c.id.as_str()).collect();
        catalog_ids.sort_unstable();
        assert_eq!(
            catalog_ids,
            vec!["a-cat-0", "a-cat-1", "a-cat-2", "b-cat-0"]
        );

        let mut collection_ids: Vec<&str> =
            snapshot.collections.iter().map(|c| c.id.as_str()).collect();
        collection_ids.sort_unstable();
        assert_eq!(
            collection_ids,
            vec!["a0-col-0", "a0-col-1", "a0-col-2", "a1-col-0", "b0-col-0"]
        );
    }

    /// The production entry point (`SNAPSHOT_PAGE_SIZE`, no test-only
    /// shrinking) walks the same result for a registry small enough to fit
    /// in one page per tenant/catalog — the common case every deployment
    /// below the internal page size actually hits.
    #[tokio::test]
    async fn snapshot_from_registry_uses_the_production_page_size_by_default() {
        let db_config = multi_tenant_db_config();
        let reader = FileRegistryReader::build(&db_config);
        let operator_config = multi_tenant_operator_config();

        let snapshot = snapshot_from_registry(&operator_config.tenants, &reader)
            .await
            .unwrap();
        assert_eq!(snapshot.catalogs.len(), 4);
        assert_eq!(snapshot.collections.len(), 5);
    }

    /// A tenant the operator config declares but the registry has nothing
    /// under is an empty slice, not an error — same "no such scope is an
    /// empty page, not a failure" convention `FileRegistryReader::paginate`
    /// already documents for a single page.
    #[tokio::test]
    async fn snapshot_from_registry_is_empty_for_a_tenant_with_nothing_published() {
        let db_config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-with-nothing-published } ]
"#,
        )
        .unwrap();
        let reader = FileRegistryReader::build(&db_config);

        let snapshot = snapshot_from_registry(&db_config.tenants, &reader)
            .await
            .unwrap();
        assert!(snapshot.catalogs.is_empty());
        assert!(snapshot.collections.is_empty());
    }

    /// A catalog scoped to a tenant the operator config never declares is
    /// simply never walked — invisible, not surfaced at all (see
    /// `snapshot_from_registry`'s own doc for why this is intentional, not
    /// a gap: there is no tenant to walk it under).
    #[tokio::test]
    async fn snapshot_from_registry_never_walks_a_catalog_under_an_undeclared_tenant() {
        let db_config = multi_tenant_db_config();
        // This operator config only knows `tenant-a` — `tenant-b`'s
        // `b-cat-0` (and its collection) exist in the registry but must
        // never appear in the walk's result.
        let operator_config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: postgis, url_env: DATABASE_URL } ]
tenants: [ { id: tenant-a } ]
"#,
        )
        .unwrap();
        let reader = FileRegistryReader::build(&db_config);

        let snapshot = snapshot_from_registry(&operator_config.tenants, &reader)
            .await
            .unwrap();
        assert!(
            snapshot.catalogs.iter().all(|c| c.tenant == "tenant-a"),
            "no tenant-b catalog should have been walked"
        );
        assert!(!snapshot.catalogs.iter().any(|c| c.id == "b-cat-0"));
        assert!(!snapshot.collections.iter().any(|c| c.id == "b0-col-0"));
    }
}
