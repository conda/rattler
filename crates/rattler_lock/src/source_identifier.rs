use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

use rattler_conda_types::PackageName;
use serde_with::{DeserializeFromStr, SerializeDisplay};
use thiserror::Error;

use crate::UrlOrPath;

/// A unique identifier for a source package in the lock file.
///
/// This type represents the format `<name>[<hash>] @ <location>` which is used
/// to uniquely identify source packages. The hash is computed from the package
/// record to disambiguate packages at the same location with different configurations.
///
/// # Examples
///
/// ```text
/// numba-cuda[9f3c2a7b] @ .
/// numba-cuda[9f3c2a7b] @ https://example.com/pkgs/...
/// numba-cuda[9f3c2a7b] @ git+https://host/org/repo@main
/// ```
#[derive(Debug, Clone, Eq, PartialEq, Hash, SerializeDisplay, DeserializeFromStr)]
pub struct SourceIdentifier {
    /// The name of the package.
    name: PackageName,

    /// A short hash (8 hex characters) computed from the package record.
    /// This is used to disambiguate packages at the same location.
    hash: String,

    /// The location of the source package (URL or path).
    location: UrlOrPath,
}

impl SourceIdentifier {
    /// Creates a new source identifier.
    ///
    /// # Arguments
    ///
    /// * `name` - The package name
    /// * `hash` - A short hash string (typically 8 hex characters)
    /// * `location` - The location of the source package
    pub fn new(name: PackageName, hash: impl Into<String>, location: UrlOrPath) -> Self {
        Self {
            name,
            hash: hash.into(),
            location,
        }
    }

    /// Returns the package name.
    pub fn name(&self) -> &PackageName {
        &self.name
    }

    /// Returns the hash.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Returns the location.
    pub fn location(&self) -> &UrlOrPath {
        &self.location
    }

    /// Consumes this identifier and returns its parts.
    pub fn into_parts(self) -> (PackageName, String, UrlOrPath) {
        (self.name, self.hash, self.location)
    }
}

impl Display for SourceIdentifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}[{}] @ {}",
            self.name.as_source(),
            self.hash,
            self.location
        )
    }
}

/// Error type for parsing a [`SourceIdentifier`] from a string.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum ParseSourceIdentifierError {
    /// Missing the opening bracket `[` for the hash.
    #[error("missing '[' after package name")]
    MissingOpenBracket,

    /// Missing the closing bracket `]` for the hash.
    #[error("missing ']' after hash")]
    MissingCloseBracket,

    /// Missing the ` @ ` separator between the identifier and location.
    #[error("missing ' @ ' separator")]
    MissingSeparator,

    /// Invalid package name.
    #[error("invalid package name: {0}")]
    InvalidPackageName(#[from] rattler_conda_types::InvalidPackageNameError),

    /// Invalid location.
    #[error("invalid location: {0}")]
    InvalidLocation(#[from] crate::url_or_path::PathOrUrlError),

    /// Empty hash.
    #[error("hash cannot be empty")]
    EmptyHash,
}

impl FromStr for SourceIdentifier {
    type Err = ParseSourceIdentifierError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Find the opening bracket
        let open_bracket = s
            .find('[')
            .ok_or(ParseSourceIdentifierError::MissingOpenBracket)?;

        // Find the closing bracket
        let close_bracket = s
            .find(']')
            .ok_or(ParseSourceIdentifierError::MissingCloseBracket)?;

        // Ensure brackets are in correct order
        if close_bracket <= open_bracket {
            return Err(ParseSourceIdentifierError::MissingCloseBracket);
        }

        // Extract the name part (before the opening bracket)
        let name_str = &s[..open_bracket];
        let name = PackageName::from_str(name_str)?;

        // Extract the hash (between brackets)
        let hash = &s[open_bracket + 1..close_bracket];
        if hash.is_empty() {
            return Err(ParseSourceIdentifierError::EmptyHash);
        }

        // The rest should be " @ <location>"
        let remainder = &s[close_bracket + 1..];
        let location_str = remainder
            .strip_prefix(" @ ")
            .ok_or(ParseSourceIdentifierError::MissingSeparator)?;

        let location = UrlOrPath::from_str(location_str)?;

        Ok(Self {
            name,
            hash: hash.to_string(),
            location,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_relative_path() {
        let id: SourceIdentifier = "numba-cuda[9f3c2a7b] @ .".parse().unwrap();
        assert_eq!(id.name().as_source(), "numba-cuda");
        assert_eq!(id.hash(), "9f3c2a7b");
        assert_eq!(id.location().as_str(), ".");
    }

    #[test]
    fn test_parse_url() {
        let id: SourceIdentifier = "my-package[abcd1234] @ https://example.com/pkgs/source"
            .parse()
            .unwrap();
        assert_eq!(id.name().as_source(), "my-package");
        assert_eq!(id.hash(), "abcd1234");
        assert_eq!(id.location().as_str(), "https://example.com/pkgs/source");
    }

    #[test]
    fn test_parse_git_url() {
        let id: SourceIdentifier = "my-pkg[deadbeef] @ git+https://github.com/org/repo@main"
            .parse()
            .unwrap();
        assert_eq!(id.name().as_source(), "my-pkg");
        assert_eq!(id.hash(), "deadbeef");
        assert_eq!(
            id.location().as_str(),
            "git+https://github.com/org/repo@main"
        );
    }

    #[test]
    fn test_display_roundtrip() {
        let original = "numba-cuda[9f3c2a7b] @ .";
        let id: SourceIdentifier = original.parse().unwrap();
        assert_eq!(id.to_string(), original);
    }

    #[test]
    fn test_display_url_roundtrip() {
        let original = "my-package[abcd1234] @ https://example.com/pkgs/source";
        let id: SourceIdentifier = original.parse().unwrap();
        assert_eq!(id.to_string(), original);
    }

    #[test]
    fn test_missing_open_bracket() {
        let result: Result<SourceIdentifier, _> = "numba-cuda9f3c2a7b] @ .".parse();
        assert!(matches!(
            result,
            Err(ParseSourceIdentifierError::MissingOpenBracket)
        ));
    }

    #[test]
    fn test_missing_close_bracket() {
        let result: Result<SourceIdentifier, _> = "numba-cuda[9f3c2a7b @ .".parse();
        assert!(matches!(
            result,
            Err(ParseSourceIdentifierError::MissingCloseBracket)
        ));
    }

    #[test]
    fn test_missing_separator() {
        let result: Result<SourceIdentifier, _> = "numba-cuda[9f3c2a7b].".parse();
        assert!(matches!(
            result,
            Err(ParseSourceIdentifierError::MissingSeparator)
        ));
    }

    #[test]
    fn test_empty_hash() {
        let result: Result<SourceIdentifier, _> = "numba-cuda[] @ .".parse();
        assert!(matches!(result, Err(ParseSourceIdentifierError::EmptyHash)));
    }

    #[test]
    fn test_invalid_package_name() {
        // A name with invalid characters should fail to parse
        let result: Result<SourceIdentifier, _> = "invalid name with spaces[hash] @ .".parse();
        assert!(
            matches!(
                result,
                Err(ParseSourceIdentifierError::InvalidPackageName(_))
            ),
            "expected InvalidPackageName error, got: {result:?}"
        );
    }

    #[test]
    fn test_serde_roundtrip() {
        let id = SourceIdentifier::new(
            PackageName::from_str("my-package").unwrap(),
            "abcd1234",
            UrlOrPath::from_str(".").unwrap(),
        );

        let serialized = serde_yaml::to_string(&id).unwrap();
        let deserialized: SourceIdentifier = serde_yaml::from_str(&serialized).unwrap();

        assert_eq!(id, deserialized);
    }
}
