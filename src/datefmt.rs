//! Pure date/time formatting for the list and the conversation status line.
//! No TUI dependency, so it's unit-tested here.

use std::time::SystemTime;

/// `2026-10-13T12:48:20.086Z` -> `2026-10-13 12:48` (as recorded, UTC).
pub fn format_timestamp(ts: &str) -> String {
    let compact: String = ts.chars().take(16).collect();
    compact.replacen('T', " ", 1)
}

/// Relative age within a week (`12m`, `3h`, `2d`), an absolute calendar date
/// beyond that (`Oct 5`, with year if not the current one), or `—` when the
/// time is unknown.
pub fn relative_time(mtime: Option<SystemTime>, now: SystemTime) -> String {
    let Some(mtime) = mtime else {
        return "—".to_string();
    };
    let secs = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);
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
        absolute_date(mtime, now)
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `Oct 5`, or `Oct 5 2025` when the year differs from now. UTC (as recorded);
/// may be a day off near midnight for distant sessions, which is fine here.
fn absolute_date(mtime: SystemTime, now: SystemTime) -> String {
    let epoch_days = |t: SystemTime| {
        (t.duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            / 86400) as i64
    };
    let (y, m, d) = civil_from_days(epoch_days(mtime));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1)); // Unix epoch
        // 2026-10-05 is 20_731 days after the epoch.
        assert_eq!(civil_from_days(20_731), (2026, 10, 5));
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
