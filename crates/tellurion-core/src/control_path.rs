//! Canonical administrative paths and the anchored segment patterns a
//! [`PathPolicy`](crate::PathPolicy) matches them with (`#215`).
//!
//! # Canonicalization is agreement, not rejection
//!
//! `#215` requires that encoded separators, duplicate slashes, dot segments
//! and aliases cannot bypass canonical policy evaluation. There are two ways
//! to get that: refuse every ambiguous rendering, or make the policy layer's
//! view of a path *provably the same view the router and the handler have*.
//!
//! This module takes the second, because the first quietly breaks a rule this
//! project holds above the feature: a deployment that declared no path scopes
//! must answer exactly as it answers today. A refusal is a new response on a
//! path that may be governed by nothing at all, and deciding whether it is
//! governed requires the canonical form the refusal is admitting it cannot
//! compute. Agreement has no such circularity.
//!
//! So [`decoded_segments`] reproduces axum's own path-parameter decoding
//! byte for byte — `percent_encoding::percent_decode(..).decode_utf8()`: a
//! `%XX` with two hex digits becomes that byte, anything else stays literal,
//! and only invalid UTF-8 fails. That is deliberately not a "stricter" or
//! "safer" decoder. A decoder that differed from the handler's *is itself*
//! the bypass: the policy layer would decide about one string while the
//! handler served another.
//!
//! Agreement plus three properties elsewhere is what closes the class:
//!
//! - **Encoded separators.** A `%2F` inside a segment decodes to a `/`
//!   *inside that one segment*; the raw path is only ever split on real
//!   separators, so it cannot become two segments. Matching is on the segment
//!   list ([`PathPattern::matches`]), never on a re-joined string, so such a
//!   segment matches a single `*` and cannot forge a deeper path shape.
//! - **Dot segments and duplicate slashes.** Neither can reach a decision:
//!   a `.`/`..` in an id position fails ownership resolution, and an empty
//!   segment matches no administrative route shape — both leave the request
//!   to the answer it already had.
//! - **Aliases.** Every external id is replaced by its internal one before a
//!   pattern is matched, so two external ids for one resource have one
//!   canonical path and therefore one decision.
//!
//! # Pattern compilation
//!
//! [`PathPattern`] is anchored, segment-wise and regex-free: a literal
//! segment matches itself, `*` matches exactly one segment, and `**` matches
//! zero or more segments but only as the final one. Compilation happens when
//! a snapshot is validated and when a policy set is built — never per
//! request, so no request can pay for (or be shaped by) pattern parsing.
//! Anchoring at both ends is what stops a prefix collision (`/acme` must not
//! match `/acme-staging`) from becoming a grant, and the "`**` last only"
//! rule keeps every pattern's segment arithmetic decidable in one
//! left-to-right pass.
//!
//! Neither function here ever sees a credential; both are pure functions of
//! the request line and the declared policy document.

use crate::error::{Error, Result};

/// The decoded segment list for `raw_path` — the exact string list every
/// policy decision is made against, and byte-for-byte what axum's own `Path`
/// extractor hands the handler for the same request.
///
/// `raw_path` is the path as it arrived on the request line
/// (`OriginalUri`), still percent-encoded. It is split on real separators
/// only, then each segment is percent-decoded exactly once with axum's own
/// rule: `%XX` with two hex digits is that byte, any other `%` is a literal
/// `%`. A decoded segment may therefore contain a `/`; that is correct and
/// deliberate — see this module's own doc.
///
/// `None` when a segment does not decode to valid UTF-8, the one case axum
/// also fails. The caller passes such a request through untouched rather
/// than inventing a refusal for it: axum answers it exactly as it did before
/// `#215` existed.
pub fn decoded_segments(raw_path: &str) -> Option<Vec<String>> {
    let trimmed = raw_path.strip_prefix('/').unwrap_or(raw_path);
    // A bare `/` is the empty segment list, not one empty segment.
    if trimmed.is_empty() {
        return Some(Vec::new());
    }
    let mut segments = Vec::new();
    for raw_segment in trimmed.split('/') {
        let bytes = raw_segment.as_bytes();
        let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            let hex = |offset: usize| {
                bytes
                    .get(index + offset)
                    .and_then(|byte| (*byte as char).to_digit(16))
            };
            match (bytes[index], hex(1), hex(2)) {
                (b'%', Some(high), Some(low)) => {
                    decoded.push(((high << 4) | low) as u8);
                    index += 3;
                }
                (byte, _, _) => {
                    decoded.push(byte);
                    index += 1;
                }
            }
        }
        segments.push(String::from_utf8(decoded).ok()?);
    }
    Some(segments)
}

/// One compiled segment of a [`PathPattern`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternSegment {
    /// Matches exactly this string.
    Literal(String),
    /// Matches exactly one segment, whatever it is.
    AnySegment,
    /// Matches zero or more remaining segments. Only ever the last element
    /// — [`PathPattern::compile`] refuses it anywhere else.
    AnySuffix,
}

/// An anchored, segment-wise path pattern — the only matching language a
/// [`PathPolicy`](crate::PathPolicy) speaks. Compiled once (snapshot
/// validation, policy-set build), matched many times, and never a regular
/// expression: see this module's own doc for why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPattern {
    segments: Vec<PatternSegment>,
}

impl PathPattern {
    /// Compiles `pattern`, or names why it cannot be compiled.
    ///
    /// Refusals, all of them at declaration time so no request ever meets an
    /// ambiguous pattern:
    ///
    /// - a pattern that is not absolute (does not start with `/`);
    /// - an empty segment (`//`), which would be unmatchable — no canonical
    ///   path can contain one, so such a pattern silently matches nothing;
    /// - a dot segment, for the same reason a request path carrying one is
    ///   refused;
    /// - `**` anywhere but last, which would make a pattern's segment
    ///   arithmetic depend on backtracking;
    /// - a segment that *contains* `*` alongside other characters
    ///   (`cat*`), which reads as a substring wildcard this language does
    ///   not have and must not appear to have.
    pub fn compile(pattern: &str) -> Result<Self> {
        let Some(body) = pattern.strip_prefix('/') else {
            return Err(Error::ControlValidation(format!(
                "path pattern '{pattern}' must be absolute"
            )));
        };
        let mut segments = Vec::new();
        if !body.is_empty() {
            let raw: Vec<&str> = body.split('/').collect();
            for (index, raw_segment) in raw.iter().enumerate() {
                let compiled = match *raw_segment {
                    "" => {
                        return Err(Error::ControlValidation(format!(
                            "path pattern '{pattern}' contains an empty segment"
                        )))
                    }
                    "." | ".." => {
                        return Err(Error::ControlValidation(format!(
                            "path pattern '{pattern}' contains a dot segment"
                        )))
                    }
                    "*" => PatternSegment::AnySegment,
                    "**" => {
                        if index + 1 != raw.len() {
                            return Err(Error::ControlValidation(format!(
                                "path pattern '{pattern}': '**' is only allowed as the final segment"
                            )));
                        }
                        PatternSegment::AnySuffix
                    }
                    other if other.contains('*') => {
                        return Err(Error::ControlValidation(format!(
                            "path pattern '{pattern}': '*' and '**' match whole segments; \
                             segment '{other}' mixes a wildcard with literal characters"
                        )))
                    }
                    other => PatternSegment::Literal(other.to_string()),
                };
                segments.push(compiled);
            }
        }
        Ok(Self { segments })
    }

    /// Whether this pattern matches `path` — anchored at both ends, one
    /// segment at a time, no backtracking (guaranteed by `**` being final).
    pub fn matches(&self, path: &[String]) -> bool {
        let mut index = 0;
        for segment in &self.segments {
            match segment {
                PatternSegment::AnySuffix => return true,
                PatternSegment::AnySegment => {
                    if index >= path.len() {
                        return false;
                    }
                    index += 1;
                }
                PatternSegment::Literal(literal) => {
                    if path.get(index).map(String::as_str) != Some(literal.as_str()) {
                        return false;
                    }
                    index += 1;
                }
            }
        }
        index == path.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segments(path: &str) -> Vec<String> {
        decoded_segments(path).expect("valid UTF-8")
    }

    #[test]
    fn a_plain_path_decodes_to_its_segments() {
        assert_eq!(
            segments("/acme/config/effective"),
            ["acme", "config", "effective"]
        );
        assert_eq!(segments("/"), Vec::<String>::new());
        assert_eq!(segments("/config"), ["config"]);
    }

    /// The agreement property (`#215`): every ambiguous rendering decodes to
    /// exactly what axum's own `Path` extractor produces for the same
    /// request, so the policy layer and the handler can never be looking at
    /// two different strings.
    ///
    /// An encoded separator becomes a `/` INSIDE one segment — never a
    /// second segment. That is the whole reason matching is done on this
    /// list and never on a re-joined string.
    #[test]
    fn an_encoded_separator_stays_inside_one_segment() {
        assert_eq!(
            segments("/acme%2Fconfig/effective"),
            ["acme/config", "effective"]
        );
        assert_eq!(segments("/acme%2fconfig"), ["acme/config"]);
        assert_eq!(segments("/acme%5Cconfig"), ["acme\\config"]);
    }

    /// Decoding happens exactly once — the same single pass axum makes. A
    /// doubly-encoded separator decodes to the literal text `%2F`, which is
    /// an ordinary (if odd) segment; decoding twice is what would make it a
    /// separator, and neither this nor axum does that.
    #[test]
    fn decoding_happens_exactly_once() {
        assert_eq!(segments("/acme%252Fconfig"), ["acme%2Fconfig"]);
    }

    /// An invalid escape stays literal rather than failing, again because
    /// axum's decoder does exactly that; a decoder that refused here would
    /// disagree with the handler, which is the divergence this module exists
    /// to prevent.
    #[test]
    fn an_invalid_escape_stays_literal_and_only_bad_utf8_fails() {
        assert_eq!(segments("/acme/%zz"), ["acme", "%zz"]);
        assert_eq!(segments("/acme/%2"), ["acme", "%2"]);
        assert_eq!(decoded_segments("/acme/%ff%fe"), None);
    }

    /// Dot segments and duplicate slashes are carried through verbatim, not
    /// resolved: `..` stays the string `..` (which no id resolves to) and an
    /// empty segment stays empty (which matches no administrative route
    /// shape). Neither is normalized into something else, which is what a
    /// traversal would need to become a bypass.
    #[test]
    fn dot_segments_and_empty_segments_are_carried_through_not_resolved() {
        assert_eq!(segments("/acme/../beta"), ["acme", "..", "beta"]);
        assert_eq!(segments("/acme/%2e%2e/beta"), ["acme", "..", "beta"]);
        assert_eq!(segments("/acme//config"), ["acme", "", "config"]);
    }

    #[test]
    fn patterns_are_anchored_at_both_ends() {
        let pattern = PathPattern::compile("/acme/config").unwrap();
        assert!(pattern.matches(&segments("/acme/config")));
        assert!(!pattern.matches(&segments("/acme/config/effective")));
        assert!(!pattern.matches(&segments("/x/acme/config")));
        // The prefix collision an unanchored match would grant.
        assert!(!PathPattern::compile("/acme")
            .unwrap()
            .matches(&segments("/acme-staging")));
    }

    #[test]
    fn a_single_star_matches_exactly_one_segment() {
        let pattern = PathPattern::compile("/*/config/effective").unwrap();
        assert!(pattern.matches(&segments("/acme/config/effective")));
        assert!(!pattern.matches(&segments("/config/effective")));
        assert!(!pattern.matches(&segments("/a/b/config/effective")));
    }

    #[test]
    fn a_double_star_matches_zero_or_more_trailing_segments() {
        let pattern = PathPattern::compile("/acme/config/**").unwrap();
        assert!(pattern.matches(&segments("/acme/config")));
        assert!(pattern.matches(&segments("/acme/config/effective")));
        assert!(pattern.matches(&segments("/acme/config/catalogs/c/effective")));
        assert!(!pattern.matches(&segments("/beta/config/effective")));
    }

    #[test]
    fn patterns_that_could_be_read_two_ways_are_refused_at_compile_time() {
        assert!(PathPattern::compile("acme/config").is_err());
        assert!(PathPattern::compile("/acme//config").is_err());
        assert!(PathPattern::compile("/acme/../config").is_err());
        assert!(PathPattern::compile("/**/config").is_err());
        assert!(PathPattern::compile("/acme/cat*").is_err());
        assert!(PathPattern::compile("/acme/**").is_ok());
        assert!(PathPattern::compile("/").is_ok());
    }
}
