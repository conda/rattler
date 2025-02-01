//! Track features wrapper type
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::fmt;
use std::str::FromStr;

/// Wrapper type for track features
#[derive(SerializeDisplay, DeserializeFromStr, Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackFeatures(Vec<String>);

impl fmt::Display for TrackFeatures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.join(","))
    }
}

impl FromStr for TrackFeatures {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            Ok(TrackFeatures(Vec::new()))
        } else {
            Ok(TrackFeatures(
                s.split(',').map(|s| s.trim().to_string()).collect(),
            ))
        }
    }
}

impl Default for TrackFeatures {
    fn default() -> Self {
        TrackFeatures(Vec::new())
    }
}

impl TrackFeatures {
    /// Return true if the track features are empty
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Return the track features
    pub fn features(&self) -> &[String] {
        &self.0
    }

    /// Return the number of track features
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Create a new track features from a list of features
    pub fn from_features(features: &[String]) -> Self {
        TrackFeatures(features.to_vec())
    }
}
