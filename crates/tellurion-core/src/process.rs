//! The process-runner seam (`#182`): what a deployment can *execute*, as a
//! boot-time named registry rather than a discovered plugin set.
//!
//! # Boot-time and named, like every other seam
//!
//! This is the `#112` extension model applied once more, and it is backed by
//! the same [`NamedRegistry`](crate::extension::NamedRegistry) the storage
//! driver seam uses — not a second hand-rolled map with its own "unknown
//! name" wording and its own iteration order. A runner exists because
//! something called [`ProcessRegistry::register`] with it in `main`, never
//! because a crate happened to link a constructor.
//!
//! # Why the registry is what the claim query filters on
//!
//! `#182`'s heterogeneous-deployment requirement is that "a worker build with
//! GDAL vs a lean API pod coexist against one ledger without misclaiming."
//! The mechanism is exactly this registry: [`ProcessRegistry::process_ids`] is
//! passed straight to [`JobStore::claim_next`](crate::job::JobStore::claim_next)
//! as the set of process ids this replica is entitled to take. A binary that
//! registers no runner claims nothing — and, per
//! `tellurion-server`'s `process_lane`, serves no Processes root either,
//! because a root advertising processes nothing can execute is the
//! half-working surface `#182` exists to avoid.
//!
//! # What a runner is not
//!
//! Not a sandbox. This slice executes a runner in-process, in the server's own
//! Tokio runtime, with the server's own privileges — container isolation is
//! `#182`'s explicitly deferred item and nothing here pretends otherwise. The
//! only processes registered in this slice are built-ins compiled into the
//! binary; there is no path by which a client-supplied payload becomes code.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::config::PolicyLane;
use crate::error::Result;
use crate::extension::NamedRegistry;
use crate::job::{JobLedger, JobRecord};

/// One value of OGC API — Processes — Part 1: Core's `jobControlOptions.yaml`
/// (Figure 7): the closed set `sync-execute | async-execute | dismiss`.
///
/// This is not decoration. Requirement 25
/// (`/req/core/process-execute-default-execution-mode`) and Requirement 26
/// (`/req/core/process-execute-auto-execution-mode`) both decide the execution
/// mode of a request *from the job control options in the process
/// description* — so a description that claims `sync-execute` for a process
/// this server only ever runs asynchronously would make the server's own
/// answer to a `Prefer`-less request wrong by the Standard's own rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobControlOption {
    SyncExecute,
    AsyncExecute,
    Dismiss,
}

impl JobControlOption {
    pub fn as_str(self) -> &'static str {
        match self {
            JobControlOption::SyncExecute => "sync-execute",
            JobControlOption::AsyncExecute => "async-execute",
            JobControlOption::Dismiss => "dismiss",
        }
    }
}

/// What `GET /processes/{processID}` answers with, and what one entry of
/// `GET /processes` summarizes.
///
/// Deliberately smaller than `process.yaml`: no `inputs`/`outputs` JSON Schema
/// blocks. Those are the OGC Process Description requirements class
/// (Requirements 47-54), which this slice does not implement and therefore
/// does not declare — see `tellurion_processes::conformance`. Carrying empty
/// `inputs: {}`/`outputs: {}` members would be worse than omitting them: a
/// client reading them would conclude the process takes nothing and returns
/// nothing.
#[derive(Debug, Clone)]
pub struct ProcessDescription {
    /// Stable identifier, and the `{processID}` path segment. Also the key the
    /// ledger stores and the claim query filters on, which is why it is the
    /// registry's own name rather than a second field alongside it.
    pub id: String,
    /// Required by `processSummary.yaml` alongside `id`. The process's own
    /// version, not the server's: a process whose behaviour changes gets a new
    /// version so a client can tell.
    pub version: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// How this process may be executed. Every built-in in this slice declares
    /// `[async-execute, dismiss]` and nothing else — see
    /// `tellurion_processes::conformance` for why synchronous execution is out
    /// of scope here rather than merely unimplemented.
    pub job_control_options: Vec<JobControlOption>,
}

/// The collection an execute request would act on, and the lane it would act
/// on it through — what a runner tells the HTTP layer so the `#34` policy
/// checkpoint can be applied to a *process submission* the same way it is
/// applied to a read or a write of that same collection.
///
/// Without this, "may this subject run this process?" would collapse into "is
/// this subject authorized for this tenant at all?", and a process that
/// rebuilds a collection's derived index would be reachable by any subject the
/// tenant boundary admits, regardless of the grants that govern the very
/// collection it touches. The runner declares the target because only the
/// runner knows what its inputs mean; the HTTP layer resolves and authorizes
/// it because only the HTTP layer has the credential.
#[derive(Debug, Clone)]
pub struct ProcessTarget {
    /// The collection's EXTERNAL id, exactly as a client typed it into the
    /// execute request (`#39`) — the HTTP layer resolves it.
    pub collection: String,
    /// Which lane's grants govern this process. A process that mutates a
    /// collection's derived state declares [`PolicyLane::Write`], not
    /// `Features`: a subject that may read a collection has not thereby been
    /// granted the right to schedule work against it.
    pub lane: PolicyLane,
}

/// Something this binary can actually execute.
///
/// One runner, one process id. A runner that could execute several would blur
/// the thing the claim query filters on, and `#182`'s whole misclaiming
/// defence is that the filter is exact.
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    /// This runner's process description — the same value `/processes` and
    /// `/processes/{processID}` project onto the wire.
    fn description(&self) -> ProcessDescription;

    /// Checks an execute request's `inputs` before a job is ever created.
    ///
    /// Refusing here rather than inside [`execute`](Self::execute) is the
    /// difference between a `400` a client can fix and a job that sits in the
    /// ledger only to fail — and OGC API — Processes Requirement 24
    /// (`/req/core/process-execute-input-validation`, clause A) puts the
    /// validation at the execute request, not at the job. The default accepts
    /// everything: a process with no required inputs has nothing to check.
    fn validate_inputs(&self, _inputs: &Value) -> Result<()> {
        Ok(())
    }

    /// Which collection, through which lane, an execute request carrying
    /// `inputs` would act on — see [`ProcessTarget`] for why this exists.
    ///
    /// `None` (the default) means the process acts on no collection, so there
    /// is no per-collection grant to check and the tenant trust boundary is
    /// the whole authorization story. Called only after
    /// [`validate_inputs`](Self::validate_inputs) has accepted the same
    /// inputs, so an implementation may read the members it just validated.
    fn target(&self, _inputs: &Value) -> Option<ProcessTarget> {
        None
    }

    /// Runs the job to completion, returning its results document.
    ///
    /// May be re-entered for the same job: a claimant that died mid-execution
    /// leaves the job to be re-claimed once its visibility window lapses, so
    /// an implementation whose effects are not idempotent is not safe here
    /// (see [`crate::job`]'s own at-least-once note).
    async fn execute(&self, job: &JobRecord) -> Result<Value>;
}

/// The boot-time registry of everything this binary can execute (`#112`,
/// `#182`).
///
/// A thin wrapper over [`NamedRegistry`] — same "named, not discovered",
/// "refuse by name", "deterministic iteration" properties, keyed by each
/// runner's own declared [`ProcessDescription::id`] so a registration cannot
/// disagree with the description it registers.
#[derive(Default)]
pub struct ProcessRegistry {
    runners: NamedRegistry<dyn ProcessRunner>,
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `runner` under its own declared process id, replacing
    /// whatever was registered under that id before — the same last-write-wins
    /// behaviour [`NamedRegistry`] gives every other seam.
    pub fn register(&mut self, runner: Arc<dyn ProcessRunner>) {
        let id = runner.description().id;
        self.runners.register(id, runner);
    }

    /// The runner for `id`, or `None` when this binary contains none —
    /// indistinguishable, on purpose, from a process whose crate was compiled
    /// out entirely. The caller turns that into `404` with the Standard's own
    /// `no-such-process` wording (Requirement 15).
    pub fn get(&self, id: &str) -> Option<&Arc<dyn ProcessRunner>> {
        self.runners.get(id)
    }

    /// Every registered process, alphabetically by id — the order
    /// `GET /processes` lists them in and the order a boot log enumerates
    /// "what this binary can actually execute" in.
    pub fn descriptions(&self) -> Vec<ProcessDescription> {
        self.runners
            .iter()
            .map(|(_, runner)| runner.description())
            .collect()
    }

    /// The claim filter (`JobStore::claim_next`'s `process_ids`): exactly the
    /// ids this replica can execute, so it can never take a job only another
    /// build knows how to run.
    pub fn process_ids(&self) -> Vec<String> {
        self.runners.names().map(str::to_string).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.runners.is_empty()
    }

    pub fn len(&self) -> usize {
        self.runners.len()
    }
}

/// Everything the Processes lane needs to answer a request, in one value: what
/// this binary can execute and where jobs are durably recorded.
///
/// Both halves are required, which is the point. `tellurion-server` builds one
/// of these at boot or none at all — a registry with no ledger would accept
/// submissions it cannot record, and a ledger with no registry would advertise
/// processes nothing can run. Neither is a Processes root worth serving, so
/// with either half missing the root answers the same `404` an unmounted
/// prefix answers.
pub struct ProcessLane {
    pub registry: ProcessRegistry,
    pub ledger: JobLedger,
}

impl ProcessLane {
    pub fn new(registry: ProcessRegistry, ledger: JobLedger) -> Self {
        Self { registry, ledger }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fake(&'static str);

    #[async_trait]
    impl ProcessRunner for Fake {
        fn description(&self) -> ProcessDescription {
            ProcessDescription {
                id: self.0.to_string(),
                version: "1.0.0".to_string(),
                title: None,
                description: None,
                job_control_options: vec![
                    JobControlOption::AsyncExecute,
                    JobControlOption::Dismiss,
                ],
            }
        }

        async fn execute(&self, _job: &JobRecord) -> Result<Value> {
            Ok(json!({}))
        }
    }

    /// An empty registry claims nothing — the property that keeps a binary
    /// with no runner from taking jobs off a shared ledger it cannot execute.
    #[test]
    fn a_binary_with_no_runner_claims_no_process_id() {
        let registry = ProcessRegistry::new();
        assert!(registry.process_ids().is_empty());
        assert!(registry.is_empty());
        assert!(registry.get("anything").is_none());
    }

    /// The claim filter is exactly the registered set, in deterministic order
    /// regardless of registration order.
    #[test]
    fn the_claim_filter_is_exactly_what_this_binary_registered() {
        let mut registry = ProcessRegistry::new();
        registry.register(Arc::new(Fake("zeta")));
        registry.register(Arc::new(Fake("alpha")));
        assert_eq!(
            registry.process_ids(),
            vec!["alpha".to_string(), "zeta".to_string()]
        );
        assert_eq!(
            registry
                .descriptions()
                .into_iter()
                .map(|description| description.id)
                .collect::<Vec<_>>(),
            vec!["alpha".to_string(), "zeta".to_string()]
        );
    }

    /// A runner is keyed by the id it declares, so a lookup by that id can
    /// never miss the runner that answers for it.
    #[test]
    fn a_runner_is_registered_under_its_own_declared_id() {
        let mut registry = ProcessRegistry::new();
        registry.register(Arc::new(Fake("index-rebuild")));
        assert!(registry.get("index-rebuild").is_some());
        assert_eq!(registry.len(), 1);
    }
}
