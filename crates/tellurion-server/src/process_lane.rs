//! Boot-time assembly of the Processes lane (`#182`) and the in-process runner
//! loop that drains it.
//!
//! # The capability gate, in one function
//!
//! [`build`] returns `Some` only when a deployment really can run jobs:
//!
//! 1. `server.processes` is declared at all (`ProcessesConfig`) — declaring it
//!    IS the opt-in, so a config written before this field existed produces
//!    `None` without reading anything else;
//! 2. the storage it names advertises a durable job ledger
//!    (`Router::resolve_job_store`, refusing by name when it does not); and
//! 3. this binary registered at least one [`ProcessRunner`].
//!
//! With any of the three missing, `app::build` mounts a Processes root that
//! answers `404` at every path — the same answer an unmounted prefix gives —
//! and no runner loop is spawned. That is `#182`'s "a deployment with no
//! runner capability does not get a half-working Processes root, it gets no
//! root" rule, executed rather than intended. The refusal is never silent: a
//! declared `server.processes` whose storage cannot hold a ledger logs the
//! named `Router::resolve_job_store` error at `error` level.
//!
//! And even with all three present, the root is invisible until an operator
//! sets `protocols.processes: enabled` (`#185`), which defaults to `disabled`.
//! Two switches, deliberately: "can this deployment run jobs" and "does this
//! catalog expose the HTTP root" are different questions with different
//! answers. The second is re-read from the current snapshot on every request,
//! so an operator can turn a catalog's root off without a restart; the first
//! is resolved once, at boot.
//!
//! **Reload.** Like `applier::spawn_all`, this is resolved from the router
//! `AppContext` was built with at boot: a config reload swaps in a new
//! router/config for future HTTP requests but does not respin the runner, and
//! does not add or remove the root (route topology stays static across a
//! reload for every root on this server). Editing `server.processes` therefore
//! needs a restart, which is stated here rather than discovered — and is the
//! same limitation every background consumer in this workspace already has.
//!
//! # Why the loop takes no lease
//!
//! Every other background consumer in this workspace that must not double-run
//! takes a `tellurion_core::lease::Lease` (`#193`). This one deliberately does
//! not, and the reason is the shape of the work rather than an oversight: a
//! lease elects ONE leader, which would funnel every job in the deployment
//! through a single replica and destroy the heterogeneous-runner design
//! `#182`'s claim filter exists for (a GDAL worker build and a lean API pod
//! sharing one ledger, each taking only what it can execute). Mutual exclusion
//! is instead per JOB, in the ledger itself: `FOR UPDATE SKIP LOCKED` stops
//! two live replicas taking the same row, and the `locked_until` visibility
//! timeout returns a row stranded by a killed replica. See
//! `tellurion-postgis::job_sql`'s own doc for why those two mechanisms are not
//! substitutes for each other.
//!
//! # What the runner is not
//!
//! Not a sandbox. A runner executes in this process, on this runtime, with
//! this server's privileges. Container isolation is `#182`'s explicitly
//! deferred item, and the only runner registered here is a built-in compiled
//! into the binary — there is no path by which a client-supplied payload
//! becomes code.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use tellurion_core::{
    AppContext, Error as CoreError, JobControlOption, JobLedger, JobOutcome, JobRecord, PolicyLane,
    ProcessDescription, ProcessLane, ProcessRegistry, ProcessRunner, ProcessTarget,
    Result as CoreResult,
};

/// The one built-in process this slice ships (`#182`'s own suggestion: "one
/// built-in process wired to an existing internal job").
pub const INDEX_REBUILD_PROCESS: &str = "index-rebuild";

/// Bounded per-pass fetch size, matching `IndexApplierConfig`'s own default —
/// the same "one pass's memory and lock footprint is fixed regardless of how
/// far behind the index has fallen" rule, applied to the same drain.
const REBUILD_BATCH_SIZE: u32 = 200;

/// Ceiling on drain passes in one job. A job is a bounded unit of work with a
/// visibility timeout; an unbounded catch-up loop would either hold a
/// reservation past its window or never finish. Hitting this ceiling is not an
/// error — the job reports how far it got, and the operator submits another.
const REBUILD_MAX_PASSES: usize = 1_000;

/// Drains a collection's outbox into its derived index, on demand (`#67`'s
/// applier, exposed as a process).
///
/// Chosen as the first built-in precisely because it is an *existing* internal
/// job rather than new work: `tellurion_core::applier::drain_once` is the same
/// function the background applier calls, so this process inherits its
/// idempotence for free — which is what makes it safe under this lane's
/// at-least-once contract. A process whose effects were not idempotent would
/// not be safe to ship here at all.
struct IndexRebuildRunner {
    ctx: Arc<AppContext>,
}

impl IndexRebuildRunner {
    /// The `collection` input, validated. A missing or non-string `collection`
    /// is refused at submission (`400`) rather than becoming a job that exists
    /// only to fail.
    fn collection_of(inputs: &Value) -> CoreResult<String> {
        match inputs.get("collection") {
            Some(Value::String(collection)) if !collection.is_empty() => Ok(collection.clone()),
            _ => Err(CoreError::Invalid(
                "input 'collection' is required and must be a non-empty string naming a collection"
                    .to_string(),
            )),
        }
    }

    /// The `ProcessTarget` half of [`ProcessRunner::target`], as an
    /// associated function: it depends on nothing but the inputs, so the
    /// tests below can pin the lane decision — the single line that decides
    /// whether a read grant suffices to schedule a rebuild — without standing
    /// up an `AppContext` that would prove nothing about it.
    fn target_of(inputs: &Value) -> Option<ProcessTarget> {
        Self::collection_of(inputs)
            .ok()
            .map(|collection| ProcessTarget {
                collection,
                lane: PolicyLane::Write,
            })
    }
}

/// The `index-rebuild` process description. A free function rather than a
/// method body so the tests below can pin the declaration — particularly its
/// job control options, which the Standard makes load-bearing — without
/// standing up an `AppContext`.
fn index_rebuild_description() -> ProcessDescription {
    ProcessDescription {
        id: INDEX_REBUILD_PROCESS.to_string(),
        // The process's own version, not the server's. Bumped when what this
        // process DOES changes, so a client can tell.
        version: "1.0.0".to_string(),
        title: Some("Rebuild a collection's derived index".to_string()),
        description: Some(
            "Drains the collection's transactional outbox into its derived index, the same \
             apply path the background index applier runs. Idempotent: obligations are \
             version-gated, so re-running converges."
                .to_string(),
        ),
        // Asynchronous only, and dismissible. Declared honestly rather than
        // generously: OGC API — Processes Requirement 25
        // (`/req/core/process-execute-default-execution-mode`) decides a
        // `Prefer`-less request's execution mode FROM these options, so
        // claiming `sync-execute` here would make this server's own
        // asynchronous answer wrong by the Standard's own rule.
        job_control_options: vec![JobControlOption::AsyncExecute, JobControlOption::Dismiss],
    }
}

#[async_trait]
impl ProcessRunner for IndexRebuildRunner {
    fn description(&self) -> ProcessDescription {
        index_rebuild_description()
    }

    fn validate_inputs(&self, inputs: &Value) -> CoreResult<()> {
        Self::collection_of(inputs).map(|_| ())
    }

    /// `PolicyLane::Write`, not `Features`. Draining a collection's outbox
    /// mutates its derived index; a subject granted only reads on a collection
    /// has not thereby been granted the right to schedule work against it.
    fn target(&self, inputs: &Value) -> Option<ProcessTarget> {
        Self::target_of(inputs)
    }

    async fn execute(&self, job: &JobRecord) -> CoreResult<Value> {
        let collection_ext = Self::collection_of(&job.inputs)?;
        let state = self.ctx.current();
        let collection_id = state
            .resolver
            .resolve_collection(&job.scope.catalog, &collection_ext)
            .await?;
        // Both refuse by name (`CapabilityUnsupported`) for a collection with
        // no write/index lane, or one whose storage does not advertise the
        // capability — the job then fails with that name rather than with a
        // generic error.
        let (decl, outbox) = state
            .router
            .resolve_outbox(&job.scope.tenant, &job.scope.catalog, &collection_id)
            .await?;
        let (_, index) = state
            .router
            .resolve_index(&job.scope.tenant, &job.scope.catalog, &collection_id)
            .await?;

        let mut applied = 0usize;
        let mut passes = 0usize;
        loop {
            let batch = tellurion_core::drain_once(
                outbox.as_ref(),
                index.as_ref(),
                &decl,
                REBUILD_BATCH_SIZE,
            )
            .await?;
            applied += batch;
            passes += 1;
            if batch == 0 {
                return Ok(json!({
                    "collection": collection_ext,
                    "applied": applied,
                    "complete": true,
                }));
            }
            if passes >= REBUILD_MAX_PASSES {
                // Not a failure: the work done is durable and the index is
                // strictly fresher than it was. `complete: false` is the
                // honest way to say "there is more", rather than reporting a
                // success that implies the backlog is gone.
                return Ok(json!({
                    "collection": collection_ext,
                    "applied": applied,
                    "complete": false,
                }));
            }
        }
    }
}

/// Assembles the lane, or explains why there is none. See this module's own
/// doc for the three conditions.
pub fn build(ctx: &Arc<AppContext>) -> Option<Arc<ProcessLane>> {
    let state = ctx.current();
    let config = state.config.server.processes.clone()?;

    let store = match state.router.resolve_job_store(&config.storage) {
        Ok(store) => store,
        Err(error) => {
            tracing::error!(
                %error,
                storage = %config.storage,
                "processes: no durable job ledger; the Processes root will not be served"
            );
            return None;
        }
    };

    let mut registry = ProcessRegistry::new();
    registry.register(Arc::new(IndexRebuildRunner {
        ctx: Arc::clone(ctx),
    }));
    if registry.is_empty() {
        tracing::error!(
            "processes: this binary registered no process runner; the Processes root will not be served"
        );
        return None;
    }

    tracing::info!(
        storage = %config.storage,
        processes = ?registry.process_ids(),
        visibility_timeout_s = config.visibility_timeout_s,
        "processes: durable job ledger resolved; the Processes root is available"
    );
    Some(Arc::new(ProcessLane::new(
        registry,
        JobLedger::new(store, Duration::from_secs(config.visibility_timeout_s)),
    )))
}

/// Spawns the in-process runner loop, if there is a lane to drain.
///
/// One task, not one per process: the claim query already filters on the whole
/// registered set in a single round trip, so a task per process would multiply
/// the poll traffic by the number of built-ins for no added parallelism this
/// slice can use.
pub fn spawn(
    ctx: &Arc<AppContext>,
    lane: Option<Arc<ProcessLane>>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let Some(lane) = lane else {
        return Vec::new();
    };
    let state = ctx.current();
    let Some(config) = state.config.server.processes.clone() else {
        // Unreachable: `build` returned `Some`, so the block was declared.
        // Returning rather than unwrapping keeps a config swapped between the
        // two calls from being a panic.
        return Vec::new();
    };
    drop(state);
    let poll_interval = Duration::from_millis(config.poll_interval_ms);
    tracing::info!(
        poll_interval_ms = config.poll_interval_ms,
        "processes: starting the in-process runner"
    );
    vec![tokio::spawn(run_runner(lane, poll_interval, shutdown))]
}

/// Claim, execute, record — until `shutdown`.
///
/// A pass that finds nothing sleeps; a pass that finds a job runs it and
/// immediately tries again, so a backlog drains at full speed rather than one
/// job per tick. A failed claim (the ledger is unreachable, or was never
/// provisioned) is logged and retried on the next tick rather than ending the
/// loop, the same rule `run_applier` follows: a stalled lane is a degradation
/// an operator can fix, not a reason for the loop to stop existing.
async fn run_runner(
    lane: Arc<ProcessLane>,
    poll_interval: Duration,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let process_ids = lane.registry.process_ids();
    loop {
        if *shutdown.borrow() {
            return;
        }
        let claimed = lane
            .ledger
            .store
            .claim_next(&process_ids, lane.ledger.visibility)
            .await;
        let idle = match claimed {
            // An ordinary answer, not an error: an idle ledger is the normal
            // state of a healthy deployment.
            Ok(None) => true,
            Ok(Some(job)) => {
                execute_and_record(&lane, job).await;
                false
            }
            Err(error) => {
                tracing::warn!(%error, "processes: could not claim a job; retrying");
                true
            }
        };
        if !idle {
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {}
            result = shutdown.changed() => {
                if result.is_err() || *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

/// Runs one claimed job and records its outcome.
///
/// `Ok(None)` from `finish` is expected, not exceptional: the job was
/// dismissed while it ran, or its visibility window lapsed and another replica
/// took it. Logged at `info` because it is the at-least-once contract being
/// visible, not a fault.
async fn execute_and_record(lane: &ProcessLane, job: JobRecord) {
    let Some(runner) = lane.registry.get(&job.process_id) else {
        // Only reachable if the claim filter and the registry disagreed, which
        // they cannot within one process — recorded as a failure rather than
        // left running until its visibility lapses and it is re-claimed
        // forever.
        let detail = format!("no runner for process '{}'", job.process_id);
        tracing::error!(job = %job.job_id, "processes: {detail}");
        record_outcome(lane, &job.job_id, JobOutcome::Failed(detail)).await;
        return;
    };
    let outcome = match runner.execute(&job).await {
        Ok(results) => JobOutcome::Succeeded(results),
        Err(error) => {
            tracing::warn!(job = %job.job_id, process = %job.process_id, %error, "processes: job failed");
            JobOutcome::Failed(error.to_string())
        }
    };
    record_outcome(lane, &job.job_id, outcome).await;
}

async fn record_outcome(lane: &ProcessLane, job_id: &str, outcome: JobOutcome) {
    match lane.ledger.store.finish(job_id, outcome).await {
        Ok(Some(_)) => {}
        Ok(None) => tracing::info!(
            job = %job_id,
            "processes: this job was no longer ours to finish (dismissed, or re-claimed after its visibility lapsed)"
        ),
        Err(error) => tracing::warn!(
            job = %job_id,
            %error,
            "processes: could not record the job outcome; it will be re-claimed once its visibility lapses"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A submission with no usable `collection` is refused at submit time,
    /// with a message naming the input — never accepted as a job that exists
    /// only to fail.
    #[test]
    fn a_rebuild_without_a_collection_input_is_refused_before_a_job_exists() {
        for inputs in [
            json!({}),
            json!({ "collection": "" }),
            json!({ "collection": 7 }),
            json!({ "collection": null }),
            json!(null),
        ] {
            let error =
                IndexRebuildRunner::collection_of(&inputs).expect_err("this input must be refused");
            assert!(
                matches!(error, CoreError::Invalid(ref message) if message.contains("collection")),
                "{inputs:?} -> {error}"
            );
        }
    }

    #[test]
    fn a_rebuild_names_its_collection() {
        assert_eq!(
            IndexRebuildRunner::collection_of(&json!({ "collection": "demo" })).unwrap(),
            "demo"
        );
    }

    /// The lane it authorizes through is the WRITE lane. A read grant must not
    /// be enough to schedule an index rebuild, and this is the single line
    /// that decides it.
    #[test]
    fn a_rebuild_is_authorized_through_the_write_lane() {
        let target = IndexRebuildRunner::target_of(&json!({ "collection": "demo" }))
            .expect("a rebuild names a target collection");
        assert_eq!(target.collection, "demo");
        assert_eq!(target.lane, PolicyLane::Write);
    }

    /// Invalid inputs name no target at all, so the authorization step is
    /// skipped and `validate_inputs`' own `400` is what the caller sees.
    #[test]
    fn invalid_inputs_name_no_target() {
        assert!(IndexRebuildRunner::target_of(&json!({})).is_none());
    }

    /// The declared job control options are what OGC API — Processes
    /// Requirement 25 reads to decide a `Prefer`-less request's execution
    /// mode. Claiming `sync-execute` here would make this server's own
    /// asynchronous answer wrong by the Standard's rule, so pin the
    /// declaration rather than trusting a comment.
    #[test]
    fn the_built_in_process_declares_only_the_modes_it_actually_supports() {
        let description = index_rebuild_description();
        let options: Vec<&str> = description
            .job_control_options
            .iter()
            .map(|option| option.as_str())
            .collect();
        assert_eq!(options, vec!["async-execute", "dismiss"]);
        assert_eq!(description.id, INDEX_REBUILD_PROCESS);
        assert!(!description.version.is_empty());
    }

    /// The registry is keyed by the description's own id, so the ids the
    /// claim query filters on are exactly the ids `/processes` advertises.
    /// A drift between those two sets is a job nothing ever picks up.
    #[test]
    fn the_claim_filter_matches_what_the_process_list_advertises() {
        let mut registry = ProcessRegistry::new();
        registry.register(Arc::new(StaticRunner(index_rebuild_description())));
        assert_eq!(
            registry.process_ids(),
            registry
                .descriptions()
                .into_iter()
                .map(|description| description.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(registry.process_ids(), vec![INDEX_REBUILD_PROCESS]);
    }

    /// A runner with no context, so the registry check above needs no
    /// `AppContext` — it is exercising the registry, not the rebuild.
    struct StaticRunner(ProcessDescription);

    #[async_trait]
    impl ProcessRunner for StaticRunner {
        fn description(&self) -> ProcessDescription {
            self.0.clone()
        }

        async fn execute(&self, _job: &JobRecord) -> CoreResult<Value> {
            Ok(json!({}))
        }
    }
}
