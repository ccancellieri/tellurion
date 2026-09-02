//! The axum router this crate exposes. Mounting is the server crate's
//! decision — `tellurion-server` nests it at
//! `/{tenant}/processes/catalogs/{catalog}`, gated by the
//! `protocols.processes` exposure key (`#185`/`#182`) *and* by the
//! deployment actually having a job ledger and at least one runner, the same
//! shape every other protocol root is mounted with plus the capability gate
//! `#182` requires.
//!
//! Paths are OGC API — Processes — Part 1: Core's own, verbatim: `/processes`
//! (Requirement 8), `/processes/{processID}` (Requirement 13),
//! `/processes/{processID}/execution` (Requirement 16), `/jobs/{jobID}`
//! (Requirements 35 and 81) and `/jobs/{jobID}/results` (Requirement 38).
//!
//! `GET /jobs` — the Job List requirements class (Requirement 64) — is
//! deliberately absent: that class mandates a parameter set (`type`,
//! `processID`, `status`, `datetime`, `minDuration`/`maxDuration`, `limit`,
//! Requirements 65-77) this slice does not implement, and a `/jobs` that
//! honoured none of them would be a resource whose conformance class could
//! never be claimed and whose behaviour no client could rely on. See
//! `crate::conformance`.

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use tellurion_core::AppContext;

use crate::handlers;

pub fn router() -> Router<Arc<AppContext>> {
    Router::new()
        .route("/processes", get(handlers::list_processes))
        .route("/processes/{processID}", get(handlers::get_process))
        .route(
            "/processes/{processID}/execution",
            post(handlers::execute_process),
        )
        .route(
            "/jobs/{jobID}",
            get(handlers::get_job).delete(handlers::dismiss_job),
        )
        .route("/jobs/{jobID}/results", get(handlers::get_job_results))
}
