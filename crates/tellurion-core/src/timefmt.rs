//! Minimal, dependency-free UTC timestamp formatting/parsing (`#115`).
//!
//! Every timestamp this workspace's outbox stores is UTC (PostGIS'
//! `timestamptz`, read back as `std::time::SystemTime` via `postgres-types`'
//! own built-in conversion; GeoPackage's own `strftime('%Y-%m-%dT%H:%M:%fZ',
//! 'now')` text column) — this module only ever needs "seconds since the
//! Unix epoch <-> proleptic Gregorian calendar fields," never a general
//! timezone-aware datetime library. [`days_from_civil`]/[`civil_from_days`]
//! are Howard Hinnant's well-known public-domain calendar algorithms
//! (<http://howardhinnant.github.io/date_algorithms.html>): chosen over
//! adding a `chrono`/`time` dependency because the two things ever needed
//! here (format a `SystemTime` for a change-feed/webhook envelope; parse
//! GeoPackage's own fixed-shape stored timestamp back into one) are smaller,
//! in total, than the dependency itself, and every real value either driver
//! ever hands this module is already UTC by construction — there is no
//! offset/DST handling to get wrong.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Days since the Unix epoch (1970-01-01) for the proleptic Gregorian
/// calendar date `(y, m, d)`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = i64::from((m + 9) % 12); // [0, 11]
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// The inverse of [`days_from_civil`]: the proleptic Gregorian `(y, m, d)`
/// for `days` days since the Unix epoch.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Formats `t` as an RFC 3339 UTC timestamp with millisecond precision
/// (`YYYY-MM-DDTHH:MM:SS.sssZ`) — the shape every change-feed/webhook
/// envelope carries its `committed_at` in. `t` before the Unix epoch (never
/// a real outbox value — every row's `committed_at` is a write-time "now")
/// falls back to the epoch itself rather than panicking.
pub fn format_rfc3339_millis(t: SystemTime) -> String {
    let duration = t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let total_secs = duration.as_secs() as i64;
    let millis = duration.subsec_millis();
    let days = total_secs.div_euclid(86_400);
    let secs_of_day = total_secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeParseError(String);

impl fmt::Display for TimeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TimeParseError {}

fn parse_field<T: std::str::FromStr>(part: Option<&str>, name: &str) -> Result<T, TimeParseError> {
    part.and_then(|value| value.parse().ok())
        .ok_or_else(|| TimeParseError(format!("missing or non-numeric {name}")))
}

/// Zero-pads or truncates `frac` (the digits after the decimal point) to
/// exactly milliseconds — GeoPackage's own `%f` strftime specifier produces
/// 3 fractional digits today, but this tolerates any count defensively
/// rather than assuming the exact digit width forever.
fn parse_millis_fraction(frac: &str) -> Result<u64, TimeParseError> {
    if frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(TimeParseError(format!(
            "'{frac}' is not a numeric fractional-second suffix"
        )));
    }
    let padded: String = frac.chars().chain(std::iter::repeat('0')).take(3).collect();
    padded
        .parse()
        .map_err(|_| TimeParseError(format!("'{frac}' fractional seconds overflowed")))
}

/// Parses a fixed-shape `YYYY-MM-DDTHH:MM:SS[.fraction]Z` UTC timestamp —
/// exactly what GeoPackage's own outbox DDL stores its `committed_at` column
/// as (`strftime('%Y-%m-%dT%H:%M:%fZ', 'now')`) — back into a
/// [`SystemTime`]. Deliberately narrow: this is not a general RFC 3339
/// parser (no non-'Z' offsets, no missing fields), because the only input it
/// is ever asked to read is this workspace's own driver-written column, in
/// this one fixed shape.
pub fn parse_utc_datetime_text(text: &str) -> Result<SystemTime, TimeParseError> {
    let body = text
        .strip_suffix('Z')
        .ok_or_else(|| TimeParseError(format!("'{text}' is not UTC ('Z'-suffixed)")))?;
    let (date, time) = body.split_once('T').ok_or_else(|| {
        TimeParseError(format!("'{text}' is missing the 'T' date/time separator"))
    })?;

    let mut date_parts = date.split('-');
    let year: i64 = parse_field(date_parts.next(), "year")?;
    let month: u32 = parse_field(date_parts.next(), "month")?;
    let day: u32 = parse_field(date_parts.next(), "day")?;
    if date_parts.next().is_some() {
        return Err(TimeParseError(format!(
            "'{text}' has an unexpected date field"
        )));
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(TimeParseError(format!(
            "'{text}' has an out-of-range month or day"
        )));
    }

    let (hms, millis) = match time.split_once('.') {
        Some((hms, frac)) => (hms, parse_millis_fraction(frac)?),
        None => (time, 0),
    };
    let mut time_parts = hms.split(':');
    let hour: i64 = parse_field(time_parts.next(), "hour")?;
    let minute: i64 = parse_field(time_parts.next(), "minute")?;
    let second: i64 = parse_field(time_parts.next(), "second")?;
    if time_parts.next().is_some() {
        return Err(TimeParseError(format!(
            "'{text}' has an unexpected time field"
        )));
    }
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) || !(0..60).contains(&second) {
        return Err(TimeParseError(format!(
            "'{text}' has an out-of-range hour, minute, or second"
        )));
    }

    let days = days_from_civil(year, month, day);
    let secs_of_day = hour * 3600 + minute * 60 + second;
    let total_secs = days * 86_400 + secs_of_day;
    if total_secs < 0 {
        return Err(TimeParseError(format!(
            "'{text}' is before the Unix epoch, which this parser does not represent"
        )));
    }
    Ok(UNIX_EPOCH + Duration::from_secs(total_secs as u64) + Duration::from_millis(millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_the_unix_epoch_itself() {
        assert_eq!(
            format_rfc3339_millis(UNIX_EPOCH),
            "1970-01-01T00:00:00.000Z"
        );
    }

    #[test]
    fn formats_a_known_date_with_milliseconds() {
        // 2026-07-21T14:32:23.456Z — cross-checked against an independently
        // computed epoch-seconds value.
        let t = UNIX_EPOCH + Duration::from_millis(1_784_644_343_456);
        assert_eq!(format_rfc3339_millis(t), "2026-07-21T14:32:23.456Z");
    }

    #[test]
    fn formats_a_leap_day() {
        // 2024-02-29T00:00:00Z.
        let t = UNIX_EPOCH + Duration::from_secs(1_709_164_800);
        assert_eq!(format_rfc3339_millis(t), "2024-02-29T00:00:00.000Z");
    }

    #[test]
    fn before_the_epoch_falls_back_to_the_epoch_rather_than_panicking() {
        let before = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(format_rfc3339_millis(before), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn parses_back_to_the_same_instant_it_formatted() {
        let t = UNIX_EPOCH + Duration::from_millis(1_784_644_343_456);
        let text = format_rfc3339_millis(t);
        assert_eq!(parse_utc_datetime_text(&text).unwrap(), t);
    }

    #[test]
    fn parses_the_geopackage_three_digit_fraction_shape() {
        let parsed = parse_utc_datetime_text("2026-07-21T14:32:23.456Z").unwrap();
        assert_eq!(
            parsed,
            UNIX_EPOCH + Duration::from_millis(1_784_644_343_456)
        );
    }

    #[test]
    fn parses_with_no_fractional_seconds_at_all() {
        let parsed = parse_utc_datetime_text("1970-01-01T00:00:01Z").unwrap();
        assert_eq!(parsed, UNIX_EPOCH + Duration::from_secs(1));
    }

    #[test]
    fn round_trips_across_a_leap_day() {
        let t = UNIX_EPOCH + Duration::from_secs(1_709_164_800) + Duration::from_millis(999);
        let text = format_rfc3339_millis(t);
        assert_eq!(parse_utc_datetime_text(&text).unwrap(), t);
    }

    #[test]
    fn rejects_a_non_utc_offset() {
        assert!(parse_utc_datetime_text("2026-07-21T14:32:23+02:00").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_utc_datetime_text("not-a-timestamp").is_err());
    }

    #[test]
    fn rejects_an_out_of_range_month() {
        assert!(parse_utc_datetime_text("2026-13-01T00:00:00Z").is_err());
    }
}
