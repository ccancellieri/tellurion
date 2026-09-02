//! `TenantReader` — the read seam between the routing layer and wherever
//! tenant *declarations* themselves live (`#143`, mirroring `#42`'s
//! `RegistryReader` for catalogs/collections — see that module's own doc for
//! the parallel). A tenant has no owning scope the way a catalog is owned by
//! a tenant or a collection by a catalog, so this trait's listing method
//! needs no scoping argument at all — the `list_all` semantics
//! `RegistryReader` deliberately never has, because every one of its listing
//! methods answers "children of this one scope," never "everything."
//!
//! `FileTenantReader`, the first implementor, is an in-memory index built
//! once from the same loaded `AppConfig` that `Router` and `StaticResolver`
//! already index — a file-backed deployment pays exactly today's cost,
//! byte-for-byte the behavior small deployments already have.
//!
//! A relational tenant backend (`#143`, second slice) is a second
//! `TenantReader` implementor, connected over the network rather than built
//! in memory — `tellurion-postgis`'s `PostgisTenantReader`. Since
//! `tellurion-core` never depends on a concrete database client crate, it
//! cannot construct one directly; it only defines
//! [`RelationalTenantFactory`], the same "trait here, driver elsewhere"
//! boundary [`RelationalRegistryFactory`](crate::registry::RelationalRegistryFactory)
//! already draws. [`build_tenant_reader`] dispatches on the SAME
//! `AppConfig.registry.backend`/`.storage` knob `build_registry_reader`
//! already reads — this slice does not introduce a second backend selector;
//! a deployment that moves its registry to a relational store moves its
//! tenant declarations there too, in the same connection. The wiring layer
//! (the `tellurion` binary) registers a concrete factory, under its own
//! declared name, for each driver crate compiled in — see
//! [`RelationalTenantFactories`] and, for the name that selects one,
//! `AppConfig.registry.implementation` (`#162`).
//!
//! Listing is keyset-paginated, never OFFSET, ordered by external id — the
//! exact same convention `RegistryReader::list_catalogs`/`.list_collections`
//! already use; see `registry.rs`'s own doc for why.
//!
//! Callers (`context::build_router_and_resolver`) walk a `TenantReader` to
//! exhaustion via [`snapshot_tenants`] to get the normalized `Vec<TenantDecl>`
//! `Router::build_from_snapshot`/`StaticResolver::build_from_snapshot` index —
//! the same "one normalized input, whichever source produced it" treatment
//! `RoutingSnapshot` already gives catalogs/collections. For the file-backed
//! default this walk is exactly `config.tenants` (no I/O, `FileTenantReader`
//! never awaits anything); the two thin wrappers, `Router::build`/
//! `StaticResolver::build`, still read `config.tenants` directly rather than
//! walking a reader for it, the same "the file path never actually
//! constructs a snapshot" convention `Router::build`/`StaticResolver::build`
//! already follow for catalogs/collections.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::config::{AppConfig, RegistryBackend, TenantDecl};
use crate::error::Result;
use crate::registry::{
    paginate, resolve_relational_database_url, select_relational_implementation, Page, PageRequest,
};

#[async_trait::async_trait]
pub trait TenantReader: Send + Sync {
    /// `external_id` -> its declaration. `Ok(None)` for an unknown external
    /// id — a single-declaration lookup that touches only this tenant's own
    /// index entry, never every other tenant in the registry.
    async fn tenant(&self, external_id: &str) -> Result<Option<TenantDecl>>;

    /// Every declared tenant, keyset-paginated by external id — unlike
    /// `RegistryReader::list_catalogs`/`.list_collections`, this takes no
    /// scoping argument: a tenant has no owning parent to scope a listing
    /// by, so this is genuinely "list every tenant."
    async fn list_tenants(&self, page: PageRequest) -> Result<Page<TenantDecl>>;
}

/// Connects a relational `TenantReader` (`#143`, second slice) — implemented
/// by a driver crate (`tellurion-postgis`'s `PostgisTenantFactory`) and
/// supplied to [`build_tenant_reader`] by the wiring layer, the same
/// "driver stays out of core" boundary
/// [`RelationalRegistryFactory`](crate::registry::RelationalRegistryFactory)
/// already draws. `connect` actually attempts a connection rather than
/// deferring it, so an unreachable database is a hard error the caller sees
/// immediately — never a `TenantReader` that silently fails its first real
/// query.
///
/// [`name`](Self::name) is the implementation's stable, config-facing name
/// (`#162`) and must match the name the SAME driver crate's
/// [`RelationalRegistryFactory`](crate::registry::RelationalRegistryFactory)
/// declares: one `registry.implementation` value selects both halves of the
/// one `registry.backend` knob, never one relational implementation for
/// catalogs and a different one for tenants.
#[async_trait::async_trait]
pub trait RelationalTenantFactory: Send + Sync {
    fn name(&self) -> &str;

    async fn connect(&self, database_url: &str) -> Result<Arc<dyn TenantReader>>;
}

/// The relational tenant seam's boot-time registry (`#112`, `#143`, `#162`) —
/// the exact shape and rationale of
/// [`RelationalRegistryFactories`](crate::registry::RelationalRegistryFactories),
/// for the tenant half of the same knob. Registered in `main` alongside it,
/// from the same driver crates, under the same names.
#[derive(Default)]
pub struct RelationalTenantFactories {
    factories: crate::extension::NamedRegistry<dyn RelationalTenantFactory>,
}

impl RelationalTenantFactories {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `factory` under its own declared name, replacing whatever
    /// was registered under that name before.
    pub fn register(&mut self, factory: Arc<dyn RelationalTenantFactory>) {
        let name = factory.name().to_string();
        self.factories.register(name, factory);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn RelationalTenantFactory>> {
        self.factories.get(name)
    }

    /// Every registered implementation name, alphabetically.
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

/// Builds the `TenantReader` a boot or reload should use for `config`:
/// `RegistryBackend::File` (the default) is always `FileTenantReader`, no
/// I/O, and never consults `factories` at all. `RegistryBackend::Relational`
/// resolves `config.registry.storage`'s `url_env` — exactly the same
/// resolution `build_registry_reader` performs, via the same
/// [`resolve_relational_database_url`] helper, since both readers connect
/// through the one `registry.backend`/`.storage` knob — and selects its
/// implementation through the same
/// [`select_relational_implementation`] the registry reader uses, off the
/// same `registry.implementation` name, so the two halves can never resolve
/// to different driver crates.
///
/// This function does no swapping of its own — it only builds and validates
/// one `TenantReader`. The caller (`main`/`reload.rs`) decides what a failure
/// means, the same way it already does for `build_registry_reader`'s own
/// result.
pub async fn build_tenant_reader(
    config: &AppConfig,
    factories: &RelationalTenantFactories,
) -> Result<Arc<dyn TenantReader>> {
    match config.registry.backend {
        RegistryBackend::File => Ok(Arc::new(FileTenantReader::build(config))),
        RegistryBackend::Relational => {
            let database_url = resolve_relational_database_url(config)?;
            let factory = select_relational_implementation(
                config.registry.implementation.as_deref(),
                &factories.factories,
                "tenant",
            )?;
            factory.connect(&database_url).await
        }
    }
}

/// Page size [`snapshot_tenants`] requests per `list_tenants` call while
/// walking a relational tenant store to exhaustion — same rationale as
/// `registry.rs`'s own `SNAPSHOT_PAGE_SIZE`.
const TENANT_SNAPSHOT_PAGE_SIZE: u32 = 1000;

/// [`snapshot_tenants`] with the page size fixed to whatever a caller
/// passes, rather than the production default — the seam a test uses to
/// exercise the "more than one page" branch of the walk below without
/// seeding a four-figure row count. Production code always calls
/// [`snapshot_tenants`] instead.
#[cfg(test)]
pub(crate) async fn snapshot_tenants_with_page_size(
    reader: &dyn TenantReader,
    page_size: u32,
) -> Result<Vec<TenantDecl>> {
    snapshot_tenants_at_page_size(reader, page_size).await
}

/// Walks `reader` to exhaustion and returns every tenant it knows about — the
/// normalized input `Router::build_from_snapshot`/`StaticResolver::
/// build_from_snapshot` index instead of reading `AppConfig.tenants`
/// directly, consumed by `context::build_router_and_resolver`. Not yet
/// validated against `config` — see [`validate_tenant_snapshot`], which
/// `build_router_and_resolver` calls before ever handing this to either
/// index builder.
pub async fn snapshot_tenants(reader: &dyn TenantReader) -> Result<Vec<TenantDecl>> {
    snapshot_tenants_at_page_size(reader, TENANT_SNAPSHOT_PAGE_SIZE).await
}

async fn snapshot_tenants_at_page_size(
    reader: &dyn TenantReader,
    page_size: u32,
) -> Result<Vec<TenantDecl>> {
    let mut tenants = Vec::new();
    let mut after = None;
    loop {
        let page = reader
            .list_tenants(PageRequest {
                limit: page_size,
                after,
            })
            .await?;
        let next = page.next;
        tenants.extend(page.items);
        match next {
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }
    Ok(tenants)
}

/// Applies the same declaration-local validation as the file-backed tenant
/// list: unique identities, reserved-segment rejection, settings shape,
/// profile references, and positive limits. Whole-snapshot relationships
/// to catalogs, policies, and final settings are validated by
/// `validate_registry_snapshot` after both relational walks complete.
pub fn validate_tenant_snapshot(config: &AppConfig, tenants: &[TenantDecl]) -> Result<()> {
    let known_profile_ids = config
        .profiles
        .iter()
        .map(|profile| profile.id.as_str())
        .collect();
    crate::config::validate_tenant_declarations(tenants, &known_profile_ids)
}

/// The first `TenantReader`: a flat in-memory index (external id ->
/// `TenantDecl`) built once from `AppConfig` at boot or reload — no I/O,
/// ever, the same convention `FileRegistryReader` already follows. Ordered
/// by `BTreeMap` so a keyset range scan (`list_tenants`) is a plain `range`
/// call, never a sort-then-slice on every request.
pub struct FileTenantReader {
    by_external: BTreeMap<String, TenantDecl>,
}

impl FileTenantReader {
    pub fn build(config: &AppConfig) -> Self {
        let mut by_external = BTreeMap::new();
        for tenant in &config.tenants {
            by_external.insert(tenant.external_id().to_string(), tenant.clone());
        }
        Self { by_external }
    }
}

#[async_trait::async_trait]
impl TenantReader for FileTenantReader {
    async fn tenant(&self, external_id: &str) -> Result<Option<TenantDecl>> {
        Ok(self.by_external.get(external_id).cloned())
    }

    async fn list_tenants(&self, page: PageRequest) -> Result<Page<TenantDecl>> {
        Ok(paginate(Some(&self.by_external), &page))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    const MULTI_TENANT_CONFIG: &str = r#"
tenants:
  - { id: acme-internal, external_id: acme }
  - { id: globex-internal, external_id: globex }
  - { id: initech-internal, external_id: initech }
"#;

    fn multi_tenant_config() -> AppConfig {
        let config: AppConfig = serde_yaml::from_str(MULTI_TENANT_CONFIG).unwrap();
        config.validate().unwrap();
        config
    }

    #[tokio::test]
    async fn tenant_lookup_is_a_direct_hit_not_a_scan() {
        let reader = FileTenantReader::build(&multi_tenant_config());
        let decl = reader
            .tenant("globex")
            .await
            .unwrap()
            .expect("globex is declared");
        assert_eq!(decl.id, "globex-internal");
    }

    #[tokio::test]
    async fn tenant_lookup_is_none_for_an_unknown_external_id() {
        let reader = FileTenantReader::build(&multi_tenant_config());
        assert!(reader.tenant("nonexistent").await.unwrap().is_none());
    }

    /// The `FileTenantReader`'s point lookup answers exactly what a direct
    /// scan of `config.tenants` (today's pre-`#143` behavior) would have —
    /// byte-identical, for every tenant this config declares.
    #[tokio::test]
    async fn file_tenant_reader_is_byte_identical_to_a_direct_config_tenants_scan() {
        let config = multi_tenant_config();
        let reader = FileTenantReader::build(&config);

        for tenant in &config.tenants {
            let via_reader = reader
                .tenant(tenant.external_id())
                .await
                .unwrap()
                .expect("every declared tenant must be found via the reader");
            let via_direct_scan = config
                .tenants
                .iter()
                .find(|t| t.external_id() == tenant.external_id())
                .expect("every declared tenant must be found via a direct scan");
            assert_eq!(&via_reader, via_direct_scan);
        }
    }

    #[tokio::test]
    async fn list_tenants_returns_everything_in_one_page_when_the_limit_is_not_exceeded() {
        let reader = FileTenantReader::build(&multi_tenant_config());
        let page = reader
            .list_tenants(PageRequest {
                limit: 10,
                after: None,
            })
            .await
            .unwrap();
        let ids: Vec<&str> = page.items.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["acme-internal", "globex-internal", "initech-internal"]
        );
        assert_eq!(page.next, None);
    }

    #[tokio::test]
    async fn list_tenants_paginates_with_a_keyset_cursor_never_an_offset() {
        let reader = FileTenantReader::build(&multi_tenant_config());

        let first = reader
            .list_tenants(PageRequest {
                limit: 2,
                after: None,
            })
            .await
            .unwrap();
        let first_ids: Vec<&str> = first.items.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(first_ids, vec!["acme-internal", "globex-internal"]);
        assert_eq!(first.next.as_deref(), Some("globex"));

        let second = reader
            .list_tenants(PageRequest {
                limit: 2,
                after: first.next.clone(),
            })
            .await
            .unwrap();
        let second_ids: Vec<&str> = second.items.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(second_ids, vec!["initech-internal"]);
        assert_eq!(second.next, None);
    }

    // -- `build_tenant_reader` -------------------------------------------

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

    /// One registry holding exactly `factories` — the shape `main` builds at
    /// boot, spelled once so a test reads as "this binary contains these
    /// implementations."
    fn factories_of(entries: Vec<Arc<dyn RelationalTenantFactory>>) -> RelationalTenantFactories {
        let mut registry = RelationalTenantFactories::new();
        for entry in entries {
            registry.register(entry);
        }
        registry
    }

    #[async_trait::async_trait]
    impl RelationalTenantFactory for RecordingFactory {
        fn name(&self) -> &str {
            self.name
        }

        async fn connect(&self, database_url: &str) -> Result<Arc<dyn TenantReader>> {
            *self.seen_url.lock().unwrap() = Some(database_url.to_string());
            Ok(Arc::new(FileTenantReader::build(&AppConfig::default())))
        }
    }

    struct AlwaysFailsFactory;

    #[async_trait::async_trait]
    impl RelationalTenantFactory for AlwaysFailsFactory {
        fn name(&self) -> &str {
            "always-fails"
        }

        async fn connect(&self, _database_url: &str) -> Result<Arc<dyn TenantReader>> {
            Err(Error::Storage("connection refused".into()))
        }
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
        let config: AppConfig = serde_yaml::from_str("storages: []").unwrap();
        let reader = build_tenant_reader(&config, &RelationalTenantFactories::new())
            .await
            .expect("file backend needs no factory");
        assert!(reader
            .list_tenants(PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }

    #[tokio::test]
    async fn relational_backend_resolves_the_referenced_storages_url_env_and_connects() {
        let env_var = "TELLURION_CORE_TENANT_TEST_URL_A";
        // Safety: this test's env var name is unique to it; no other test in
        // this crate reads or writes it concurrently.
        unsafe {
            std::env::set_var(env_var, "postgres://example/tenant-test");
        }
        let config = relational_backend_config("main", env_var);
        let factory = Arc::new(RecordingFactory::new());

        build_tenant_reader(&config, &factories_of(vec![factory.clone()]))
            .await
            .expect("connects via the recording factory");

        assert_eq!(
            factory.seen_url.lock().unwrap().as_deref(),
            Some("postgres://example/tenant-test")
        );
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    #[tokio::test]
    async fn relational_backend_with_no_factory_is_a_config_error() {
        let env_var = "TELLURION_CORE_TENANT_TEST_URL_B";
        unsafe {
            std::env::set_var(env_var, "postgres://example/tenant-test");
        }
        let config = relational_backend_config("main", env_var);

        let result = build_tenant_reader(&config, &RelationalTenantFactories::new()).await;
        assert!(
            matches!(result, Err(Error::Config(_))),
            "no factory means no way to build a relational reader"
        );
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    #[tokio::test]
    async fn a_connect_failure_propagates_as_an_error() {
        let env_var = "TELLURION_CORE_TENANT_TEST_URL_C";
        unsafe {
            std::env::set_var(env_var, "postgres://example/tenant-test");
        }
        let config = relational_backend_config("main", env_var);

        let result =
            build_tenant_reader(&config, &factories_of(vec![Arc::new(AlwaysFailsFactory)])).await;
        assert!(
            matches!(result, Err(Error::Storage(_))),
            "a factory that fails to connect must surface as an error"
        );
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    // -- named registry backends (`#162`) ------------------------------------

    /// The tenant half of the one `registry.backend` knob obeys the SAME
    /// `registry.implementation` name the catalog/collection half does — a
    /// deployment cannot end up reading its catalogs through one driver crate
    /// and its tenants through another.
    #[tokio::test]
    async fn the_same_implementation_name_selects_the_tenant_half_too() {
        let env_var = "TELLURION_CORE_TENANT_TEST_URL_NAMED";
        unsafe {
            std::env::set_var(env_var, "postgres://example/tenant-named");
        }
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: postgis, url_env: {env_var} }} ]
registry: {{ backend: relational, storage: main, implementation: zulu }}
"#
        ))
        .unwrap();
        config.validate().unwrap();
        let alpha = Arc::new(RecordingFactory::named("alpha"));
        let zulu = Arc::new(RecordingFactory::named("zulu"));

        build_tenant_reader(&config, &factories_of(vec![alpha.clone(), zulu.clone()]))
            .await
            .expect("the named implementation connects");

        assert_eq!(
            zulu.seen_url.lock().unwrap().as_deref(),
            Some("postgres://example/tenant-named")
        );
        assert!(alpha.seen_url.lock().unwrap().is_none());
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    /// The tenant seam's refusal names itself as the tenant seam, so an
    /// operator can tell which half of the knob failed to resolve.
    #[tokio::test]
    async fn an_unregistered_implementation_name_is_refused_by_name_for_the_tenant_seam() {
        let env_var = "TELLURION_CORE_TENANT_TEST_URL_UNKNOWN";
        unsafe {
            std::env::set_var(env_var, "postgres://example/tenant-unknown");
        }
        let config: AppConfig = serde_yaml::from_str(&format!(
            r#"
storages: [ {{ id: main, driver: postgis, url_env: {env_var} }} ]
registry: {{ backend: relational, storage: main, implementation: sqlite }}
"#
        ))
        .unwrap();
        config.validate().unwrap();

        match build_tenant_reader(
            &config,
            &factories_of(vec![Arc::new(RecordingFactory::named("postgis"))]),
        )
        .await
        {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("sqlite") && message.contains("registry.implementation"),
                    "the refusal must name the key and the unknown name: {message}"
                );
                assert!(
                    message.contains("tenant"),
                    "the refusal must say which seam could not resolve it: {message}"
                );
                assert!(
                    message.contains("postgis"),
                    "the refusal must list what IS registered: {message}"
                );
            }
            other => panic!("expected a named Error::Config, got ok={}", other.is_ok()),
        }
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    /// Same pre-`#162` sentence as the registry seam's own, with `tenant` in
    /// place of `registry` — unchanged from what a driverless binary always
    /// printed.
    #[tokio::test]
    async fn an_empty_registry_keeps_the_pre_162_compiled_out_wording() {
        let env_var = "TELLURION_CORE_TENANT_TEST_URL_EMPTY";
        unsafe {
            std::env::set_var(env_var, "postgres://example/tenant-empty");
        }
        let config = relational_backend_config("main", env_var);

        match build_tenant_reader(&config, &RelationalTenantFactories::new()).await {
            Err(Error::Config(message)) => assert_eq!(
                message,
                "registry.backend is 'relational' but this binary was built with no driver \
                 providing a relational tenant factory"
            ),
            other => panic!("expected a named Error::Config, got ok={}", other.is_ok()),
        }
        unsafe {
            std::env::remove_var(env_var);
        }
    }

    // -- `snapshot_tenants` ------------------------------------------------

    #[tokio::test]
    async fn snapshot_walk_crosses_multiple_pages_and_collects_everything_exactly_once() {
        let reader = FileTenantReader::build(&multi_tenant_config());
        let tenants = snapshot_tenants_with_page_size(&reader, 1)
            .await
            .expect("the walk succeeds across multiple pages");
        let mut ids: Vec<&str> = tenants.iter().map(|t| t.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec!["acme-internal", "globex-internal", "initech-internal"]
        );
    }

    #[tokio::test]
    async fn snapshot_tenants_uses_the_production_page_size_by_default() {
        let reader = FileTenantReader::build(&multi_tenant_config());
        let tenants = snapshot_tenants(&reader).await.unwrap();
        assert_eq!(tenants.len(), 3);
    }

    // -- `validate_tenant_snapshot` ----------------------------------------

    #[test]
    fn validate_tenant_snapshot_accepts_unique_ids() {
        let config = multi_tenant_config();
        validate_tenant_snapshot(&config, &config.tenants).unwrap();
    }

    #[test]
    fn validate_tenant_snapshot_rejects_a_duplicate_internal_id() {
        let tenants = vec![
            TenantDecl {
                id: "dup".to_string(),
                external_id: Some("first".to_string()),
                settings: Default::default(),
            },
            TenantDecl {
                id: "dup".to_string(),
                external_id: Some("second".to_string()),
                settings: Default::default(),
            },
        ];
        match validate_tenant_snapshot(&AppConfig::default(), &tenants) {
            Err(Error::Config(message)) => {
                assert!(message.contains("duplicate"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    #[test]
    fn validate_tenant_snapshot_rejects_a_duplicate_external_id() {
        let tenants = vec![
            TenantDecl {
                id: "first".to_string(),
                external_id: Some("shared".to_string()),
                settings: Default::default(),
            },
            TenantDecl {
                id: "second".to_string(),
                external_id: Some("shared".to_string()),
                settings: Default::default(),
            },
        ];
        match validate_tenant_snapshot(&AppConfig::default(), &tenants) {
            Err(Error::Config(message)) => {
                assert!(message.contains("duplicate"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    #[test]
    fn relational_snapshot_rejects_reserved_external_ids_like_file_tenants() {
        let tenant: TenantDecl = serde_yaml::from_str("id: db-tenant\nexternal_id: metrics\n")
            .expect("valid tenant syntax");

        match validate_tenant_snapshot(&AppConfig::default(), &[tenant]) {
            Err(Error::Config(message)) => {
                assert!(message.contains("reserved"), "message was: {message}");
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    #[test]
    fn relational_snapshot_rejects_unknown_profile_references_like_file_tenants() {
        let tenant: TenantDecl =
            serde_yaml::from_str("id: db-tenant\nsettings:\n  profile: missing-profile\n")
                .expect("valid tenant syntax");

        match validate_tenant_snapshot(&AppConfig::default(), &[tenant]) {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("unknown profile"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }

    #[test]
    fn relational_snapshot_rejects_non_positive_limits_like_file_tenants() {
        let tenant: TenantDecl =
            serde_yaml::from_str("id: db-tenant\nsettings:\n  max_request_body_bytes: 0\n")
                .expect("valid tenant syntax");

        match validate_tenant_snapshot(&AppConfig::default(), &[tenant]) {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("max_request_body_bytes must be greater than 0"),
                    "message was: {message}"
                );
            }
            other => panic!("expected Err(Config(_)), got {}", other.is_ok()),
        }
    }
}
