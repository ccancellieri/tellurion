//! An OGC API — Processes — Part 1: Core surface (`#182`): describe, execute,
//! monitor, fetch results, dismiss — over a **durable job ledger the server
//! never provisions** and **runners this binary was compiled with**.
//!
//! # The two capabilities this root cannot exist without
//!
//! Tellurion's background work has always been a fixed internal consumer set.
//! `#182` adds an operator-visible one, and the platform rule it inherits
//! rather than reinvents is that a lane exists only where the capabilities it
//! needs really do:
//!
//! - a **ledger** — [`tellurion_core::JobStore`], advertised through the
//!   `Option`-shaped `StorageDriver::job_store` accessor that defaults to
//!   `None`, over a table `tellurion-ingest processes create-tables` creates
//!   and this server refuses by name when it is absent
//!   (`tellurion-postgis`'s `JobsTableMissing`); and
//! - a **runner set** — [`tellurion_core::ProcessRegistry`], the `#112`
//!   boot-time named registry, whose ids are exactly what the ledger's claim
//!   query filters on so a lean API pod cannot take a job only a worker build
//!   can execute.
//!
//! With either missing, `tellurion-server` mounts no Processes root at all and
//! the prefix answers the `404` an unmounted one answers. That is deliberate
//! and it is the whole point: a root that accepts a job it cannot record, or
//! advertises a process nothing can run, is worse than no root.
//!
//! And with both present, the root is *still* invisible until an operator asks
//! for it — `protocols.processes` defaults to `disabled`, exactly as
//! `protocols.records` does (`#192`), because "what this deployment already
//! did" is nothing at all.
//!
//! # Conformance
//!
//! This crate declares **no** `ogcapi-processes-1` conformance class, and the
//! reasoning per class — with the requirement identifiers behind each refusal,
//! and two defects found in the published Standard along the way — is in
//! [`conformance`]'s own module documentation. Read it before adding one.
//!
//! # Explicit non-goals of this slice
//!
//! Recorded here rather than discovered later: no synchronous execution, no
//! inputs by reference, no `GET /jobs` job list, no callbacks, no OGC Process
//! Description `inputs`/`outputs` schemas, no progress reporting, no
//! `pg_notify` wake-up (the runner polls), no backoff/dead-letter escalation,
//! and no container isolation — a runner executes in the server's own process
//! with the server's own privileges, which is why the only processes
//! registered are built-ins compiled into the binary. Exactly-once execution
//! is a permanent non-goal, not a deferred one (`#182`'s own words); the
//! contract is at-least-once and [`tellurion_core::job`] says so.

/// The conformance stance — read this before adding a class. Public so the
/// per-class refusal rationale (with the OGC requirement identifiers behind
/// each one) is reachable from the rendered documentation, not just from the
/// source.
pub mod conformance;
mod handlers;
mod model;
mod problem;
mod router;

pub use conformance::{JOB_TYPE_PROCESS, JSON_MEDIA_TYPE, REL_PROCESSES};
pub use handlers::{ExecuteRequest, DEFAULT_CATALOG, DEFAULT_TENANT};
pub use model::{Link, ProcessList, ProcessSummary, StatusInfo};
pub use problem::ApiError;
pub use router::router;

/// The conformance classes this root cites beyond the OGC API — Common ones
/// every protocol root in this workspace cites — deliberately empty.
///
/// Present as a named, empty constant rather than absent so the server's
/// `landing::conformance_classes` can extend from it exactly the way it
/// extends from `tellurion_features::CONFORMANCE_CLASSES` and
/// `tellurion_records::CONFORMANCE_CLASSES`, and so that a later slice which
/// genuinely earns a class has one obvious place to add it — with
/// [`conformance`]'s refusal rationale sitting right next to it.
pub const CONFORMANCE_CLASSES: &[&str] = &[];
