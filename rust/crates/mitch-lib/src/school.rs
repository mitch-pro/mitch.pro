//! School-day time helpers bucketed by America/Los_Angeles wall time.
//!
//! The LA offset is derived from the US Pacific DST rule (PST −8 / PDT −7),
//! decided on the UTC timeline so both transition instants and the
//! fall-back ambiguous hour resolve exactly as ICU does — no tz database
//! dependency.

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Civil (year, month, day, hour) from a Unix-ms timestamp with the given
/// UTC offset already applied.
fn civil_from_ms(ms: i64) -> (i64, i64, i64, i64) {
    let days = ms.div_euclid(86_400_000);
    let secs = ms.rem_euclid(86_400_000) / 1000;
    let hour = secs / 3600;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hour)
}

/// Weekday of a civil date, 0=Sunday..6=Saturday (1970-01-01 = Thursday).
fn weekday_of(year: i64, month: i64, day: i64) -> i64 {
    (days_from_civil(year, month, day) + 4).rem_euclid(7)
}

fn first_sunday_of(year: i64, month: i64) -> i64 {
    let wd = weekday_of(year, month, 1);
    1 + (7 - wd) % 7
}

/// Is DST active at the given UTC instant (US 2007 rule)? Decided on the
/// UTC timeline — the spring transition instant is 02:00 PST = 10:00Z on
/// the second Sunday of March, the fall transition is 02:00 PDT = 09:00Z
/// on the first Sunday of November. This matches ICU exactly, including
/// which side of the fall-back ambiguous hour a given instant lands on.
fn pacific_dst_active_at_ms(ms: i64) -> bool {
    let (py, _, _, _) = civil_from_ms(ms - 8 * 3_600_000);
    let start_day = first_sunday_of(py, 3) + 7;
    let end_day = first_sunday_of(py, 11);
    let start = days_from_civil(py, 3, start_day) * 86_400_000 + 10 * 3_600_000;
    let end = days_from_civil(py, 11, end_day) * 86_400_000 + 9 * 3_600_000;
    ms >= start && ms < end
}

/// Split a Unix-ms timestamp into America/Los_Angeles wall-clock parts:
/// (year, month 1-12, day, hour, weekday 0=Sun, minute).
pub fn la_local_parts(ms: i64) -> (i64, i64, i64, i64, i64, i64) {
    let offset_hours = if pacific_dst_active_at_ms(ms) { 7 } else { 8 };
    let (year, month, day, hour) = civil_from_ms(ms - offset_hours * 3_600_000);
    let wd = weekday_of(year, month, day);
    let minute = (ms.rem_euclid(86_400_000) / 60_000) % 60;
    (year, month, day, hour, wd, minute)
}

/// Current Unix time in milliseconds.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `new Date().toISOString().slice(0, 10)` — the UTC calendar day, `YYYY-MM-DD`.
pub fn utc_iso_day(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// `formatSchoolHour(hour)` (server.js:25381).
pub fn format_school_hour(hour: i64) -> String {
    let start_hour = if hour % 12 == 0 { 12 } else { hour % 12 };
    let start_ampm = if hour < 12 { "AM" } else { "PM" };
    let end_raw = hour + 1;
    let end_hour = if end_raw % 12 == 0 { 12 } else { end_raw % 12 };
    let end_ampm = if end_raw < 12 { "AM" } else { "PM" };
    format!("{start_hour}:00 {start_ampm} - {end_hour}:00 {end_ampm} PDT")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_iso_day_matches_js() {
        // 2026-09-14T00:00:00Z (UTC day already rolled over from local).
        assert_eq!(utc_iso_day(1789344000000), "2026-09-14");
        // 2026-09-13T23:59:59Z is still the 13th in UTC.
        assert_eq!(utc_iso_day(1789343999999), "2026-09-13");
        // Epoch + pre-epoch days.
        assert_eq!(utc_iso_day(0), "1970-01-01");
        assert_eq!(utc_iso_day(-1), "1969-12-31");
        assert_eq!(utc_iso_day(86_400_000), "1970-01-02");
    }

    #[test]
    fn la_parts_handle_dst_boundaries() {
        // 2026-03-08 is the second Sunday of March. 09:59:59Z = 01:59 PST.
        let (_, _, _, h, wd, _) = la_local_parts(1772963999000);
        assert_eq!((h, wd), (1, 0));
        // 10:00:00Z = 03:00 PDT (the skipped hour).
        let (_, _, _, h, _, _) = la_local_parts(1772964000000);
        assert_eq!(h, 3);
        // Summer: 2026-07-04T20:00:00Z = 13:00 PDT.
        let (_, _, _, h, _, _) = la_local_parts(1783195200000);
        assert_eq!(h, 13);
        // Winter: 2026-01-15T20:00:00Z = 12:00 PST.
        let (_, _, _, h, _, _) = la_local_parts(1768507200000);
        assert_eq!(h, 12);
        // November: 2026-11-01T09:00Z = 01:00 PST (the fall-back instant).
        let (_, _, _, h, _, _) = la_local_parts(1793523600000);
        assert_eq!(h, 1);
        // One millisecond later is unambiguously 01:00 PST (ICU parity).
        let (_, _, _, h, _, _) = la_local_parts(1793523600001);
        assert_eq!(h, 1);
        // Still pre-transition: 2026-11-01T08:59:59Z = 01:59:59 PDT.
        let (_, _, _, h, _, _) = la_local_parts(1793523599000);
        assert_eq!(h, 1);
    }

    #[test]
    fn format_school_hour_matches_js() {
        assert_eq!(format_school_hour(12), "12:00 PM - 1:00 PM PDT");
        assert_eq!(format_school_hour(8), "8:00 AM - 9:00 AM PDT");
        assert_eq!(format_school_hour(11), "11:00 AM - 12:00 PM PDT");
        assert_eq!(format_school_hour(14), "2:00 PM - 3:00 PM PDT");
    }

    /// Values captured from `Intl.DateTimeFormat` with
    /// `timeZone: 'America/Los_Angeles'` under bun (ICU) — the reference
    /// this port must match, including both fall-back ambiguous-hour
    /// occurrences and both transition instants.
    #[test]
    fn la_parts_matches_js_reference() {
        const REF: &[(i64, i64, i64, i64, i64, i64, i64)] = &[
            (1772963999000, 2026, 3, 8, 1, 0, 59),
            (1772964000000, 2026, 3, 8, 3, 0, 0),
            (1772964000001, 2026, 3, 8, 3, 0, 0),
            (1783195200000, 2026, 7, 4, 13, 6, 0),
            (1768507200000, 2026, 1, 15, 12, 4, 0),
            (1793523599000, 2026, 11, 1, 1, 0, 59),
            (1793523600000, 2026, 11, 1, 1, 0, 0),
            (1793523600001, 2026, 11, 1, 1, 0, 0),
            (1793530000000, 2026, 11, 1, 2, 0, 46),
            (1793610000000, 2026, 11, 2, 1, 1, 0),
            (1741234567890, 2025, 3, 5, 20, 3, 16),
            (1673512345000, 2023, 1, 12, 0, 4, 32),
        ];
        for &(ms, year, month, day, hour, wd, minute) in REF {
            assert_eq!(
                la_local_parts(ms),
                (year, month, day, hour, wd, minute),
                "mismatch at {ms}"
            );
        }
    }
}
