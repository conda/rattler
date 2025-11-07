//! Defines the `ExcludeNewer` type which is used to exclude packages based on
//! their timestamp.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use std::fmt;
use std::str::FromStr;

/// A wrapper around a chrono `DateTime` that is used to exclude packages after a
/// certain point in time.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ExcludeNewer(pub DateTime<Utc>);

impl From<ExcludeNewer> for DateTime<Utc> {
    fn from(value: ExcludeNewer) -> Self {
        value.0
    }
}

impl FromStr for ExcludeNewer {
    type Err = chrono::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First try to parse as an RFC 3339 timestamp
        if let Ok(datetime) = DateTime::parse_from_rfc3339(s) {
            return Ok(ExcludeNewer(datetime.with_timezone(&Utc)));
        }

        // Otherwise, try to parse as a date (YYYY-MM-DD)
        // If we only have a date, we use the next day at midnight
        // This ensures that packages from the entire day are included
        let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")?;
        let next_day = date
            .succ_opt()
            .ok_or_else(|| NaiveDate::parse_from_str("invalid", "%Y-%m-%d").unwrap_err())?;
        let datetime = Utc
            .from_utc_datetime(&next_day.and_hms_opt(0, 0, 0).unwrap())
            .with_timezone(&Utc);

        Ok(ExcludeNewer(datetime))
    }
}

impl fmt::Display for ExcludeNewer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.to_rfc3339())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rfc3339() {
        let exclude_newer = ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap();
        assert_eq!(
            exclude_newer.0,
            DateTime::parse_from_rfc3339("2006-12-02T02:07:43Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn test_parse_date() {
        // When parsing a date, we should get midnight of the next day
        let exclude_newer = ExcludeNewer::from_str("2006-12-02").unwrap();
        assert_eq!(
            exclude_newer.0,
            DateTime::parse_from_rfc3339("2006-12-03T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn test_display() {
        let exclude_newer = ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap();
        assert_eq!(exclude_newer.to_string(), "2006-12-02T02:07:43+00:00");
    }
}
