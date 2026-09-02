//! `LinkContributor` — the cross-protocol link seam (`#186`). Protocol
//! crates are deliberately decoupled (no cross-imports, ever — see this
//! crate's own top-level doc), which also means no protocol crate can, by
//! itself, advertise the sibling endpoints another protocol root serves for
//! the same collection: a STAC Collection can't name its tiles lane, a
//! Features Collection can't name the stylesheet that styles it. This seam
//! closes that gap without re-coupling anything: a contributor receives a
//! protocol-neutral [`ResourceRef`] and answers with [`ContributedLink`]s
//! tagged by [`LinkAnchor`]; each protocol's own serializer maps anchors
//! into its own document shape (a STAC Collection/Item's `links`, an OGC
//! Features Collection's `links`), so the vocabulary here never names a
//! protocol.
//!
//! Three rules, inherited from the seams this one is modeled on:
//!
//! - **Capability-derived, never hardcoded.** A contributor derives its
//!   links from the routing declaration and driver capabilities the
//!   [`Router`] passed into [`contribute`](LinkContributor::contribute)
//!   already enforces for handlers — a collection whose tiles lane doesn't
//!   resolve gets no tiles link, no stub, no dead href. This is the same
//!   resolve-time honesty `Router::resolve_tiles` itself applies: a link is
//!   only ever a claim the server can back.
//! - **Named, boot-time registration (`#112`).** [`LinkContributors`] is a
//!   [`NamedRegistry`](crate::extension::NamedRegistry)-backed seam like
//!   `router::Registry`: the wiring layer (the `tellurion` binary) registers
//!   contributors by name at boot, and a binary that registers none — every
//!   test in every protocol crate, and any embedder that never calls
//!   `AppContext::with_link_contributors` — contributes no links and pays no
//!   cost at all: responses are byte-for-byte what they were before this
//!   seam existed.
//! - **Deterministic order.** Contributors run in the registry's
//!   deterministic (alphabetical-by-name) iteration order regardless of
//!   registration order, so a response's link order never depends on the
//!   order `register` calls happened to run in at boot.

use std::sync::Arc;

use crate::extension::NamedRegistry;
use crate::router::Router;

/// Where a contributed link belongs in whatever document a protocol
/// serializer is building. Protocol-neutral on purpose: "collection" here is
/// the routing-layer concept, not any one protocol's document type — a STAC
/// Collection and an OGC Features Collection both map [`Collection`](Self::
/// Collection)-anchored links into their own `links` array, and neither
/// mapping is this module's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkAnchor {
    /// The protocol root / landing document for the `(tenant, catalog)`
    /// scope. No contributor in the first slice emits this anchor; it exists
    /// so the vocabulary is complete from day one and a landing-page
    /// consumer never needs a breaking enum change.
    ResourceRoot,
    /// A collection-level document (a STAC Collection, an OGC API Features
    /// Collection, ...).
    Collection,
    /// An item-level document (a STAC Item, a GeoJSON Feature, ...).
    Item,
}

/// The protocol-neutral description of "which resource is this response
/// about" a contributor derives links for.
///
/// `tenant`/`catalog`/`collection`/`item_id` are EXTERNAL ids (`#39`) —
/// contributed hrefs go on the wire, and an internal id must never serialize
/// — while `tenant_id`/`catalog_id`/`collection_id` are the INTERNAL ids the
/// [`Router`]'s resolve entry points require for capability probes. Both id
/// spaces travel here because the calling handler already holds both (it
/// resolved external -> internal to serve the request at all); making a
/// contributor re-derive either side would re-pay resolver work per link for
/// nothing.
///
/// `item_id` is `None` for a collection-level response. A contributor whose
/// links don't depend on the specific item (every first-slice contributor:
/// tiles and stylesheets are per-collection facts) emits its
/// [`LinkAnchor::Item`]-anchored links regardless, so a caller building an
/// items *page* can contribute once per collection and reuse the result for
/// every item on the page instead of once per row.
///
/// `base_url` is the prefix every contributed href is joined onto — `""` for
/// the server-relative hrefs every response in this workspace already serves
/// (see `tellurion-stac`'s `assets` module for the same convention), left in
/// the contract so a deployment that must emit absolute URLs has a seam for
/// it rather than a rewrite.
#[derive(Debug, Clone, Copy)]
pub struct ResourceRef<'a> {
    /// Tenant external id, exactly as the request's path carried it.
    pub tenant: &'a str,
    /// Catalog external id.
    pub catalog: &'a str,
    /// Collection external id.
    pub collection: &'a str,
    /// Item (feature) external id, when the response is about one item.
    pub item_id: Option<&'a str>,
    /// Href prefix; `""` means server-relative.
    pub base_url: &'a str,
    /// Tenant internal id — what `Router::resolve_*` expects.
    pub tenant_id: &'a str,
    /// Catalog internal id.
    pub catalog_id: &'a str,
    /// Collection internal id.
    pub collection_id: &'a str,
}

/// One link a contributor answered with, still protocol-neutral: the
/// consuming serializer decides what document member each field becomes
/// (every protocol crate's `Link` DTO in this workspace has the same
/// `href`/`rel`/`type` core, plus the optional `title`/`templated` members
/// this carries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributedLink {
    /// Where this link belongs — the consuming serializer filters on this,
    /// taking only the anchor(s) its document shape has a place for.
    pub anchor: LinkAnchor,
    /// Link relation type. Not `Option`: a rel-less link means nothing to
    /// any consumer in this workspace.
    pub rel: String,
    /// Complete href, `base_url` already applied — the serializer
    /// concatenates nothing.
    pub href: String,
    /// Media type. Not `Option` either: every `Link` DTO this feeds
    /// requires a `type` member, so a contributor that can't name one has
    /// nothing servable to contribute.
    pub media_type: String,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// RFC 6570-style templated href (`{tileMatrix}`-shaped placeholders a
    /// client substitutes) — `false` for a directly dereferenceable link.
    pub templated: bool,
}

/// A contributor derives cross-protocol links for one resource from what the
/// `router` can actually resolve for it — see the module doc's
/// "capability-derived, never hardcoded" rule. Infallible by design: "this
/// collection has no such capability" is the ordinary empty answer, not an
/// error, and a contributor that can't check (a probe failed) contributes
/// nothing rather than guessing — links are claims, and an unverifiable
/// claim is not contributed. Nothing here may fail the enclosing request:
/// links are metadata, never worth a 500 (the same never-fail-the-request
/// rule extent/capability metadata already follows in the protocol crates).
#[async_trait::async_trait]
pub trait LinkContributor: Send + Sync {
    async fn contribute(&self, router: &Router, resource: &ResourceRef<'_>)
        -> Vec<ContributedLink>;
}

/// The boot-time link-contributor registry (`#112`-model seam, `#186`):
/// backed by [`NamedRegistry`] like `router::Registry`, registered once by
/// the wiring layer, then held (immutable) on `AppContext` for the process
/// lifetime — reloads swap the `Router` a contribution consults, never the
/// contributor set, so a reload's capability changes flow through the
/// `router` argument with no re-registration. Empty (the default) means
/// every call to [`contribute`](Self::contribute) answers `Vec::new()`
/// without invoking anything.
#[derive(Default)]
pub struct LinkContributors {
    registry: NamedRegistry<dyn LinkContributor>,
}

impl LinkContributors {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `contributor` under `name`, replacing any earlier entry
    /// with the same name — `NamedRegistry`'s own last-write-wins rule.
    pub fn register(&mut self, name: impl Into<String>, contributor: Arc<dyn LinkContributor>) {
        self.registry.register(name, contributor);
    }

    pub fn is_empty(&self) -> bool {
        self.registry.is_empty()
    }

    /// Every registered contributor name, alphabetically — what the boot
    /// log line enumerating this seam's contents reports (`#112`).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.registry.names()
    }

    /// Every registered contributor's links for `resource`, concatenated in
    /// the registry's deterministic order. The caller filters by
    /// [`LinkAnchor`] for the document it is building. An empty registry
    /// returns an empty vec without touching `router` at all — the
    /// "unregistered means byte-for-byte unchanged, at zero cost" guarantee
    /// the module doc states.
    pub async fn contribute(
        &self,
        router: &Router,
        resource: &ResourceRef<'_>,
    ) -> Vec<ContributedLink> {
        let mut links = Vec::new();
        for (_, contributor) in self.registry.iter() {
            links.extend(contributor.contribute(router, resource).await);
        }
        links
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CatalogSource, PhysicalCollection};
    use crate::config::{AppConfig, StorageDecl};
    use crate::error::Result;
    use crate::router::{DriverFactory, Registry, StorageDriver};

    /// Satisfies `StorageDriver`'s one mandatory capability; these tests
    /// exercise registry mechanics, not capability resolution (that's the
    /// wiring layer's contributors' own test suite).
    struct EmptyCatalog;

    #[async_trait::async_trait]
    impl CatalogSource for EmptyCatalog {
        async fn collections(&self) -> Result<Vec<PhysicalCollection>> {
            Ok(vec![])
        }
    }

    struct FakeDriver;

    impl StorageDriver for FakeDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(EmptyCatalog)
        }
    }

    struct FakeFactory;

    impl DriverFactory for FakeFactory {
        fn name(&self) -> &str {
            "fake"
        }

        fn build(&self, _decl: &StorageDecl) -> Result<Arc<dyn StorageDriver>> {
            Ok(Arc::new(FakeDriver))
        }
    }

    fn test_router() -> Router {
        let config: AppConfig = serde_yaml::from_str(
            r#"
storages: [ { id: main, driver: fake, url_env: DATABASE_URL } ]
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
        registry.register(Arc::new(FakeFactory));
        Router::build(&config, &registry).unwrap()
    }

    fn resource<'a>() -> ResourceRef<'a> {
        ResourceRef {
            tenant: "public",
            catalog: "default",
            collection: "demo",
            item_id: None,
            base_url: "",
            tenant_id: "public",
            catalog_id: "default",
            collection_id: "demo",
        }
    }

    /// Emits one fixed link so ordering/aggregation is observable; records
    /// nothing about the router on purpose (see `EmptyCatalog`'s doc).
    struct FixedContributor {
        rel: &'static str,
        anchor: LinkAnchor,
    }

    #[async_trait::async_trait]
    impl LinkContributor for FixedContributor {
        async fn contribute(
            &self,
            _router: &Router,
            resource: &ResourceRef<'_>,
        ) -> Vec<ContributedLink> {
            vec![ContributedLink {
                anchor: self.anchor,
                rel: self.rel.to_string(),
                href: format!(
                    "{}/{}/x/{}/{}",
                    resource.base_url, resource.tenant, resource.catalog, resource.collection
                ),
                media_type: "application/json".to_string(),
                title: None,
                templated: false,
            }]
        }
    }

    #[tokio::test]
    async fn an_empty_registry_contributes_nothing() {
        let contributors = LinkContributors::new();
        assert!(contributors.is_empty());
        let links = contributors.contribute(&test_router(), &resource()).await;
        assert!(links.is_empty());
    }

    #[tokio::test]
    async fn contributions_come_back_in_deterministic_name_order_not_registration_order() {
        let mut contributors = LinkContributors::new();
        contributors.register(
            "zulu",
            Arc::new(FixedContributor {
                rel: "z-rel",
                anchor: LinkAnchor::Collection,
            }),
        );
        contributors.register(
            "alpha",
            Arc::new(FixedContributor {
                rel: "a-rel",
                anchor: LinkAnchor::Collection,
            }),
        );

        let links = contributors.contribute(&test_router(), &resource()).await;
        let rels: Vec<&str> = links.iter().map(|l| l.rel.as_str()).collect();
        assert_eq!(rels, vec!["a-rel", "z-rel"]);
        assert_eq!(
            contributors.names().collect::<Vec<_>>(),
            vec!["alpha", "zulu"]
        );
    }

    #[tokio::test]
    async fn registering_the_same_name_twice_replaces_the_earlier_contributor() {
        let mut contributors = LinkContributors::new();
        contributors.register(
            "styles",
            Arc::new(FixedContributor {
                rel: "first",
                anchor: LinkAnchor::Collection,
            }),
        );
        contributors.register(
            "styles",
            Arc::new(FixedContributor {
                rel: "second",
                anchor: LinkAnchor::Collection,
            }),
        );

        let links = contributors.contribute(&test_router(), &resource()).await;
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].rel, "second");
    }

    #[tokio::test]
    async fn callers_filter_by_anchor_for_the_document_they_build() {
        let mut contributors = LinkContributors::new();
        contributors.register(
            "collection-level",
            Arc::new(FixedContributor {
                rel: "coll-rel",
                anchor: LinkAnchor::Collection,
            }),
        );
        contributors.register(
            "item-level",
            Arc::new(FixedContributor {
                rel: "item-rel",
                anchor: LinkAnchor::Item,
            }),
        );

        let links = contributors.contribute(&test_router(), &resource()).await;
        let collection_rels: Vec<&str> = links
            .iter()
            .filter(|l| l.anchor == LinkAnchor::Collection)
            .map(|l| l.rel.as_str())
            .collect();
        assert_eq!(collection_rels, vec!["coll-rel"]);
        let item_rels: Vec<&str> = links
            .iter()
            .filter(|l| l.anchor == LinkAnchor::Item)
            .map(|l| l.rel.as_str())
            .collect();
        assert_eq!(item_rels, vec!["item-rel"]);
    }

    #[tokio::test]
    async fn hrefs_are_contributed_complete_with_the_base_url_applied() {
        let mut contributors = LinkContributors::new();
        contributors.register(
            "fixed",
            Arc::new(FixedContributor {
                rel: "r",
                anchor: LinkAnchor::Collection,
            }),
        );

        let resource = ResourceRef {
            base_url: "https://example.test",
            ..resource()
        };
        let links = contributors.contribute(&test_router(), &resource).await;
        assert_eq!(links[0].href, "https://example.test/public/x/default/demo");
    }
}
