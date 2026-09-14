//! Defines the `ExcludeNewer` type which is used to exclude packages based on
//! their timestamp.

use jiff::{Timestamp, civil::Date, tz::TimeZone};
use std::{fmt, str::FromStr, time::Duration};

/// A point in time or an age used to exclude newer packages.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ExcludeNewer {
    Timestamp(Timestamp),
    Duration(Duration),
}

impl From<ExcludeNewer> for rattler_solve::ExcludeNewer {
    fn from(value: ExcludeNewer) -> Self {
        match value {
            ExcludeNewer::Timestamp(timestamp) => Self::from_datetime(timestamp),
            ExcludeNewer::Duration(duration) => Self::from_duration(duration),
        }
    }
}

#[derive(Debug)]
pub struct ParseExcludeNewerError;

impl fmt::Display for ParseExcludeNewerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "expected an RFC 3339 timestamp, a date, or a duration (for example, 3d)"
        )
    }
}

impl std::error::Error for ParseExcludeNewerError {}

impl FromStr for ExcludeNewer {
    type Err = ParseExcludeNewerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Try parsing as full timestamp first
        if let Ok(timestamp) = s.parse::<Timestamp>() {
            return Ok(ExcludeNewer::Timestamp(timestamp));
        }
        // For a date-only value, use the start of the next day in UTC so that
        // packages from the entire specified day are included.
        if let Ok(date) = s.parse::<Date>() {
            let next_day = date
                .tomorrow()
                .map_err(|_date_error| ParseExcludeNewerError)?;
            let timestamp = next_day
                .at(0, 0, 0, 0)
                .to_zoned(TimeZone::UTC)
                .map_err(|_timezone_error| ParseExcludeNewerError)?
                .timestamp();
            return Ok(ExcludeNewer::Timestamp(timestamp));
        }

        humantime::parse_duration(s)
            .map(ExcludeNewer::Duration)
            .map_err(|_duration_error| ParseExcludeNewerError)
    }
}

impl fmt::Display for ExcludeNewer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExcludeNewer::Timestamp(timestamp) => timestamp.fmt(f),
            ExcludeNewer::Duration(duration) => humantime::format_duration(*duration).fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rfc3339() {
        let exclude_newer = ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap();
        assert_eq!(
            exclude_newer,
            ExcludeNewer::Timestamp("2006-12-02T02:07:43Z".parse::<Timestamp>().unwrap())
        );
    }

    #[test]
    fn test_parse_date() {
        // When parsing a date, we should get midnight of the next day
        let exclude_newer = ExcludeNewer::from_str("2006-12-02").unwrap();
        assert_eq!(
            exclude_newer,
            ExcludeNewer::Timestamp("2006-12-03T00:00:00Z".parse::<Timestamp>().unwrap())
        );
    }

    #[test]
    fn test_display() {
        let exclude_newer = ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap();
        assert_eq!(exclude_newer.to_string(), "2006-12-02T02:07:43Z");
    }
}
