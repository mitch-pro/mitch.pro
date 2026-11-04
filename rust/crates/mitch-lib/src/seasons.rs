//! Holiday theme seasons, by date. The same windows are in
//! `webserver/site-theme.js` (`holidayFor`); keep the two in step (both have
//! tests with the same dates). See `docs/holiday-themes.md`.
//!
//! | Season | Window |
//! | --- | --- |
//! | `halloween` | Oct 15 - Oct 31 |
//! | `thanksgiving` | Nov 1 - Thanksgiving Day (fourth Thursday of November) |
//! | `christmas` | the day after Thanksgiving - Dec 26 |
//! | `newyear` | Dec 27 - Jan 3 |
//! | `valentines` | Feb 7 - Feb 14 |

use chrono::{Datelike, NaiveDate, Weekday};

/// Every season, in calendar order from Halloween on.
pub const ALL: &[&str] = &[
    "halloween",
    "thanksgiving",
    "christmas",
    "newyear",
    "valentines",
];

/// Day of the month (November) Thanksgiving falls on in `year`.
pub fn thanksgiving_day(year: i32) -> u32 {
    let Some(first) = NaiveDate::from_ymd_opt(year, 11, 1) else {
        return 22; // not reachable for a valid year; the earliest possible day
    };
    let to_thursday =
        (Weekday::Thu.num_days_from_monday() + 7 - first.weekday().num_days_from_monday()) % 7;
    1 + to_thursday + 21
}

/// The season `date` is in and the date it started, or `None` between seasons.
/// New Year's started on Dec 27 even for the days in January.
pub fn current(date: NaiveDate) -> Option<(&'static str, NaiveDate)> {
    let (year, month, day) = (date.year(), date.month(), date.day());
    let on = NaiveDate::from_ymd_opt;
    match month {
        10 if day >= 15 => Some(("halloween", on(year, 10, 15)?)),
        11 => {
            let tg = thanksgiving_day(year);
            if day <= tg {
                Some(("thanksgiving", on(year, 11, 1)?))
            } else {
                Some(("christmas", on(year, 11, tg + 1)?))
            }
        }
        12 if day <= 26 => Some(("christmas", on(year, 11, thanksgiving_day(year) + 1)?)),
        12 => Some(("newyear", on(year, 12, 27)?)),
        1 if day <= 3 => Some(("newyear", on(year - 1, 12, 27)?)),
        2 if (7..=14).contains(&day) => Some(("valentines", on(year, 2, 7)?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn season(y: i32, m: u32, d: u32) -> Option<&'static str> {
        current(NaiveDate::from_ymd_opt(y, m, d).unwrap()).map(|(name, _)| name)
    }

    /// The same cases as `tests/test_holiday_theme.mjs`.
    #[test]
    fn windows_match_the_page() {
        assert_eq!(season(2026, 10, 14), None);
        assert_eq!(season(2026, 10, 15), Some("halloween"));
        assert_eq!(season(2026, 10, 31), Some("halloween"));
        assert_eq!(season(2026, 11, 1), Some("thanksgiving"));
        assert_eq!(season(2026, 11, 26), Some("thanksgiving"));
        assert_eq!(season(2026, 11, 27), Some("christmas"));
        assert_eq!(season(2026, 12, 26), Some("christmas"));
        assert_eq!(season(2026, 12, 27), Some("newyear"));
        assert_eq!(season(2027, 1, 3), Some("newyear"));
        assert_eq!(season(2027, 1, 4), None);
        assert_eq!(season(2027, 2, 6), None);
        assert_eq!(season(2027, 2, 7), Some("valentines"));
        assert_eq!(season(2027, 2, 14), Some("valentines"));
        assert_eq!(season(2027, 2, 15), None);
        assert_eq!(season(2027, 7, 4), None);
        // Thanksgiving moves with the calendar.
        assert_eq!(thanksgiving_day(2025), 27);
        assert_eq!(thanksgiving_day(2026), 26);
        assert_eq!(thanksgiving_day(2027), 25);
        assert_eq!(thanksgiving_day(2029), 22);
        assert_eq!(season(2029, 11, 23), Some("christmas"));
    }

    #[test]
    fn a_season_reports_the_day_it_started() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        assert_eq!(
            current(d(2026, 11, 4)),
            Some(("thanksgiving", d(2026, 11, 1)))
        );
        assert_eq!(
            current(d(2026, 12, 20)),
            Some(("christmas", d(2026, 11, 27)))
        );
        // The January days belong to the New Year's that began the December before.
        assert_eq!(current(d(2027, 1, 2)), Some(("newyear", d(2026, 12, 27))));
    }
}
