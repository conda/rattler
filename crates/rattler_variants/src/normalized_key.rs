use rattler_conda_types::PackageName;
use serde::{Deserialize, Serialize};
use std::hash::Hash;

/// A key in a variant configuration. Keys are normalized by replacing '-', '_' and '.' with '_'.
#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct NormalizedKey(pub String);

impl NormalizedKey {
    /// Returns the normalized form of the key.
    pub fn normalize(&self) -> String {
        self.0
            .chars()
            .map(|c| match c {
                '-' | '_' | '.' => '_',
                x => x,
            })
            .collect()
    }
}

impl Serialize for NormalizedKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.normalize().serialize(serializer)
    }
}

impl From<String> for NormalizedKey {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for NormalizedKey {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<&PackageName> for NormalizedKey {
    fn from(package: &PackageName) -> Self {
        package.as_normalized().into()
    }
}
