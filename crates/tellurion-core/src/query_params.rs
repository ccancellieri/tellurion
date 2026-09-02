//! Shared parsing for the query parameters `tellurion-features` and
//! `tellurion-stac` both accept in byte-identical shape: a paged `limit`,
//! `bbox`, `datetime`, and the percent-encoding both crates' own href
//! builders use to echo a parsed parameter back into a `next` link.
//! Framework-free (no axum dependency, no RFC 3339 crate) — the same
//! "core stays a plain-data, zero-framework crate" discipline `problem.rs`
//! and `crs.rs` already keep.
//!
//! `tellurion_core::storage::DatetimeRange`'s own doc says the *type* stays
//! raw strings so parsing/validation can live in "whichever crate ultimately
//! builds a query" — this module doesn't change that: `ItemsQuery`/
//! `SearchRequest` are still assembled by `tellurion-features`/
//! `tellurion-stac` themselves, from their own query-parameter structs,
//! which genuinely differ crate to crate (STAC's `/items` slice has no
//! `filter`/`crs`; `/search` adds `intersects`/`ids`/`collections`). What's
//! shared here is only the leaf-level string-to-value parsing underneath
//! that assembly, which an audit found byte-identical between every
//! caller — the same "one implementation, several callers" treatment
//! `crs.rs` already gives `crs`/`bbox-crs` resolution.

use crate::error::{Error, Result};
use crate::storage::DatetimeRange;

/// Parses a `limit` query parameter against a caller-supplied `default`
/// (used when the parameter is absent) and `max` (a value above it is
/// clamped, not rejected — the OGC API Features Part 1 Core behavior every
/// caller in this workspace implements the same way): `0` is the one value
/// always rejected, since a page of size zero can never be satisfied.
/// Shared by both crates' own `/items` and `/collections` limit parsing —
/// the two differ only in which `(default, max)` pair they pass.
pub fn parse_bounded_limit(limit: Option<u32>, default: u32, max: u32) -> Result<u32> {
    match limit {
        None => Ok(default),
        Some(0) => Err(Error::Invalid("limit must be >= 1".to_string())),
        Some(value) => Ok(value.min(max)),
    }
}

/// Parses a `bbox` query parameter: exactly four comma-separated numbers, in
/// whatever axis order the caller supplied them (axis-order resolution
/// against a requested CRS is the caller's own job — see `crs.rs`).
pub fn parse_bbox(raw: &str) -> Result<[f64; 4]> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 4 {
        return Err(Error::Invalid(format!(
            "bbox must have exactly 4 comma-separated numbers, got {}",
            parts.len()
        )));
    }
    let mut values = [0.0f64; 4];
    for (i, part) in parts.iter().enumerate() {
        values[i] = part
            .trim()
            .parse::<f64>()
            .map_err(|_| Error::Invalid(format!("bbox value '{part}' is not a valid number")))?;
    }
    Ok(values)
}

/// Accepts a single instant, a `start/end` interval, or an open-ended
/// interval using `..` on either side (never both).
pub fn parse_datetime(raw: &str) -> Result<DatetimeRange> {
    if raw.is_empty() {
        return Err(Error::Invalid("datetime must not be empty".to_string()));
    }

    let parts: Vec<&str> = raw.split('/').collect();
    match parts.as_slice() {
        [single] => {
            validate_instant(single)?;
            Ok(DatetimeRange {
                start: Some((*single).to_string()),
                end: Some((*single).to_string()),
            })
        }
        [start, end] => {
            let start = validate_bound(start)?;
            let end = validate_bound(end)?;
            if start.is_none() && end.is_none() {
                return Err(Error::Invalid(
                    "datetime interval cannot be open on both ends".to_string(),
                ));
            }
            Ok(DatetimeRange { start, end })
        }
        _ => Err(Error::Invalid(format!(
            "datetime '{raw}' must be a single value or a single '/' interval"
        ))),
    }
}

fn validate_bound(raw: &str) -> Result<Option<String>> {
    if raw == ".." {
        Ok(None)
    } else {
        validate_instant(raw)?;
        Ok(Some(raw.to_string()))
    }
}

/// Syntax + numeric-range check for a single RFC 3339 instant — shape only,
/// not a calendar validator (days-in-month and leap years are not checked).
/// OGC API Features Part 1 Requirement 9 needs an invalid `datetime` value to
/// produce a 400, not a 500 from a driver's own `::timestamptz` cast
/// downstream; catching the "not shaped like a timestamp at all" case here
/// closes that gap without a full RFC 3339 dependency.
fn validate_instant(raw: &str) -> Result<()> {
    let invalid = || {
        Error::Invalid(format!(
            "datetime value '{raw}' is not a valid RFC 3339 instant"
        ))
    };

    let bytes = raw.as_bytes();
    if bytes.len() < 20 {
        return Err(invalid());
    }
    let digit = |i: usize| bytes[i].is_ascii_digit();
    let lit = |i: usize, c: u8| bytes[i] == c;

    let shape_ok = (0..4).all(digit)
        && lit(4, b'-')
        && (5..7).all(digit)
        && lit(7, b'-')
        && (8..10).all(digit)
        && matches!(bytes[10], b'T' | b't')
        && (11..13).all(digit)
        && lit(13, b':')
        && (14..16).all(digit)
        && lit(16, b':')
        && (17..19).all(digit);
    if !shape_ok {
        return Err(invalid());
    }

    // Every byte checked above is single-byte ASCII, so these are all valid
    // char-boundary slices.
    let month: u32 = raw[5..7].parse().unwrap_or(0);
    let day: u32 = raw[8..10].parse().unwrap_or(0);
    let hour: u32 = raw[11..13].parse().unwrap_or(0);
    let minute: u32 = raw[14..16].parse().unwrap_or(0);
    let second: u32 = raw[17..19].parse().unwrap_or(0);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return Err(invalid());
    }

    let mut rest = &raw[19..];
    if let Some(after_dot) = rest.strip_prefix('.') {
        let frac_len = after_dot.bytes().take_while(u8::is_ascii_digit).count();
        if frac_len == 0 {
            return Err(invalid());
        }
        rest = &after_dot[frac_len..];
    }

    let offset_ok = match rest.as_bytes() {
        [b'Z'] | [b'z'] => true,
        [b'+' | b'-', h1, h2, b':', m1, m2]
            if [h1, h2, m1, m2].iter().all(|b| b.is_ascii_digit()) =>
        {
            true
        }
        _ => false,
    };
    if !offset_ok {
        return Err(invalid());
    }

    Ok(())
}

/// Minimal RFC 3986 percent-encoding of query values: unreserved characters
/// pass through, everything else is escaped. No external dependency needed
/// for the small, self-controlled set of values this workspace ever emits
/// into a `next`/self `href`.
pub fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_limit_defaults_when_absent() {
        assert_eq!(parse_bounded_limit(None, 10, 10_000).unwrap(), 10);
    }

    #[test]
    fn bounded_limit_rejects_zero() {
        assert!(matches!(
            parse_bounded_limit(Some(0), 10, 10_000),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn bounded_limit_clamps_to_max() {
        assert_eq!(
            parse_bounded_limit(Some(50_000), 10, 10_000).unwrap(),
            10_000
        );
    }

    #[test]
    fn bbox_parses_four_numbers() {
        assert_eq!(parse_bbox("1,2,3,4").unwrap(), [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn bbox_rejects_wrong_count() {
        assert!(matches!(parse_bbox("1,2,3"), Err(Error::Invalid(_))));
    }

    #[test]
    fn bbox_rejects_non_numeric() {
        assert!(matches!(parse_bbox("a,2,3,4"), Err(Error::Invalid(_))));
    }

    #[test]
    fn datetime_single_instant() {
        let range = parse_datetime("2020-01-01T00:00:00Z").unwrap();
        assert_eq!(range.start.as_deref(), Some("2020-01-01T00:00:00Z"));
        assert_eq!(range.end.as_deref(), Some("2020-01-01T00:00:00Z"));
    }

    #[test]
    fn datetime_closed_interval() {
        let range = parse_datetime("2020-01-01T00:00:00Z/2021-01-01T00:00:00Z").unwrap();
        assert_eq!(range.start.as_deref(), Some("2020-01-01T00:00:00Z"));
        assert_eq!(range.end.as_deref(), Some("2021-01-01T00:00:00Z"));
    }

    #[test]
    fn datetime_open_start() {
        let range = parse_datetime("../2021-01-01T00:00:00Z").unwrap();
        assert_eq!(range.start, None);
        assert_eq!(range.end.as_deref(), Some("2021-01-01T00:00:00Z"));
    }

    #[test]
    fn datetime_open_end() {
        let range = parse_datetime("2020-01-01T00:00:00Z/..").unwrap();
        assert_eq!(range.start.as_deref(), Some("2020-01-01T00:00:00Z"));
        assert_eq!(range.end, None);
    }

    #[test]
    fn datetime_rejects_double_open() {
        assert!(matches!(parse_datetime("../.."), Err(Error::Invalid(_))));
    }

    #[test]
    fn datetime_rejects_extra_slashes() {
        assert!(matches!(parse_datetime("a/b/c"), Err(Error::Invalid(_))));
    }

    #[test]
    fn datetime_rejects_a_syntactically_invalid_single_instant() {
        assert!(matches!(parse_datetime("notadate"), Err(Error::Invalid(_))));
    }

    #[test]
    fn datetime_accepts_fractional_seconds_and_a_numeric_offset() {
        assert!(parse_datetime("2020-01-01T00:00:00.123Z").is_ok());
        assert!(parse_datetime("2020-01-01T00:00:00+02:00").is_ok());
    }

    #[test]
    fn datetime_rejects_out_of_range_month() {
        assert!(matches!(
            parse_datetime("2020-13-01T00:00:00Z"),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn percent_encode_passes_unreserved_and_escapes_the_rest() {
        assert_eq!(percent_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("a=b"), "a%3Db");
    }
}
