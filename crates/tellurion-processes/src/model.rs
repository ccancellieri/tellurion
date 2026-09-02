//! Response DTOs for the OGC API — Processes surface. Field order in each
//! struct is JSON key order (serde serializes struct fields in declaration
//! order), the same convention `tellurion_features::model` documents.
//!
//! Deliberately small, for the same reason `tellurion_records::model` is: each
//! type carries what this lane can source honestly and nothing more. No
//! fabricated `progress`, no empty `inputs`/`outputs` blocks, no `started`
//! backfilled from `created` so a field looks populated. OGC API — Processes —
//! Part 1: Core marks every `statusInfo.yaml` member except `jobID`, `status`
//! and `type` optional (Figure 20), and every `processSummary.yaml` member
//! except `id` and `version` optional (Figure 6), which makes the absent ones
//! absent rather than wrong.

use serde::Serialize;

/// A link in a process or job document. Structurally identical to
/// `tellurion_features::Link`'s core three fields; a separate type because no
/// protocol crate in this workspace depends on another (see each crate's own
/// `Cargo.toml` — they all depend on `tellurion-core` alone), the same
/// duplication `tellurion_records::model::Link` already carries.
#[derive(Debug, Clone, Serialize)]
pub struct Link {
    pub href: String,
    pub rel: String,
    #[serde(rename = "type")]
    pub media_type: String,
}

impl Link {
    pub fn new(
        href: impl Into<String>,
        rel: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Self {
        Self {
            href: href.into(),
            rel: rel.into(),
            media_type: media_type.into(),
        }
    }
}

/// One entry of `GET /processes`, and the whole body of
/// `GET /processes/{processID}`.
///
/// The Core class deliberately "does not mandate the use of a specific process
/// description" (clause 7.10.2, following Requirement 14) — the fuller OGC
/// Process Description is a separate requirements class this slice does not
/// implement, so this is the summary shape for both resources. `id` and
/// `version` are the two members `processSummary.yaml` requires.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessSummary {
    pub id: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `jobControlOptions.yaml` (Figure 7). Never empty: a process no client
    /// could ever legally invoke has no business being listed.
    #[serde(rename = "jobControlOptions")]
    pub job_control_options: Vec<String>,
    pub links: Vec<Link>,
}

/// `GET /processes` — `processList.yaml` (Figure 5) requires both members.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessList {
    pub processes: Vec<ProcessSummary>,
    pub links: Vec<Link>,
}

/// `statusInfo.yaml` (Figure 20): the body of `GET /jobs/{jobID}`, of a
/// successful dismissal, and of the `201` an asynchronous execution answers
/// with (Requirement 34 clause C).
///
/// `jobID`, `status` and `type` are the three required members; everything
/// else is optional and omitted when this lane has nothing true to put there.
/// `progress` in particular is absent throughout: no runner in this slice
/// reports incremental progress, and a hardcoded `0` would be a number the
/// server made up.
#[derive(Debug, Clone, Serialize)]
pub struct StatusInfo {
    #[serde(rename = "jobID")]
    pub job_id: String,
    #[serde(rename = "processID")]
    pub process_id: String,
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub created: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<String>,
    pub updated: String,
    pub links: Vec<Link>,
}
