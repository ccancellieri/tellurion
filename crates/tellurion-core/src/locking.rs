//! OGC API Features — Part 4 (OGC 20-002r1, currently `1.0.0-draft.2`,
//! "draft for Public Comment", verified 2026-07 against
//! `https://docs.ogc.org/DRAFTS/20-002r1.html") Optimistic Locking machinery:
//! the two requirement classes `req/optimistic-locking-etags` and
//! `req/optimistic-locking-timestamps` (`#107`). Framework-free domain logic
//! only — no axum/http types here — the same "domain logic here, framework
//! wiring in the protocol crate" split `asset.rs`'s own RFC 9530 digest
//! machinery already follows; `tellurion-features::write_handlers`/
//! `handlers` are the callers that turn these primitives into real request
//! preconditions and response headers.
//!
//! ## ETags: strong, content-derived, storage-canonical
//!
//! An ETag is a `sha2::Sha256` digest of the exact JSON representation
//! [`crate::storage::FeatureSource::item`] returns for a feature —
//! [`compute_feature_etag`] reuses [`crate::asset::compute_sha256`], the
//! same digest primitive RFC 9530 `Repr-Digest` already uses, rather than a
//! second hashing implementation. This is a STRONG validator (RFC 7232
//! section 2.1: byte-for-byte equivalence, not merely "semantically
//! equivalent"): this workspace's `serde_json::Map` is its default
//! `BTreeMap`-backed shape (no `preserve_order` feature enabled anywhere in
//! this dependency graph), so serializing the same underlying JSON value
//! twice always produces the same bytes regardless of the order a driver
//! happened to build its properties object in — hashing it twice with no
//! write landing in between always yields the identical digest.
//!
//! Deliberately computed from the CANONICAL representation — the same one
//! `FeatureSource::item` (never `item_with_crs`) returns regardless of a
//! request's own `?crs=`/`?bbox-crs=` choice — never from whatever
//! CRS-reprojected body a given `GET` happens to serve. A `PUT`/`DELETE`'s
//! `If-Match` header carries no CRS context of its own (a write request
//! takes no `?crs=` parameter; `Content-Crs` on the request body is a
//! different, input-side concern), so the write-side comparison has exactly
//! one representation to compare against; a `GET`'s own `ETag` response
//! header has to be the same one, or a client that fetched in a non-default
//! CRS would see its own, perfectly fresh `If-Match` value spuriously
//! rejected by a later write — not because the resource actually changed,
//! but because two reads of an UNCHANGED resource hashed two different
//! bodies. Practically this means the ETag changes whenever a write lands a
//! different STORED geometry (e.g. the default-CRS transform path — a write
//! that reprojects on the way in, or any other write that changes what's
//! actually persisted), since it is always recomputed from a fresh read of
//! storage, never cached from the write's own input — but it does NOT
//! change just because a later `GET` asks for a different output CRS.
//!
//! ## Timestamps: never fabricated, only ever a real declared column
//!
//! There is no server-wide "modified" concept here — a collection's
//! `Last-Modified`/`If-Unmodified-Since` support depends entirely on
//! whether its own declaration (`CollectionDecl::modified_column`) names a
//! real backend column, and, if so, that column's value reaches this module
//! exactly the way any other attribute does: through
//! `FeatureSource::item`'s own `properties`. This module only parses,
//! compares, and formats; it never invents a value no declared source
//! produced.
//!
//! ## Closing the check-to-apply window (`#150`)
//!
//! Everything above evaluates a precondition against ONE read of the
//! target's current state. That read and the write it guards are two
//! separate round trips, so between them a concurrent writer can commit and
//! invalidate the precondition that just passed — both writers' checks
//! succeed, both writes land, and the second silently clobbers the first:
//! the exact lost update optimistic locking exists to prevent.
//!
//! [`RowVersion`] closes that window. It is an opaque, driver-minted witness
//! of one row's current version, captured at precondition time and handed
//! back to the driver with the mutation ([`crate::outbox::WriteSink::
//! apply_conditional`]), which re-verifies it as a predicate the DATABASE
//! evaluates in the same transaction as the write. The ETag itself is never
//! weakened: it stays a hash of the served representation, computed in Rust,
//! and the witness only has to answer the narrower question "has this row
//! changed since I read it?" — which is what a stored row version can answer
//! atomically and a content hash cannot.

use std::fmt;
use std::time::{Duration, SystemTime};

use crate::asset::compute_sha256;

/// OGC API Features — Part 4 (20-002r1, draft), Optimistic Locking: ETags
/// requirement class URI.
pub const OPTIMISTIC_LOCKING_ETAGS_CLASS: &str =
    "http://www.opengis.net/spec/ogcapi-features-4/1.0/req/optimistic-locking-etags";
/// OGC API Features — Part 4 (20-002r1, draft), Optimistic Locking:
/// Timestamps requirement class URI.
pub const OPTIMISTIC_LOCKING_TIMESTAMPS_CLASS: &str =
    "http://www.opengis.net/spec/ogcapi-features-4/1.0/req/optimistic-locking-timestamps";

/// The full candidate set [`crate::router::Router::locking_conformance_classes`]'s
/// workspace-wide intersection fold starts from — mirrors
/// [`crate::filter::CQL2_CONFORMANCE_CLASSES`]'s own role for CQL2 exactly.
/// When no write-capable driver participates, the fold's capability policy
/// discards this seed because the deployment has nowhere to honour it.
/// Contains only the ETags class: Timestamps is never driver-declared (see
/// this module's own doc) — it is a per-collection fact
/// (`CollectionDecl::modified_column.is_some()`), so it has no place in a
/// workspace-wide, driver-keyed fold and never appears in this seed.
pub const LOCKING_CONFORMANCE_CLASSES: &[&str] = &[OPTIMISTIC_LOCKING_ETAGS_CLASS];

/// An opaque witness of one stored row's current version, minted by a
/// driver that can compare it against the row's live version INSIDE the
/// write transaction (`#150`). PostGIS mints the row's `xmin` — the
/// transaction id that last wrote it, which every `INSERT`/`UPDATE` on that
/// row necessarily changes.
///
/// Deliberately opaque and never compared in Rust: the only sound comparison
/// is the one the database performs against the row it is about to write,
/// under the same lock. A Rust-side `==` here would just rebuild the same
/// check-to-apply window one layer up. It exists as a distinct type rather
/// than a bare `String` for exactly that reason — it is not a value a caller
/// may reason about, only one it must carry back to the driver that minted
/// it.
///
/// Never a substitute for the ETag: an ETag names *which representation* a
/// client last saw, and a witness only names *when* a row last changed. The
/// guard needs both — the ETag comparison decides whether the caller is
/// entitled to write at all, the witness decides whether that decision is
/// still true at the instant the write lands.
///
/// A witness may become stale for a reason no client caused (PostgreSQL
/// freezing a row's `xmin` during `VACUUM`, say). That only ever produces a
/// spurious refusal, never a spurious acceptance — the safe direction, and
/// the only one this type is allowed to err in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowVersion(String);

impl RowVersion {
    /// Wraps a driver's own token. Called by driver implementations only.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The token verbatim — what a driver binds as a query parameter when it
    /// re-verifies this witness in-transaction.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RowVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// A strong ETag (RFC 7232 section 2.3: `"<opaque-tag>"`, quoted, no `W/`
/// weak-indicator prefix) for `feature` — the exact JSON value a
/// `FeatureSource::item`/`item_with_crs` call returned, hashed before any
/// response-only decoration (hypermedia `links`, ...) is attached. See this
/// module's own doc for why every caller must pass the CANONICAL
/// representation here, never a CRS-reprojected one.
pub fn compute_feature_etag(feature: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(feature).expect("serde_json::Value serialization is infallible");
    let digest = compute_sha256(&bytes);
    format!("\"{}\"", hex_encode(&digest.value))
}

/// Strips an entity-tag's optional `W/` weak-indicator prefix and
/// surrounding quotes, exposing the opaque tag text underneath for
/// comparison. Tolerated on input even though [`compute_feature_etag`]
/// never emits a weak tag itself — a client that echoes a weak-prefixed
/// value back should still compare sensibly against the same opaque text.
fn unquote_etag(raw: &str) -> &str {
    let raw = raw.trim();
    raw.strip_prefix("W/").unwrap_or(raw).trim_matches('"')
}

/// Whether an `If-Match` header's value is satisfied by `current_etag`
/// (already in [`compute_feature_etag`]'s own quoted wire form). The
/// literal wildcard `*` (RFC 7232 section 3.1) matches any concrete
/// representation — callers only reach this function once a resource is
/// already known to exist; `*` against a MISSING resource is its own,
/// narrower refusal (`/req/create-replace-delete/put-rid-exception` clause
/// B), evaluated separately and earlier, never through this function (see
/// `tellurion-features::write_handlers`). Otherwise the header's
/// comma-separated list of entity-tags is compared, unquoted, against
/// `current_etag`'s own unquoted text — satisfied if any one of them
/// matches.
pub fn if_match_satisfied(header_value: &str, current_etag: &str) -> bool {
    let header_value = header_value.trim();
    if header_value == "*" {
        return true;
    }
    let current = unquote_etag(current_etag);
    header_value
        .split(',')
        .any(|raw| unquote_etag(raw) == current)
}

/// Parses a stored modification-timestamp property's own RFC 3339 text
/// (whatever `FeatureSource::item`'s JSON reports for a collection's
/// declared `modified_column` — e.g. PostGIS's `to_jsonb` rendering of a
/// `timestamptz` column, `"2026-07-20T12:34:56.789+00:00"`) into a
/// [`SystemTime`]. `None` when the value isn't a parseable RFC 3339
/// timestamp at all — a collection that declared a real column but whose
/// stored value is somehow unparseable degrades to "no `Last-Modified` for
/// this one response" rather than a hard failure; see each caller for how
/// it treats that.
pub fn parse_stored_timestamp(value: &str) -> Option<SystemTime> {
    let parsed = chrono::DateTime::parse_from_rfc3339(value).ok()?;
    let utc = parsed.with_timezone(&chrono::Utc);
    let secs = u64::try_from(utc.timestamp()).ok()?;
    let nanos = utc.timestamp_subsec_nanos();
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(secs, nanos))
}

/// Formats `time` as an RFC 7231 `IMF-fixdate` — the wire form both a
/// `Last-Modified` response and an `If-Unmodified-Since` request use.
pub fn format_http_date(time: SystemTime) -> String {
    httpdate::fmt_http_date(time)
}

/// Parses an RFC 7231 HTTP-date — `If-Unmodified-Since`'s own wire form.
/// `None` on anything that doesn't parse: RFC 7232 section 3.4 requires a
/// recipient to ignore an unparseable precondition date (treat the request
/// as if the header were absent), never to error the request over it.
pub fn parse_http_date(value: &str) -> Option<SystemTime> {
    httpdate::parse_http_date(value).ok()
}

/// Truncates `time` to whole seconds — `If-Unmodified-Since`/`Last-Modified`
/// are HTTP-dates, which structurally cannot carry sub-second precision.
fn truncate_to_seconds(time: SystemTime) -> SystemTime {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// Whether `stored` (a collection's true last-modified instant, already
/// parsed via [`parse_stored_timestamp`]) is compatible with an
/// `If-Unmodified-Since: since` precondition (already parsed via
/// [`parse_http_date`]): `true` when the write may proceed (the resource
/// has NOT been modified after `since`), `false` when it must be refused
/// with `412`. Both sides are truncated to whole seconds before comparing
/// ([`truncate_to_seconds`]) — comparing a sub-second-precise `stored`
/// against an HTTP-date directly would refuse a write over `stored`'s own
/// sub-second jitter, a difference the client could never have expressed in
/// the header it sent in the first place.
pub fn is_unmodified_since(stored: SystemTime, since: SystemTime) -> bool {
    truncate_to_seconds(stored) <= truncate_to_seconds(since)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- compute_feature_etag / if_match_satisfied ---------------------------

    #[test]
    fn compute_feature_etag_is_stable_for_the_same_value() {
        let feature = json!({"type": "Feature", "id": "1", "properties": {"a": 1, "b": 2}});
        assert_eq!(
            compute_feature_etag(&feature),
            compute_feature_etag(&feature)
        );
    }

    #[test]
    fn compute_feature_etag_is_independent_of_object_key_order() {
        // `serde_json::Value::Object` in this workspace is `BTreeMap`-backed
        // (no `preserve_order` feature enabled) — two values built with
        // properties inserted in a different order still serialize
        // identically, so their digests must match too.
        let a = json!({"type": "Feature", "id": "1", "properties": {"a": 1, "b": 2}});
        let b = json!({"properties": {"b": 2, "a": 1}, "id": "1", "type": "Feature"});
        assert_eq!(compute_feature_etag(&a), compute_feature_etag(&b));
    }

    #[test]
    fn compute_feature_etag_changes_when_the_stored_representation_changes() {
        let before = json!({"type": "Feature", "id": "1", "properties": {"name": "old"}});
        let after = json!({"type": "Feature", "id": "1", "properties": {"name": "new"}});
        assert_ne!(compute_feature_etag(&before), compute_feature_etag(&after));
    }

    #[test]
    fn compute_feature_etag_is_a_quoted_strong_tag() {
        let feature = json!({"type": "Feature", "id": "1", "properties": {}});
        let etag = compute_feature_etag(&feature);
        assert!(etag.starts_with('"') && etag.ends_with('"'));
        assert!(!etag.starts_with("W/"));
    }

    #[test]
    fn if_match_satisfied_accepts_the_wildcard() {
        assert!(if_match_satisfied("*", "\"abc\""));
    }

    #[test]
    fn if_match_satisfied_accepts_an_exact_match() {
        assert!(if_match_satisfied("\"abc\"", "\"abc\""));
    }

    #[test]
    fn if_match_satisfied_rejects_a_mismatch() {
        assert!(!if_match_satisfied("\"abc\"", "\"def\""));
    }

    #[test]
    fn if_match_satisfied_accepts_any_entry_in_a_comma_separated_list() {
        assert!(if_match_satisfied("\"xyz\", \"abc\"", "\"abc\""));
    }

    #[test]
    fn if_match_satisfied_tolerates_a_weak_prefix_on_the_headers_own_value() {
        assert!(if_match_satisfied("W/\"abc\"", "\"abc\""));
    }

    // -- parse_stored_timestamp / format_http_date / parse_http_date --------

    #[test]
    fn parse_stored_timestamp_accepts_a_z_suffixed_rfc3339_value() {
        assert!(parse_stored_timestamp("2026-07-20T12:34:56Z").is_some());
    }

    #[test]
    fn parse_stored_timestamp_accepts_a_numeric_offset_with_fractional_seconds() {
        // The shape PostGIS's `to_jsonb` renders a `timestamptz` column as.
        assert!(parse_stored_timestamp("2026-07-20T12:34:56.789+00:00").is_some());
    }

    #[test]
    fn parse_stored_timestamp_rejects_garbage() {
        assert!(parse_stored_timestamp("not a timestamp").is_none());
    }

    #[test]
    fn parse_http_date_rejects_an_rfc3339_value_directly() {
        // `If-Unmodified-Since` is an HTTP-date (RFC 7231), not RFC 3339 —
        // the two are deliberately kept separate parsers, not one relaxed
        // one, so a caller can never accidentally accept the wrong wire
        // format for either header.
        assert!(parse_http_date("2026-07-20T12:34:56Z").is_none());
    }

    #[test]
    fn format_http_date_round_trips_through_parse_http_date() {
        let now = truncate_to_seconds(SystemTime::now());
        let formatted = format_http_date(now);
        assert_eq!(parse_http_date(&formatted), Some(now));
    }

    // -- is_unmodified_since --------------------------------------------------

    #[test]
    fn is_unmodified_since_is_true_when_stored_is_earlier_than_since() {
        let since = SystemTime::now();
        let stored = since - Duration::from_secs(60);
        assert!(is_unmodified_since(stored, since));
    }

    #[test]
    fn is_unmodified_since_is_false_when_stored_is_later_than_since() {
        let since = SystemTime::now();
        let stored = since + Duration::from_secs(60);
        assert!(!is_unmodified_since(stored, since));
    }

    #[test]
    fn is_unmodified_since_ignores_sub_second_jitter_at_equality() {
        let since = SystemTime::now();
        // `stored` is a few hundred milliseconds later than `since` but
        // within the same whole second once both are truncated — must not
        // register as "modified after `since`" purely from sub-second noise
        // a client could never have named in an HTTP-date header.
        let same_second = truncate_to_seconds(since) + Duration::from_millis(500);
        assert!(is_unmodified_since(same_second, truncate_to_seconds(since)));
    }
}
