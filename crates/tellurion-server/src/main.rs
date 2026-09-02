//! Tellurion server binary: loads config, builds the storage registry and
//! router, wires the axum app (middleware, metrics), and serves it with a
//! bounded graceful shutdown.

mod admission;
mod app;
mod applier;
mod config_mutation;
mod config_view;
mod control_api;
mod control_bootstrap;
mod control_browser_auth;
mod control_checkpoint;
mod control_consumer;
mod control_session;
mod generation_consumer;
mod landing;
mod link_contributors;
mod log_redact;
mod metrics;
mod openapi;
mod process_lane;
mod protocol;
mod readiness;
mod reload;
mod request_id;
mod retention_consumer;
mod runtime_activation;
mod shutdown;
#[cfg(feature = "ui")]
mod ui_assets;
mod webhook_admin;
mod webhook_consumer;

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;

#[cfg(feature = "cog")]
use tellurion_cog::{CogDriverFactory, MosaicDriverFactory};
use tellurion_core::{
    build_authorizer_with_bindings, build_registry_reader, build_router_and_resolver,
    build_tenant_reader, AppContext, BootEnvelope, CacheConfig, ConfigStore, ConfigVersion,
    ControlBrowserAuthConfig, ControlRuntimeStatus, ControlStoreLocator, FileConfigStore,
    FileStyleStore, L2CacheConfig, LinkContributors, MetricsTileCache, MokaTileCache, Registry,
    RegistryValidationMode, RelationalRegistryFactories, RelationalTenantFactories, SeedStatus,
    StyleStore, TileCache,
};
#[cfg(all(feature = "public-demo", feature = "ui"))]
use tellurion_core::{AppConfig, Resolver, Router as CoreRouter, ServerConfig, StaticResolver};
#[cfg(feature = "valkey")]
use tellurion_core::{L2Cache, L2CacheAdapter, L2Tier, LayeredCache, ValkeyL2Cache};
#[cfg(feature = "duckdb")]
use tellurion_duckdb::DuckdbDriverFactory;
#[cfg(feature = "flatgeobuf")]
use tellurion_flatgeobuf::FlatgeobufDriverFactory;
#[cfg(feature = "geopackage")]
use tellurion_geopackage::GeopackageDriverFactory;
#[cfg(feature = "geoparquet")]
use tellurion_geoparquet::GeoparquetDriverFactory;
#[cfg(feature = "iceberg")]
use tellurion_iceberg::IcebergDriverFactory;
#[cfg(feature = "pmtiles")]
use tellurion_pmtiles::PmtilesDriverFactory;
#[cfg(feature = "postgis")]
use tellurion_postgis::{PostgisDriverFactory, PostgisRegistryFactory, PostgisTenantFactory};
#[cfg(feature = "shapefile")]
use tellurion_shapefile::ShapefileDriverFactory;
#[cfg(feature = "zarr")]
use tellurion_zarr::ZarrDriverFactory;

/// Config lookup order: `--config`, then `$TELLURION_CONFIG`, then this.
const DEFAULT_CONFIG_PATH: &str = "./config.yaml";
const RSS_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// Hard ceiling on the boot-time `cache.l2` connect (`#161`). Not an
/// operator tunable and deliberately not a config key: it is what makes
/// [`build_cache`]'s own "a cache tier being down must not be the reason the
/// server can't restart" contract actually TRUE.
///
/// `redis`'s `ConnectionManager` awaits its initial connection and retries it
/// on an exponential schedule built from `backon`'s `ExponentialBuilder` with
/// a *factor of 100* — the successive waits are roughly 1s, 100s, 10000s, and
/// so on. Against an unreachable backend that is indistinguishable from a
/// hang: without this deadline the process never finishes `build_cache`, so
/// it never binds a port, never answers `/healthz`, and never reaches the
/// readiness probe that exists to report exactly this situation. Ten seconds
/// leaves room for the client's own first retry (a Valkey coming up
/// alongside the server) while keeping "down" bounded and boot-safe.
#[cfg(feature = "valkey")]
const L2_CONNECT_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Parser)]
#[command(name = "tellurion", about = "Tellurion geospatial serving engine")]
struct Cli {
    /// Path to the YAML config file.
    #[arg(long)]
    config: Option<String>,

    /// Serve only the stateless public evaluation UI and HTTPS-source sandbox.
    #[arg(long)]
    public_demo_only: bool,
}

#[cfg(all(feature = "public-demo", feature = "ui"))]
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PublicDemoBootConfig {
    server: PublicDemoServerConfig,
    cache: PublicDemoCacheConfig,
}

#[cfg(all(feature = "public-demo", feature = "ui"))]
#[derive(Debug, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PublicDemoServerConfig {
    port: u16,
    request_timeout_s: u64,
    log_json: bool,
    max_concurrency: Option<usize>,
}

#[cfg(all(feature = "public-demo", feature = "ui"))]
impl Default for PublicDemoServerConfig {
    fn default() -> Self {
        let defaults = ServerConfig::default();
        Self {
            port: defaults.port,
            request_timeout_s: defaults.request_timeout_s,
            log_json: defaults.log_json,
            max_concurrency: defaults.max_concurrency,
        }
    }
}

#[cfg(all(feature = "public-demo", feature = "ui"))]
#[derive(Debug, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PublicDemoCacheConfig {
    memory_percent: f64,
}

#[cfg(all(feature = "public-demo", feature = "ui"))]
impl Default for PublicDemoCacheConfig {
    fn default() -> Self {
        Self {
            memory_percent: CacheConfig::default().memory_percent,
        }
    }
}

#[cfg(all(feature = "public-demo", feature = "ui"))]
fn parse_public_demo_config(raw: &str) -> anyhow::Result<AppConfig> {
    let boot: PublicDemoBootConfig = serde_yaml::from_str(raw)?;
    let mut config = AppConfig::default();
    config.server.port = boot.server.port;
    config.server.request_timeout_s = boot.server.request_timeout_s;
    config.server.log_json = boot.server.log_json;
    config.server.max_concurrency = boot.server.max_concurrency;
    config.cache.memory_percent = boot.cache.memory_percent;
    config.validate()?;
    Ok(config)
}

#[cfg(all(feature = "public-demo", feature = "ui"))]
fn public_demo_port(configured: u16, render_port: Option<String>) -> anyhow::Result<u16> {
    let Some(raw) = render_port else {
        return Ok(configured);
    };
    let port = raw
        .parse::<u16>()
        .map_err(|_| anyhow::anyhow!("PORT must be an integer from 1 through 65535"))?;
    if port == 0 {
        anyhow::bail!("PORT must be an integer from 1 through 65535");
    }
    Ok(port)
}

fn config_path(cli_path: Option<String>, env_path: Option<String>) -> String {
    cli_path
        .or(env_path)
        .unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string())
}

fn resolve_control_browser_secret_from(
    browser: Option<&ControlBrowserAuthConfig>,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> anyhow::Result<Option<String>> {
    let Some(variable) = browser.and_then(|browser| browser.client_secret_env.as_deref()) else {
        return Ok(None);
    };
    let secret = lookup(variable)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("browser client secret environment variable '{variable}' is not set")
        })?;
    Ok(Some(secret))
}

/// Builds the tile cache from `cache.l2`: `L2CacheConfig::None` (the
/// default) is exactly today's behavior — an L1-only `MokaTileCache` wrapped
/// for metrics, nothing else in the picture. A `valkey` selection adds an
/// `L2CacheAdapter` layer ahead of it via `LayeredCache`, so every existing
/// caller (which only ever sees `Arc<dyn TileCache>`) needs no change.
///
/// Connecting is eager, but the two boot-time failure classes part ways
/// here: a `url_env` variable that isn't set is a config error and fails
/// startup (same "bad config fails fast" contract `Router::validate_catalog`
/// applies to storages), while a set-but-unreachable backend is an *absent
/// optional component* — the server logs the error and serves L1-only,
/// per the architecture stance that absence degrades a feature, never boot.
/// A cache tier being down must not be the reason the server can't restart.
/// Once connected, a *later* outage degrades silently per-request via
/// `L2CacheAdapter` (and the client auto-reconnects when the tier returns).
///
/// `#161`: both `valkey` outcomes now also DECLARE the tier through
/// `LayeredCache::with_l2_tier`, so `TileCache::l2_tier()` can tell a
/// deployment that configured no L2 (`None`, unchanged) apart from one whose
/// configured L2 never connected (`Some`, `NeverConnected`). The declaration
/// is metadata only — the unreachable case still composes exactly the
/// L1-only serving stack it composed before, with no L2 layer, so no request
/// pays an extra hop and no write spawns a doomed background task.
async fn build_cache(config: &CacheConfig) -> anyhow::Result<Arc<dyn TileCache>> {
    let l1 = Arc::new(MokaTileCache::from_memory_percent(config.memory_percent));
    match &config.l2 {
        L2CacheConfig::None => Ok(Arc::new(MetricsTileCache::new(l1))),
        L2CacheConfig::Valkey { url_env, ttl_s } => {
            #[cfg(feature = "valkey")]
            {
                if std::env::var(url_env).is_err() {
                    anyhow::bail!(
                        "cache.l2: url_env names '{url_env}', but that environment variable is not set"
                    );
                }
                // Two ways the backend can fail to arrive, one outcome: the
                // tier is declared as never-connected, boot continues, and
                // readiness reports it by name. See `L2_CONNECT_DEADLINE`
                // for why the deadline is not optional.
                let reason = match tokio::time::timeout(
                    L2_CONNECT_DEADLINE,
                    ValkeyL2Cache::connect(url_env),
                )
                .await
                {
                    Ok(Ok(backend)) => {
                        let backend: Arc<dyn L2Cache> = Arc::new(backend);
                        let l2 = Arc::new(L2CacheAdapter::new(
                            Arc::clone(&backend),
                            Duration::from_secs(*ttl_s),
                        ));
                        let layered = LayeredCache::with_l2_tier(
                            vec![l1 as Arc<dyn TileCache>, l2 as Arc<dyn TileCache>],
                            L2Tier::connected("valkey", backend),
                        );
                        return Ok(Arc::new(MetricsTileCache::new(Arc::new(layered))));
                    }
                    Ok(Err(err)) => err.to_string(),
                    Err(_) => format!(
                        "cache.l2: valkey backend did not connect within {}s",
                        L2_CONNECT_DEADLINE.as_secs()
                    ),
                };
                tracing::error!(
                    error = %reason,
                    "cache.l2: valkey backend unreachable at boot; serving L1-only until restart"
                );
                let layered = LayeredCache::with_l2_tier(
                    vec![l1 as Arc<dyn TileCache>],
                    L2Tier::never_connected("valkey", reason),
                );
                Ok(Arc::new(MetricsTileCache::new(Arc::new(layered))))
            }
            #[cfg(not(feature = "valkey"))]
            {
                let _ = (l1, url_env, ttl_s);
                anyhow::bail!(
                    "cache.l2 selects the valkey backend, but this binary was built without the `valkey` feature"
                );
            }
        }
    }
}

fn init_tracing(json: bool) {
    let build_filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
    };
    // `#189`: both shapes emit through the redacting writer, so a DSN that
    // reaches a rendered error chain is scrubbed at the last boundary before
    // stdout regardless of the output format.
    if json {
        tracing_subscriber::fmt()
            .with_env_filter(build_filter())
            .with_writer(log_redact::RedactingStdout)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(build_filter())
            .with_writer(log_redact::RedactingStdout)
            .init();
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let public_demo_only = cli.public_demo_only;
    let path = config_path(cli.config, env::var("TELLURION_CONFIG").ok());

    if public_demo_only {
        #[cfg(all(feature = "public-demo", feature = "ui"))]
        {
            return run_public_demo(path).await;
        }
        #[cfg(not(all(feature = "public-demo", feature = "ui")))]
        {
            anyhow::bail!(
                "--public-demo-only requires a binary built with the `public-demo` and `ui` features"
            );
        }
    }

    let raw_envelope = std::fs::read_to_string(&path);
    let parsed_envelope = raw_envelope
        .as_ref()
        .map_err(|error| anyhow::anyhow!("reading '{path}': {error}"))
        .and_then(|raw| {
            serde_yaml::from_str::<BootEnvelope>(raw)
                .map_err(|error| anyhow::anyhow!("parsing boot envelope '{path}': {error}"))
        });

    // The json-log toggle lives inside the config we're in the middle of
    // loading; fall back to plain-text so a load failure can still be
    // logged legibly right after this.
    init_tracing(
        parsed_envelope
            .as_ref()
            .map(|envelope| envelope.seed.server.log_json)
            .unwrap_or(false),
    );

    let envelope = parsed_envelope.map_err(|err| {
        tracing::error!(error = %err, path = %path, "failed to load configuration");
        anyhow::anyhow!("failed to load configuration from '{path}': {err}")
    })?;
    if matches!(envelope.control_store, ControlStoreLocator::LegacyFile) {
        envelope.validate()?;
    } else {
        // Durable stores decide under their own initialization lock whether
        // seed semantics must be validated or ignored as restart drift.
        envelope.validate_locator()?;
    }

    // `#215`: `initial_path_policies` rides alongside `initial_role_bindings`
    // and is empty for the legacy file backend, which `BootEnvelope::validate`
    // has already refused to let declare any — see its own doc for why
    // honouring them there would be a grant the first reload silently
    // withdrew.
    let (
        mut config,
        config_version,
        legacy_mode,
        dynamic_control_store,
        initial_role_bindings,
        initial_path_policies,
    ) = match &envelope.control_store {
        ControlStoreLocator::LegacyFile => {
            let versioned = FileConfigStore::new(&path).load_versioned()?;
            (
                versioned.config,
                versioned.version,
                true,
                None,
                Vec::new(),
                Vec::new(),
            )
        }
        _ => {
            let opened = control_bootstrap::open_and_initialize(&envelope).await?;
            match &opened.startup.seed_status {
                SeedStatus::Drift { changed_sections } => tracing::warn!(
                    revision = opened.startup.authoritative.revision,
                    changed_sections = ?changed_sections,
                    "control-store seed drift detected; durable state remains authoritative"
                ),
                status => tracing::info!(
                    revision = opened.startup.authoritative.revision,
                    ?status,
                    "dynamic control store initialized"
                ),
            }
            let version = ConfigVersion::from_wire(format!(
                "control-revision-{}",
                opened.startup.authoritative.revision
            ));
            let poll_interval = Duration::from_millis(
                envelope
                    .control_store
                    .poll_interval_ms()
                    .expect("dynamic locator"),
            );
            (
                opened.startup.authoritative.snapshot.config,
                version,
                false,
                Some((
                    opened.store,
                    opened.startup.authoritative.revision,
                    poll_interval,
                )),
                opened.startup.authoritative.snapshot.role_bindings,
                opened.startup.authoritative.snapshot.path_policies,
            )
        }
    };

    tracing::info!(
        backend = match &envelope.control_store {
            ControlStoreLocator::LegacyFile => "legacy_file",
            ControlStoreLocator::Sqlite { .. } => "sqlite",
            ControlStoreLocator::Postgres { .. } => "postgres",
        },
        "extension registry: config store"
    );

    runtime_activation::apply_process_overrides(&mut config);

    let control_browser_client_secret =
        resolve_control_browser_secret_from(config.auth.browser.as_ref(), |name| {
            env::var(name).ok()
        })?;
    let control_browser_boot = config.auth.browser.clone().map(|browser| {
        let issuers = config
            .auth
            .oidc
            .iter()
            .chain(config.auth.trusted_issuers.iter())
            .cloned()
            .collect::<Vec<_>>();
        (browser, control_browser_client_secret, issuers)
    });

    let port = config.server.port;
    let request_timeout_s = config.server.request_timeout_s;
    let drain_timeout = Duration::from_secs(config.server.drain_timeout_s);
    let readiness_probe_interval = Duration::from_secs(config.server.readiness_probe_interval_s);
    let readiness_probe_timeout = Duration::from_secs(config.server.readiness_probe_timeout_s);

    // `mut` is only needed to call `register` below, which every driver
    // feature gates out entirely — unused only when none of them are on.
    #[cfg_attr(
        not(any(
            feature = "postgis",
            feature = "pmtiles",
            feature = "flatgeobuf",
            feature = "geoparquet",
            feature = "shapefile",
            feature = "cog",
            feature = "zarr",
            feature = "iceberg",
            feature = "geopackage",
            feature = "duckdb"
        )),
        allow(unused_mut)
    )]
    let mut driver_registry = Registry::new();
    #[cfg(feature = "postgis")]
    driver_registry.register(Arc::new(PostgisDriverFactory::new(request_timeout_s)));
    #[cfg(feature = "pmtiles")]
    driver_registry.register(Arc::new(PmtilesDriverFactory::new()));
    #[cfg(feature = "flatgeobuf")]
    driver_registry.register(Arc::new(FlatgeobufDriverFactory::new()));
    #[cfg(feature = "geoparquet")]
    driver_registry.register(Arc::new(GeoparquetDriverFactory::new()));
    #[cfg(feature = "shapefile")]
    driver_registry.register(Arc::new(ShapefileDriverFactory::new()));
    #[cfg(feature = "cog")]
    driver_registry.register(Arc::new(CogDriverFactory::new()));
    // `#254`: the bounded COG mosaic driver ships behind the SAME `cog`
    // feature as the single-COG driver it composes — it adds no crate and no
    // capability the `cog` feature did not already pull in, and gating it
    // separately would only invite a deployment that has one and not the
    // other. Registered under its own `cog-mosaic` driver name, so a config
    // still selects between them explicitly.
    #[cfg(feature = "cog")]
    driver_registry.register(Arc::new(MosaicDriverFactory::new()));
    #[cfg(feature = "zarr")]
    driver_registry.register(Arc::new(ZarrDriverFactory::new()));
    #[cfg(feature = "iceberg")]
    driver_registry.register(Arc::new(IcebergDriverFactory::new()));
    #[cfg(feature = "geopackage")]
    driver_registry.register(Arc::new(GeopackageDriverFactory::new()));
    #[cfg(feature = "duckdb")]
    driver_registry.register(Arc::new(DuckdbDriverFactory::new()));
    // `#112`: the storage-driver seam's own boot log line — the names below
    // are exactly what `driver_registry.register` calls above actually ran,
    // in the registry's deterministic (alphabetical) order rather than
    // registration order.
    tracing::info!(
        implementations = ?driver_registry.names().collect::<Vec<_>>(),
        "extension registry: storage drivers"
    );
    // Shared with the config-reload trigger (`reload.rs`, `#47`), which
    // rebuilds a `Router` against this same registry on every reload
    // attempt — drivers are registered once, at boot, never re-registered.
    let driver_registry = Arc::new(driver_registry);

    // The relational registry backends (`#42`, second slice; named and
    // registered by `#162`): every driver crate this binary was compiled with
    // registers exactly one factory here, under its own declared name.
    // Registered here (not inside `driver_registry`, which is about storage
    // drivers, a distinct concern) and reused by both this boot and every
    // reload attempt, the same "built once, consulted every time" treatment
    // `driver_registry` itself gets from `reload::run`.
    #[cfg_attr(not(feature = "postgis"), allow(unused_mut))]
    let mut relational_registry_factories = RelationalRegistryFactories::new();
    #[cfg(feature = "postgis")]
    relational_registry_factories
        .register(Arc::new(PostgisRegistryFactory::new(request_timeout_s)));
    // The relational tenant backends (`#143`) — same "compiled in only when a
    // driver crate provides one, reused by both boot and every reload
    // attempt" treatment the registry factories just above already get, and
    // registered under the SAME names, because a deployment moves its tenant
    // declarations to the same relational store its registry uses, never a
    // second, independently selected backend (see `RegistryConfig`'s own
    // doc).
    #[cfg_attr(not(feature = "postgis"), allow(unused_mut))]
    let mut relational_tenant_factories = RelationalTenantFactories::new();
    #[cfg(feature = "postgis")]
    relational_tenant_factories.register(Arc::new(PostgisTenantFactory::new(request_timeout_s)));
    // `#112`, `#162`: `file` is the direct built-in backend — always compiled
    // in, no factory to name. The relational implementations are exactly what
    // the `register` calls above actually ran, in the registries' own
    // deterministic (alphabetical) order rather than registration order. This
    // is what "the registry backend seam" contains in this binary,
    // independent of which one `config.registry.backend`/`.implementation`
    // happens to select this run.
    tracing::info!(
        builtin = "file",
        relational_registry_implementations =
            ?relational_registry_factories.names().collect::<Vec<_>>(),
        relational_tenant_implementations =
            ?relational_tenant_factories.names().collect::<Vec<_>>(),
        "extension registry: catalog/collection registry backend"
    );
    let relational_registry_factories = Arc::new(relational_registry_factories);
    let relational_tenant_factories = Arc::new(relational_tenant_factories);
    // `registry.backend: relational` unreachable at boot fails boot with a
    // named error — the same "bad config fails fast" contract
    // `Router::validate_catalog` already applies; `file` (the default) never
    // reaches the fallible branch at all (`build_registry_reader`'s own
    // doc). Built before `Router`/`Resolver` now (`#42`, third slice): a
    // relational backend's indexes are walked FROM this reader — see
    // `build_router_and_resolver`.
    let registry_reader = build_registry_reader(&config, &relational_registry_factories)
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "failed to build the registry reader at boot");
            anyhow::anyhow!("failed to build the registry reader: {err}")
        })?;
    // `#143`: the tenant reader's own boot-time build, same "unreachable
    // relational backend fails boot" contract `registry_reader` just above
    // already gets.
    let tenant_reader = build_tenant_reader(&config, &relational_tenant_factories)
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "failed to build the tenant reader at boot");
            anyhow::anyhow!("failed to build the tenant reader: {err}")
        })?;

    // `#42`, third slice: dispatches on `config.registry.backend` — `file`
    // builds straight from `config.tenants`/`.catalogs`/`.collections` (no
    // I/O, `registry_reader`/`tenant_reader` unused); `relational` walks
    // both to exhaustion and validates the result before indexing it, so a
    // collection published to the database routes exactly like one declared
    // in YAML. Builds `Router` and `Resolver` together from the SAME walks —
    // see that function's own doc for why they must never come from
    // independent ones.
    let (router, resolver, tenants) = build_router_and_resolver(
        &config,
        &driver_registry,
        registry_reader.as_ref(),
        tenant_reader.as_ref(),
    )
    .await
    .map_err(|err| {
        tracing::error!(error = %err, "failed to build the router/resolver at boot");
        anyhow::anyhow!("failed to build the router/resolver: {err}")
    })?;
    // `registry.validation: lazy` (`#42`) skips this eager, O(collections)
    // boot sweep entirely — a collection is validated the first time a
    // request resolves it instead, with the verdict cached by `Router`
    // (see `resolved_descriptor`). `eager`, the default, keeps today's
    // behavior: a misconfigured collection fails startup here, before the
    // first request ever arrives.
    if config.registry.validation == RegistryValidationMode::Eager {
        router.validate_catalog().await.map_err(|err| {
            tracing::error!(error = %err, "catalog validation failed at boot");
            anyhow::anyhow!("catalog validation failed: {err}")
        })?;
    }
    // Built alongside `build_router_and_resolver` from the same config, same
    // as `router`/`resolver` (`#17`) — `reload.rs`'s `attempt_reload`
    // rebuilds this the same way on every reload attempt, so an `auth:` edit
    // takes effect with no restart.
    // `#144`: a `bearer_tokens` entry naming a `token_env` that is not set
    // fails boot by name here, the same "bad config fails fast" contract a
    // missing `storages[].url_env` already has — never a server that starts
    // with one principal quietly missing from its authorizer.
    let authorizer =
        build_authorizer_with_bindings(&config.auth, &initial_role_bindings).map_err(|err| {
            tracing::error!(error = %err, "resolving auth.bearer_tokens failed at boot");
            anyhow::anyhow!("resolving auth.bearer_tokens failed: {err}")
        })?;

    // `#112`: `l1-moka` is always present; `l2-valkey` only when this binary
    // was built with the `valkey` feature — not whether `cache.l2` actually
    // selects it this run (`build_cache`'s own doc covers that "set but
    // unreachable degrades, not boots" distinction).
    tracing::info!(
        implementations = ?{
            let mut names = vec!["l1-moka"];
            if cfg!(feature = "valkey") {
                names.push("l2-valkey");
            }
            names
        },
        "extension registry: tile cache tiers"
    );
    // The file-backed style store is not fallible to construct, but its
    // inventory belongs before the cache setup as well: a compiled-out cache
    // backend must still leave the complete extension inventory in its boot
    // diagnostics.
    tracing::info!(implementations = ?["file"], "extension registry: style store");
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&config.styles));
    // `#186`: the cross-protocol link-contributor seam, registered by name
    // at boot like every other `#112` registry above. Contributions are
    // derived per request from whatever `Router` is then current, so a
    // config reload's capability changes flow through with nothing
    // re-registered here. Built (and its inventory logged) BEFORE the
    // fallible cache setup below, for the same reason the style-store
    // inventory is: a boot that fails on a compiled-out cache backend must
    // still leave the complete extension inventory in its diagnostics.
    let mut contributors = LinkContributors::new();
    contributors.register("tiles", Arc::new(link_contributors::TilesLinkContributor));
    // `#37`: the OGC API — Maps `map` link (OGC 20-058 Requirement 46). Its
    // own contributor rather than a branch inside the tiles one, because it
    // is gated on the `routing.maps` lane resolving — a lane an operator can
    // point at a different storage than `routing.tiles`. See
    // `MapsLinkContributor`'s own doc.
    contributors.register("maps", Arc::new(link_contributors::MapsLinkContributor));
    contributors.register(
        "styles",
        Arc::new(link_contributors::StylesLinkContributor::new(Arc::clone(
            &style_store,
        ))),
    );
    // `#220`: the 3D Tiles tileset link. A contributor of its own rather
    // than another branch inside the tiles one, because `3dtiles` is a root
    // of its own with its own `settings.protocols` exposure key — turning
    // it off must silence exactly this link and nothing else.
    contributors.register(
        "3dtiles",
        Arc::new(link_contributors::Places3dLinkContributor),
    );
    // `#112`: this seam's own boot log line — deterministic (alphabetical)
    // order, same as every registry inventory above.
    tracing::info!(
        implementations = ?contributors.names().collect::<Vec<_>>(),
        "extension registry: link contributors"
    );

    let cache = build_cache(&config.cache).await?;
    // `#110`: the writable seam the config-mutation control lane persists
    // through — same path this boot already loaded from, so a mutation's
    // write and this instance's own boot/reload reads are always talking
    // to the same file.
    // `#215`: compiled here, before the context exists, so a pattern that
    // cannot be parsed fails boot by name rather than producing a server
    // whose policy set is quietly missing a statement — the same contract
    // `#144` gave an unset `token_env` a few lines above.
    let control_policy = Arc::new(
        tellurion_core::ControlPolicySet::compile(&initial_role_bindings, &initial_path_policies)
            .map_err(|err| {
            tracing::error!(error = %err, "compiling path policies failed at boot");
            anyhow::anyhow!("compiling path policies failed: {err}")
        })?,
    );
    if !control_policy.is_empty() {
        tracing::info!(
            statements = control_policy.statement_count(),
            bindings = control_policy.binding_count(),
            "hierarchical path-scoped administration policy activated"
        );
        if !control_policy.unhonoured_conditions().is_empty() {
            tracing::warn!(
                policies = ?control_policy.unhonoured_conditions(),
                "path policies declare conditions of a kind this build does not implement; \
                 such a statement can deny but can never allow (#215)"
            );
        }
        if !control_policy.roleless_statements().is_empty() {
            tracing::warn!(
                policies = ?control_policy.roleless_statements(),
                "path policies name no role, so no principal can satisfy them; \
                 the paths they match remain default-deny (#215)"
            );
        }
    }
    let mut context = AppContext::new_with_registry_version_and_policy(
        config,
        tenants,
        router,
        resolver,
        authorizer,
        registry_reader,
        cache,
        style_store,
        config_version.clone(),
        control_policy,
        dynamic_control_store
            .as_ref()
            .map(|(_, revision, _)| *revision),
    )
    .with_link_contributors(contributors);
    let control_runtime_status = Arc::new(ControlRuntimeStatus::new(
        dynamic_control_store
            .as_ref()
            .map_or(0, |(_, revision, _)| *revision),
    ));
    context = context.with_control_runtime_status(Arc::clone(&control_runtime_status));
    if legacy_mode {
        let config_store: Arc<dyn ConfigStore> = Arc::new(FileConfigStore::new(&path));
        context = context.with_config_store(config_store);
    }
    if let Some((store, _, _)) = dynamic_control_store.as_ref() {
        context = context.with_control_store(Arc::clone(store));
    }
    let ctx = Arc::new(context);
    let control_browser = control_browser_boot
        .map(|(browser, secret, issuers)| {
            control_browser_auth::ControlBrowserAuth::new(browser, secret, issuers, &ctx)
        })
        .transpose()?;

    let readiness = readiness::Readiness::new();

    // Config-reload trigger (`#47`): SIGHUP or a change under the config
    // file's directory rebuilds and, if it validates, swaps in a new
    // config/router/resolver/registry. Runs for the process lifetime; a
    // failed reload logs and leaves `ctx` exactly as it was.
    if legacy_mode {
        tokio::spawn(reload::run(
            Arc::clone(&ctx),
            PathBuf::from(path.clone()),
            Arc::clone(&driver_registry),
            relational_registry_factories.clone(),
            relational_tenant_factories.clone(),
            readiness.clone(),
        ));
    }
    let prometheus_handle = metrics::install_recorder()?;
    if let Some((store, revision, poll_interval)) = dynamic_control_store {
        tokio::spawn(control_consumer::run_control_consumer(
            control_consumer::ControlConsumerContext::new(
                Arc::clone(&ctx),
                Arc::clone(&driver_registry),
                relational_registry_factories,
                relational_tenant_factories,
                readiness.clone(),
            ),
            store,
            poll_interval,
            revision,
            control_runtime_status,
        ));
    }
    metrics::spawn_rss_sampler(RSS_SAMPLE_INTERVAL);
    // `#110`: the per-instance config-version gauge, set once at boot and
    // again on every successful reload (`reload.rs::attempt_reload`) — see
    // `metrics::set_config_version_gauge`'s own doc for why this is a
    // plain numeric fingerprint, not a version-labeled series.
    metrics::set_config_version_gauge(&config_version);

    tokio::spawn(readiness::run(
        Arc::clone(&ctx),
        readiness.clone(),
        readiness_probe_interval,
        readiness_probe_timeout,
    ));

    // Shared shutdown edge (SIGINT/SIGTERM): the readiness state, HTTP
    // server, index applier tasks, the tile-generation invalidation
    // consumer tasks, webhook delivery tasks, and outbox retention tasks all
    // react to this one process edge.
    let shutdown_rx = shutdown::watch_signal();
    let applier_handles = applier::spawn_all(&ctx, shutdown_rx.clone()).await;
    // `#113`: wires the store into `ctx` before the app starts serving, so
    // the very first request's generation lookup already sees it — an
    // `enabled: false` config (the default) returns `GenerationStore::
    // empty()` here, byte-for-byte identical to no store existing at all.
    let (generation_store, generation_handles) =
        generation_consumer::spawn_all(&ctx, shutdown_rx.clone()).await;
    ctx.set_generations(Arc::clone(&generation_store));
    // `#115`: webhook delivery is spawned before the retention floor task,
    // which needs the registry it returns to fold each subscription's own
    // cursor into the floor computation.
    let (webhook_registry, webhook_handles) =
        webhook_consumer::spawn_all(&ctx, shutdown_rx.clone()).await;
    let retention_handles = retention_consumer::spawn_all(
        &ctx,
        generation_store,
        Arc::clone(&webhook_registry),
        shutdown_rx.clone(),
    )
    .await;
    // `#182`: assembled once here and shared between the HTTP root and the
    // runner loop, so the two can never disagree about which processes exist
    // or where jobs are recorded. `None` — no `server.processes` block, no
    // ledger capability, or no registered runner — means no runner is spawned
    // AND no Processes root is served.
    let process_lane = process_lane::build(&ctx);
    let process_handles = process_lane::spawn(&ctx, process_lane.clone(), shutdown_rx.clone());
    let background_handles: Vec<_> = applier_handles
        .into_iter()
        .chain(generation_handles)
        .chain(webhook_handles)
        .chain(retention_handles)
        .chain(process_handles)
        .collect();
    let applier_supervisor = tokio::spawn(shutdown::supervise_tasks(
        background_handles,
        shutdown_rx.clone(),
        drain_timeout,
    ));

    let app = app::build_with_webhook_registry_and_control_browser(
        ctx,
        prometheus_handle,
        request_timeout_s,
        readiness.clone(),
        webhook_registry,
        process_lane,
        control_browser,
    );

    let bind_addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    // Log the address the OS actually bound (not the requested one) so a
    // `port: 0` request — an ephemeral port, e.g. for tests — resolves to
    // something a caller can parse out of this line rather than "port 0".
    let addr = listener.local_addr()?;
    tracing::info!(%addr, config_path = %path, %config_version, "tellurion listening");

    shutdown::serve_until(
        listener,
        app,
        readiness,
        drain_timeout,
        shutdown::wait_for_shutdown(shutdown_rx),
    )
    .await?;
    applier_supervisor.await?;

    Ok(())
}

#[cfg(all(feature = "public-demo", feature = "ui"))]
async fn run_public_demo(path: String) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("reading '{path}': {error}"));
    let parsed = raw
        .as_deref()
        .map_err(|error| anyhow::anyhow!(error.to_string()))
        .and_then(|raw| {
            parse_public_demo_config(raw)
                .map_err(|error| anyhow::anyhow!("parsing public demo config '{path}': {error}"))
        });
    init_tracing(
        parsed
            .as_ref()
            .map(|config| config.server.log_json)
            .unwrap_or(false),
    );
    let mut config = parsed.map_err(|error| {
        tracing::error!(error = %error, path = %path, "failed to load public demo configuration");
        error
    })?;
    config.server.port = public_demo_port(config.server.port, env::var("PORT").ok())?;
    runtime_activation::apply_process_overrides(&mut config);

    let port = config.server.port;
    let request_timeout_s = config.server.request_timeout_s;
    let drain_timeout = Duration::from_secs(config.server.drain_timeout_s);
    let readiness_probe_interval = Duration::from_secs(config.server.readiness_probe_interval_s);
    let readiness_probe_timeout = Duration::from_secs(config.server.readiness_probe_timeout_s);

    let storage_registry = Registry::new();
    let router = CoreRouter::build(&config, &storage_registry)?;
    let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
    let cache: Arc<dyn TileCache> = Arc::new(MetricsTileCache::new(Arc::new(
        MokaTileCache::from_memory_percent(config.cache.memory_percent),
    )));
    let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
    let ctx = Arc::new(AppContext::new(
        config,
        router,
        resolver,
        None,
        cache,
        style_store,
    ));

    let _prometheus_handle = metrics::install_recorder()?;
    metrics::spawn_rss_sampler(RSS_SAMPLE_INTERVAL);
    let readiness = readiness::Readiness::new();
    tokio::spawn(readiness::run(
        Arc::clone(&ctx),
        readiness.clone(),
        readiness_probe_interval,
        readiness_probe_timeout,
    ));

    let shutdown_rx = shutdown::watch_signal();
    let app = app::build_public_demo(ctx, request_timeout_s, readiness.clone());
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port))).await?;
    let addr = listener.local_addr()?;
    tracing::info!(%addr, config_path = %path, "Tellurion public demo listening");

    shutdown::serve_until(
        listener,
        app,
        readiness,
        drain_timeout,
        shutdown::wait_for_shutdown(shutdown_rx),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_path_wins_over_env() {
        assert_eq!(
            config_path(Some("a.yaml".to_string()), Some("b.yaml".to_string())),
            "a.yaml"
        );
    }

    #[test]
    fn env_path_wins_when_cli_is_absent() {
        assert_eq!(config_path(None, Some("b.yaml".to_string())), "b.yaml");
    }

    #[test]
    fn falls_back_to_the_default_path() {
        assert_eq!(config_path(None, None), DEFAULT_CONFIG_PATH);
    }

    #[test]
    fn browser_client_secret_is_resolved_by_variable_name_without_entering_config() {
        let config: tellurion_core::AppConfig = serde_yaml::from_str(
            r#"
auth:
  browser:
    issuer: https://id.example.com
    client_id: control-ui
    client_secret_env: CONTROL_BROWSER_SECRET
    public_origin: https://control.example.com
"#,
        )
        .unwrap();
        let browser = config.auth.browser.as_ref().unwrap();

        let secret = resolve_control_browser_secret_from(Some(browser), |name| {
            (name == "CONTROL_BROWSER_SECRET").then(|| "resolved-secret-value".to_string())
        })
        .unwrap();
        assert_eq!(secret.as_deref(), Some("resolved-secret-value"));
        assert!(!format!("{config:?}").contains("resolved-secret-value"));

        let error = resolve_control_browser_secret_from(Some(browser), |_| None).unwrap_err();
        assert!(error.to_string().contains("CONTROL_BROWSER_SECRET"));
        assert!(!error.to_string().contains("resolved-secret-value"));
    }

    #[test]
    fn public_demo_only_is_an_explicit_boot_mode() {
        let ordinary = Cli::try_parse_from(["tellurion"]).unwrap();
        assert!(!ordinary.public_demo_only);

        let demo = Cli::try_parse_from(["tellurion", "--public-demo-only"]).unwrap();
        assert!(demo.public_demo_only);
    }

    #[cfg(all(feature = "public-demo", feature = "ui"))]
    #[test]
    fn public_demo_config_is_fail_closed_and_stateless() {
        let config = parse_public_demo_config(
            r#"
server:
  port: 9000
  request_timeout_s: 30
  log_json: true
  max_concurrency: 64
cache:
  memory_percent: 10.0
"#,
        )
        .unwrap();
        assert_eq!(config.server.port, 9000);
        assert!(config.storages.is_empty());
        assert!(config.tenants.is_empty());

        for forbidden in [
            "storages: []",
            "control_store: { backend: sqlite, path: /tmp/control.db }",
            "server: { processes: {} }",
            "cache: { l2: { backend: valkey, url_env: VALKEY_URL } }",
        ] {
            assert!(
                parse_public_demo_config(forbidden).is_err(),
                "public demo config unexpectedly accepted: {forbidden}"
            );
        }
    }

    #[cfg(all(feature = "public-demo", feature = "ui"))]
    #[test]
    fn public_demo_render_port_override_is_strict() {
        assert_eq!(public_demo_port(8080, None).unwrap(), 8080);
        assert_eq!(
            public_demo_port(8080, Some("10000".to_owned())).unwrap(),
            10000
        );
        assert!(public_demo_port(8080, Some("0".to_owned())).is_err());
        assert!(public_demo_port(8080, Some("not-a-port".to_owned())).is_err());
    }

    /// Resolves a path relative to the workspace root regardless of the test
    /// binary's own working directory (`cargo test` runs with the crate
    /// directory as cwd, not the workspace root).
    fn workspace_path(relative: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative)
    }

    /// Guards `config/example.yaml` and `config/e2e.yaml` against drifting
    /// out of sync with `AppConfig`: both must still parse, validate, and
    /// (per the 3D-places milestone) declare at least one `places3d`
    /// collection and a non-empty `styles` list whose referenced documents
    /// are real, parseable MapLibre Style JSON.
    fn assert_reference_config_has_3d_places(relative_path: &str) {
        let config = FileConfigStore::new(workspace_path(relative_path))
            .load()
            .unwrap_or_else(|err| panic!("{relative_path} failed to load: {err}"));

        assert!(
            config
                .collections
                .iter()
                .any(|collection| collection.places3d.is_some()),
            "{relative_path} should declare places3d on at least one collection"
        );
        assert!(
            !config.styles.is_empty(),
            "{relative_path} should declare at least one style"
        );

        for style in &config.styles {
            let style_path = workspace_path(&style.path);
            let contents = std::fs::read_to_string(&style_path).unwrap_or_else(|err| {
                panic!(
                    "style '{}' path '{}' should be readable: {err}",
                    style.id,
                    style_path.display()
                )
            });
            let document: serde_json::Value = serde_json::from_str(&contents)
                .unwrap_or_else(|err| panic!("style '{}' should be valid JSON: {err}", style.id));
            assert!(
                document.get("layers").and_then(|v| v.as_array()).is_some(),
                "style '{}' should declare a layers array",
                style.id
            );
        }
    }

    #[test]
    fn example_config_declares_places3d_and_a_loadable_style() {
        assert_reference_config_has_3d_places("config/example.yaml");
    }

    #[test]
    fn e2e_config_declares_places3d_and_a_loadable_style() {
        assert_reference_config_has_3d_places("config/e2e.yaml");
    }

    #[test]
    fn deployment_probes_and_grace_follow_the_server_contract() {
        let dockerfile = std::fs::read_to_string(workspace_path("docker/Dockerfile"))
            .expect("docker/Dockerfile should be readable");
        let healthcheck = dockerfile
            .lines()
            .find(|line| line.trim_start().starts_with("CMD curl "))
            .expect("the image should declare a curl health-check command");
        assert_eq!(
            healthcheck.trim(),
            "CMD curl -fsS http://localhost:8080/healthz || exit 1",
            "the image health check should use exactly the dependency-free liveness route"
        );

        let manifest = std::fs::read_to_string(workspace_path("deploy/k8s/base/deployment.yaml"))
            .expect("the base Kubernetes deployment should be readable");
        let deployment: serde_yaml::Value =
            serde_yaml::from_str(&manifest).expect("the base deployment should be valid YAML");
        let pod_spec = &deployment["spec"]["template"]["spec"];
        let container = &pod_spec["containers"][0];

        assert_eq!(
            container["livenessProbe"]["httpGet"]["path"],
            serde_yaml::Value::String("/healthz".to_string())
        );
        assert_eq!(
            container["readinessProbe"]["httpGet"]["path"],
            serde_yaml::Value::String("/readyz".to_string())
        );

        let default_drain = tellurion_core::ServerConfig::default().drain_timeout_s;
        assert_eq!(
            pod_spec["terminationGracePeriodSeconds"].as_u64(),
            Some(default_drain + 5),
            "Kubernetes should leave five seconds beyond the default drain deadline"
        );
    }
}
