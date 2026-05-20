//! Defines the `ExcludeNewer` type which is used to exclude packages based on
//! their timestamp.

use jiff::{civil::Date, tz::TimeZone, Timestamp};
use std::fmt;
use std::str::FromStr;

/// A wrapper around a jiff `Timestamp` that is used to exclude packages after a
/// certain point in time.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ExcludeNewer(pub Timestamp);

impl From<ExcludeNewer> for Timestamp {
    fn from(value: ExcludeNewer) -> Self {
        value.0
    }
}

impl From<ExcludeNewer> for rattler_solve::ExcludeNewer {
    fn from(value: ExcludeNewer) -> Self {
        rattler_solve::ExcludeNewer::from_datetime(value.0)
    }
}

impl FromStr for ExcludeNewer {
    type Err = jiff::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Try parsing as full timestamp first
        if let Ok(timestamp) = s.parse::<Timestamp>() {
            return Ok(ExcludeNewer(timestamp));
        }
        // Fall back to date-only (use start of next day in UTC)
        let date = s.parse::<Date>()?;
        let next_day = date.tomorrow()?;
        let timestamp = next_day.at(0, 0, 0, 0).to_zoned(TimeZone::UTC)?.timestamp();
        Ok(ExcludeNewer(timestamp))
    }
}

impl fmt::Display for ExcludeNewer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
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
            "2006-12-02T02:07:43Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn test_parse_date() {
        // When parsing a date, we should get midnight of the next day
        let exclude_newer = ExcludeNewer::from_str("2006-12-02").unwrap();
        assert_eq!(
            exclude_newer.0,
            "2006-12-03T00:00:00Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn test_display() {
        let exclude_newer = ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap();
        assert_eq!(exclude_newer.to_string(), "2006-12-02T02:07:43Z");
    }
}
