//! Triggers `AppContext::reload` (`#39`) from a running process — until this
//! module existed, nothing ever called it, so a config edit meant a restart
//! (`#47`).
//!
//! Two triggers feed one debounced pipeline:
//!
//! - `SIGHUP` — the classic operator contract (`kill -HUP <pid>`).
//! - A filesystem watch on the config file's *parent directory*, not the
//!   file itself. A mounted Kubernetes ConfigMap update doesn't write the
//!   file in place; kubelet stages the new content under a fresh
//!   `..data_<timestamp>` directory and repoints the `..data` symlink (and
//!   the file's own symlink) at it — the file's inode never changes, so a
//!   watch on the inode would silently stop seeing updates the moment the
//!   first symlink swap happens. Watching the directory catches every one of
//!   those swaps, plus a plain in-place edit from a local editor.
//!
//! Both triggers land on the same channel and are debounced together
//! ([`DEBOUNCE_WINDOW`]): an editor's save (often a temp-file-then-rename)
//! and a kubelet symlink swap (two renames) each fire more than one
//! filesystem event per logical change, and there's no reason a `SIGHUP`
//! arriving mid-burst should cause a second reload a moment later.
//!
//! A trigger firing only ever *attempts* a reload — [`attempt_reload`] loads
//! the file from disk, rebuilds `Router` + `Resolver` against it, and
//! validates fully (referential integrity, boot-time driver checks) before
//! calling [`AppContext::reload`]. Any failure along that path is logged and
//! the previous, still-valid state keeps serving: a bad edit must never take
//! the server down. The tile cache and style store are untouched here, same
//! as every other reload path — see `AppContext`'s own doc for why.
//!
//! **An unchanged document is never activated (`#260`).** The watch is on a
//! directory and filters nothing (see [`install_file_watch_trigger`]), so
//! every write to any sibling file — a `server.log` the process itself is
//! appending to, an editor's swap file, a kubelet restaging byte-identical
//! ConfigMap content — arrives here as a reload attempt. Activating on one
//! of those is not free: `Readiness::reload_and_invalidate` resets the
//! readiness probe generation, so each activation drops `/readyz` to 503
//! until the next probe lands, and a directory that churns keeps an
//! otherwise healthy instance out of load-balancer rotation. So
//! [`attempt_reload`] compares the `ConfigVersion` it just read — the
//! SHA-256 of the file's raw bytes that `ConfigStore::load_versioned`
//! already computes, not a second notion of document identity — against
//! the version the live `AppContext` is serving, and declines to activate
//! when they are equal. This changes behaviour only where nothing changed.
//!
//! "Nothing changed" is a claim about the whole input to an activation, not
//! just about the file, so the guard applies under `registry.backend: file`
//! and only there. Under `file`, `build_registry_reader` and
//! `build_tenant_reader` are pure functions of the document
//! (`FileRegistryReader::build`, `FileTenantReader::build` — no I/O), so
//! identical bytes can only rebuild an identical router, resolver and
//! tenant snapshot. Under `relational` they are not: both connect to and
//! walk an external store, and a reload against an unchanged file is
//! exactly how an operator forces that re-read (`SIGHUP` with no edit —
//! see `manual_reload_rereads_relational_tenants_and_swaps_the_server_
//! snapshot`). A relational deployment therefore still activates on every
//! trigger, and still flaps readiness if its config directory churns;
//! fixing that needs a comparison of the REBUILT state rather than of the
//! document, which is a different change from this one.
//!
//! The one real cost, stated rather than left to be discovered: `touch
//! config.yaml` no longer forces a recycle. It never meant "recycle" — it
//! meant "re-run activation", which happened to rebuild the router and
//! reset readiness as a side effect — but operators do use it that way. To
//! actually re-activate, change the document; to actually recycle the
//! process, restart it. A declined activation is never silent: it logs at
//! `INFO` naming the path and the version, and increments
//! `tellurion_config_reload_skipped_unchanged_total`.
//!
//! The guard lives here, in the file/`SIGHUP` lane, and deliberately not in
//! `runtime_activation::activate_config` where the dynamic control lane
//! would also pick it up. Two reasons, both from the code: that lane's
//! version token is `control-revision-<n>`, a revision label rather than a
//! digest of anything, so equality there would be answering a different
//! question; and its candidate carries `role_bindings` and `path_policies`
//! that no config-document digest covers, so skipping on that digest could
//! drop a policy change. It needs no such guard in any case —
//! `control_consumer::refresh_once` already returns `NoChange` without
//! activating whenever the store's revision has not advanced past the
//! applied one.
//!
//! **Change propagation and its staleness bound (`#110`).** This is the ONE
//! propagation path a config-mutation write also rides: the mutation
//! control lane (`config_mutation.rs`) never touches a live `AppContext`
//! directly — it only asks `ConfigStore::write` to persist a validated
//! document to the same file this pipeline already watches, so a mutation
//! and a hand edit converge through the exact same trigger, debounce, and
//! validate-then-swap steps. A per-instance config-version gauge
//! (`metrics::set_config_version_gauge`, set at boot and on every
//! successful swap below) makes convergence *measurable*: compare an
//! instance's gauge value against the fingerprint of the version a write
//! response reported.
//!
//! The documented bound, not a vibe: under normal operation, a change is
//! visible on an instance within [`DEBOUNCE_WINDOW`] (250ms) plus one
//! filesystem-notification delivery (inotify/kqueue; single-digit
//! milliseconds on every platform this project targets) plus this
//! function's own processing time (a parse, a referential-integrity walk,
//! and — under `registry.validation: eager`, the default — one boot-style
//! catalog sweep; low milliseconds for a config sized like this project's
//! own examples). This module's own tests (`a_valid_edit_is_visible_on_the_
//! next_request...`, `a_valid_config_mutation_propagates_through_the_real_
//! reload_pipeline`) bound total observed convergence at 5 seconds — a
//! deliberately generous ceiling for shared, possibly loaded CI hardware,
//! not the expected steady-state latency (low hundreds of milliseconds).
//! `SIGHUP` skips the filesystem-notification term entirely. Neither
//! trigger is available at all is the one unbounded case this module can't
//! paper over — see [`run`]'s own doc for that degraded (restart-only)
//! state.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tellurion_core::{
    AppContext, ConfigStore, FileConfigStore, Registry, RegistryBackend,
    RelationalRegistryFactories, RelationalTenantFactories,
};
use tokio::sync::mpsc;

use crate::readiness::Readiness;

/// How long to wait for the filesystem/signal traffic to go quiet before
/// actually reloading, once the first trigger of a burst arrives. Long
/// enough to coalesce an editor's save (write + rename is common) or a
/// kubelet ConfigMap symlink swap (two renames) into one reload; short
/// enough that an operator watching `kill -HUP` take effect doesn't notice
/// the wait.
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(250);

/// Where a coalesced reload attempt came from — carried through only for the
/// debug log naming the burst's last trigger; `attempt_reload` itself
/// doesn't care which source fired.
#[derive(Debug)]
enum Trigger {
    Signal,
    FileWatch,
}

/// Installs the `SIGHUP` handler and forwards every signal onto `tx` from a
/// dedicated task. Absent on non-Unix targets (`tokio::signal::unix` doesn't
/// exist there) — the file watch is still available, same "degrade, don't
/// refuse to boot" stance the rest of this module takes toward a missing
/// trigger.
#[cfg(unix)]
fn install_sighup_trigger(tx: mpsc::UnboundedSender<Trigger>) -> bool {
    match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
        Ok(mut sighup) => {
            tokio::spawn(async move {
                loop {
                    sighup.recv().await;
                    if tx.send(Trigger::Signal).is_err() {
                        // The main reload loop exited (channel closed) — no
                        // one left to hand a signal to.
                        return;
                    }
                }
            });
            true
        }
        Err(err) => {
            tracing::error!(
                error = %err,
                "config reload: failed to install the SIGHUP handler; the signal trigger is disabled for this run"
            );
            false
        }
    }
}

#[cfg(not(unix))]
fn install_sighup_trigger(_tx: mpsc::UnboundedSender<Trigger>) -> bool {
    false
}

/// Watches `config_path`'s parent directory (see the module doc for why the
/// directory, not the file) and forwards a [`Trigger::FileWatch`] for every
/// filesystem event `notify` reports there — no filtering by filename or
/// event kind, since debouncing already absorbs the burst a real change
/// produces and this callback cannot tell a real change from a sibling
/// file's write without re-reading the document anyway. That unfiltered
/// delivery is precisely why [`attempt_reload`] declines to activate a
/// document identical to the one already serving (`#260`): the extra
/// *attempt* is cheap, but the activation it used to end in was not — it
/// reset the readiness probe generation every time.
/// Returns the watcher itself, which the caller must keep
/// alive for as long as watching should continue — dropping it stops
/// delivery.
fn install_file_watch_trigger(
    config_path: &Path,
    tx: mpsc::UnboundedSender<Trigger>,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::Watcher;

    // `Path::parent()` on a bare filename (`"config.yaml"`, no directory
    // component) returns `Some("")`, not `None` — normalize that to `.` so
    // `watch` gets a real, watchable directory either way.
    let parent = match config_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };

    let mut watcher =
        notify::recommended_watcher(move |result: notify::Result<notify::Event>| match result {
            Ok(_event) => {
                let _ = tx.send(Trigger::FileWatch);
            }
            Err(err) => {
                tracing::warn!(error = %err, "config reload: file watch reported an error");
            }
        })?;
    watcher.watch(&parent, notify::RecursiveMode::NonRecursive)?;
    tracing::info!(dir = %parent.display(), "config reload: watching for config changes");
    Ok(watcher)
}

/// Loads `config_path` from disk, rebuilds `RegistryReader` + `Router` +
/// `Resolver` against it, and validates fully — `ConfigStore::load`'s
/// `AppConfig::validate` (referential integrity: unique ids at their scope,
/// resolvable references, reserved tenant segments, sane zoom ranges) plus,
/// when `registry.backend: relational` (`#42`, second slice) — a real
/// connection attempt via `build_registry_reader`, plus, still under
/// `relational`, a walk of that reader and the same referential-integrity
/// check against its result (`build_router_and_resolver`, `#42` third
/// slice) — plus, under `registry.validation: eager` (the default), `Router::
/// validate_catalog` (driver boot validation: every configured collection's
/// physical target actually exists, every explicitly routed lane's
/// capability is really supported) — the exact same steps `main` takes at
/// startup, in the exact same order (the registry reader, and therefore the
/// router, must exist before anything downstream of either can). Under
/// `lazy`, the catalog sweep is skipped here too; a collection's validity is
/// discovered on its first request after the reload instead. Only on full
/// success does it call [`AppContext::reload_with_registry`]; any failure
/// logs loudly (naming the path and the underlying error) and returns with
/// the previous state untouched — including the previous, still-connected
/// registry reader — so a bad edit, or a relational registry that's gone
/// unreachable since the last successful reload (whether at the connection
/// attempt or partway through the walk), degrades to "the reload didn't
/// happen," never to a dead server.
///
/// Before any of that rebuilding, one comparison (`#260`): if the
/// `ConfigVersion` the versioned read just produced equals the one the live
/// `AppContext` is already serving — and the deployment is on
/// `registry.backend: file`, where the document is the whole input to an
/// activation rather than one of two — then the running state is
/// byte-for-byte what the document describes and there is nothing to
/// activate, so nothing is activated and this returns after one `INFO`
/// line and one increment of
/// `tellurion_config_reload_skipped_unchanged_total`. See the module doc
/// for why an unconditional activation was actively harmful (it reset the
/// readiness probe generation on every sibling file's write) and for what
/// this costs (`touch config.yaml` no longer recycles).
async fn attempt_reload(
    ctx: &AppContext,
    config_path: &Path,
    registry: &Registry,
    relational_registry_factories: &RelationalRegistryFactories,
    relational_tenant_factories: &RelationalTenantFactories,
    readiness: &Readiness,
) {
    let started = Instant::now();
    let versioned = match FileConfigStore::new(config_path).load_versioned() {
        Ok(versioned) => versioned,
        Err(err) => {
            tracing::error!(
                error = %err,
                path = %config_path.display(),
                "config reload: failed to load or validate the config file; keeping the previous configuration"
            );
            return;
        }
    };
    let config_version = versioned.version;
    let backend = versioned.config.registry.backend;
    // `#260`: nothing changed, so nothing is activated. Compared against the
    // version the live snapshot carries — the same `ConfigVersion` this same
    // versioned read produced on whichever earlier pass installed it, never a
    // second digest computed here. Named, not silent: one log line and one
    // counter, so an operator who edits a file and sees no reload can find
    // out why without reading this source.
    //
    // Conditioned on `registry.backend` because that is what decides whether
    // the document is the WHOLE input to an activation. Under `file` it is:
    // `build_registry_reader`/`build_tenant_reader` return
    // `FileRegistryReader::build(config)`/`FileTenantReader::build(config)`,
    // pure functions of the document with no I/O, so byte-identical bytes
    // rebuild a byte-identical router, resolver and tenant snapshot and the
    // activation could only ever be a no-op that reset readiness. Under
    // `relational` it is not: both readers connect to and walk an external
    // store, and a reload against an unchanged file — `SIGHUP` with no edit
    // — is precisely how an operator forces that re-read. Skipping there
    // would silently withdraw a capability that works today, which is a
    // worse failure than the flap this guard exists to stop. Not a new
    // knob: this reads the backend a deployment already declared.
    if backend == RegistryBackend::File && config_version == ctx.current().config_version {
        crate::metrics::record_reload_skipped_unchanged();
        tracing::info!(
            path = %config_path.display(),
            %config_version,
            "config reload: the document on disk is byte-for-byte identical to the one already \
             serving; not activating (#260). `touch` alone no longer re-activates or recycles \
             — change the document to reload it, or restart the process to recycle it"
        );
        return;
    }
    if let Err(err) = crate::runtime_activation::activate_config(
        ctx,
        crate::runtime_activation::RuntimeCandidate::from(versioned.config),
        config_version.clone(),
        registry,
        relational_registry_factories,
        relational_tenant_factories,
        readiness,
    )
    .await
    {
        tracing::error!(
            error = %err,
            path = %config_path.display(),
            "config reload: candidate activation failed; keeping the previous configuration"
        );
        return;
    }

    tracing::info!(
        path = %config_path.display(),
        %config_version,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "config reload: applied a new configuration"
    );
}
/// Runs the trigger pipeline forever: installs whichever of `SIGHUP`/file
/// watch is available, then loops "wait for a trigger, drain the burst until
/// [`DEBOUNCE_WINDOW`] passes quietly, attempt one reload." Returns (without
/// ever attempting a reload) if neither trigger could be installed — logged
/// as a warning, since the server is then only reconfigurable by restart,
/// which is a degraded-but-not-broken state, same stance every other
/// optional-component failure in this codebase takes.
///
/// Intended to be `tokio::spawn`ed once from `main`, for the process
/// lifetime; `ctx`/`registry`/`relational_registry_factories`/
/// `relational_tenant_factories` are the same handles `main` already built
/// for the initial boot — either registry may be empty when no driver crate
/// providing a relational factory was compiled in, which only matters if
/// `registry.backend` is later edited to `relational` on a running process
/// (a boot with that backend already selected would have failed before
/// `run` was ever spawned). A reload naming an unregistered
/// `registry.implementation` fails the same way: by name, leaving the
/// previous configuration serving.
pub async fn run(
    ctx: Arc<AppContext>,
    config_path: PathBuf,
    registry: Arc<Registry>,
    relational_registry_factories: Arc<RelationalRegistryFactories>,
    relational_tenant_factories: Arc<RelationalTenantFactories>,
    readiness: Readiness,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Trigger>();

    let signal_installed = install_sighup_trigger(tx.clone());
    let watcher = match install_file_watch_trigger(&config_path, tx.clone()) {
        Ok(watcher) => Some(watcher),
        Err(err) => {
            tracing::error!(
                error = %err,
                path = %config_path.display(),
                "config reload: failed to watch the config directory; the file-watch trigger is disabled for this run"
            );
            None
        }
    };
    // This function's own sender: the signal task (if installed) and the
    // watcher's callback each hold their own clone, so dropping this one
    // doesn't close the channel while either is still alive.
    drop(tx);

    if !signal_installed && watcher.is_none() {
        tracing::warn!(
            "config reload: neither SIGHUP nor file-watch could be installed; the running server can only be reconfigured by restart"
        );
        return;
    }

    while let Some(first) = rx.recv().await {
        let mut last = first;
        let mut coalesced = 1u32;
        loop {
            match tokio::time::timeout(DEBOUNCE_WINDOW, rx.recv()).await {
                Ok(Some(next)) => {
                    last = next;
                    coalesced += 1;
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
        tracing::debug!(
            coalesced,
            last_trigger = ?last,
            "config reload: triggering after a quiet debounce window"
        );
        attempt_reload(
            &ctx,
            &config_path,
            &registry,
            &relational_registry_factories,
            &relational_tenant_factories,
            &readiness,
        )
        .await;
    }

    // `rx.recv()` only returns `None` once every sender is gone, i.e. both
    // the signal task exited and the watcher was dropped — nothing left to
    // trigger on, so this task has no reason to keep running either. `watcher`
    // (kept alive up to here so it keeps delivering events for the whole
    // loop above) drops here, at the natural end of scope.
    let _ = watcher;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tellurion_core::{Resolver, Router, StaticResolver};

    use std::future::Future;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use axum::response::Response;
    use bytes::Bytes;
    use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
    use tower::ServiceExt;

    use tellurion_core::{
        build_registry_reader, build_router_and_resolver, build_tenant_reader, CatalogDecl,
        CatalogSource, CollectionDecl, DriverFactory, Error, FileStyleStore, Filter, MokaTileCache,
        Page, PageRequest, PhysicalCollection, RegistryReader, RelationalRegistryFactory,
        RelationalTenantFactory, Result as CoreResult, StorageDecl, StorageDriver, StyleStore,
        TenantDecl, TenantReader, TileCache, TileCoord, TileSource,
    };

    /// A fixed-payload `TileSource` that counts every call and, when built
    /// via [`TestTiles::new_slow`], sleeps before answering — long enough
    /// for a test to observe the call as "in flight" and act while it's
    /// still pending. Shared (via `Arc`) across every `Router` a test builds
    /// from old and new config, the same way `AppContext::reload` shares the
    /// tile cache across a reload: proves a rename is a cache hit against
    /// the SAME backend instance, not merely an equivalent new one.
    struct TestTiles {
        calls: AtomicUsize,
        payload: Bytes,
        sleep_for: Duration,
    }

    impl TestTiles {
        fn new(payload: &'static [u8]) -> Arc<Self> {
            Self::new_slow(payload, Duration::ZERO)
        }

        fn new_slow(payload: &'static [u8], sleep_for: Duration) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                payload: Bytes::from_static(payload),
                sleep_for,
            })
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl TileSource for TestTiles {
        async fn mvt_tile(
            &self,
            _collection: &CollectionDecl,
            _coord: TileCoord,
            _filter: Option<&Filter>,
        ) -> CoreResult<Option<Bytes>> {
            // Counted (and, for the slow variant, slept on) BEFORE
            // answering, so a caller polling `call_count` sees this call as
            // "entered" before its response is available — the signal a
            // test uses to know a request is genuinely in flight.
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.sleep_for.is_zero() {
                tokio::time::sleep(self.sleep_for).await;
            }
            Ok(Some(self.payload.clone()))
        }
    }

    /// Reports exactly one physical collection named `table` — enough for
    /// `Router::validate_catalog`'s table-existence check to pass; every
    /// other `CatalogSource` method (extent, row estimate, ...) keeps the
    /// trait's own "backend can't answer" default.
    struct FixedCatalog {
        table: &'static str,
    }

    #[async_trait::async_trait]
    impl CatalogSource for FixedCatalog {
        async fn collections(&self) -> CoreResult<Vec<PhysicalCollection>> {
            Ok(vec![PhysicalCollection {
                name: self.table.to_string(),
                geometry_column: None,
                primary_key: None,
                srid: None,
                geometry_type: None,
            }])
        }
    }

    struct TestDriver {
        table: &'static str,
        tiles: Arc<TestTiles>,
    }

    impl StorageDriver for TestDriver {
        fn catalog_source(&self) -> Arc<dyn CatalogSource> {
            Arc::new(FixedCatalog { table: self.table })
        }

        fn tile_source(&self) -> Option<Arc<dyn TileSource>> {
            Some(Arc::clone(&self.tiles) as Arc<dyn TileSource>)
        }
    }

    /// Builds a fresh `TestDriver` wrapping the SAME `Arc<TestTiles>` every
    /// time — every `Router::build` call across a reload gets its own driver
    /// instance, but they all count calls on one shared backend, matching
    /// how a real driver factory wraps one real connection pool.
    struct TestFactory {
        name: &'static str,
        table: &'static str,
        tiles: Arc<TestTiles>,
    }

    impl DriverFactory for TestFactory {
        fn name(&self) -> &str {
            self.name
        }

        fn build(&self, _decl: &StorageDecl) -> CoreResult<Arc<dyn StorageDriver>> {
            Ok(Arc::new(TestDriver {
                table: self.table,
                tiles: Arc::clone(&self.tiles),
            }))
        }
    }

    /// A single tenant/catalog/collection config naming `driver_name` as its
    /// one storage's driver and `external_id` as the collection's public
    /// name — everything a test needs to vary between an old and a new
    /// config is these two parameters plus the file's own content.
    fn make_config(external_id: &str, table: &str, driver_name: &str) -> String {
        format!(
            r#"
storages: [ {{ id: main, driver: {driver_name}, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: reload-test-collection-internal
    external_id: {external_id}
    catalog: default
    storage: main
    table: {table}
    geometry: geom
    pk: id
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
"#
        )
    }

    /// A fresh, private directory under the OS temp dir for one test to
    /// write its config file into — private so concurrently running tests
    /// (`cargo test` runs them in parallel) each get their own directory to
    /// watch rather than all sharing (and generating watch noise in) the
    /// bare temp root.
    fn temp_config_dir(label: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "tellurion-reload-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("creates the test's private config directory");
        dir
    }

    async fn build_ctx(config_path: &Path, registry: &Registry) -> Arc<AppContext> {
        let config = FileConfigStore::new(config_path)
            .load()
            .expect("initial config loads and validates");
        let router = Router::build(&config, registry).expect("initial router builds");
        router
            .validate_catalog()
            .await
            .expect("initial catalog validates");
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer = tellurion_core::build_authorizer(&config.auth)
            .expect("no bearer principal in this fixture reads a token_env");
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        Arc::new(AppContext::new(
            config,
            router,
            resolver,
            authorizer,
            cache,
            style_store,
        ))
    }

    fn test_metrics_handle() -> PrometheusHandle {
        PrometheusBuilder::new().build_recorder().handle()
    }

    fn tile_path(tenant: &str, catalog: &str, collection: &str) -> String {
        format!(
            "/{tenant}/tiles/catalogs/{catalog}/collections/{collection}/tiles/WebMercatorQuad/0/0/0"
        )
    }

    async fn get(app: &axum::Router, path: &str) -> Response {
        app.clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// Polls `check` (roughly every 50ms) until it returns `true` or
    /// `timeout` elapses. The reload pipeline's timing (debounce window,
    /// filesystem watch latency) is real, not mocked, so tests wait on
    /// observable behavior instead of a fixed sleep guess.
    async fn wait_until<F, Fut>(timeout: Duration, mut check: F) -> bool
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = bool>,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if check().await {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Gives a freshly spawned `run` task enough time to install its
    /// triggers (the SIGHUP handler, the directory watch) before a test
    /// starts writing config changes it needs that task to observe.
    async fn let_triggers_install() {
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    #[tokio::test]
    async fn successful_reload_invalidates_readiness_until_the_new_generation_is_probed() {
        let dir = temp_config_dir("readiness-successful-reload");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-readiness-success";

        let tiles = TestTiles::new(b"readiness-success-payload");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles,
        }));
        let registry = Arc::new(registry);
        std::fs::write(
            &config_path,
            make_config("before-reload", "demo", driver_name),
        )
        .unwrap();

        let ctx = build_ctx(&config_path, &registry).await;
        let readiness = crate::readiness::Readiness::new();
        let app = crate::app::build_with_readiness(
            Arc::clone(&ctx),
            test_metrics_handle(),
            60,
            readiness.clone(),
        );
        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        assert_eq!(get(&app, "/readyz").await.status(), StatusCode::OK);

        std::fs::write(
            &config_path,
            make_config("after-reload", "demo", driver_name),
        )
        .unwrap();
        attempt_reload(
            &ctx,
            &config_path,
            &registry,
            &RelationalRegistryFactories::new(),
            &RelationalTenantFactories::new(),
            &readiness,
        )
        .await;

        assert_eq!(
            get(&app, "/readyz").await.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "a successful swap must not inherit readiness from the previous generation"
        );
        assert_eq!(
            get(&app, &tile_path("public", "default", "after-reload"))
                .await
                .status(),
            StatusCode::OK,
            "the new configuration must already be serving while readiness awaits its probe"
        );

        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        assert_eq!(get(&app, "/readyz").await.status(), StatusCode::OK);
    }

    /// Builds a context the way `main` really boots a file-backed instance:
    /// through `ConfigStore::load_versioned`, so the live snapshot carries
    /// the SHA-256 of the config file's own raw bytes. `build_ctx` above
    /// cannot be used for the `#260` tests — it goes through
    /// `AppContext::new`, whose version is the re-serialization fallback
    /// `context.rs::derive_config_version` produces, which by construction
    /// never equals a digest of the raw file bytes, so every reload would
    /// look like a change.
    async fn build_ctx_from_versioned_read(
        config_path: &Path,
        registry: &Registry,
    ) -> Arc<AppContext> {
        let versioned = FileConfigStore::new(config_path)
            .load_versioned()
            .expect("initial config loads and validates");
        let config = versioned.config;
        let router = Router::build(&config, registry).expect("initial router builds");
        router
            .validate_catalog()
            .await
            .expect("initial catalog validates");
        let resolver: Arc<dyn Resolver> = Arc::new(StaticResolver::build(&config));
        let authorizer = tellurion_core::build_authorizer(&config.auth)
            .expect("no bearer principal in this fixture reads a token_env");
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let registry_reader: Arc<dyn tellurion_core::RegistryReader> =
            Arc::new(tellurion_core::FileRegistryReader::build(&config));
        let tenants = config.tenants.clone();
        Arc::new(AppContext::new_with_registry_and_version(
            config,
            tenants,
            router,
            resolver,
            authorizer,
            registry_reader,
            cache,
            style_store,
            versioned.version,
        ))
    }

    /// `#260`: the file watch is on the config file's *directory* and
    /// filters nothing, so a sibling file's writes (a `server.log` beside
    /// `config.yaml` is the reported case) deliver reload triggers with the
    /// config document itself untouched. Each such activation used to reset
    /// the readiness probe generation, so `/readyz` answered 503 until the
    /// next probe — a churning directory kept a perfectly healthy instance
    /// flapping out of rotation.
    ///
    /// This is that scenario with the timing removed: five reload attempts
    /// with not one byte written to the config file between them. None may
    /// activate, and every one of them must say so — a log line and the
    /// `tellurion_config_reload_skipped_unchanged_total` counter, asserted
    /// here at exactly five so a silent skip cannot pass as a correct one.
    #[tokio::test]
    async fn an_unchanged_document_is_never_activated_and_never_flaps_readiness() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let skip_metrics = recorder.handle();
        let _recorder_guard = metrics::set_default_local_recorder(&recorder);

        let dir = temp_config_dir("unchanged-digest-skip");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-unchanged-digest-skip";

        let tiles = TestTiles::new(b"unchanged-digest-payload");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles,
        }));
        let registry = Arc::new(registry);
        std::fs::write(&config_path, make_config("unchanged", "demo", driver_name)).unwrap();

        let ctx = build_ctx_from_versioned_read(&config_path, &registry).await;
        let readiness = crate::readiness::Readiness::new();
        let app = crate::app::build_with_readiness(
            Arc::clone(&ctx),
            test_metrics_handle(),
            60,
            readiness.clone(),
        );
        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        assert_eq!(get(&app, "/readyz").await.status(), StatusCode::OK);

        // The config file is not written to anywhere below this line.
        for attempt in 1..=5 {
            attempt_reload(
                &ctx,
                &config_path,
                &registry,
                &RelationalRegistryFactories::new(),
                &RelationalTenantFactories::new(),
                &readiness,
            )
            .await;
            assert_eq!(
                get(&app, "/readyz").await.status(),
                StatusCode::OK,
                "reload attempt {attempt} against a byte-identical document must not invalidate readiness (#260)"
            );
        }

        assert_eq!(
            get(&app, &tile_path("public", "default", "unchanged"))
                .await
                .status(),
            StatusCode::OK,
            "the unchanged configuration must still be serving"
        );

        let rendered = skip_metrics.render();
        assert!(
            rendered.contains("tellurion_config_reload_skipped_unchanged_total 5"),
            "every declined activation must move the counter, not pass silently; got:\n{rendered}"
        );
    }

    /// `#260`'s other half, and the reason the guard is an equality test on
    /// a content digest rather than any coarser check: a document that
    /// really did change must still activate, exactly as before. Inverting
    /// the comparison in `attempt_reload` makes this fail on the readiness
    /// assertion, on the request for the renamed collection, and on the
    /// counter.
    #[tokio::test]
    async fn a_genuinely_changed_document_is_still_activated() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let skip_metrics = recorder.handle();
        let _recorder_guard = metrics::set_default_local_recorder(&recorder);

        let dir = temp_config_dir("changed-digest-activates");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-changed-digest-activates";

        let tiles = TestTiles::new(b"changed-digest-payload");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles,
        }));
        let registry = Arc::new(registry);
        std::fs::write(
            &config_path,
            make_config("before-change", "demo", driver_name),
        )
        .unwrap();

        let ctx = build_ctx_from_versioned_read(&config_path, &registry).await;
        let readiness = crate::readiness::Readiness::new();
        let app = crate::app::build_with_readiness(
            Arc::clone(&ctx),
            test_metrics_handle(),
            60,
            readiness.clone(),
        );
        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        assert_eq!(get(&app, "/readyz").await.status(), StatusCode::OK);

        std::fs::write(
            &config_path,
            make_config("after-change", "demo", driver_name),
        )
        .unwrap();
        attempt_reload(
            &ctx,
            &config_path,
            &registry,
            &RelationalRegistryFactories::new(),
            &RelationalTenantFactories::new(),
            &readiness,
        )
        .await;

        assert_eq!(
            get(&app, "/readyz").await.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "a changed document must still activate, and an activation must still invalidate readiness"
        );
        assert_eq!(
            get(&app, &tile_path("public", "default", "after-change"))
                .await
                .status(),
            StatusCode::OK,
            "the changed configuration must really be serving"
        );

        let rendered = skip_metrics.render();
        assert!(
            !rendered.contains("tellurion_config_reload_skipped_unchanged_total"),
            "a changed document must not be recorded as an unchanged-document skip; got:\n{rendered}"
        );
    }

    /// `#260` as reported, reproduced through the REAL pipeline (`run`, with
    /// its real directory watch and real debounce — not a direct
    /// `attempt_reload` call): a config file living in the same directory as
    /// a file that keeps being written to, which in the report was a
    /// `server.log` beside `config.yaml`. The watch is on the directory and
    /// filters nothing, so every append delivers a reload trigger with the
    /// config document itself untouched.
    ///
    /// The sibling file is appended to every 400ms — deliberately wider than
    /// [`DEBOUNCE_WINDOW`]. Writing faster than the debounce window makes the
    /// pipeline coalesce the entire run into a single attempt and the
    /// scenario never occurs, which is a way to write this test that passes
    /// whether or not the guard exists; a real log file is not considerate
    /// enough to stay inside a 250ms window either.
    ///
    /// `/readyz` is sampled 200 times across the churn, the same shape as
    /// the reported measurement. Readiness is probed once mid-window rather
    /// than continuously, because that is the production ratio: the default
    /// `readiness_probe_interval_s` is 5 and this window is ~4 seconds, so a
    /// real instance gets about one probe in which to recover. Before the
    /// digest guard, every trigger became an activation and every activation
    /// reset the probe generation; the reported run saw 5 activations and
    /// 190 of 200 probes non-200. The requirement here is zero.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_churning_sibling_file_never_flaps_readiness_through_the_real_watch() {
        use std::io::Write as _;

        let dir = temp_config_dir("sibling-churn-readiness-flap");
        let config_path = dir.join("config.yaml");
        let sibling_log_path = dir.join("server.log");
        let driver_name = "reload-test-sibling-churn-flap";

        let tiles = TestTiles::new(b"sibling-churn-payload");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles,
        }));
        let registry = Arc::new(registry);
        std::fs::write(&config_path, make_config("churn", "demo", driver_name)).unwrap();

        let ctx = build_ctx_from_versioned_read(&config_path, &registry).await;
        let readiness = crate::readiness::Readiness::new();
        let app = crate::app::build_with_readiness(
            Arc::clone(&ctx),
            test_metrics_handle(),
            60,
            readiness.clone(),
        );
        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        assert_eq!(get(&app, "/readyz").await.status(), StatusCode::OK);

        tokio::spawn(run(
            Arc::clone(&ctx),
            config_path.clone(),
            Arc::clone(&registry),
            Arc::new(RelationalRegistryFactories::new()),
            Arc::new(RelationalTenantFactories::new()),
            readiness.clone(),
        ));
        let_triggers_install().await;

        // The config document is not written to anywhere below this line —
        // only its neighbour is.
        let mut non_200 = 0usize;
        for sample in 0..200 {
            if sample % 20 == 0 {
                let mut log = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&sibling_log_path)
                    .unwrap();
                writeln!(log, "sample {sample}: a log line beside the config file").unwrap();
                log.sync_all().unwrap();
            }
            if sample == 100 {
                crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
            }
            if get(&app, "/readyz").await.status() != StatusCode::OK {
                non_200 += 1;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert_eq!(
            non_200, 0,
            "a sibling file's writes must not flap readiness: {non_200} of 200 /readyz samples were non-200 while only server.log changed (#260)"
        );
        assert_eq!(
            get(&app, &tile_path("public", "default", "churn"))
                .await
                .status(),
            StatusCode::OK,
            "the unchanged configuration must still be serving throughout"
        );
    }

    #[tokio::test]
    async fn failed_reload_keeps_the_previous_readiness_generation() {
        let dir = temp_config_dir("readiness-failed-reload");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-readiness-failure";

        let tiles = TestTiles::new(b"readiness-failure-payload");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles,
        }));
        let registry = Arc::new(registry);
        std::fs::write(
            &config_path,
            make_config("still-serving", "demo", driver_name),
        )
        .unwrap();

        let ctx = build_ctx(&config_path, &registry).await;
        let readiness = crate::readiness::Readiness::new();
        let app = crate::app::build_with_readiness(
            Arc::clone(&ctx),
            test_metrics_handle(),
            60,
            readiness.clone(),
        );
        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;

        std::fs::write(&config_path, "not a valid Tellurion configuration").unwrap();
        attempt_reload(
            &ctx,
            &config_path,
            &registry,
            &RelationalRegistryFactories::new(),
            &RelationalTenantFactories::new(),
            &readiness,
        )
        .await;

        assert_eq!(get(&app, "/readyz").await.status(), StatusCode::OK);
        assert_eq!(
            get(&app, &tile_path("public", "default", "still-serving"))
                .await
                .status(),
            StatusCode::OK,
            "a failed reload must preserve the old state and its readiness result"
        );
    }

    #[tokio::test]
    async fn successful_reload_cannot_restore_readiness_while_draining() {
        let dir = temp_config_dir("readiness-draining-reload");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-readiness-draining";

        let tiles = TestTiles::new(b"readiness-draining-payload");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles,
        }));
        let registry = Arc::new(registry);
        std::fs::write(
            &config_path,
            make_config("before-drain", "demo", driver_name),
        )
        .unwrap();

        let ctx = build_ctx(&config_path, &registry).await;
        let readiness = crate::readiness::Readiness::new();
        let app = crate::app::build_with_readiness(
            Arc::clone(&ctx),
            test_metrics_handle(),
            60,
            readiness.clone(),
        );
        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;
        readiness.begin_draining();

        std::fs::write(
            &config_path,
            make_config("after-drain", "demo", driver_name),
        )
        .unwrap();
        attempt_reload(
            &ctx,
            &config_path,
            &registry,
            &RelationalRegistryFactories::new(),
            &RelationalTenantFactories::new(),
            &readiness,
        )
        .await;
        crate::readiness::probe_once(&ctx, &readiness, Duration::from_secs(1)).await;

        assert_eq!(
            get(&app, "/readyz").await.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "reload invalidation and later probes must not reverse draining"
        );
    }

    /// Property 1 (`#47`): a valid edit made through a real file-watch
    /// trigger is visible on the next request — and a request already in
    /// flight when the trigger fires completes normally, using the config it
    /// started with, rather than being disrupted by the swap.
    #[tokio::test]
    async fn a_valid_edit_is_visible_on_the_next_request_without_dropping_an_in_flight_one() {
        let dir = temp_config_dir("valid-edit");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-valid-edit";

        let tiles = TestTiles::new_slow(b"old-payload", Duration::from_millis(600));
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles: Arc::clone(&tiles),
        }));
        let registry = Arc::new(registry);

        std::fs::write(&config_path, make_config("old-name", "demo", driver_name)).unwrap();

        let ctx = build_ctx(&config_path, &registry).await;
        let app = crate::app::build(Arc::clone(&ctx), test_metrics_handle(), 60);

        tokio::spawn(run(
            Arc::clone(&ctx),
            config_path.clone(),
            Arc::clone(&registry),
            Arc::new(RelationalRegistryFactories::new()),
            Arc::new(RelationalTenantFactories::new()),
            Readiness::new(),
        ));
        let_triggers_install().await;

        let slow_app = app.clone();
        let path = tile_path("public", "default", "old-name");
        let in_flight = tokio::spawn(async move { get(&slow_app, &path).await });

        // Don't rewrite the config until the slow request has genuinely
        // entered the driver — otherwise there's no guarantee it resolved
        // against the OLD config before the swap.
        wait_until(Duration::from_secs(2), || async { tiles.call_count() >= 1 }).await;

        std::fs::write(&config_path, make_config("new-name", "demo", driver_name)).unwrap();

        let in_flight_response = in_flight.await.unwrap();
        assert_eq!(
            in_flight_response.status(),
            StatusCode::OK,
            "a reload mid-flight must not disrupt a request already resolved against the old config"
        );
        let in_flight_body = to_bytes(in_flight_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            in_flight_body.as_ref(),
            b"old-payload",
            "the in-flight request must complete against the config it started with"
        );

        let new_app = app.clone();
        let reloaded = wait_until(Duration::from_secs(5), move || {
            let app = new_app.clone();
            async move {
                get(&app, &tile_path("public", "default", "new-name"))
                    .await
                    .status()
                    == StatusCode::OK
            }
        })
        .await;
        assert!(
            reloaded,
            "a valid edit should become visible on the next request"
        );

        let old_name_gone = get(&app, &tile_path("public", "default", "old-name")).await;
        assert_eq!(old_name_gone.status(), StatusCode::NOT_FOUND);
    }

    /// Property 2 (`#47`): an edit that fails validation (here, a collection
    /// referencing a catalog that doesn't exist) is rejected, and the
    /// previous, still-valid config keeps serving — a bad edit must never
    /// take the server down.
    #[tokio::test]
    async fn an_invalid_edit_is_rejected_and_the_old_config_keeps_serving() {
        let dir = temp_config_dir("invalid-edit");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-invalid-edit";

        let tiles = TestTiles::new(b"still-old-payload");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles: Arc::clone(&tiles),
        }));
        let registry = Arc::new(registry);

        std::fs::write(
            &config_path,
            make_config("stays-the-same", "demo", driver_name),
        )
        .unwrap();

        let ctx = build_ctx(&config_path, &registry).await;
        let app = crate::app::build(Arc::clone(&ctx), test_metrics_handle(), 60);

        tokio::spawn(run(
            Arc::clone(&ctx),
            config_path.clone(),
            Arc::clone(&registry),
            Arc::new(RelationalRegistryFactories::new()),
            Arc::new(RelationalTenantFactories::new()),
            Readiness::new(),
        ));
        let_triggers_install().await;

        // Referentially broken: `catalog` names a catalog that was never
        // declared — `AppConfig::validate` refuses this at load time.
        let broken = format!(
            r#"
storages: [ {{ id: main, driver: {driver_name}, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: reload-test-collection-internal
    external_id: stays-the-same
    catalog: nonexistent-catalog
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
"#
        );
        std::fs::write(&config_path, broken).unwrap();

        // Give the trigger pipeline plenty of time to notice the change,
        // debounce it, and attempt (and fail) the reload.
        tokio::time::sleep(Duration::from_secs(1)).await;

        let response = get(&app, &tile_path("public", "default", "stays-the-same")).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "an invalid edit must never take the server down"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            body.as_ref(),
            b"still-old-payload",
            "the old config must still be the one serving"
        );
    }

    /// Property 3 (`#47`): the rename-is-a-cache-hit guarantee (`#39`) holds
    /// through a REAL trigger — a file-watch-driven reload, not a direct
    /// `AppContext::reload` call — proving the trigger pipeline itself
    /// preserves the same tile cache and driver instances `AppContext::new`
    /// wired up at boot.
    #[tokio::test]
    async fn renaming_a_collection_is_a_cache_hit_through_a_real_file_watch_trigger() {
        let dir = temp_config_dir("rename-cache-hit");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-rename";

        let tiles = TestTiles::new(b"mvt-bytes-for-real-trigger-rename-test");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles: Arc::clone(&tiles),
        }));
        let registry = Arc::new(registry);

        std::fs::write(&config_path, make_config("demo-old", "demo", driver_name)).unwrap();

        let ctx = build_ctx(&config_path, &registry).await;
        let app = crate::app::build(Arc::clone(&ctx), test_metrics_handle(), 60);

        let before = get(&app, &tile_path("public", "default", "demo-old")).await;
        assert_eq!(before.status(), StatusCode::OK);
        let before_body = to_bytes(before.into_body(), usize::MAX).await.unwrap();
        assert_eq!(tiles.call_count(), 1);

        tokio::spawn(run(
            Arc::clone(&ctx),
            config_path.clone(),
            Arc::clone(&registry),
            Arc::new(RelationalRegistryFactories::new()),
            Arc::new(RelationalTenantFactories::new()),
            Readiness::new(),
        ));
        let_triggers_install().await;

        std::fs::write(&config_path, make_config("demo-new", "demo", driver_name)).unwrap();

        let poll_app = app.clone();
        let reloaded = wait_until(Duration::from_secs(5), move || {
            let app = poll_app.clone();
            async move {
                get(&app, &tile_path("public", "default", "demo-new"))
                    .await
                    .status()
                    == StatusCode::OK
            }
        })
        .await;
        assert!(
            reloaded,
            "the rename should become visible through the real trigger"
        );

        let after = get(&app, &tile_path("public", "default", "demo-new")).await;
        assert_eq!(after.status(), StatusCode::OK);
        let after_body = to_bytes(after.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            before_body, after_body,
            "the renamed collection must serve byte-identical cached content"
        );
        assert_eq!(
            tiles.call_count(),
            1,
            "the driver must not be called again — this must be a cache HIT under the new name"
        );

        let old_name_gone = get(&app, &tile_path("public", "default", "demo-old")).await;
        assert_eq!(old_name_gone.status(), StatusCode::NOT_FOUND);
    }

    /// The `SIGHUP` half of the trigger pipeline: sending the real signal to
    /// this process (the classic operator contract, `kill -HUP <pid>`) picks
    /// up a config already rewritten on disk with no filesystem event
    /// needed to explain the reload.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_sighup_trigger_causes_a_reload_to_be_visible_on_the_next_request() {
        let dir = temp_config_dir("sighup");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-sighup";

        let tiles = TestTiles::new(b"sighup-trigger-payload");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles: Arc::clone(&tiles),
        }));
        let registry = Arc::new(registry);

        std::fs::write(&config_path, make_config("sighup-old", "demo", driver_name)).unwrap();

        let ctx = build_ctx(&config_path, &registry).await;
        let app = crate::app::build(Arc::clone(&ctx), test_metrics_handle(), 60);

        tokio::spawn(run(
            Arc::clone(&ctx),
            config_path.clone(),
            Arc::clone(&registry),
            Arc::new(RelationalRegistryFactories::new()),
            Arc::new(RelationalTenantFactories::new()),
            Readiness::new(),
        ));
        let_triggers_install().await;

        std::fs::write(&config_path, make_config("sighup-new", "demo", driver_name)).unwrap();

        let status = std::process::Command::new("kill")
            .arg("-HUP")
            .arg(std::process::id().to_string())
            .status()
            .expect("sends SIGHUP to this test process");
        assert!(
            status.success(),
            "kill -HUP should succeed against our own pid"
        );

        let poll_app = app.clone();
        let reloaded = wait_until(Duration::from_secs(5), move || {
            let app = poll_app.clone();
            async move {
                get(&app, &tile_path("public", "default", "sighup-new"))
                    .await
                    .status()
                    == StatusCode::OK
            }
        })
        .await;
        assert!(
            reloaded,
            "a SIGHUP should trigger a reload visible on the next request"
        );
    }

    /// Same as [`make_config`], plus an `auth:` section authorizing exactly
    /// `token` for `tenant_id` — used by the `#17` reload test below.
    fn make_auth_config(table: &str, driver_name: &str, tenant_id: &str, token: &str) -> String {
        format!(
            r#"
storages: [ {{ id: main, driver: {driver_name}, url_env: DATABASE_URL }} ]
tenants: [ {{ id: {tenant_id} }} ]
catalogs: [ {{ id: default, tenant: {tenant_id} }} ]
collections:
  - id: reload-auth-test-collection-internal
    catalog: default
    storage: main
    table: {table}
    geometry: geom
    pk: id
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
auth:
  bearer_tokens:
    - token: {token}
      tenants: [{tenant_id}]
"#
        )
    }

    /// Same as [`get`], but with an `Authorization: Bearer <token>` header
    /// when `bearer` is `Some` — the `#17` reload test's own request helper.
    async fn get_with_bearer(app: &axum::Router, path: &str, bearer: Option<&str>) -> Response {
        let mut builder = Request::builder().uri(path);
        if let Some(token) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        app.clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// `#17`: the tenant authorizer is part of the same atomically-swapped
    /// state as `config`/`router`/`resolver` — a config edit that changes
    /// which token authorizes a tenant takes effect through the real reload
    /// trigger pipeline, with no restart, and the OLD token stops working
    /// the moment the new config is live.
    #[tokio::test]
    async fn a_config_reload_picks_up_a_changed_bearer_token_and_the_old_one_stops_working() {
        let dir = temp_config_dir("auth-token-change");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-auth-token-change";
        let tenant_id = "reload-auth-tenant";

        let tiles = TestTiles::new(b"auth-reload-payload");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles: Arc::clone(&tiles),
        }));
        let registry = Arc::new(registry);

        std::fs::write(
            &config_path,
            make_auth_config("demo", driver_name, tenant_id, "old-token"),
        )
        .unwrap();

        let ctx = build_ctx(&config_path, &registry).await;
        let app = crate::app::build(Arc::clone(&ctx), test_metrics_handle(), 60);
        let collection_path = format!(
            "/{tenant_id}/tiles/catalogs/default/collections/reload-auth-test-collection-internal/tiles/WebMercatorQuad/0/0/0"
        );

        let allowed_before = get_with_bearer(&app, &collection_path, Some("old-token")).await;
        assert_eq!(allowed_before.status(), StatusCode::OK);
        let denied_before = get_with_bearer(&app, &collection_path, Some("new-token")).await;
        assert_eq!(denied_before.status(), StatusCode::FORBIDDEN);

        tokio::spawn(run(
            Arc::clone(&ctx),
            config_path.clone(),
            Arc::clone(&registry),
            Arc::new(RelationalRegistryFactories::new()),
            Arc::new(RelationalTenantFactories::new()),
            Readiness::new(),
        ));
        let_triggers_install().await;

        std::fs::write(
            &config_path,
            make_auth_config("demo", driver_name, tenant_id, "new-token"),
        )
        .unwrap();

        let poll_app = app.clone();
        let poll_path = collection_path.clone();
        let reloaded = wait_until(Duration::from_secs(5), move || {
            let app = poll_app.clone();
            let path = poll_path.clone();
            async move {
                get_with_bearer(&app, &path, Some("new-token"))
                    .await
                    .status()
                    == StatusCode::OK
            }
        })
        .await;
        assert!(
            reloaded,
            "the new token should become authorized after reload"
        );

        let old_token_now = get_with_bearer(&app, &collection_path, Some("old-token")).await;
        assert_eq!(
            old_token_now.status(),
            StatusCode::FORBIDDEN,
            "the old token must stop working once the reloaded config no longer grants it"
        );
    }

    // -- relational backend reload semantics (`#42`, third slice) -----------

    /// An in-memory `RegistryReader` standing in for a relational registry's
    /// rows: reports one fixed catalog/collection while `healthy` is `true`,
    /// and fails every call once the test flips it to `false` — simulating a
    /// database that's gone unreachable between one reload attempt and the
    /// next.
    struct FlakyRegistryReader {
        catalog: CatalogDecl,
        collection: CollectionDecl,
        healthy: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl RegistryReader for FlakyRegistryReader {
        async fn catalog(&self, _: &str, _: &str) -> CoreResult<Option<CatalogDecl>> {
            unreachable!("not exercised by this test")
        }
        async fn collection(&self, _: &str, _: &str) -> CoreResult<Option<CollectionDecl>> {
            unreachable!("not exercised by this test")
        }
        async fn list_catalogs(
            &self,
            tenant_internal_id: &str,
            _page: PageRequest,
        ) -> CoreResult<Page<CatalogDecl>> {
            if !self.healthy.load(Ordering::SeqCst) {
                return Err(Error::Storage("registry unreachable".into()));
            }
            let items = if self.catalog.tenant == tenant_internal_id {
                vec![self.catalog.clone()]
            } else {
                vec![]
            };
            Ok(Page { items, next: None })
        }
        async fn list_collections(
            &self,
            catalog_internal_id: &str,
            _page: PageRequest,
        ) -> CoreResult<Page<CollectionDecl>> {
            if !self.healthy.load(Ordering::SeqCst) {
                return Err(Error::Storage("registry unreachable".into()));
            }
            let items = if self.collection.catalog == catalog_internal_id {
                vec![self.collection.clone()]
            } else {
                vec![]
            };
            Ok(Page { items, next: None })
        }
    }

    /// The name both fakes below register under (`#162`). These tests never
    /// write `registry.implementation`, so they also stand as the
    /// backwards-compatibility path: a config that names no implementation
    /// resolves to the sole registered one.
    const RELOAD_TEST_IMPLEMENTATION: &str = "reload-test";

    /// One-entry registries, the shape `main` builds at boot.
    fn registry_factories_of(
        factory: Arc<dyn RelationalRegistryFactory>,
    ) -> Arc<RelationalRegistryFactories> {
        let mut registry = RelationalRegistryFactories::new();
        registry.register(factory);
        Arc::new(registry)
    }

    fn tenant_factories_of(
        factory: Arc<dyn RelationalTenantFactory>,
    ) -> Arc<RelationalTenantFactories> {
        let mut registry = RelationalTenantFactories::new();
        registry.register(factory);
        Arc::new(registry)
    }

    /// Fails `connect` once `reader.healthy` is `false` — the connection-time
    /// failure `build_registry_reader` surfaces before ever reaching
    /// `build_router_and_resolver`'s own walk.
    struct FlakyRelationalFactory {
        reader: Arc<FlakyRegistryReader>,
    }

    struct MutableTenantReader {
        tenant: Arc<Mutex<TenantDecl>>,
    }

    #[async_trait::async_trait]
    impl TenantReader for MutableTenantReader {
        async fn tenant(&self, external_id: &str) -> CoreResult<Option<TenantDecl>> {
            let tenant = self.tenant.lock().unwrap().clone();
            Ok((tenant.external_id() == external_id).then_some(tenant))
        }

        async fn list_tenants(&self, page: PageRequest) -> CoreResult<Page<TenantDecl>> {
            let tenant = self.tenant.lock().unwrap().clone();
            let included = page
                .after
                .as_deref()
                .is_none_or(|after| tenant.external_id() > after);
            Ok(Page {
                items: included.then_some(tenant).into_iter().collect(),
                next: None,
            })
        }
    }

    struct MutableRelationalTenantFactory {
        reader: Arc<MutableTenantReader>,
    }

    #[async_trait::async_trait]
    impl RelationalTenantFactory for MutableRelationalTenantFactory {
        fn name(&self) -> &str {
            RELOAD_TEST_IMPLEMENTATION
        }

        async fn connect(&self, _database_url: &str) -> CoreResult<Arc<dyn TenantReader>> {
            Ok(Arc::clone(&self.reader) as Arc<dyn TenantReader>)
        }
    }

    #[async_trait::async_trait]
    impl RelationalRegistryFactory for FlakyRelationalFactory {
        fn name(&self) -> &str {
            RELOAD_TEST_IMPLEMENTATION
        }

        async fn connect(&self, _database_url: &str) -> CoreResult<Arc<dyn RegistryReader>> {
            if !self.reader.healthy.load(Ordering::SeqCst) {
                return Err(Error::Storage("connection refused".into()));
            }
            Ok(Arc::clone(&self.reader) as Arc<dyn RegistryReader>)
        }
    }

    #[tokio::test]
    async fn manual_reload_rereads_relational_tenants_and_swaps_the_server_snapshot() {
        let dir = temp_config_dir("relational-tenant-reread");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-relational-tenant";
        let env_var = "TELLURION_RELOAD_TEST_RELATIONAL_TENANT_URL";
        unsafe {
            std::env::set_var(env_var, "postgres://example/relational-tenant-reload-test");
        }

        std::fs::write(
            &config_path,
            format!(
                "storages: [ {{ id: main, driver: {driver_name}, url_env: {env_var} }} ]\n\
                 registry: {{ backend: relational, storage: main }}\n"
            ),
        )
        .unwrap();

        let tiles = TestTiles::new(b"relational-tenant-reload-payload");
        let mut driver_registry = Registry::new();
        driver_registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles,
        }));
        let driver_registry = Arc::new(driver_registry);

        let healthy = Arc::new(AtomicBool::new(true));
        let catalog = CatalogDecl {
            id: "relational-tenant-catalog".to_string(),
            external_id: Some("default".to_string()),
            tenant: "database-tenant".to_string(),
            settings: Default::default(),
            visibility: Default::default(),
        };
        let collection: CollectionDecl = serde_yaml::from_str(
            "id: relational-tenant-collection\nexternal_id: demo\ncatalog: relational-tenant-catalog\nstorage: main\ntable: demo\ngeometry: geom\npk: id\n",
        )
        .unwrap();
        let registry_reader = Arc::new(FlakyRegistryReader {
            catalog,
            collection,
            healthy,
        });
        let registry_factory: Arc<dyn RelationalRegistryFactory> =
            Arc::new(FlakyRelationalFactory {
                reader: Arc::clone(&registry_reader),
            });

        let tenant = Arc::new(Mutex::new(TenantDecl {
            id: "database-tenant".to_string(),
            external_id: Some("before-reload".to_string()),
            settings: Default::default(),
        }));
        let tenant_reader = Arc::new(MutableTenantReader {
            tenant: Arc::clone(&tenant),
        });
        let tenant_factory: Arc<dyn RelationalTenantFactory> =
            Arc::new(MutableRelationalTenantFactory {
                reader: Arc::clone(&tenant_reader),
            });

        let versioned = FileConfigStore::new(&config_path).load_versioned().unwrap();
        let config = versioned.config;
        let registry_factories = registry_factories_of(Arc::clone(&registry_factory));
        let tenant_factories = tenant_factories_of(Arc::clone(&tenant_factory));
        let registry = build_registry_reader(&config, &registry_factories)
            .await
            .unwrap();
        let tenants = build_tenant_reader(&config, &tenant_factories)
            .await
            .unwrap();
        let (router, resolver, tenant_snapshot) = build_router_and_resolver(
            &config,
            &driver_registry,
            registry.as_ref(),
            tenants.as_ref(),
        )
        .await
        .unwrap();
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let styles: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new_with_registry_and_version(
            config,
            tenant_snapshot,
            router,
            resolver,
            None,
            registry,
            cache,
            styles,
            versioned.version,
        ));
        let app = crate::app::build(Arc::clone(&ctx), test_metrics_handle(), 60);

        assert_eq!(
            get(&app, &tile_path("before-reload", "default", "demo"))
                .await
                .status(),
            StatusCode::OK
        );

        // The config file is deliberately NOT rewritten here: the change is
        // in the relational tenant store, and a reload against an unchanged
        // document is exactly how an operator forces that store to be
        // re-read. `#260`'s unchanged-document guard is scoped to
        // `registry.backend: file` precisely so this keeps working — see
        // `attempt_reload`.
        tenant.lock().unwrap().external_id = Some("after-reload".to_string());
        attempt_reload(
            &ctx,
            &config_path,
            &driver_registry,
            &registry_factories,
            &tenant_factories,
            &Readiness::new(),
        )
        .await;

        assert_eq!(
            get(&app, &tile_path("after-reload", "default", "demo"))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            get(&app, &tile_path("before-reload", "default", "demo"))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(ctx.current().tenants[0].external_id(), "after-reload");

        // `#162`, the reload half of "refuse by name": an edit that renames
        // `registry.implementation` to something this binary never registered
        // must be a failed activation, not a server that quietly re-resolves
        // to the sole registered factory anyway (which would make the name
        // decorative) and not a server that falls back to the file backend
        // (which would serve a YAML file this deployment thought it had
        // migrated off). The previous configuration keeps serving, exactly
        // as it does for an unreachable registry.
        std::fs::write(
            &config_path,
            format!(
                "storages: [ {{ id: main, driver: {driver_name}, url_env: {env_var} }} ]\n\
                 registry: {{ backend: relational, storage: main, \
                 implementation: never-registered }}\n"
            ),
        )
        .unwrap();
        attempt_reload(
            &ctx,
            &config_path,
            &driver_registry,
            &registry_factories,
            &tenant_factories,
            &Readiness::new(),
        )
        .await;

        assert_eq!(
            get(&app, &tile_path("after-reload", "default", "demo"))
                .await
                .status(),
            StatusCode::OK,
            "the previous configuration must still be serving"
        );
        assert_eq!(ctx.current().tenants[0].external_id(), "after-reload");

        unsafe {
            std::env::remove_var(env_var);
        }
    }

    /// `#42`, third slice: a reload against a `registry.backend: relational`
    /// config whose registry has gone unreachable since the last successful
    /// reload keeps the previous state entirely — the same "a bad edit must
    /// never take the server down" guarantee
    /// `an_invalid_edit_is_rejected_and_the_old_config_keeps_serving` already
    /// proves for the file backend, now proven for the relational one,
    /// through the real trigger pipeline (`reload::run`), not a bespoke call
    /// to `attempt_reload`. The SAME `TestTiles` instance answering a second
    /// request (not merely an equivalent new one) is what proves the old
    /// `ContextState` — router, resolver, and all — was genuinely never
    /// swapped, rather than swapped for an indistinguishable rebuild.
    #[tokio::test]
    async fn a_relational_registry_reload_failure_keeps_the_old_state_serving() {
        let dir = temp_config_dir("relational-reload-failure");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-test-relational";
        let tenant = "relational-reload-tenant";
        let env_var = "TELLURION_RELOAD_TEST_RELATIONAL_URL";
        // Safety: this env var name is unique to this test; no other test in
        // this crate reads or writes it concurrently.
        unsafe {
            std::env::set_var(env_var, "postgres://example/relational-reload-test");
        }

        let tiles = TestTiles::new(b"relational-reload-payload");
        let mut driver_registry = Registry::new();
        driver_registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles: Arc::clone(&tiles),
        }));
        let driver_registry = Arc::new(driver_registry);

        let config_yaml = format!(
            r#"
storages: [ {{ id: main, driver: {driver_name}, url_env: {env_var} }} ]
registry: {{ backend: relational, storage: main }}
"#
        );
        std::fs::write(&config_path, &config_yaml).unwrap();

        let healthy = Arc::new(AtomicBool::new(true));
        let catalog = CatalogDecl {
            id: "relational-reload-catalog".to_string(),
            external_id: Some("default".to_string()),
            tenant: tenant.to_string(),
            settings: Default::default(),
            visibility: Default::default(),
        };
        let collection: CollectionDecl = serde_yaml::from_str(&format!(
            "id: relational-reload-collection-internal\nexternal_id: relational-demo\ncatalog: {}\nstorage: main\ntable: demo\ngeometry: geom\npk: id\ntiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}\n",
            catalog.id
        ))
        .unwrap();
        let reader = Arc::new(FlakyRegistryReader {
            catalog,
            collection,
            healthy: Arc::clone(&healthy),
        });
        let relational_factory: Arc<dyn RelationalRegistryFactory> =
            Arc::new(FlakyRelationalFactory {
                reader: Arc::clone(&reader),
            });
        let tenant_reader = Arc::new(MutableTenantReader {
            tenant: Arc::new(Mutex::new(TenantDecl {
                id: tenant.to_string(),
                external_id: None,
                settings: Default::default(),
            })),
        });
        let tenant_factory: Arc<dyn RelationalTenantFactory> =
            Arc::new(MutableRelationalTenantFactory {
                reader: tenant_reader,
            });

        // Boot: the same sequence `main` follows — build the registry
        // reader first, then `Router`/`Resolver` together from it.
        let config = FileConfigStore::new(&config_path)
            .load()
            .expect("initial relational config loads and validates");
        let registry_factories = registry_factories_of(Arc::clone(&relational_factory));
        let tenant_factories = tenant_factories_of(Arc::clone(&tenant_factory));
        let registry_reader = build_registry_reader(&config, &registry_factories)
            .await
            .expect("initial connect succeeds while healthy");
        let tenant_reader = build_tenant_reader(&config, &tenant_factories)
            .await
            .expect("initial tenant connect succeeds while healthy");
        let (initial_router, initial_resolver, initial_tenants) = build_router_and_resolver(
            &config,
            &driver_registry,
            registry_reader.as_ref(),
            tenant_reader.as_ref(),
        )
        .await
        .expect("initial relational build succeeds while healthy");
        let authorizer = tellurion_core::build_authorizer(&config.auth)
            .expect("no bearer principal in this fixture reads a token_env");
        let cache: Arc<dyn TileCache> = Arc::new(MokaTileCache::with_byte_budget(1_000_000));
        let style_store: Arc<dyn StyleStore> = Arc::new(FileStyleStore::new(&[]));
        let ctx = Arc::new(AppContext::new_with_registry_and_version(
            config,
            initial_tenants,
            initial_router,
            initial_resolver,
            authorizer,
            registry_reader,
            cache,
            style_store,
            tellurion_core::ConfigVersion::from_wire("relational-reload-test"),
        ));
        let app = crate::app::build(Arc::clone(&ctx), test_metrics_handle(), 60);

        let path = format!(
            "/{tenant}/tiles/catalogs/default/collections/relational-demo/tiles/WebMercatorQuad/0/0/0"
        );
        let before = get(&app, &path).await;
        assert_eq!(before.status(), StatusCode::OK);
        assert_eq!(tiles.call_count(), 1);

        tokio::spawn(run(
            Arc::clone(&ctx),
            config_path.clone(),
            Arc::clone(&driver_registry),
            registry_factories,
            tenant_factories,
            Readiness::new(),
        ));
        let_triggers_install().await;

        // Simulate the registry going unreachable, then touch the config
        // file (still otherwise valid) to trigger a reload attempt against
        // it.
        healthy.store(false, Ordering::SeqCst);
        std::fs::write(
            &config_path,
            format!("{config_yaml}# a harmless trailing comment to change the file's bytes\n"),
        )
        .unwrap();

        // Give the trigger pipeline plenty of time to notice the change,
        // debounce it, and attempt (and fail) the reload.
        tokio::time::sleep(Duration::from_secs(1)).await;

        // A DIFFERENT tile coordinate than `before` — the tile cache is not
        // part of the swapped `ContextState` (see `context.rs`'s own doc),
        // so repeating `before`'s exact request would be a cache hit
        // regardless of whether the reload actually swapped anything,
        // proving nothing about which `Router` is still live. A fresh
        // coordinate forces a real call through whatever router `ctx`
        // currently holds.
        let after_path = format!(
            "/{tenant}/tiles/catalogs/default/collections/relational-demo/tiles/WebMercatorQuad/1/0/0"
        );
        let after = get(&app, &after_path).await;
        assert_eq!(
            after.status(),
            StatusCode::OK,
            "a relational registry going unreachable must never take the server down"
        );
        assert_eq!(
            tiles.call_count(),
            2,
            "the SAME driver/router instance must still be serving — a second call on \
             the same TestTiles, not a rebuilt one that happens to look equivalent"
        );

        unsafe {
            std::env::remove_var(env_var);
        }
    }

    // -- config-mutation control lane composes with the reload pipeline
    // (`#110`) -------------------------------------------------------------
    //
    // The mutation endpoint (`config_mutation.rs`) never touches the live
    // `AppContext` directly — it only asks `ConfigStore::write` to persist a
    // validated document to the file this same `reload::run` pipeline
    // already watches. These two tests prove that composition actually
    // holds end to end: a real HTTP `PUT /config`, through the real
    // file-watch trigger, becomes visible to a real subsequent request (or,
    // for an invalid candidate, never does — the previous document keeps
    // really serving).

    const MUTATION_PROPAGATION_ADMIN_TOKEN: &str = "reload-mutation-admin-token";
    const MUTATION_PROPAGATION_ADMIN_PRINCIPAL: &str = "reload-mutation-admin";

    /// Same shape as [`build_ctx`], plus a real `FileConfigStore` attached
    /// over `config_path` — the seam the config-mutation control lane needs
    /// (`AppContext::config_store`), which `build_ctx` itself never
    /// attaches (every other test in this module never mutates over HTTP).
    async fn build_ctx_with_config_store(
        config_path: &Path,
        registry: &Registry,
    ) -> Arc<AppContext> {
        let ctx = build_ctx(config_path, registry).await;
        let ctx =
            Arc::try_unwrap(ctx).unwrap_or_else(|_| unreachable!("freshly built, sole owner"));
        Arc::new(ctx.with_config_store(Arc::new(FileConfigStore::new(config_path))))
    }

    fn mutation_propagation_config(driver_name: &str) -> String {
        format!(
            r#"
storages: [ {{ id: main, driver: {driver_name}, url_env: DATABASE_URL }} ]
tenants: [ {{ id: public }} ]
catalogs: [ {{ id: default, tenant: public }} ]
collections:
  - id: reload-mutation-collection-internal
    external_id: original
    catalog: default
    storage: main
    table: demo
    geometry: geom
    pk: id
    tiles: {{ minzoom: 0, maxzoom: 5, caps: {{}} }}
auth:
  bearer_tokens:
    - token: {MUTATION_PROPAGATION_ADMIN_TOKEN}
      tenants: [public]
      platform_admin: true
      principal: {MUTATION_PROPAGATION_ADMIN_PRINCIPAL}
"#
        )
    }

    async fn put_config_via_http(
        app: &axum::Router,
        expected_version: &str,
        body: &serde_json::Value,
    ) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/config")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {MUTATION_PROPAGATION_ADMIN_TOKEN}"),
                    )
                    .header("x-config-expected-version", expected_version)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn get_config_via_http(app: &axum::Router) -> Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/config")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {MUTATION_PROPAGATION_ADMIN_TOKEN}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// End-to-end propagation (`#110`, box 2 + box 4 composing): a real
    /// `PUT /config` adding a second collection, applied through the real
    /// file-watch trigger pipeline (`reload::run`, not a bespoke
    /// `attempt_reload` call), becomes servable over a real tile request —
    /// no direct call from the mutation handler into `AppContext::reload*`
    /// anywhere in this path.
    #[tokio::test]
    async fn a_valid_config_mutation_propagates_through_the_real_reload_pipeline() {
        let dir = temp_config_dir("mutation-propagation-valid");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-mutation-propagation-valid";

        let tiles = TestTiles::new(b"mutation-propagation-payload");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles: Arc::clone(&tiles),
        }));
        let registry = Arc::new(registry);

        std::fs::write(&config_path, mutation_propagation_config(driver_name)).unwrap();

        let ctx = build_ctx_with_config_store(&config_path, &registry).await;
        let app = crate::app::build(Arc::clone(&ctx), test_metrics_handle(), 60);

        tokio::spawn(run(
            Arc::clone(&ctx),
            config_path.clone(),
            Arc::clone(&registry),
            Arc::new(RelationalRegistryFactories::new()),
            Arc::new(RelationalTenantFactories::new()),
            Readiness::new(),
        ));
        let_triggers_install().await;

        // Read the current document over HTTP, add a second collection,
        // and write it back — exactly how a real operator would use this
        // endpoint.
        let current = get_config_via_http(&app).await;
        assert_eq!(current.status(), StatusCode::OK);
        let version = current
            .headers()
            .get("x-config-version")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let body = to_bytes(current.into_body(), usize::MAX).await.unwrap();
        let mut candidate: serde_json::Value = serde_json::from_slice(&body).unwrap();
        candidate["collections"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "id": "reload-mutation-collection-internal-2",
                "external_id": "added-by-mutation",
                "catalog": "default",
                "storage": "main",
                "table": "demo",
                "geometry": "geom",
                "pk": "id",
                "tiles": { "minzoom": 0, "maxzoom": 5, "caps": {} }
            }));

        let write_response = put_config_via_http(&app, &version, &candidate).await;
        assert_eq!(write_response.status(), StatusCode::OK);

        // Never touched directly: the mutation handler only wrote the
        // file. The already-running `reload::run` watch is what must pick
        // it up.
        // `auth:` is configured for this fixture (needed for the platform-
        // admin gate), which also activates `enforce_tenant_auth` for every
        // other tenant-scoped route — so the poll and the follow-up check
        // below present the same token (it authorizes tenant `public` too),
        // via `get_with_bearer`, not the plain unauthenticated `get`.
        let poll_app = app.clone();
        let reloaded = wait_until(Duration::from_secs(5), move || {
            let app = poll_app.clone();
            async move {
                get_with_bearer(
                    &app,
                    &tile_path("public", "default", "added-by-mutation"),
                    Some(MUTATION_PROPAGATION_ADMIN_TOKEN),
                )
                .await
                .status()
                    == StatusCode::OK
            }
        })
        .await;
        assert!(
            reloaded,
            "the collection added by the mutation endpoint should become servable once the \
             existing reload pipeline picks up the file it wrote"
        );

        // The original collection is untouched throughout.
        let original = get_with_bearer(
            &app,
            &tile_path("public", "default", "original"),
            Some(MUTATION_PROPAGATION_ADMIN_TOKEN),
        )
        .await;
        assert_eq!(original.status(), StatusCode::OK);
    }

    /// The issue's named test, end to end: an invalid `PUT /config` is
    /// refused by the mutation endpoint itself (never reaches the file),
    /// and a real tile request against the original, still-served
    /// collection succeeds throughout — serving continuity proved through
    /// an actual request, not merely by inspecting the store.
    #[tokio::test]
    async fn an_invalid_config_mutation_is_refused_and_the_old_config_keeps_really_serving() {
        let dir = temp_config_dir("mutation-propagation-invalid");
        let config_path = dir.join("config.yaml");
        let driver_name = "reload-mutation-propagation-invalid";

        let tiles = TestTiles::new(b"still-serving-after-refused-mutation");
        let mut registry = Registry::new();
        registry.register(Arc::new(TestFactory {
            name: driver_name,
            table: "demo",
            tiles: Arc::clone(&tiles),
        }));
        let registry = Arc::new(registry);

        std::fs::write(&config_path, mutation_propagation_config(driver_name)).unwrap();

        let ctx = build_ctx_with_config_store(&config_path, &registry).await;
        let app = crate::app::build(Arc::clone(&ctx), test_metrics_handle(), 60);

        tokio::spawn(run(
            Arc::clone(&ctx),
            config_path.clone(),
            Arc::clone(&registry),
            Arc::new(RelationalRegistryFactories::new()),
            Arc::new(RelationalTenantFactories::new()),
            Readiness::new(),
        ));
        let_triggers_install().await;

        let current = get_config_via_http(&app).await;
        let version = current
            .headers()
            .get("x-config-version")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Referentially broken: a collection naming a catalog nothing
        // declares.
        let invalid = serde_json::json!({
            "storages": [ { "id": "main", "driver": driver_name, "url_env": "DATABASE_URL" } ],
            "collections": [
                { "id": "broken", "catalog": "nonexistent", "storage": "nonexistent",
                  "table": "demo", "geometry": "geom", "pk": "id" }
            ]
        });

        let write_response = put_config_via_http(&app, &version, &invalid).await;
        assert_eq!(write_response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        // Give the (never-triggered, since the file was never touched)
        // reload pipeline every chance it would need to notice a change.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let response = get_with_bearer(
            &app,
            &tile_path("public", "default", "original"),
            Some(MUTATION_PROPAGATION_ADMIN_TOKEN),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "a refused mutation must never take the server down or disrupt serving"
        );
        let response_body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            response_body.as_ref(),
            b"still-serving-after-refused-mutation",
            "the original config must still be the one serving"
        );
    }
}
