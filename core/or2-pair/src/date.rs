//! UTC calendar dates from the Unix clock, without a date library: the date in a key's
//! comment and the stamp of a backup file name.

use std::time::{SystemTime, UNIX_EPOCH};

/// A UTC date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl DateTime {
    pub fn now() -> Self {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        Self::from_unix(i64::try_from(seconds).unwrap_or(0))
    }

    /// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    pub fn from_unix(seconds: i64) -> Self {
        let days = seconds.div_euclid(86_400);
        let rest = seconds.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
        let month = if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        } as u32;
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        Self {
            year,
            month,
            day,
            hour: (rest / 3_600) as u32,
            minute: (rest % 3_600 / 60) as u32,
            second: (rest % 60) as u32,
        }
    }

    /// `2026-10-01`.
    pub fn date(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// `20261001-134502`.
    pub fn stamp(&self) -> String {
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_known_instants() {
        for (seconds, date, stamp) in [
            (0, "1970-01-01", "19700101-000000"),
            (951_782_400, "2000-02-29", "20000229-000000"),
            (1_782_864_000 + 3_661, "2026-07-01", "20260701-010101"),
            (4_102_444_799, "2099-12-31", "20991231-235959"),
            (-1, "1969-12-31", "19691231-235959"),
        ] {
            let time = DateTime::from_unix(seconds);
            assert_eq!(time.date(), date, "{seconds}");
            assert_eq!(time.stamp(), stamp, "{seconds}");
        }
    }

    #[test]
    fn leap_days_and_century_rules() {
        assert_eq!(DateTime::from_unix(1_709_164_800).date(), "2024-02-29");
        // 2100 is not a leap year: 2100-03-01 follows 2100-02-28.
        assert_eq!(DateTime::from_unix(4_107_456_000).date(), "2100-02-28");
        assert_eq!(
            DateTime::from_unix(4_107_456_000 + 86_400).date(),
            "2100-03-01"
        );
    }
}
