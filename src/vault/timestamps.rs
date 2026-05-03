//! Frontmatter timestamp emit + parse.
//!
//! Plugin-driven writes emit `created_at` / `updated_at` as ISO 8601 with
//! local offset (e.g., `2026-05-03T22:30:00+08:00`). Filters and the
//! search reader accept either that form or a bare `YYYY-MM-DD`, so a
//! mixed corpus (legacy `created:` date strings + new datetime values)
//! still compares cleanly during the migration window.

use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};

/// Current local time formatted as ISO 8601 with offset.
pub fn now_iso8601_local() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

/// Parse a `created_at` / `updated_at` (or legacy `created`) frontmatter value.
///
/// Accepts:
///   - RFC 3339 datetime with offset: `2026-05-03T22:30:00+08:00`, `...Z`
///   - Bare date: `2026-05-03` — interpreted as local-midnight.
pub fn parse_value(s: &str) -> Option<DateTime<FixedOffset>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt);
    }
    parse_date_as_local_midnight(s)
}

/// Parse a CLI filter date. `--created-after 2026-05-02` → local-midnight on
/// that date. Datetime input is also accepted for power users.
pub fn parse_filter(s: &str) -> Option<DateTime<FixedOffset>> {
    parse_value(s)
}

fn parse_date_as_local_midnight(s: &str) -> Option<DateTime<FixedOffset>> {
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    let naive = NaiveDateTime::new(date, NaiveTime::from_hms_opt(0, 0, 0)?);
    let local = Local.from_local_datetime(&naive).single()?;
    Some(local.fixed_offset())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_with_offset() {
        let dt = parse_value("2026-05-03T22:30:00+08:00").expect("rfc3339");
        assert_eq!(dt.to_rfc3339(), "2026-05-03T22:30:00+08:00");
    }

    #[test]
    fn parses_rfc3339_zulu() {
        let dt = parse_value("2026-05-03T14:30:00Z").expect("zulu");
        assert_eq!(dt.timezone().local_minus_utc(), 0);
    }

    #[test]
    fn parses_bare_date_as_local_midnight() {
        let dt = parse_value("2026-05-03").expect("bare date");
        // Hour=0 means local midnight (offset varies with TZ but the wall-clock
        // is always 00:00).
        assert_eq!(dt.format("%H:%M:%S").to_string(), "00:00:00");
    }

    #[test]
    fn empty_returns_none() {
        assert!(parse_value("").is_none());
        assert!(parse_value("   ").is_none());
    }

    #[test]
    fn now_has_offset_suffix() {
        let s = now_iso8601_local();
        // Expect a trailing offset like +08:00 or -07:00 or +00:00.
        let last = &s[s.len().saturating_sub(6)..];
        assert!(
            last.starts_with('+') || last.starts_with('-'),
            "missing offset in {s}"
        );
        assert_eq!(&last[3..4], ":");
    }
}
