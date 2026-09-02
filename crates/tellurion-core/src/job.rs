//! The durable job ledger seam (`#182`): the capability a backend advertises
//! when it can hold operator-visible jobs, and the value types that ledger
//! stores.
//!
//! # Why a capability, not a component
//!
//! Tellurion's background work has always been a fixed internal consumer set
//! (index applier, tile invalidation, webhooks, retention). `#182` adds a
//! *user-facing* one, and the platform rule that makes that affordable is the
//! same one [`crate::lease`] already follows: the coordinator is the database
//! a write deployment already runs, reached through an `Option`-shaped
//! [`StorageDriver`](crate::router::StorageDriver) accessor
//! ([`job_store`](crate::router::StorageDriver::job_store)) that defaults to
//! `None`. A deployment whose storages advertise no [`JobStore`] does not get
//! a degraded Processes lane — it gets no Processes root at all
//! (`tellurion-server`'s `process_lane`).
//!
//! # The server never creates the ledger
//!
//! A `JobStore` implementation reads and writes one table it did not create.
//! `tellurion-ingest processes create-tables` owns that DDL, exactly as
//! `outbox`/`index`/`assets`/`stac` already do, and an implementation whose
//! table is absent refuses **by name** (`tellurion-postgis`'s
//! `JobsTableMissing`) rather than provisioning one on first submission. A
//! job submission that lazily created its own ledger would be the server
//! issuing DDL through the front door.
//!
//! # At-least-once, never exactly-once
//!
//! [`JobStore::claim_next`] hands one job to one claimant for a bounded
//! visibility window; a claimant that dies before finishing leaves the job to
//! be re-claimed once that window expires. That is at-least-once execution,
//! the same contract the outbox applier runs under, and `#182` states
//! exactly-once as an explicit non-goal. A process whose effects are not
//! idempotent is therefore not a safe process; the one built-in this slice
//! ships (`index-rebuild`) is idempotent by construction because
//! [`crate::applier::drain_once`] is.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;

/// A job's lifecycle state.
///
/// The vocabulary is closed and is exactly OGC API — Processes — Part 1: Core
/// (OGC 18-062r2, 1.0.0)'s `statusCode.yaml` (Figure 21): `accepted`,
/// `running`, `successful`, `failed`, `dismissed`. Spelled here rather than in
/// the protocol crate because the *ledger* is what has to enforce it — a
/// status column with a `CHECK` constraint the wire vocabulary disagrees with
/// is a bug nobody notices until a job is stuck.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Accepted,
    Running,
    Successful,
    Failed,
    Dismissed,
}

impl JobStatus {
    /// The wire/storage spelling — the one place the string form is written.
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Accepted => "accepted",
            JobStatus::Running => "running",
            JobStatus::Successful => "successful",
            JobStatus::Failed => "failed",
            JobStatus::Dismissed => "dismissed",
        }
    }

    /// Parses a stored/received status. `None` for anything outside the closed
    /// vocabulary — a row carrying an unknown status is a storage anomaly, and
    /// silently mapping it onto `failed` would invent a verdict the ledger
    /// never recorded.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "accepted" => Some(JobStatus::Accepted),
            "running" => Some(JobStatus::Running),
            "successful" => Some(JobStatus::Successful),
            "failed" => Some(JobStatus::Failed),
            "dismissed" => Some(JobStatus::Dismissed),
            _ => None,
        }
    }

    /// Whether the job will never change state again on its own. A terminal
    /// job is never claimed, never re-run, and — for the ledger's dedup index
    /// — no longer occupies its `dedup_key`.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobStatus::Successful | JobStatus::Failed | JobStatus::Dismissed
        )
    }

    /// Every status, in lifecycle order — what a `CHECK` constraint and a
    /// round-trip test enumerate without re-spelling the strings.
    pub const ALL: [JobStatus; 5] = [
        JobStatus::Accepted,
        JobStatus::Running,
        JobStatus::Successful,
        JobStatus::Failed,
        JobStatus::Dismissed,
    ];
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `(tenant, catalog)` a job belongs to, in INTERNAL ids (`#39`).
///
/// Jobs are scoped rather than global because the Processes root is mounted
/// per `(tenant, catalog)` like every other protocol root: a job submitted
/// under one catalog must not be readable — or dismissible — through another.
/// The ledger itself is deployment-wide (`#182`'s "one ledger"), so the scope
/// is a filter on every read, never a separate table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobScope {
    pub tenant: String,
    pub catalog: String,
}

impl JobScope {
    pub fn new(tenant: impl Into<String>, catalog: impl Into<String>) -> Self {
        Self {
            tenant: tenant.into(),
            catalog: catalog.into(),
        }
    }
}

/// What a caller hands [`JobStore::enqueue`].
///
/// `job_id` is minted by the caller, not the store: the HTTP layer needs the
/// id to build the `Location` header it must return, and a store that minted
/// its own would make the idempotent-enqueue case (`dedup_key` already
/// claimed) return an id the caller never saw.
#[derive(Debug, Clone)]
pub struct JobSubmission {
    pub job_id: String,
    pub process_id: String,
    pub scope: JobScope,
    /// The execute request's `inputs` member, verbatim. Stored, never
    /// interpreted here — what an input means is the runner's question.
    pub inputs: Value,
    /// Opt-in enqueue idempotency. `Some(key)` makes a second submission
    /// carrying the same key, in the same scope, for the same process, while
    /// an earlier job is still non-terminal, return that earlier job instead
    /// of creating a second one. `None` — the default, and what a client that
    /// sends no `Idempotency-Key` gets — dedups nothing: two identical
    /// submissions are two jobs, because two identical submissions are, in
    /// general, two deliberate requests.
    pub dedup_key: Option<String>,
}

/// One row of the ledger.
///
/// Field-for-field the subset of OGC API — Processes' `statusInfo.yaml`
/// (Figure 20) this slice can source honestly, plus the scope and the stored
/// `results`. Nothing here is fabricated: `started`/`finished` are `None`
/// until a runner actually starts/finishes, rather than being backfilled from
/// `created` so a field looks populated.
#[derive(Debug, Clone)]
pub struct JobRecord {
    pub job_id: String,
    pub process_id: String,
    pub scope: JobScope,
    pub status: JobStatus,
    /// Free text explaining a terminal state — the `message` member of
    /// `statusInfo.yaml`. `None` while a job is merely accepted or running:
    /// "Process started" would be a message the server invented.
    pub message: Option<String>,
    pub inputs: Value,
    /// The execution result, present only once `status` is
    /// [`JobStatus::Successful`]. `GET /jobs/{jobID}/results` refuses by name
    /// for every other state rather than answering an empty document.
    pub results: Option<Value>,
    pub created: SystemTime,
    pub started: Option<SystemTime>,
    pub finished: Option<SystemTime>,
    pub updated: SystemTime,
    /// How many times this job has been handed to a claimant. Exists so an
    /// operator can see a job that keeps being re-claimed after a runner
    /// death; this slice never uses it to give up (see this module's doc on
    /// what is deliberately deferred).
    pub attempts: i32,
}

/// The verdict a runner reports back through [`JobStore::finish`].
#[derive(Debug, Clone)]
pub enum JobOutcome {
    /// The process produced this results document.
    Succeeded(Value),
    /// The process refused or faulted, with a message safe to show a client —
    /// the runner is responsible for not putting anything sensitive here, the
    /// same rule every `Problem` `detail` in this workspace follows.
    Failed(String),
}

/// A backend that can durably hold jobs (`#182`).
///
/// Advertised through
/// [`StorageDriver::job_store`](crate::router::StorageDriver::job_store), the
/// same `Option`-shaped "this driver never claims this capability" default
/// every other capability accessor uses. Advertising it says nothing about
/// whether the ledger table has actually been provisioned — that is a
/// request-time question each method answers with its backend's own named
/// refusal, exactly the way `write_sink`'s outbox table does.
#[async_trait]
pub trait JobStore: Send + Sync {
    /// Durably records a new job in [`JobStatus::Accepted`], or — when
    /// `submission.dedup_key` names a key an existing non-terminal job in the
    /// same scope already holds — returns that existing job untouched.
    ///
    /// Never DDL. A store whose table is absent refuses by name.
    async fn enqueue(&self, submission: &JobSubmission) -> Result<JobRecord>;

    /// The job with this id inside this scope, or `Ok(None)` when there is
    /// none. A job that exists under a *different* scope answers `Ok(None)`
    /// too: from this catalog's point of view it does not exist, which is the
    /// same non-disclosure rule the protocol-exposure gate follows.
    async fn get(&self, scope: &JobScope, job_id: &str) -> Result<Option<JobRecord>>;

    /// Atomically takes the oldest claimable job whose `process_id` is one of
    /// `process_ids`, marks it [`JobStatus::Running`], and reserves it for
    /// `visibility`.
    ///
    /// - `Ok(Some(job))` — this caller owns the job for the next `visibility`.
    /// - `Ok(None)` — nothing to do right now. An ordinary answer, not an
    ///   error, exactly like [`crate::lease::Lease::try_acquire`]'s own
    ///   `Ok(None)`: an idle ledger is the normal state of a healthy
    ///   deployment.
    ///
    /// `process_ids` is what makes heterogeneous deployments safe against one
    /// ledger (`#182`): a replica only ever claims work it actually has a
    /// runner for, so a lean API pod cannot take a job a GDAL worker build is
    /// the only thing able to execute. An empty slice claims nothing.
    ///
    /// The reservation is a *visibility timeout*, not a lock: a claimant that
    /// dies leaves the job re-claimable once the window lapses. That is the
    /// at-least-once half of this module's contract.
    async fn claim_next(
        &self,
        process_ids: &[String],
        visibility: Duration,
    ) -> Result<Option<JobRecord>>;

    /// Records a claimed job's terminal outcome. `Ok(None)` when no job with
    /// that id is still claimable-or-running — a job dismissed while it ran,
    /// or one whose visibility lapsed and was re-claimed by somebody else.
    /// The caller logs that and moves on; it is not an error, it is the
    /// at-least-once contract being visible.
    async fn finish(&self, job_id: &str, outcome: JobOutcome) -> Result<Option<JobRecord>>;

    /// Moves a non-terminal job to [`JobStatus::Dismissed`] and returns it.
    ///
    /// `Ok(None)` when the scope holds no such job. A job that is ALREADY
    /// terminal is returned unchanged rather than re-dismissed: OGC API —
    /// Processes Requirement 82 (`/req/dismiss/job-dismiss-success`) requires
    /// a dismissal response to carry `status: "dismissed"`, and rewriting a
    /// `successful` job's recorded status to satisfy a response would be
    /// falsifying the ledger — the protocol crate refuses that case instead.
    ///
    /// Dismissal does not stop an in-flight runner in this slice; it stops the
    /// job being re-claimed, and `finish` on a dismissed job is a no-op
    /// (`Ok(None)`).
    async fn dismiss(&self, scope: &JobScope, job_id: &str) -> Result<Option<JobRecord>>;
}

/// A [`JobStore`] resolved for a deployment, alongside the visibility window
/// its claimants use — the two values every runner-side caller needs together,
/// bundled for the same reason [`crate::lease::LeaseBinding`] bundles its own
/// pair: a store with no window, or a window with no store, cannot be
/// constructed and therefore is not a case anyone has to handle.
#[derive(Clone)]
pub struct JobLedger {
    pub store: Arc<dyn JobStore>,
    pub visibility: Duration,
}

impl JobLedger {
    pub fn new(store: Arc<dyn JobStore>, visibility: Duration) -> Self {
        Self { store, visibility }
    }
}

impl fmt::Debug for JobLedger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobLedger")
            .field("visibility", &self.visibility)
            .finish_non_exhaustive()
    }
}

/// An in-memory [`JobStore`], for tests only.
///
/// **Read this before trusting it.** A fixture that builds its own world
/// passes for the wrong reason unless it enforces what the real backing store
/// enforces, so this deliberately reproduces every invariant the `tellurion_jobs`
/// table and `tellurion-postgis::job_sql`'s statements impose, and nothing
/// looser:
///
/// - `get`/`dismiss` filter on the job's scope, like the `tenant`/`catalog`
///   predicates in `build_get_plan`/`build_dismiss_plan`;
/// - `enqueue` dedups only on a **present** key held by a **non-terminal** job,
///   like the partial unique index's `dedup_key IS NOT NULL AND status IN
///   ('accepted','running')` predicate;
/// - `claim_next` takes the oldest job whose `process_id` is in the caller's
///   set and which is either `accepted` or a `running` job whose reservation
///   lapsed, like `build_claim_plan`'s subquery, and stamps
///   `attempts`/`started`/`locked_until` the same way;
/// - `finish` only applies to a job that is still `running`, like
///   `build_finish_plan`'s `AND status = 'running'` guard;
/// - `dismiss` returns an already-terminal job unchanged rather than rewriting
///   its status.
///
/// What it cannot reproduce is concurrency: there is no `SKIP LOCKED` here,
/// only a mutex. Claim exclusivity under real contention is asserted on the
/// SQL text itself (`job_sql`'s own tests), which is the only place it is
/// actually decided.
#[cfg(any(test, feature = "test-support"))]
pub struct InMemoryJobStore {
    jobs: std::sync::Mutex<Vec<InMemoryJobRow>>,
}

/// One stored row: the record a caller sees, plus the two columns the real
/// table carries that a [`JobRecord`] deliberately does not — the dedup key
/// (an operator-facing idempotency detail, not part of a job's status) and the
/// visibility reservation (a claimant-facing one).
#[cfg(any(test, feature = "test-support"))]
struct InMemoryJobRow {
    record: JobRecord,
    dedup_key: Option<String>,
    locked_until: Option<SystemTime>,
}

#[cfg(any(test, feature = "test-support"))]
impl Default for InMemoryJobStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl InMemoryJobStore {
    pub fn new() -> Self {
        Self {
            jobs: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl JobStore for InMemoryJobStore {
    async fn enqueue(&self, submission: &JobSubmission) -> Result<JobRecord> {
        let mut jobs = self.jobs.lock().expect("job store mutex");
        if let Some(key) = submission.dedup_key.as_deref() {
            if let Some(existing) = jobs.iter().find(|row| {
                row.dedup_key.as_deref() == Some(key)
                    && row.record.scope == submission.scope
                    && row.record.process_id == submission.process_id
                    && !row.record.status.is_terminal()
            }) {
                return Ok(existing.record.clone());
            }
        }
        if jobs
            .iter()
            .any(|row| row.record.job_id == submission.job_id)
        {
            return Err(crate::error::Error::Conflict(format!(
                "job id '{}' is already recorded in the ledger",
                submission.job_id
            )));
        }
        let now = SystemTime::now();
        let record = JobRecord {
            job_id: submission.job_id.clone(),
            process_id: submission.process_id.clone(),
            scope: submission.scope.clone(),
            status: JobStatus::Accepted,
            message: None,
            inputs: submission.inputs.clone(),
            results: None,
            created: now,
            started: None,
            finished: None,
            updated: now,
            attempts: 0,
        };
        jobs.push(InMemoryJobRow {
            record: record.clone(),
            dedup_key: submission.dedup_key.clone(),
            locked_until: None,
        });
        Ok(record)
    }

    async fn get(&self, scope: &JobScope, job_id: &str) -> Result<Option<JobRecord>> {
        let jobs = self.jobs.lock().expect("job store mutex");
        Ok(jobs
            .iter()
            .find(|row| row.record.job_id == job_id && &row.record.scope == scope)
            .map(|row| row.record.clone()))
    }

    async fn claim_next(
        &self,
        process_ids: &[String],
        visibility: Duration,
    ) -> Result<Option<JobRecord>> {
        let now = SystemTime::now();
        let mut jobs = self.jobs.lock().expect("job store mutex");
        let mut claimable: Vec<usize> = (0..jobs.len())
            .filter(|index| {
                let row = &jobs[*index];
                if !process_ids.contains(&row.record.process_id) {
                    return false;
                }
                match row.record.status {
                    JobStatus::Accepted => true,
                    JobStatus::Running => row.locked_until.is_some_and(|until| until < now),
                    _ => false,
                }
            })
            .collect();
        claimable.sort_by_key(|index| jobs[*index].record.created);
        let Some(index) = claimable.first().copied() else {
            return Ok(None);
        };
        let row = &mut jobs[index];
        row.record.status = JobStatus::Running;
        row.record.attempts += 1;
        row.record.started = row.record.started.or(Some(now));
        row.record.updated = now;
        row.locked_until = Some(now + visibility);
        Ok(Some(row.record.clone()))
    }

    async fn finish(&self, job_id: &str, outcome: JobOutcome) -> Result<Option<JobRecord>> {
        let now = SystemTime::now();
        let mut jobs = self.jobs.lock().expect("job store mutex");
        let Some(row) = jobs
            .iter_mut()
            .find(|row| row.record.job_id == job_id && row.record.status == JobStatus::Running)
        else {
            return Ok(None);
        };
        match outcome {
            JobOutcome::Succeeded(results) => {
                row.record.status = JobStatus::Successful;
                row.record.results = Some(results);
            }
            JobOutcome::Failed(message) => {
                row.record.status = JobStatus::Failed;
                row.record.message = Some(message);
            }
        }
        row.record.finished = Some(now);
        row.record.updated = now;
        row.locked_until = None;
        Ok(Some(row.record.clone()))
    }

    async fn dismiss(&self, scope: &JobScope, job_id: &str) -> Result<Option<JobRecord>> {
        let now = SystemTime::now();
        let mut jobs = self.jobs.lock().expect("job store mutex");
        let Some(row) = jobs
            .iter_mut()
            .find(|row| row.record.job_id == job_id && &row.record.scope == scope)
        else {
            return Ok(None);
        };
        if row.record.status.is_terminal() {
            return Ok(Some(row.record.clone()));
        }
        row.record.status = JobStatus::Dismissed;
        row.record.message = Some("Job dismissed".to_string());
        row.record.finished = Some(now);
        row.record.updated = now;
        row.locked_until = None;
        Ok(Some(row.record.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The closed vocabulary round-trips, in both directions, for every
    /// variant. Worth pinning because the same five strings are written into a
    /// `CHECK` constraint by `tellurion-ingest processes create-tables` and
    /// into a `statusInfo` response by `tellurion-processes`: a variant whose
    /// spelling drifts here would be a row the ledger accepts and this crate
    /// then refuses to read back.
    #[test]
    fn every_status_round_trips_through_its_wire_spelling() {
        for status in JobStatus::ALL {
            assert_eq!(JobStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(JobStatus::ALL.len(), 5);
    }

    /// A status outside the vocabulary is `None`, never coerced onto a
    /// neighbour.
    #[test]
    fn an_unknown_status_is_refused_rather_than_coerced() {
        assert_eq!(JobStatus::parse("succeeded"), None);
        assert_eq!(JobStatus::parse("ACCEPTED"), None);
        assert_eq!(JobStatus::parse(""), None);
    }

    /// Exactly the three states OGC API — Processes' own job list treats as
    /// "completed execution" are terminal here (Requirement 75's own wording:
    /// "have completed execution (`successful`, `failed` or `dismissed`)").
    #[test]
    fn terminal_states_are_exactly_the_three_completed_ones() {
        let terminal: Vec<&str> = JobStatus::ALL
            .into_iter()
            .filter(|status| status.is_terminal())
            .map(JobStatus::as_str)
            .collect();
        assert_eq!(terminal, vec!["successful", "failed", "dismissed"]);
    }

    fn submission(job_id: &str, dedup_key: Option<&str>) -> JobSubmission {
        JobSubmission {
            job_id: job_id.to_string(),
            process_id: "p".to_string(),
            scope: JobScope::new("tenant-1", "catalog-1"),
            inputs: serde_json::json!({}),
            dedup_key: dedup_key.map(str::to_string),
        }
    }

    /// The fixture's own contract, checked against what the real table
    /// enforces (see [`InMemoryJobStore`]'s doc): a key is only honoured while
    /// the job holding it is non-terminal, and no key means no dedup at all.
    #[tokio::test]
    async fn an_idempotency_key_dedups_only_while_its_job_is_still_in_play() {
        let store = InMemoryJobStore::new();
        let first = store.enqueue(&submission("a", Some("k"))).await.unwrap();
        let second = store.enqueue(&submission("b", Some("k"))).await.unwrap();
        assert_eq!(first.job_id, second.job_id, "the key must return job 'a'");

        // Two unkeyed submissions are two jobs.
        let c = store.enqueue(&submission("c", None)).await.unwrap();
        let d = store.enqueue(&submission("d", None)).await.unwrap();
        assert_ne!(c.job_id, d.job_id);

        // Once 'a' is terminal the key is reusable.
        store
            .dismiss(&JobScope::new("tenant-1", "catalog-1"), "a")
            .await
            .unwrap();
        let reused = store.enqueue(&submission("e", Some("k"))).await.unwrap();
        assert_eq!(reused.job_id, "e");
    }

    /// A claim only ever takes work this caller can execute, and only takes
    /// each job once until its reservation lapses.
    #[tokio::test]
    async fn a_claim_takes_only_a_registered_process_and_reserves_it() {
        let store = InMemoryJobStore::new();
        store.enqueue(&submission("a", None)).await.unwrap();

        assert!(store
            .claim_next(&["other".to_string()], Duration::from_secs(60))
            .await
            .unwrap()
            .is_none());
        assert!(store
            .claim_next(&[], Duration::from_secs(60))
            .await
            .unwrap()
            .is_none());

        let claimed = store
            .claim_next(&["p".to_string()], Duration::from_secs(60))
            .await
            .unwrap()
            .expect("the only accepted job");
        assert_eq!(claimed.status, JobStatus::Running);
        assert_eq!(claimed.attempts, 1);
        assert!(claimed.started.is_some());

        // Reserved: a second claimant gets nothing while the window holds.
        assert!(store
            .claim_next(&["p".to_string()], Duration::from_secs(60))
            .await
            .unwrap()
            .is_none());
    }

    /// A lapsed reservation returns the job to the pool — the at-least-once
    /// recovery path for a claimant that died mid-execution.
    #[tokio::test]
    async fn a_lapsed_reservation_returns_the_job_to_the_pool() {
        let store = InMemoryJobStore::new();
        store.enqueue(&submission("a", None)).await.unwrap();
        store
            .claim_next(&["p".to_string()], Duration::from_millis(1))
            .await
            .unwrap()
            .expect("first claim");
        tokio::time::sleep(Duration::from_millis(5)).await;
        let reclaimed = store
            .claim_next(&["p".to_string()], Duration::from_secs(60))
            .await
            .unwrap()
            .expect("the lapsed job is claimable again");
        assert_eq!(reclaimed.attempts, 2);
    }

    /// An outcome may only be recorded by a claimant that still owns the job.
    #[tokio::test]
    async fn an_outcome_from_a_claimant_that_lost_the_job_is_not_an_error() {
        let store = InMemoryJobStore::new();
        let scope = JobScope::new("tenant-1", "catalog-1");
        store.enqueue(&submission("a", None)).await.unwrap();
        store
            .claim_next(&["p".to_string()], Duration::from_secs(60))
            .await
            .unwrap()
            .expect("claim");
        store.dismiss(&scope, "a").await.unwrap();
        assert!(store
            .finish("a", JobOutcome::Succeeded(serde_json::json!({})))
            .await
            .unwrap()
            .is_none());
        // The ledger still records the dismissal, not a success.
        let record = store.get(&scope, "a").await.unwrap().unwrap();
        assert_eq!(record.status, JobStatus::Dismissed);
    }

    /// Reads are scoped: another catalog's job does not exist here.
    #[tokio::test]
    async fn a_job_is_invisible_outside_its_own_catalog() {
        let store = InMemoryJobStore::new();
        store.enqueue(&submission("a", None)).await.unwrap();
        assert!(store
            .get(&JobScope::new("tenant-1", "other-catalog"), "a")
            .await
            .unwrap()
            .is_none());
        assert!(store
            .dismiss(&JobScope::new("other-tenant", "catalog-1"), "a")
            .await
            .unwrap()
            .is_none());
    }

    /// Dismissing an already-finished job reports it unchanged rather than
    /// rewriting its recorded status.
    #[tokio::test]
    async fn dismissing_a_finished_job_never_rewrites_its_verdict() {
        let store = InMemoryJobStore::new();
        let scope = JobScope::new("tenant-1", "catalog-1");
        store.enqueue(&submission("a", None)).await.unwrap();
        store
            .claim_next(&["p".to_string()], Duration::from_secs(60))
            .await
            .unwrap()
            .expect("claim");
        store
            .finish(
                "a",
                JobOutcome::Succeeded(serde_json::json!({ "ok": true })),
            )
            .await
            .unwrap()
            .expect("finish");
        let after = store.dismiss(&scope, "a").await.unwrap().unwrap();
        assert_eq!(after.status, JobStatus::Successful);
        assert_eq!(after.results, Some(serde_json::json!({ "ok": true })));
    }
}
