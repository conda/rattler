/// An error that can occur when parsing a platform from a string.
#[derive(Debug, Clone, thiserror::Error, Eq, PartialEq)]
#[allow(missing_docs)]
pub enum ParsePlatformError {
    #[error("Failed to parse '{0}' as a PlatformName")]
    ParsePlatformNameError(String),

    #[error("Failed to parse '{0}' as a Subdir")]
    ParseSubdirError(#[from] rattler_conda_types::ParsePlatformError),
}

/// A valid name for a platform
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PlatformName(String);

impl std::convert::TryFrom<String> for PlatformName {
    type Error = ParsePlatformError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            Ok(Self(value))
        } else {
            Err(ParsePlatformError::ParsePlatformNameError(value))
        }
    }
}

impl std::fmt::Display for PlatformName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PlatformName {
    type Err = ParsePlatformError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.to_string();
        if value
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            Ok(Self(value))
        } else {
            Err(ParsePlatformError::ParsePlatformNameError(value))
        }
    }
}

impl std::ops::Deref for PlatformName {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Represents a package with platform-specific information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Platform {
    /// The name of the platform.
    pub name: PlatformName,
    /// The subdir of the platform.
    pub subdir: rattler_conda_types::Platform,
    /// The list of virtual conda packages.
    pub virtual_packages: Vec<String>,
}

impl std::hash::Hash for Platform {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_name() {
        assert!(PlatformName::try_from(String::from("test_1-2_3")).is_ok());
        assert!(PlatformName::try_from(String::from("linux-64")).is_ok());
        assert!(PlatformName::try_from(String::from("linux 64")).is_err());
        assert!(PlatformName::try_from(String::from("linux+64")).is_err());
    }
}
