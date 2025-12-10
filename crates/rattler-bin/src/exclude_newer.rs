//! Defines the `ExcludeNewer` type which is used to exclude packages based on
//! their timestamp.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::Timestamp;
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

/// Error type for parsing `ExcludeNewer` values.
#[derive(Debug, Clone)]
pub struct ParseExcludeNewerError {
    message: String,
}

impl fmt::Display for ParseExcludeNewerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseExcludeNewerError {}

impl FromStr for ExcludeNewer {
    type Err = ParseExcludeNewerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // First try to parse as an RFC 3339 timestamp
        if let Ok(timestamp) = s.parse::<Timestamp>() {
            return Ok(ExcludeNewer(timestamp));
        }

        // Otherwise, try to parse as a date (YYYY-MM-DD)
        // If we only have a date, we use the next day at midnight UTC
        // This ensures that packages from the entire day are included
        let date = s.parse::<Date>().map_err(|e| ParseExcludeNewerError {
            message: format!("failed to parse date: {e}"),
        })?;

        let next_day = date
            .tomorrow()
            .map_err(|e| ParseExcludeNewerError {
                message: format!("failed to get next day: {e}"),
            })?;

        let timestamp = next_day
            .at(0, 0, 0, 0)
            .to_zoned(TimeZone::UTC)
            .map_err(|e| ParseExcludeNewerError {
                message: format!("failed to convert to timestamp: {e}"),
            })?
            .timestamp();

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
        // jiff formats timestamps in a standard way
        assert!(exclude_newer.to_string().contains("2006-12-02"));
    }
}
