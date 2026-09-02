//! Per-request read-lane hints (`#183`) — the third routing axis, after
//! capability (a structural fact about a driver) and lane (operator policy):
//! a hint is a per-request *adjective* a client attaches via `?hints=`, and
//! it can only ever express a preference within what the operator already
//! configured, never widen it.
//!
//! Constraints, all deliberate:
//!
//! - **Closed vocabulary.** [`Hint`] is a closed enum; a token that parses
//!   to none of its variants is dropped harmlessly ([`Hints::parse`] never
//!   fails), so a typo can never 400 a request that would otherwise
//!   succeed. This slice's whole vocabulary is `prefer:<storage-id>`.
//! - **Reorder, never extend.** `prefer:` moves the named entry of an
//!   already-resolved read chain to the front; the non-preferred entries
//!   remain behind it as the ordinary fallback tail (`#21`), so a miss on
//!   the preferred entry falls through instead of 404ing. A name matching
//!   no entry in the chain — including a storage id that exists in the
//!   config but was never routed into this lane — is a no-op, exactly like
//!   an unknown token.
//! - **Read lanes only.** Hints apply to `Router::resolve_features_read`
//!   and `Router::resolve_search_read` and nothing else; the write lane's
//!   resolution (`Router::resolve_write`) has no hints parameter at all, so
//!   a prefer token can never redirect a write — same discipline as the
//!   existing single-primary write rule (`#25`).
//! - **Never wider than policy.** Reordering neither adds nor removes chain
//!   entries, and it happens against a chain the ABAC checkpoint already
//!   scoped: every capability answer a policy decision consumes from a
//!   multi-entry chain is an order-independent intersection over all
//!   entries (`FallbackFeatureSource::filter_capable` and friends), and the
//!   decision itself keys on `(tenant, catalog, collection, lane)`, never
//!   on chain order — so a hint cannot change a policy outcome.
//!
//! The observability counterpart is [`READ_SOURCE_HEADER`]: successful
//! feature reads name the chain entry that actually served them, which is
//! what makes chain divergence (index vs main answering differently)
//! diagnosable without a config edit and reload.

/// Response header naming the storage id of the chain entry that actually
/// served a read (`#183`) — set by `tellurion-features`' item(s) handlers on
/// every successful read, hinted or not. Lowercase because `http`'s
/// `HeaderName::from_static` requires it; the wire spelling is
/// case-insensitive (`X-Tellurion-Source`) per RFC 9110. Kept here (this
/// crate is framework-free, so it's a plain `&str`, mirroring
/// `webhooks::SIGNATURE_HEADER`) so every protocol crate that grows a read
/// lane emits the same name.
pub const READ_SOURCE_HEADER: &str = "x-tellurion-source";

/// The `prefer:` token's prefix on the wire.
const PREFER_PREFIX: &str = "prefer:";

/// One recognized `?hints=` token — the closed vocabulary (`#183`). First
/// slice: only [`Hint::Prefer`]; behavioral hints (e.g. a
/// `geometry-simplified` adjective once a second feature source can serve
/// one collection) are future variants of this same enum, which is exactly
/// why it exists as an enum rather than `Hints` holding loose fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hint {
    /// `prefer:<storage-id>`: move the named entry of the resolved read
    /// chain to the front, keeping the rest as the fallback tail. The name
    /// is matched verbatim (case-sensitive) against the chain's storage
    /// ids — the same boot-time-registry names `routing:` declarations and
    /// boot error messages already use.
    Prefer(String),
}

impl Hint {
    /// Parses one comma-separated token, `None` for anything outside the
    /// closed vocabulary — including a bare `prefer:` with an empty name,
    /// which prefers nothing and is dropped like any other unknown token
    /// rather than invented into a meaning.
    fn parse(token: &str) -> Option<Hint> {
        let token = token.trim();
        let name = token.strip_prefix(PREFER_PREFIX)?.trim();
        if name.is_empty() {
            return None;
        }
        Some(Hint::Prefer(name.to_string()))
    }
}

/// A request's parsed `?hints=` value (`#183`): the recognized tokens,
/// collapsed to at most one meaning per hint kind. [`Hints::none`] — also
/// what parsing an absent/empty parameter yields — is the identity: every
/// hint-aware resolve path behaves byte-for-byte like its unhinted
/// counterpart under it, which is what keeps requests that never heard of
/// hints entirely unaffected by this feature existing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hints {
    prefer: Option<String>,
}

impl Hints {
    /// No hints at all — the identity every unhinted resolve delegates
    /// through.
    pub fn none() -> Self {
        Self::default()
    }

    /// Parses a raw `?hints=` value: comma-separated tokens, each matched
    /// against the closed [`Hint`] vocabulary; unrecognized tokens are
    /// dropped harmlessly (a typo never 400s — see the module doc). When
    /// several `prefer:` tokens survive, the first wins: a chain has one
    /// front to move an entry to, and taking the first matches how the
    /// chain itself resolves (earlier entries outrank later ones).
    /// `None` parses to [`Hints::none`].
    pub fn parse(raw: Option<&str>) -> Self {
        let mut hints = Hints::default();
        let Some(raw) = raw else {
            return hints;
        };
        for token in raw.split(',') {
            if let Some(Hint::Prefer(name)) = Hint::parse(token) {
                if hints.prefer.is_none() {
                    hints.prefer = Some(name);
                }
            }
        }
        hints
    }

    /// The storage id a `prefer:` token named, if any.
    pub fn prefer(&self) -> Option<&str> {
        self.prefer.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_and_empty_raw_values_parse_to_no_hints() {
        assert_eq!(Hints::parse(None), Hints::none());
        assert_eq!(Hints::parse(Some("")), Hints::none());
        assert_eq!(Hints::parse(Some(",,")), Hints::none());
    }

    #[test]
    fn prefer_token_is_recognized() {
        let hints = Hints::parse(Some("prefer:index"));
        assert_eq!(hints.prefer(), Some("index"));
    }

    #[test]
    fn unknown_tokens_are_dropped_without_disturbing_recognized_ones() {
        let hints = Hints::parse(Some("geometry-simplified,prefer:main,unknown"));
        assert_eq!(
            hints.prefer(),
            Some("main"),
            "an unrecognized token must never invalidate the whole parameter"
        );
    }

    #[test]
    fn only_unknown_tokens_parse_to_no_hints() {
        assert_eq!(Hints::parse(Some("bogus,also-bogus")), Hints::none());
    }

    #[test]
    fn the_first_of_several_prefer_tokens_wins() {
        let hints = Hints::parse(Some("prefer:first,prefer:second"));
        assert_eq!(hints.prefer(), Some("first"));
    }

    #[test]
    fn a_bare_prefer_with_no_name_is_dropped_not_invented() {
        assert_eq!(Hints::parse(Some("prefer:")), Hints::none());
        assert_eq!(Hints::parse(Some("prefer:  ")), Hints::none());
    }

    #[test]
    fn tokens_are_trimmed_of_surrounding_whitespace() {
        let hints = Hints::parse(Some(" prefer:index , other"));
        assert_eq!(hints.prefer(), Some("index"));
    }

    #[test]
    fn prefer_names_are_matched_verbatim_not_case_folded() {
        // Storage ids are case-sensitive config identifiers; the parse must
        // not fold case on the client's behalf (`Prefer:Index` is simply an
        // unknown token, and `prefer:Index` names `Index`, not `index`).
        assert_eq!(Hints::parse(Some("Prefer:index")), Hints::none());
        assert_eq!(Hints::parse(Some("prefer:Index")).prefer(), Some("Index"));
    }
}
