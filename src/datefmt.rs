//! Pure date/time parsing and formatting for the list and the conversation
//! status line.
//! No TUI dependency, so it's unit-tested here.

use std::time::SystemTime;

/// `2026-10-13T12:48:20.086Z` -> `2026-10-13 12:48 UTC`. A value
/// [`parse_timestamp`] can't read is shown as recorded, cut to the minute and
/// without the label, since its zone is unknown.
pub fn format_timestamp(ts: &str) -> String {
    let Some(time) = parse_timestamp(ts) else {
        let compact: String = ts.chars().take(16).collect();
        return compact.replacen('T', " ", 1);
    };
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let (h, mi) = (secs % 86_400 / 3600, secs % 3600 / 60);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02} UTC")
}

/// `2026-10-13T12:48:20.086Z` as a point in time. Fractional seconds are
/// dropped; anything else, including an out-of-range field or a value with an
/// offset, is `None`. Both agents with record timestamps write UTC (`Z`), and
/// a value without one must stay `None`: [`format_timestamp`] labels whatever
/// parses as UTC.
pub fn parse_timestamp(ts: &str) -> Option<SystemTime> {
    // A fixed-width year also bounds the arithmetic below against overflow.
    const SHAPE: &[u8] = b"dddd-dd-ddTdd:dd:dd";
    let fits = |(&p, &c): (&u8, &u8)| {
        if p == b'd' {
            c.is_ascii_digit()
        } else {
            c == p
        }
    };
    if ts.len() < SHAPE.len() || !SHAPE.iter().zip(ts.as_bytes()).all(fits) {
        return None;
    }
    let n = |at: usize, len: usize| ts.get(at..at + len)?.parse::<i64>().ok();
    let (y, m, d, h, mi, s) = (
        n(0, 4)?,
        n(5, 2)?,
        n(8, 2)?,
        n(11, 2)?,
        n(14, 2)?,
        n(17, 2)?,
    );
    let days = days_from_civil(y, m as u32, d as u32);
    // An impossible month or day (Feb 31, month 13) lands on another date.
    if h > 23 || mi > 59 || s > 59 || civil_from_days(days) != (y, m as u32, d as u32) {
        return None;
    }
    let rest = &ts[SHAPE.len()..];
    let zone = match rest.strip_prefix('.') {
        Some(frac) if frac.starts_with(|c: char| c.is_ascii_digit()) => {
            frac.trim_start_matches(|c: char| c.is_ascii_digit())
        }
        _ => rest,
    };
    if zone != "Z" {
        return None;
    }
    let secs = u64::try_from(days * 86_400 + h * 3600 + mi * 60 + s).ok()?;
    Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

/// Relative age within a week (`12m`, `3h`, `2d`), an absolute calendar date
/// beyond that (`Oct 5`, with year if not the current one), or `—` when the
/// time is unknown.
pub fn relative_time(time: Option<SystemTime>, now: SystemTime) -> String {
    let Some(time) = time else {
        return "—".to_string();
    };
    let secs = now.duration_since(time).map(|d| d.as_secs()).unwrap_or(0);
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    if secs < MIN {
        "now".to_string()
    } else if secs < HOUR {
        format!("{}m", secs / MIN)
    } else if secs < DAY {
        format!("{}h", secs / HOUR)
    } else if secs < WEEK {
        format!("{}d", secs / DAY)
    } else {
        absolute_date(time, now)
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `Oct 5`, or `Oct 5 2025` when the year differs from now. UTC (as recorded);
/// may be a day off near midnight for distant sessions, which is fine here.
fn absolute_date(time: SystemTime, now: SystemTime) -> String {
    let epoch_days = |t: SystemTime| {
        (t.duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            / 86400) as i64
    };
    let (y, m, d) = civil_from_days(epoch_days(time));
    let (cy, _, _) = civil_from_days(epoch_days(now));
    let mon = MONTHS[(m - 1) as usize];
    if y == cy {
        format!("{mon} {d}")
    } else {
        format!("{mon} {d} {y}")
    }
}

/// Civil (Y, M, D) from days since the Unix epoch, UTC. Howard Hinnant's
/// `civil_from_days` algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Days since the Unix epoch from civil (Y, M, D), UTC. The inverse of
/// [`civil_from_days`] (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1)); // Unix epoch
        // 2026-10-05 is 20_731 days after the epoch.
        assert_eq!(civil_from_days(20_731), (2026, 10, 5));
        assert_eq!(days_from_civil(2026, 10, 5), 20_731);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
    }

    #[test]
    fn format_timestamp_labels_only_what_it_read_as_utc() {
        assert_eq!(
            format_timestamp("2026-10-05T12:34:56.789Z"),
            "2026-10-05 12:34 UTC"
        );
        // An offset or a missing zone is shown as recorded, unlabeled.
        assert_eq!(
            format_timestamp("2026-10-06T08:00:00+09:00"),
            "2026-10-06 08:00"
        );
        assert_eq!(format_timestamp("2026-10-05T12:34"), "2026-10-05 12:34");
    }

    #[test]
    fn parse_timestamp_reads_only_valid_utc() {
        let at = |secs| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
        let base = 20_731 * 86_400 + 12 * 3600 + 34 * 60 + 56;
        assert_eq!(parse_timestamp("2026-10-05T12:34:56Z"), at(base));
        assert_eq!(parse_timestamp("2026-10-05T12:34:56.789Z"), at(base));
        assert_eq!(parse_timestamp("2026-10-05T21:34:56+09:00"), None);
        assert_eq!(parse_timestamp("2026-10-05T12:34:56"), None);
        assert_eq!(parse_timestamp("2026-10-05T12x34y56Z"), None);
        assert_eq!(parse_timestamp("2026-10-05T12:34:5あZ"), None);
        assert_eq!(parse_timestamp("not a time"), None);
        // Out-of-range fields, and a year too wide for the arithmetic.
        assert_eq!(parse_timestamp("2026-02-31T00:00:00Z"), None);
        assert_eq!(parse_timestamp("2026-10-05T99:99:99Z"), None);
        assert_eq!(parse_timestamp("2026-10-05T12:34:5612Z"), None);
        assert_eq!(parse_timestamp("2026-10-05T12:34:56.Z"), None);
        assert_eq!(parse_timestamp("2026-13-05T12:34:56Z"), None);
        assert_eq!(parse_timestamp("1969-12-31T23:59:59Z"), None);
        assert_eq!(parse_timestamp("9999999999999999-01-01T00:00:00Z"), None);
        assert!(parse_timestamp("2024-02-29T00:00:00Z").is_some());
    }

    #[test]
    fn relative_time_buckets_and_absolute() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(20_731 * 86_400 + 12 * 3600);
        let ago = |secs| relative_time(Some(now - Duration::from_secs(secs)), now);
        assert_eq!(ago(30 * 60), "30m");
        assert_eq!(ago(3 * 3600), "3h");
        assert_eq!(ago(2 * 86_400), "2d");
        // Beyond a week -> absolute date (same year, no year suffix).
        assert_eq!(ago(10 * 86_400), "Sep 25");
        assert_eq!(relative_time(None, now), "—");
    }
}
