//! Calendar dates from Unix time and back, in integers (proleptic Gregorian, UTC), for the
//! screen's clock and the board's real-time clock chip. H. Hinnant's algorithms.

/// A UTC date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// 0 = Sunday.
    pub weekday: u8,
}

impl Civil {
    pub fn from_unix(unix: u32) -> Self {
        let t = i64::from(unix);
        let days = t.div_euclid(86_400);
        let secs = t.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            hour: (secs / 3600) as u8,
            minute: (secs / 60 % 60) as u8,
            second: (secs % 60) as u8,
            weekday: (days + 4).rem_euclid(7) as u8,
        }
    }

    /// `None` for a field out of range or a time outside `u32` (the weekday is not checked).
    pub fn to_unix(&self) -> Option<u32> {
        let days_in_month = match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if is_leap(self.year) => 29,
            2 => 28,
            _ => return None,
        };
        if self.day == 0 || self.day > days_in_month || self.hour > 23 || self.minute > 59 {
            return None;
        }
        if self.second > 59 {
            return None;
        }
        let days = days_from_civil(self.year, self.month, self.day);
        let t = days * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second);
        u32::try_from(t).ok()
    }
}

pub fn is_leap(y: i32) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// Days since 1970-01-01 to (year, month, day).
pub fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    (year, month, day)
}

/// (year, month 1-12, day 1-31) to days since 1970-01-01.
pub fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 0..=99 to packed BCD.
pub fn to_bcd(v: u8) -> u8 {
    let v = v % 100;
    ((v / 10) << 4) | (v % 10)
}

/// Packed BCD to 0..=99; `None` if a nibble is not a digit.
pub fn from_bcd(b: u8) -> Option<u8> {
    let (hi, lo) = (b >> 4, b & 0x0F);
    (hi < 10 && lo < 10).then_some(hi * 10 + lo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        let c = Civil::from_unix(0);
        assert_eq!((c.year, c.month, c.day, c.weekday), (1970, 1, 1, 4));
        // 2026-11-08 14:05:09 UTC, a Sunday.
        let c = Civil::from_unix(1_794_146_709);
        assert_eq!(
            (
                c.year, c.month, c.day, c.hour, c.minute, c.second, c.weekday
            ),
            (2026, 11, 8, 14, 5, 9, 0)
        );
        assert_eq!(c.to_unix(), Some(1_794_146_709));
        // 2000-02-29 and 2100-03-01 (2100 is not leap).
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        assert_eq!(civil_from_days(days_from_civil(2100, 3, 1)), (2100, 3, 1));
        assert!(!is_leap(2100) && is_leap(2000) && is_leap(2028));
    }

    #[test]
    fn every_day_of_the_century_round_trips() {
        let first = days_from_civil(2000, 1, 1);
        let last = days_from_civil(2099, 12, 31);
        for d in first..=last {
            let (y, m, day) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, day), d);
            let unix = (d * 86_400 + 86_399) as u32;
            assert_eq!(Civil::from_unix(unix).to_unix(), Some(unix));
        }
        assert_eq!(last - first + 1, 36_525);
    }

    #[test]
    fn rejects_impossible_dates_and_bcd() {
        let mut c = Civil::from_unix(1_794_146_709);
        c.month = 2;
        c.day = 29;
        c.year = 2027;
        assert_eq!(c.to_unix(), None);
        c.year = 2028;
        assert!(c.to_unix().is_some());
        c.month = 13;
        assert_eq!(c.to_unix(), None);
        assert_eq!((to_bcd(59), from_bcd(0x59)), (0x59, Some(59)));
        assert_eq!(from_bcd(0x5A), None);
        assert_eq!(from_bcd(0xA0), None);
    }
}
