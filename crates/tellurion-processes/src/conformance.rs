//! What this crate claims, and — at greater length, because it is the more
//! consequential half — what it deliberately does not.
//!
//! # No `ogcapi-processes-1` conformance class is declared
//!
//! OGC API — Processes — Part 1: Core is an approved OGC Standard (OGC
//! 18-062r2, version 1.0.0 — verified 2026-08 against the published document
//! at `docs.ogc.org/is/18-062r2/18-062r2.html`), so, exactly as with OGC API —
//! Records in `tellurion_records::conformance`, this is not "no class exists
//! yet to cite" (the OGC API — Styles / 3D GeoVolumes situation
//! `tellurion_places::conformance` describes). Eight real class URIs exist
//! (Table 8). This crate declares none of them, and every refusal below traces
//! to a requirement this slice does not honour.
//!
//! ## The Core class, and why every other class falls with it
//!
//! Seven of the eight classes — OGC Process Description, JSON, HTML, OpenAPI
//! 3.0, Job List, Callback, Dismiss — each list **OGC API — Processes Core**
//! as a dependency in their own Requirements-class table (clauses 8.1, 9.2,
//! 9.3, 10.1, 11.1, 12.1, 13.1). So `.../conf/core` is the gate: withhold it
//! and nothing else may be claimed either, regardless of how completely the
//! dependent class itself is implemented. Two independent requirements of the
//! Core class are unmet here:
//!
//! - **Requirement 18 (`/req/core/process-execute-inputs`), clause B**: "The
//!   server SHALL support process input values specified by reference (i.e.
//!   using a link)", and Requirement 24
//!   (`/req/core/process-execute-input-validation`) clause B adds that such a
//!   value SHALL be resolved and then validated as if it had been inline. This
//!   lane resolves nothing: an execute request's `inputs` are stored verbatim
//!   and handed to the runner. Dereferencing a client-supplied URL from inside
//!   the server is a deliberate non-feature in a first slice — it is a
//!   server-side request forgery primitive that would need its own allow-list
//!   design — so the clause is unmet by choice, not by omission.
//!
//! - **Requirement 9 (`/req/core/pl-limit-definition`) and Requirement 10
//!   (`/req/core/pl-limit-response`)**: `GET /processes` SHALL support a
//!   `limit` parameter. This lane serves the whole registered process list,
//!   which is bounded by what the binary was compiled with rather than by
//!   anything a client can page through, and implements no `limit` at all.
//!
//! Requirement 2 (`/req/core/landingpage-success`) is a third, weaker
//! obstacle worth recording because it is the one this workspace could not fix
//! locally even if it wanted the class: it requires the landing page to carry
//! a `/processes` link with relation type
//! `http://www.opengis.net/def/rel/ogc/1.0/processes` **and** a `/conformance`
//! link with relation type `http://www.opengis.net/def/rel/ogc/1.0/conformance`.
//! Every protocol root in this server shares one landing-page builder
//! (`tellurion-server`'s `landing::protocol_landing`) which emits the short
//! `conformance` relation, and changing that would change five other roots'
//! responses — the exact "an unconfigured deployment is byte-for-byte what it
//! was" rule this campaign runs under. The `processes` link IS emitted with
//! the Standard's own relation URI, since that arm is new and breaks nothing.
//!
//! ## The other classes, each on its own terms
//!
//! Even setting the Core dependency aside, none of these is earned:
//!
//! - **`.../conf/ogc-process-description`** (Requirements class 2) requires,
//!   in Requirement 47 (`/req/ogc-process-description/json-encoding`), that a
//!   process description validate against `process.yaml`, and Requirements 48
//!   and 52 (`/req/ogc-process-description/inputs-def`, `/outputs-def`)
//!   require declared `inputs`/`outputs` blocks. This slice's
//!   `ProcessDescription` carries neither — see that type's own doc for why
//!   empty blocks would be worse than absent ones.
//!
//! - **`.../conf/json`** (Requirements class 3) is the one class this lane
//!   comes closest to satisfying: Requirement 55 (`/req/json/definition`)
//!   asks that `application/json` be supported on `/`, `/conformance`,
//!   `/processes`, `/processes/{processID}`, `/jobs/{jobID}` and — for an
//!   asynchronous execution — `/processes/{processID}/execution`, and every
//!   one of those is served as `application/json` here. It still cannot be
//!   claimed: its own Requirements-class table lists Core as a dependency, and
//!   Core is withheld above.
//!
//! - **`.../conf/html`** (Requirements class 4): Requirement 56
//!   (`/req/html/definition`) makes `text/html` mandatory on **every**
//!   200-response. This server serves no HTML representation of anything.
//!
//! - **`.../conf/oas30`** (Requirements class 5): Requirement 58
//!   (`/req/oas30/oas-definition-1`) requires that both a JSON API definition
//!   *and* "a HTML version of the API definition using the media type
//!   `text/html`" be available. `/api` serves JSON only. (The `oas30` class
//!   this root's `/conformance` does list is OGC API — Common's, which every
//!   root in this workspace has always listed — a different class URI with
//!   different requirements, not this one.)
//!
//! - **`.../conf/job-list`** (Requirements class 6): `GET /jobs` is not served
//!   at all in this slice, so Requirement 64 (`/req/job-list/job-list-op`)
//!   fails at the first step, along with the whole parameter set Requirements
//!   65-77 mandate (`type`, `processID`, `status`, `datetime`,
//!   `minDuration`/`maxDuration`, `limit`).
//!
//! - **`.../conf/callback`** (Requirements class 7): Requirement 80
//!   (`/req/callback/job-callback`) requires the server to POST results to the
//!   `subscriber` URIs of an execute request. Not implemented; the
//!   `subscriber` member is not read.
//!
//! - **`.../conf/dismiss`** (Requirements class 8) is, like `json`, materially
//!   satisfied on its own terms — `DELETE /jobs/{jobID}` is served
//!   (Requirement 81, `/req/dismiss/job-dismiss-op`) and answers `200` with a
//!   `statusInfo` whose status is `dismissed` (Requirement 82,
//!   `/req/dismiss/job-dismiss-success`) — and is withheld solely on its Core
//!   dependency.
//!
//! # Two defects in the published Standard, recorded because they were hit
//!
//! Both were found by reading OGC 18-062r2 itself rather than reciting it, and
//! neither changes anything this crate does — they are noted so the next
//! reader does not spend the same time deciding which spelling is
//! authoritative:
//!
//! 1. **The execute/job requirements have two different identifier families.**
//!    The normative body names them `/req/core/process-execute-op` (Requirement
//!    16), `/req/core/process-execute-request` (17),
//!    `/req/core/process-execute-inputs` (18), and so on. Annex A's Abstract
//!    Test Suite — itself normative — cites the same requirements as
//!    `/req/core/job-creation-op`, `/req/core/job-creation-request`,
//!    `/req/core/job-creation-inputs`, … identifiers that appear nowhere in the
//!    requirements clauses. Requirement 15's identifier likewise reads
//!    `/req/core/process-exception/no-such-process` in clause 7.10.3 and
//!    `/req/core/process-exception-no-such-process` in Annex A. This crate
//!    cites the requirements-clause spellings throughout, since those are the
//!    ones attached to the requirement text being quoted.
//!
//! 2. **Requirement 71 and Requirement 75 share one identifier.** Both are
//!    labelled `/req/job-list/status-response`, but Requirement 71 governs the
//!    `status` parameter's filtering and Requirement 75 governs
//!    `minDuration`/`maxDuration` (its own clauses D-G talk only about
//!    durations). The second is presumably meant to be
//!    `/req/job-list/duration-response`. Irrelevant here only because the Job
//!    List class is withheld entirely.
//!
//! # Shapes followed without claiming conformance
//!
//! Everything this lane serves still follows the Standard's own shapes, the
//! same way `tellurion-records` follows Records' and `tellurion-places`
//! follows 3D GeoVolumes': the paths, the `statusInfo`/`processList` document
//! shapes, the closed status vocabulary, the `201`-plus-`Location` answer to an
//! asynchronous execution (Requirement 34), and the exception `type` URIs for
//! `no-such-process`/`no-such-job`/`result-not-ready`. A client that knows OGC
//! API — Processes finds what it expects, and a later slice that closes the
//! gaps above can declare the classes without reshaping a single response.

/// `statusInfo.yaml`'s required `type` member (Figure 20), whose schema is an
/// enum with the single value `process`.
pub const JOB_TYPE_PROCESS: &str = "process";

/// The link relation OGC API — Processes Requirement 2
/// (`/req/core/landingpage-success`) gives the process list on a landing page.
/// Emitted even though the Core class is withheld: this arm of the landing
/// page is new, so using the Standard's own relation costs no other root a
/// byte — see this module's doc for the `conformance` relation that could not
/// be changed the same way.
pub const REL_PROCESSES: &str = "http://www.opengis.net/def/rel/ogc/1.0/processes";

/// The relation a job's own status document uses to point at its process
/// (Recommendation 16's `processID` is the machine-readable half; this is the
/// navigable one).
pub const REL_SELF: &str = "self";

/// The relation Requirement 33 (`/req/core/job-results-success-sync`) gives a
/// link to a created job, reused here on the asynchronous path alongside the
/// mandatory `Location` header: `Location` is the Standard's requirement,
/// `rel="monitor"` is what `#182` asks for so a client that follows links
/// rather than headers can find the job too.
pub const REL_MONITOR: &str = "monitor";

/// The relation from a job's status document to its results.
pub const REL_RESULTS: &str = "http://www.opengis.net/def/rel/ogc/1.0/results";

/// Requirement 15 (`/req/core/process-exception/no-such-process`): "The type
/// of the exception SHALL be
/// `http://www.opengis.net/def/exceptions/ogcapi-processes-1/1.0/no-such-process`."
pub const EXCEPTION_NO_SUCH_PROCESS: &str =
    "http://www.opengis.net/def/exceptions/ogcapi-processes-1/1.0/no-such-process";

/// Requirement 37 (`/req/core/job-exception-no-such-job`) and Requirement 44
/// (`/req/core/job-results-exception/no-such-job`), which mandate the same
/// exception type for an unknown job on the status and results resources
/// respectively.
pub const EXCEPTION_NO_SUCH_JOB: &str =
    "http://www.opengis.net/def/exceptions/ogcapi-processes-1/1.0/no-such-job";

/// Requirement 45 (`/req/core/job-results-exception/results-not-ready`): the
/// results of a job that has not finished are a `404` whose exception type is
/// `.../result-not-ready`.
///
/// Note the Standard's own singular/plural mismatch: the requirement is
/// identified `.../results-not-ready` while the type value it mandates is
/// `.../result-not-ready`. The value below is the one the requirement text
/// actually specifies, which is what a client would match on.
pub const EXCEPTION_RESULT_NOT_READY: &str =
    "http://www.opengis.net/def/exceptions/ogcapi-processes-1/1.0/result-not-ready";

pub const JSON_MEDIA_TYPE: &str = "application/json";
