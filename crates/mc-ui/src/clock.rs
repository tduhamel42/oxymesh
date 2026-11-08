//! Local date and time from the device clock, in integers (no time zone database: a fixed
//! offset in minutes, set in Settings → Time).

use alloc::format;
use alloc::string::String;
use mc_core::civil::civil_from_days;

/// Before this, the device clock has not been set (2020-09-13).
const CLOCK_SET: u32 = 1_600_000_000;

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    /// 0 = Sunday.
    pub weekday: u8,
}

/// Local time, or `None` while the clock is not set.
pub fn local(unix: u32, tz_offset_min: i16) -> Option<DateTime> {
    if unix < CLOCK_SET {
        return None;
    }
    let t = i64::from(unix) + i64::from(tz_offset_min) * 60;
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Some(DateTime {
        year,
        month,
        day,
        hour: (secs / 3600) as u8,
        minute: (secs / 60 % 60) as u8,
        weekday: (days + 4).rem_euclid(7) as u8,
    })
}

/// `14:02`, or `--:--` while the clock is not set.
pub fn hhmm(t: Option<DateTime>) -> String {
    match t {
        Some(t) => format!("{:02}:{:02}", t.hour, t.minute),
        None => String::from("--:--"),
    }
}

/// `Thu 6 Nov 2026`, or `Clock not set`.
pub fn date(t: Option<DateTime>) -> String {
    match t {
        Some(t) => format!(
            "{} {} {} {}",
            WEEKDAYS.get(usize::from(t.weekday)).unwrap_or(&"?"),
            t.day,
            MONTHS
                .get(usize::from(t.month.saturating_sub(1)))
                .unwrap_or(&"?"),
            t.year
        ),
        None => String::from("Clock not set"),
    }
}

/// A fixed timestamp for lists: `14:02` today, `yesterday`, else `6 Nov`.
pub fn stamp(unix: u32, now: u32, tz_offset_min: i16) -> String {
    let (Some(t), Some(n)) = (local(unix, tz_offset_min), local(now, tz_offset_min)) else {
        return String::from("?");
    };
    let day = |x: u32| (i64::from(x) + i64::from(tz_offset_min) * 60).div_euclid(86_400);
    match day(now) - day(unix) {
        0 => hhmm(Some(t)),
        1 => String::from("yesterday"),
        _ if t.year == n.year => format!(
            "{} {}",
            t.day,
            MONTHS
                .get(usize::from(t.month.saturating_sub(1)))
                .unwrap_or(&"?")
        ),
        _ => format!("{}/{}/{}", t.day, t.month, t.year % 100),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        // 2026-11-06 14:02:00 UTC.
        let t = local(1_793_973_720, 0).unwrap();
        assert_eq!(
            (t.year, t.month, t.day, t.hour, t.minute),
            (2026, 11, 6, 14, 2)
        );
        assert_eq!(date(Some(t)), "Fri 6 Nov 2026");
        // Paris in winter: UTC+1.
        assert_eq!(hhmm(local(1_793_973_720, 60)), "15:02");
        // Leap day.
        let t = local(1_709_208_000, 0).unwrap();
        assert_eq!((t.year, t.month, t.day), (2024, 2, 29));
        assert_eq!(hhmm(local(5, 0)), "--:--");
    }

    #[test]
    fn stamps() {
        let now = 1_793_973_720;
        assert_eq!(stamp(now - 60, now, 0), "14:01");
        assert_eq!(stamp(now - 86_400, now, 0), "yesterday");
        assert_eq!(stamp(now - 5 * 86_400, now, 0), "1 Nov");
        assert_eq!(stamp(now - 400 * 86_400, now, 0), "2/10/25");
    }
}
