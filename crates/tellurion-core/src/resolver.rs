//! The seam between a request's URL (external ids only) and everything
//! below it (internal ids only) — `#39`. A `Resolver` turns
//! `(tenant_external_id, catalog_external_id, collection_external_id)` into
//! the internal ids `Router` and the tile cache key on, and turns internal
//! ids back into external ones for building response links. It is the ONLY
//! place in this codebase that ever looks at an external id below the HTTP
//! boundary; a protocol handler resolves once, at the top of the handler,
//! then works entirely in internal ids.
//!
//! The trait is async so the registry epic (`#42`) can later back it with a
//! real store (a lookup that hits a database or cache) without touching a
//! single route or handler — `StaticResolver`, the only implementation
//! today, answers every call from an in-memory index built once from
//! `AppConfig` and never actually awaits anything.
//!
//! `AppContext` holds a `Resolver` behind the same atomically-swapped state
//! as `Router`/`AppConfig` (see `context.rs`), so a config reload replaces
//! both together — a request that resolves against the old resolver never
//! reaches a `Router` built from the new config, or vice versa.

use std::collections::HashMap;

use crate::config::{AppConfig, CatalogDecl, CollectionDecl, TenantDecl};
use crate::error::{Error, Result};

#[async_trait::async_trait]
pub trait Resolver: Send + Sync {
    /// `tenant_external_id` -> the tenant's internal id. `Err(NotFound)` for
    /// an unknown external id — indistinguishable, deliberately, from any
    /// other unresolvable path segment.
    async fn resolve_tenant(&self, tenant_external_id: &str) -> Result<String>;

    /// `catalog_external_id` -> the catalog's internal id, scoped to
    /// `tenant_internal_id` (catalog external ids are unique per tenant, not
    /// globally — see `config::AppConfig::validate`).
    async fn resolve_catalog(
        &self,
        tenant_internal_id: &str,
        catalog_external_id: &str,
    ) -> Result<String>;

    /// `collection_external_id` -> the collection's internal id, scoped to
    /// `catalog_internal_id`.
    async fn resolve_collection(
        &self,
        catalog_internal_id: &str,
        collection_external_id: &str,
    ) -> Result<String>;

    /// Reverse of [`resolve_tenant`](Self::resolve_tenant) — used to build
    /// links (e.g. the `/{tenant}/` directory doc) when only the internal id
    /// is in hand. Most handlers never need this: the tenant/catalog/
    /// collection external id is already the path segment the client typed,
    /// so it's cheaper to echo that back than to reverse-look it up.
    fn tenant_external_id(&self, tenant_internal_id: &str) -> Option<&str>;

    fn catalog_external_id(&self, catalog_internal_id: &str) -> Option<&str>;

    fn collection_external_id(&self, collection_internal_id: &str) -> Option<&str>;

    /// Every catalog owned by `tenant_internal_id`, as `(internal_id,
    /// external_id)` pairs sorted by external id — a synchronous, in-memory
    /// view of this `Resolver`'s own snapshot. The tenant directory doc
    /// (`landing::tenant_directory`) no longer reads this: it lists a
    /// tenant's catalogs through the registry seam instead
    /// (`RegistryReader::list_catalogs`, `#59`), which is both paginated and
    /// — under the relational backend — never stale relative to the last
    /// full registry walk the way this index inherently can be. Kept as
    /// public `Resolver` API regardless: still the cheapest way to answer
    /// "which catalogs does this tenant currently route to" with no I/O.
    fn catalogs_for_tenant(&self, tenant_internal_id: &str) -> Vec<(&str, &str)>;

    /// How many catalogs this `Resolver` actually indexed, across every
    /// tenant (`#42`, `#59`) — the snapshot's own count, same rationale as
    /// `Router::collection_count`: identical to `config.catalogs.len()` for
    /// the file-backed default, but the only correct source under
    /// `registry.backend: relational`, where `AppConfig.catalogs` is always
    /// empty by the double-source rule. `Router` and `Resolver` are always
    /// built from the same snapshot (`context::build_router_and_resolver`'s
    /// own doc), so this and `collection_count` report the same registry
    /// walk's two counts.
    fn catalog_count(&self) -> usize;
}

struct CatalogEntry {
    external_id: String,
    tenant_internal_id: String,
}

/// The only `Resolver` today: a flat in-memory index built once from
/// `AppConfig` at boot (or on reload) — no I/O, ever. `build` assumes
/// `config` already passed `AppConfig::validate` (external ids unique at
/// their scope, every reference resolvable), the same precondition `Router::
/// build` assumes.
pub struct StaticResolver {
    tenants_by_external: HashMap<String, String>,
    tenants_by_internal: HashMap<String, String>,
    catalogs_by_external: HashMap<(String, String), String>,
    catalogs_by_internal: HashMap<String, CatalogEntry>,
    collections_by_external: HashMap<(String, String), String>,
    collections_by_internal: HashMap<String, String>,
}

impl StaticResolver {
    /// Indexes `config.tenants` alongside `config.catalogs`/`.collections`
    /// themselves — the file-backed default, and the only behavior this
    /// ever had before `#42`'s relational registry backend existed. Thin
    /// wrapper over [`build_from_snapshot`](Self::build_from_snapshot); see
    /// `context::build_router_and_resolver` for the entry point that
    /// dispatches on `config.registry.backend` instead of always reading
    /// `config.tenants`/`.catalogs`/`.collections` directly.
    pub fn build(config: &AppConfig) -> Self {
        Self::build_from_snapshot(&config.tenants, &config.catalogs, &config.collections)
    }

    /// Indexes `tenants` alongside `catalogs`/`collections` — the
    /// normalized routing input (`#42`, third slice; `#143` for `tenants`
    /// itself) shared with `Router::build_from_snapshot`. Each is either
    /// straight off `AppConfig` for the file-backed default
    /// ([`build`](Self::build)) or a walked snapshot for the relational
    /// backend (`context::build_router_and_resolver`): `catalogs`/
    /// `collections` from a [`RoutingSnapshot`](crate::config::RoutingSnapshot)
    /// walked via a `RegistryReader`, `tenants` from
    /// `tenant::snapshot_tenants` walked via a `TenantReader`. This
    /// function never reads `AppConfig` itself — every caller resolves its
    /// own three inputs first, so there is exactly one place (the caller)
    /// that decides which source produced them. A collection published to a
    /// relational registry needs to resolve its external id here just as
    /// much as it needs to route through `Router` — a `Router` with no
    /// matching `Resolver` entry is unreachable from any URL regardless of
    /// what it can serve once resolved, so both are always built from the
    /// SAME snapshot by the one caller that fetches it (never two
    /// independent registry walks, which could otherwise observe two
    /// different snapshots of a registry mutated in between).
    pub fn build_from_snapshot(
        tenants: &[TenantDecl],
        catalogs: &[CatalogDecl],
        collections: &[CollectionDecl],
    ) -> Self {
        let mut tenants_by_external = HashMap::with_capacity(tenants.len());
        let mut tenants_by_internal = HashMap::with_capacity(tenants.len());
        for tenant in tenants {
            tenants_by_external.insert(tenant.external_id().to_string(), tenant.id.clone());
            tenants_by_internal.insert(tenant.id.clone(), tenant.external_id().to_string());
        }

        let mut catalogs_by_external = HashMap::with_capacity(catalogs.len());
        let mut catalogs_by_internal = HashMap::with_capacity(catalogs.len());
        for catalog in catalogs {
            catalogs_by_external.insert(
                (catalog.tenant.clone(), catalog.external_id().to_string()),
                catalog.id.clone(),
            );
            catalogs_by_internal.insert(
                catalog.id.clone(),
                CatalogEntry {
                    external_id: catalog.external_id().to_string(),
                    tenant_internal_id: catalog.tenant.clone(),
                },
            );
        }

        let mut collections_by_external = HashMap::with_capacity(collections.len());
        let mut collections_by_internal = HashMap::with_capacity(collections.len());
        for collection in collections {
            collections_by_external.insert(
                (
                    collection.catalog.clone(),
                    collection.external_id().to_string(),
                ),
                collection.id.clone(),
            );
            collections_by_internal
                .insert(collection.id.clone(), collection.external_id().to_string());
        }

        Self {
            tenants_by_external,
            tenants_by_internal,
            catalogs_by_external,
            catalogs_by_internal,
            collections_by_external,
            collections_by_internal,
        }
    }
}

#[async_trait::async_trait]
impl Resolver for StaticResolver {
    async fn resolve_tenant(&self, tenant_external_id: &str) -> Result<String> {
        self.tenants_by_external
            .get(tenant_external_id)
            .cloned()
            .ok_or(Error::NotFound)
    }

    async fn resolve_catalog(
        &self,
        tenant_internal_id: &str,
        catalog_external_id: &str,
    ) -> Result<String> {
        self.catalogs_by_external
            .get(&(
                tenant_internal_id.to_string(),
                catalog_external_id.to_string(),
            ))
            .cloned()
            .ok_or(Error::NotFound)
    }

    async fn resolve_collection(
        &self,
        catalog_internal_id: &str,
        collection_external_id: &str,
    ) -> Result<String> {
        self.collections_by_external
            .get(&(
                catalog_internal_id.to_string(),
                collection_external_id.to_string(),
            ))
            .cloned()
            .ok_or(Error::NotFound)
    }

    fn tenant_external_id(&self, tenant_internal_id: &str) -> Option<&str> {
        self.tenants_by_internal
            .get(tenant_internal_id)
            .map(String::as_str)
    }

    fn catalog_external_id(&self, catalog_internal_id: &str) -> Option<&str> {
        self.catalogs_by_internal
            .get(catalog_internal_id)
            .map(|entry| entry.external_id.as_str())
    }

    fn collection_external_id(&self, collection_internal_id: &str) -> Option<&str> {
        self.collections_by_internal
            .get(collection_internal_id)
            .map(String::as_str)
    }

    fn catalogs_for_tenant(&self, tenant_internal_id: &str) -> Vec<(&str, &str)> {
        let mut catalogs: Vec<(&str, &str)> = self
            .catalogs_by_internal
            .iter()
            .filter(|(_, entry)| entry.tenant_internal_id == tenant_internal_id)
            .map(|(internal_id, entry)| (internal_id.as_str(), entry.external_id.as_str()))
            .collect();
        catalogs.sort_by_key(|(_, external_id)| *external_id);
        catalogs
    }

    fn catalog_count(&self) -> usize {
        self.catalogs_by_internal.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_two_tenants_sharing_names() -> AppConfig {
        serde_yaml::from_str(
            r#"
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
    storage: main
  - id: collection-b-internal
    external_id: demo
    catalog: catalog-b-internal
    storage: main
"#,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn resolves_tenant_catalog_collection_chain_to_internal_ids() {
        let config = config_with_two_tenants_sharing_names();
        let resolver = StaticResolver::build(&config);

        let tenant = resolver.resolve_tenant("acme").await.unwrap();
        assert_eq!(tenant, "tenant-a-internal");
        let catalog = resolver.resolve_catalog(&tenant, "default").await.unwrap();
        assert_eq!(catalog, "catalog-a-internal");
        let collection = resolver.resolve_collection(&catalog, "demo").await.unwrap();
        assert_eq!(collection, "collection-a-internal");
    }

    /// The acceptance-critical case (`#39`): two tenants that both declare a
    /// catalog external id `default` and a collection external id `demo`
    /// resolve to their OWN, distinct internal ids — no collision.
    #[tokio::test]
    async fn identical_external_ids_under_different_tenants_resolve_to_distinct_internal_ids() {
        let config = config_with_two_tenants_sharing_names();
        let resolver = StaticResolver::build(&config);

        let tenant_a = resolver.resolve_tenant("acme").await.unwrap();
        let catalog_a = resolver
            .resolve_catalog(&tenant_a, "default")
            .await
            .unwrap();
        let collection_a = resolver
            .resolve_collection(&catalog_a, "demo")
            .await
            .unwrap();

        let tenant_b = resolver.resolve_tenant("globex").await.unwrap();
        let catalog_b = resolver
            .resolve_catalog(&tenant_b, "default")
            .await
            .unwrap();
        let collection_b = resolver
            .resolve_collection(&catalog_b, "demo")
            .await
            .unwrap();

        assert_ne!(tenant_a, tenant_b);
        assert_ne!(catalog_a, catalog_b);
        assert_ne!(collection_a, collection_b);
    }

    #[tokio::test]
    async fn unknown_tenant_external_id_is_not_found() {
        let config = config_with_two_tenants_sharing_names();
        let resolver = StaticResolver::build(&config);
        assert!(matches!(
            resolver.resolve_tenant("nonexistent").await,
            Err(Error::NotFound)
        ));
    }

    #[tokio::test]
    async fn catalog_external_id_from_the_wrong_tenant_is_not_found() {
        let config = config_with_two_tenants_sharing_names();
        let resolver = StaticResolver::build(&config);
        // "default" exists, but not scoped under a tenant that never
        // resolved to "tenant-a-internal" or "tenant-b-internal".
        assert!(matches!(
            resolver
                .resolve_catalog("nonexistent-tenant", "default")
                .await,
            Err(Error::NotFound)
        ));
    }

    #[tokio::test]
    async fn reverse_maps_answer_the_external_id_for_a_known_internal_id() {
        let config = config_with_two_tenants_sharing_names();
        let resolver = StaticResolver::build(&config);

        assert_eq!(
            resolver.tenant_external_id("tenant-a-internal"),
            Some("acme")
        );
        assert_eq!(
            resolver.catalog_external_id("catalog-a-internal"),
            Some("default")
        );
        assert_eq!(
            resolver.collection_external_id("collection-a-internal"),
            Some("demo")
        );
    }

    #[test]
    fn reverse_maps_are_none_for_an_unknown_internal_id() {
        let config = config_with_two_tenants_sharing_names();
        let resolver = StaticResolver::build(&config);
        assert_eq!(resolver.tenant_external_id("nope"), None);
        assert_eq!(resolver.catalog_external_id("nope"), None);
        assert_eq!(resolver.collection_external_id("nope"), None);
    }

    #[test]
    fn catalogs_for_tenant_lists_only_that_tenants_catalogs() {
        let config = config_with_two_tenants_sharing_names();
        let resolver = StaticResolver::build(&config);

        let catalogs = resolver.catalogs_for_tenant("tenant-a-internal");
        assert_eq!(catalogs, vec![("catalog-a-internal", "default")]);
    }

    /// `#59`: `catalog_count` counts every catalog across every tenant, not
    /// just one tenant's own slice — two catalogs total here, one per
    /// tenant, even though `catalogs_for_tenant` only ever answers for one
    /// tenant at a time.
    #[test]
    fn catalog_count_counts_every_catalog_across_every_tenant() {
        let config = config_with_two_tenants_sharing_names();
        let resolver = StaticResolver::build(&config);
        assert_eq!(resolver.catalog_count(), 2);
    }

    /// A tenant with no external_id declared falls back to its internal id
    /// as the external one (`TenantDecl::external_id`'s own default), and
    /// the resolver honors that default the same as an explicit value.
    #[tokio::test]
    async fn tenant_with_no_explicit_external_id_resolves_by_its_internal_id() {
        let config: AppConfig = serde_yaml::from_str(
            r#"
tenants: [ { id: public } ]
catalogs: [ { id: default, tenant: public } ]
collections:
  - id: demo
    catalog: default
    storage: main
"#,
        )
        .unwrap();
        let resolver = StaticResolver::build(&config);
        assert_eq!(resolver.resolve_tenant("public").await.unwrap(), "public");
    }

    // -- `build_from_snapshot` (`#42`, third slice) ---------------------------

    /// `StaticResolver::build_from_snapshot`, fed `tenants`/`catalogs`/
    /// `collections` slices that are NOT `config.tenants`/`.catalogs`/
    /// `.collections` (which stay empty here) still resolves the external id
    /// chain — proving the given slices, not `config`'s own fields, are the
    /// real source.
    #[tokio::test]
    async fn build_from_snapshot_resolves_using_the_given_snapshot_not_configs_own_fields() {
        let config: AppConfig = serde_yaml::from_str("storages: []").unwrap();
        assert!(
            config.tenants.is_empty()
                && config.catalogs.is_empty()
                && config.collections.is_empty()
        );

        let tenants = vec![TenantDecl {
            id: "public".to_string(),
            external_id: None,
            settings: crate::config::SettingsDecl::default(),
        }];
        let catalogs = vec![CatalogDecl {
            id: "default".to_string(),
            external_id: None,
            tenant: "public".to_string(),
            settings: crate::config::SettingsDecl::default(),
            visibility: crate::config::VisibilityDecl::default(),
        }];
        let collections = vec![serde_yaml::from_str::<CollectionDecl>(
            "id: demo\nexternal_id: demo-ext\ncatalog: default\nstorage: main\n",
        )
        .unwrap()];

        let resolver = StaticResolver::build_from_snapshot(&tenants, &catalogs, &collections);

        let tenant = resolver.resolve_tenant("public").await.unwrap();
        assert_eq!(tenant, "public");
        let catalog = resolver.resolve_catalog(&tenant, "default").await.unwrap();
        assert_eq!(catalog, "default");
        let collection = resolver
            .resolve_collection(&catalog, "demo-ext")
            .await
            .unwrap();
        assert_eq!(collection, "demo");
    }

    /// `#59`: `catalog_count` reports the snapshot's own count — one here —
    /// even though `config.catalogs` (the relational-backend double-source
    /// rule's empty section) is zero. This is the exact gap the reload-log
    /// fix relies on: `config.catalogs.len()` alone would have reported `0`
    /// for a relational-backend reload no matter how many catalogs the
    /// registry actually indexed.
    #[test]
    fn catalog_count_reflects_the_snapshot_not_configs_own_catalogs_field() {
        let tenants = vec![TenantDecl {
            id: "public".to_string(),
            external_id: None,
            settings: crate::config::SettingsDecl::default(),
        }];
        let catalogs = vec![CatalogDecl {
            id: "default".to_string(),
            external_id: None,
            tenant: "public".to_string(),
            settings: crate::config::SettingsDecl::default(),
            visibility: crate::config::VisibilityDecl::default(),
        }];
        let resolver = StaticResolver::build_from_snapshot(&tenants, &catalogs, &[]);
        assert_eq!(resolver.catalog_count(), 1);
    }

    /// `StaticResolver::build`, `build_from_snapshot`'s thin wrapper for the
    /// file-backed default, resolves identically to calling
    /// `build_from_snapshot` directly with `config.catalogs`/`.collections` —
    /// the "YAML path stays byte-for-byte identical" guarantee (`#42`, third
    /// slice).
    #[tokio::test]
    async fn build_is_equivalent_to_build_from_snapshot_with_configs_own_declarations() {
        let config = config_with_two_tenants_sharing_names();

        let via_build = StaticResolver::build(&config);
        let via_snapshot = StaticResolver::build_from_snapshot(
            &config.tenants,
            &config.catalogs,
            &config.collections,
        );

        let tenant_a = via_build.resolve_tenant("acme").await.unwrap();
        let catalog_a = via_build
            .resolve_catalog(&tenant_a, "default")
            .await
            .unwrap();
        let collection_a = via_build
            .resolve_collection(&catalog_a, "demo")
            .await
            .unwrap();

        assert_eq!(via_snapshot.resolve_tenant("acme").await.unwrap(), tenant_a);
        assert_eq!(
            via_snapshot
                .resolve_catalog(&tenant_a, "default")
                .await
                .unwrap(),
            catalog_a
        );
        assert_eq!(
            via_snapshot
                .resolve_collection(&catalog_a, "demo")
                .await
                .unwrap(),
            collection_a
        );
    }
}
