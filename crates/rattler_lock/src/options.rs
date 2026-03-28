//! Defines [`SolveOptions`] and reexports from `rattler_solve` that are used.

use chrono::{DateTime, Utc};

// Reexport these fields.
pub use rattler_solve::{ChannelPriority, SolveStrategy};

fn format_duration(duration: std::time::Duration) -> String {
    let mut remaining = duration.as_secs();
    let mut formatted = String::new();

    for (unit, suffix) in [(86_400, "d"), (3_600, "h"), (60, "m"), (1, "s")] {
        let count = remaining / unit;
        if count > 0 {
            formatted.push_str(&format!("{count}{suffix}"));
            remaining %= unit;
        }
    }

    let nanos = duration.subsec_nanos();
    let millis = nanos / 1_000_000;
    let micros = (nanos % 1_000_000) / 1_000;
    let nanos = nanos % 1_000;

    if millis > 0 {
        formatted.push_str(&format!("{millis}ms"));
    }
    if micros > 0 {
        formatted.push_str(&format!("{micros}us"));
    }
    if nanos > 0 {
        formatted.push_str(&format!("{nanos}ns"));
    }

    if formatted.is_empty() {
        formatted.push_str("0s");
    }

    formatted
}

/// Specifies how package candidates newer than a cutoff should be excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExcludeNewer {
    /// An absolute cutoff timestamp.
    Timestamp(DateTime<Utc>),
    /// A relative duration cutoff, resolved against the current time.
    Duration(std::time::Duration),
}

impl ExcludeNewer {
    /// Returns the effective cutoff for the current time.
    pub fn cutoff(&self) -> DateTime<Utc> {
        match self {
            Self::Timestamp(cutoff) => *cutoff,
            Self::Duration(duration) => {
                let duration = chrono::Duration::from_std(*duration)
                    .expect("exclude-newer duration is too large");
                Utc::now() - duration
            }
        }
    }
}

impl From<DateTime<Utc>> for ExcludeNewer {
    fn from(value: DateTime<Utc>) -> Self {
        Self::Timestamp(value)
    }
}

impl From<std::time::Duration> for ExcludeNewer {
    fn from(value: std::time::Duration) -> Self {
        Self::Duration(value)
    }
}

impl From<ExcludeNewer> for rattler_solve::ExcludeNewer {
    fn from(value: ExcludeNewer) -> Self {
        match value {
            ExcludeNewer::Timestamp(cutoff) => Self::from_datetime(cutoff),
            ExcludeNewer::Duration(duration) => Self::from_duration(duration),
        }
    }
}

impl From<&ExcludeNewer> for rattler_solve::ExcludeNewer {
    fn from(value: &ExcludeNewer) -> Self {
        value.clone().into()
    }
}

impl serde::Serialize for ExcludeNewer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Timestamp(cutoff) => cutoff.serialize(serializer),
            Self::Duration(duration) => serializer.serialize_str(&format_duration(*duration)),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ExcludeNewer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum RawExcludeNewer {
            Timestamp(DateTime<Utc>),
            Duration(String),
        }

        match RawExcludeNewer::deserialize(deserializer)? {
            RawExcludeNewer::Timestamp(cutoff) => Ok(ExcludeNewer::Timestamp(cutoff)),
            RawExcludeNewer::Duration(duration) => humantime::parse_duration(&duration)
                .map(ExcludeNewer::Duration)
                .map_err(serde::de::Error::custom),
        }
    }
}

/// The prerelease mode used to resolve `PyPI` dependencies.
///
/// This controls how the resolver handles pre-release versions of packages.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
#[serde(rename_all = "kebab-case")]
pub enum PypiPrereleaseMode {
    /// Disallow all pre-release versions.
    Disallow,
    /// Allow all pre-release versions.
    Allow,
    /// Allow pre-release versions if necessary to satisfy the requirements.
    IfNecessary,
    /// Allow pre-release versions for packages explicitly requested.
    Explicit,
    /// Allow pre-release versions if necessary or explicitly requested.
    /// This is the default mode.
    #[default]
    IfNecessaryOrExplicit,
}

/// Options that were used during the resolution of the packages stored in the
/// lock-file. These options strongly influence the outcome of the solve and are
/// therefore stored along with the locked packages.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SolveOptions {
    /// The strategy that was used to solve the dependencies.
    #[serde(default, skip_serializing_if = "crate::utils::serde::is_default")]
    pub strategy: SolveStrategy,

    /// The channel priority that was used to solve the dependencies.
    #[serde(default, skip_serializing_if = "crate::utils::serde::is_default")]
    pub channel_priority: ChannelPriority,

    /// Packages after this date have been excluded from the lock file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_newer: Option<ExcludeNewer>,

    /// The prerelease mode that was used to resolve `PyPI` dependencies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pypi_prerelease_mode: Option<PypiPrereleaseMode>,
}

#[cfg(test)]
mod tests {
    use super::ExcludeNewer;

    #[test]
    fn serializes_duration_with_short_unit_suffixes() {
        let value = serde_yaml::to_string(&ExcludeNewer::Duration(std::time::Duration::from_secs(
            3 * 24 * 60 * 60,
        )))
        .unwrap();

        assert_eq!(value.trim(), "3d");
    }

    #[test]
    fn deserializes_duration_strings() {
        let value: ExcludeNewer = serde_yaml::from_str("3d").unwrap();
        assert_eq!(
            value,
            ExcludeNewer::Duration(std::time::Duration::from_secs(3 * 24 * 60 * 60))
        );
    }
}
