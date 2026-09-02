//! The pull change feed (`#115`): a per-collection, cursor-paged view over
//! the same transactional outbox (`crate::outbox`) the search/tile-cache
//! consumers already drain — never a second, independently written log
//! (`crate::outbox`'s own "one log, N consumers, independent cursors"
//! invariant). Entries are compact envelopes only: a sequence, an operation,
//! the item id, a schema version, and a timestamp — never the mutation's own
//! payload. The items resource already serves current state; inlining a
//! payload here would turn a backlog page into megabytes exactly when the
//! backlog matters most, which is the one failure mode this lane exists to
//! avoid.
//!
//! Cursoring is keyset, never OFFSET: the opaque `since`/`next` token IS the
//! outbox [`Sequence`] a page ended at, written out in decimal — plain in
//! shape, opaque in practice, the same convention `tellurion-postgis`'s own
//! item tokens and `tellurion_core::registry`'s external-id tokens already
//! use. [`FeedEntry`] is also the exact wire shape a webhook delivery POSTs
//! (`crate::webhooks`) and a dead-lettered entry is stored as — "the same
//! compact-envelope discipline" the feed and webhook lanes share by
//! construction, not by convention two modules happen to agree on.

use crate::error::{Error, Result};
use crate::outbox::{MutationKind, Obligation, Sequence};

/// Bump whenever [`FeedEntry`]'s own fields change in a way an existing
/// consumer could not tolerate (a field removed, renamed, or reinterpreted)
/// — never for a purely additive field, the same "the record grows, it is
/// never reinterpreted" rule [`Obligation`] itself follows.
pub const FEED_ENVELOPE_SCHEMA_VERSION: u32 = 1;

/// One obligation's kind, as the feed/webhook envelope reports it — see the
/// module doc for why this is never the mutation's own payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedOperation {
    Upsert,
    Delete,
}

impl From<&MutationKind> for FeedOperation {
    fn from(kind: &MutationKind) -> Self {
        match kind {
            MutationKind::Upsert(_) => FeedOperation::Upsert,
            MutationKind::Delete => FeedOperation::Delete,
        }
    }
}

/// One change-feed/webhook/dead-letter entry — the compact envelope the
/// module doc describes. `schema_version` rides on every entry (not only the
/// page wrapper) so a consumer that persists entries individually never
/// loses track of which shape a given one was written under.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedEntry {
    pub schema_version: u32,
    pub sequence: u64,
    pub collection: String,
    pub operation: FeedOperation,
    pub item_id: String,
    /// RFC 3339, UTC, millisecond precision — see
    /// [`crate::timefmt::format_rfc3339_millis`].
    pub committed_at: String,
}

impl FeedEntry {
    pub fn from_obligation(collection: &str, obligation: &Obligation) -> Self {
        Self {
            schema_version: FEED_ENVELOPE_SCHEMA_VERSION,
            sequence: obligation.sequence.0,
            collection: collection.to_string(),
            operation: FeedOperation::from(&obligation.kind),
            item_id: obligation.feature_id.clone(),
            committed_at: crate::timefmt::format_rfc3339_millis(obligation.committed_at),
        }
    }
}

/// One page: entries plus the cursor for the next page. `next` is `Some`
/// only when this page returned at least `requested_limit` entries — a page
/// short of the requested limit already means "caught up" per
/// [`crate::outbox::OutboxSource::read_after`]'s own contract, so a `next`
/// link there would send a client to poll a page that is honestly empty
/// rather than tell it plainly there is nothing more right now. Mirrors
/// `tellurion_core::registry::Page`'s own "a next link only appears once
/// there is reason to believe more remain" rule.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedPage {
    pub entries: Vec<FeedEntry>,
    pub next: Option<String>,
}

/// Builds a page from one [`crate::outbox::OutboxSource::read_after`] batch.
/// `requested_limit` must be the exact `limit` that batch was fetched with —
/// see [`FeedPage::next`]'s own doc for why a short batch never gets a
/// `next` token.
pub fn build_page(collection: &str, obligations: &[Obligation], requested_limit: u32) -> FeedPage {
    let entries: Vec<FeedEntry> = obligations
        .iter()
        .map(|obligation| FeedEntry::from_obligation(collection, obligation))
        .collect();
    let next = if !obligations.is_empty() && obligations.len() as u32 >= requested_limit {
        obligations.last().map(|last| encode_cursor(last.sequence))
    } else {
        None
    };
    FeedPage { entries, next }
}

/// Encodes a [`Sequence`] as this feed's opaque cursor token. Plain decimal
/// today; callers must still treat it as opaque (see the module doc).
pub fn encode_cursor(sequence: Sequence) -> String {
    sequence.0.to_string()
}

/// Decodes a caller-supplied `since` token back into a [`Sequence`] — `Err`
/// for anything that is not a plain non-negative integer, never a panic and
/// never a silent fallback to `0`: a garbled cursor is a client error to
/// report by name, not "start over from the beginning."
pub fn decode_cursor(token: &str) -> Result<Sequence> {
    token
        .parse::<u64>()
        .map(Sequence)
        .map_err(|_| Error::Invalid(format!("'{token}' is not a valid change-feed cursor")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn obligation(sequence: u64, kind: MutationKind) -> Obligation {
        Obligation {
            sequence: Sequence(sequence),
            feature_id: format!("f{sequence}"),
            kind,
            version: Sequence(sequence),
            committed_at: SystemTime::UNIX_EPOCH + Duration::from_secs(sequence),
            extent: crate::outbox::ObligationExtent::Unrecorded,
        }
    }

    #[test]
    fn cursor_round_trips_through_encode_and_decode() {
        let sequence = Sequence(42);
        assert_eq!(decode_cursor(&encode_cursor(sequence)).unwrap(), sequence);
    }

    #[test]
    fn decode_cursor_rejects_non_numeric_garbage() {
        assert!(decode_cursor("not-a-cursor").is_err());
    }

    #[test]
    fn decode_cursor_rejects_a_negative_number() {
        assert!(decode_cursor("-1").is_err());
    }

    #[test]
    fn decode_cursor_rejects_a_fractional_number() {
        assert!(decode_cursor("1.5").is_err());
    }

    #[test]
    fn decode_cursor_rejects_an_empty_token() {
        assert!(decode_cursor("").is_err());
    }

    #[test]
    fn an_upsert_obligation_maps_to_the_upsert_operation_with_no_payload_field() {
        let entry = FeedEntry::from_obligation(
            "demo",
            &obligation(
                1,
                MutationKind::Upsert(serde_json::json!({"secret": "payload"})),
            ),
        );
        assert_eq!(entry.operation, FeedOperation::Upsert);
        assert_eq!(entry.item_id, "f1");
        assert_eq!(entry.sequence, 1);
        assert_eq!(entry.collection, "demo");
        assert_eq!(entry.schema_version, FEED_ENVELOPE_SCHEMA_VERSION);
        // `FeedEntry` has no field capable of carrying the obligation's own
        // payload at all — serializing it can never leak one, by
        // construction, not by omission at this call site.
        let serialized = serde_json::to_value(&entry).unwrap();
        assert!(serialized.get("payload").is_none());
        assert!(!serialized.to_string().contains("secret"));
    }

    #[test]
    fn a_delete_obligation_maps_to_the_delete_operation() {
        let entry = FeedEntry::from_obligation("demo", &obligation(2, MutationKind::Delete));
        assert_eq!(entry.operation, FeedOperation::Delete);
    }

    #[test]
    fn a_full_page_gets_a_next_token_at_its_last_sequence() {
        let obligations = vec![
            obligation(1, MutationKind::Delete),
            obligation(2, MutationKind::Delete),
        ];
        let page = build_page("demo", &obligations, 2);
        assert_eq!(page.next, Some("2".to_string()));
        assert_eq!(page.entries.len(), 2);
    }

    #[test]
    fn a_short_page_never_gets_a_next_token_even_with_entries() {
        let obligations = vec![obligation(1, MutationKind::Delete)];
        let page = build_page("demo", &obligations, 10);
        assert_eq!(page.next, None);
    }

    #[test]
    fn an_empty_page_never_gets_a_next_token() {
        let page = build_page("demo", &[], 10);
        assert_eq!(page.next, None);
        assert!(page.entries.is_empty());
    }
}
