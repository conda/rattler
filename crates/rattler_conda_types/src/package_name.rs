use std::borrow::{Borrow, Cow};
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, DeserializeFromStr};
use thiserror::Error;

use crate::package::CondaArchiveIdentifier;
use crate::utils::serde::DeserializeFromStrUnchecked;

/// A representation of a conda package name. This struct both stores the source string from which
/// this instance was created as well as a normalized name that can be used to compare different
/// names. The normalized name is guaranteed to be a valid conda package name.
///
/// Conda package names are always lowercase and can only contain ascii characters.
///
/// This struct explicitly does not implement [`std::fmt::Display`] because its ambiguous if that
/// would display the source or the normalized version. Simply call `as_source` or `as_normalized`
/// to make the distinction.
#[derive(Debug, Clone, Eq, DeserializeFromStr)]
pub struct PackageName {
    normalized: Option<String>,
    source: String,
}

impl PackageName {
    /// Constructs a new `PackageName` from a string without checking if the string is actually a
    /// valid or normalized conda package name. This should only be used if you are sure that the
    /// input string is valid, otherwise use the `TryFrom` implementations.
    pub fn new_unchecked<S: Into<String>>(normalized: S) -> Self {
        Self {
            normalized: None,
            source: normalized.into(),
        }
    }

    /// Returns the source representation of the package name. This is the string from which this
    /// instance was created.
    pub fn as_source(&self) -> &str {
        &self.source
    }

    /// Returns the normalized version of the package name. The normalized string is guaranteed to
    /// be a valid conda package name.
    pub fn as_normalized(&self) -> &str {
        self.normalized.as_ref().unwrap_or(&self.source)
    }

    /// Parses the package name part from a matchspec string without parsing
    /// the entire matchspec.
    ///
    /// This extracts the package name by splitting on whitespace or version
    /// constraint characters (`>`, `<`, `=`, `!`, `~`, `;`).
    ///
    /// # Examples
    ///
    /// ```
    /// use rattler_conda_types::PackageName;
    ///
    /// let name = PackageName::from_matchspec_str("pillow >=10").unwrap();
    /// assert_eq!(name.as_source(), "pillow");
    ///
    /// let name = PackageName::from_matchspec_str("numpy>=1.0,<2.0").unwrap();
    /// assert_eq!(name.as_source(), "numpy");
    /// ```
    pub fn from_matchspec_str(spec: &str) -> Result<Self, InvalidPackageNameError> {
        Self::try_from(name_from_matchspec_str(spec))
    }

    /// Parses the package name part from a matchspec string without parsing
    /// the entire matchspec. This function assumes the matchspec string is a
    /// valid matchspec.
    ///
    /// This extracts the package name by splitting on whitespace or version
    /// constraint characters (`>`, `<`, `=`, `!`, `~`, `;`). The original
    /// capitalization is preserved in the source, while the normalized version
    /// is lowercase.
    ///
    /// # Safety
    ///
    /// This function does not validate the package name. If the package name
    /// is not valid, the returned `PackageName` may not behave correctly.
    /// Use [`Self::from_matchspec_str`] for a fallible version.
    ///
    /// # Examples
    ///
    /// ```
    /// use rattler_conda_types::PackageName;
    ///
    /// let name = PackageName::from_matchspec_str_unchecked("Pillow >=10");
    /// assert_eq!(name.as_source(), "Pillow");
    /// assert_eq!(name.as_normalized(), "pillow");
    /// ```
    pub fn from_matchspec_str_unchecked(spec: &str) -> Self {
        let package_name_str = name_from_matchspec_str(spec);

        // Compute the normalized version only if there are uppercase characters
        let normalized = if package_name_str.chars().any(|c| c.is_ascii_uppercase()) {
            Some(package_name_str.to_ascii_lowercase())
        } else {
            None
        };

        Self {
            normalized,
            source: package_name_str.to_string(),
        }
    }

    /// Extracts and normalizes the package name part from a matchspec string
    /// without constructing a full `PackageName` instance.
    ///
    /// This is a lightweight alternative to [`Self::from_matchspec_str_unchecked`]
    /// that avoids allocation when the package name is already lowercase.
    /// Returns a borrowed string slice when no normalization is needed, or an
    /// owned string when the name contains uppercase characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use rattler_conda_types::PackageName;
    /// use std::borrow::Cow;
    ///
    /// // Lowercase names are borrowed (no allocation)
    /// let name = PackageName::normalized_name_from_matchspec_str("numpy>=1.0");
    /// assert!(matches!(name, Cow::Borrowed("numpy")));
    ///
    /// // Uppercase names are normalized and owned (allocation required)
    /// let name = PackageName::normalized_name_from_matchspec_str("Pillow >=10");
    /// assert!(matches!(name, Cow::Owned(_)));
    /// assert_eq!(name, "pillow");
    /// ```
    pub fn normalized_name_from_matchspec_str(spec: &str) -> Cow<'_, str> {
        let package_name_str = name_from_matchspec_str(spec);

        if package_name_str.chars().any(|c| c.is_ascii_uppercase()) {
            Cow::Owned(package_name_str.to_ascii_lowercase())
        } else {
            Cow::Borrowed(package_name_str)
        }
    }
}

/// Extracts the package name part from a matchspec string by splitting on
/// whitespace or version constraint characters (`>`, `<`, `=`, `!`, `~`, `;`).
fn name_from_matchspec_str(spec: &str) -> &str {
    spec.split_once(|c: char| c.is_whitespace() || matches!(c, '>' | '<' | '=' | '!' | '~' | ';'))
        .map_or(spec, |(name, _)| name)
}

/// An error that is returned when conversion from a string to a [`PackageName`] fails.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum InvalidPackageNameError {
    /// The package name contains illegal characters
    #[error("'{0}' is not a valid package name. Package names can only contain 0-9, a-z, A-Z, -, _, or .")]
    InvalidCharacters(String),
}

impl TryFrom<&String> for PackageName {
    type Error = InvalidPackageNameError;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        value.clone().try_into()
    }
}

impl TryFrom<CondaArchiveIdentifier> for PackageName {
    type Error = InvalidPackageNameError;

    fn try_from(value: CondaArchiveIdentifier) -> Result<Self, Self::Error> {
        value.name.try_into()
    }
}

impl TryFrom<String> for PackageName {
    type Error = InvalidPackageNameError;

    fn try_from(source: String) -> Result<Self, Self::Error> {
        // Ensure that the string only contains valid characters
        if !source
            .chars()
            .all(|c| matches!(c, 'a'..='z'|'A'..='Z'|'0'..='9'|'-'|'_'|'.'))
        {
            return Err(InvalidPackageNameError::InvalidCharacters(source));
        }

        // Convert all characters to lowercase but only if it actually contains uppercase. This way
        // we dont allocate the memory of the string if it is already lowercase.
        let normalized = if source.chars().any(|c| c.is_ascii_uppercase()) {
            Some(source.to_ascii_lowercase())
        } else {
            None
        };

        Ok(Self { normalized, source })
    }
}

impl<'a> TryFrom<&'a str> for PackageName {
    type Error = InvalidPackageNameError;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        value.to_owned().try_into()
    }
}

impl FromStr for PackageName {
    type Err = InvalidPackageNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.to_owned().try_into()
    }
}

impl Hash for PackageName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_normalized().hash(state);
    }
}

impl PartialEq for PackageName {
    fn eq(&self, other: &Self) -> bool {
        self.as_normalized().eq(other.as_normalized())
    }
}

impl PartialOrd for PackageName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageName {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_normalized().cmp(other.as_normalized())
    }
}

impl Serialize for PackageName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.as_source().serialize(serializer)
    }
}

impl<'de> DeserializeAs<'de, PackageName> for DeserializeFromStrUnchecked {
    fn deserialize_as<D>(deserializer: D) -> Result<PackageName, D::Error>
    where
        D: Deserializer<'de>,
    {
        let source = String::deserialize(deserializer)?;
        Ok(PackageName::new_unchecked(source))
    }
}

impl Borrow<str> for PackageName {
    fn borrow(&self) -> &str {
        self.as_normalized()
    }
}

#[cfg(test)]
mod test {
    use rstest::rstest;

    use super::*;

    #[test]
    fn test_package_name_basics() {
        let name1 = PackageName::try_from("cuDNN").unwrap();
        assert_eq!(name1.as_source(), "cuDNN");
        assert_eq!(name1.as_normalized(), "cudnn");

        let name2 = PackageName::try_from("cudnn").unwrap();
        assert_eq!(name2.as_source(), "cudnn");
        assert_eq!(name2.as_normalized(), "cudnn");

        assert_eq!(name1, name2);

        assert!(PackageName::try_from("invalid$").is_err());
    }

    #[rstest]
    #[case("pillow", "pillow")]
    #[case("pillow >=10", "pillow")]
    #[case("pillow>=10,<12", "pillow")]
    #[case("pillow >=10, <12", "pillow")]
    #[case("numpy", "numpy")]
    #[case("numpy>=1.0", "numpy")]
    #[case("numpy!=1.5", "numpy")]
    #[case("numpy~=1.0", "numpy")]
    // Conditional dependency syntax
    #[case("package; if __osx", "package")]
    #[case("osx-dependency; if __osx", "osx-dependency")]
    #[case("linux-dependency; if __linux", "linux-dependency")]
    #[case("numpy; if python >=3.9", "numpy")]
    #[case("pkg-a; if python>=3.8 and python<3.9.5", "pkg-a")]
    fn test_from_matchspec_str(#[case] spec: &str, #[case] expected: &str) {
        let name = PackageName::from_matchspec_str(spec).unwrap();
        assert_eq!(name.as_source(), expected);
    }

    #[rstest]
    #[case("pillow", "pillow", "pillow")]
    #[case("pillow >=10", "pillow", "pillow")]
    #[case("numpy>=1.0,<2.0", "numpy", "numpy")]
    #[case("Pillow >=10", "Pillow", "pillow")]
    fn test_from_matchspec_str_unchecked(
        #[case] spec: &str,
        #[case] expected_source: &str,
        #[case] expected_normalized: &str,
    ) {
        let name = PackageName::from_matchspec_str_unchecked(spec);
        assert_eq!(name.as_source(), expected_source);
        assert_eq!(name.as_normalized(), expected_normalized);
    }

    #[test]
    fn test_from_matchspec_str_invalid() {
        // Invalid package name characters should fail
        let result = PackageName::from_matchspec_str("invalid$package >=1.0");
        assert!(result.is_err());
    }

    #[rstest]
    #[case("numpy>=1.0", "numpy", true)]
    #[case("pillow >=10", "pillow", true)]
    #[case("Pillow >=10", "pillow", false)]
    #[case("NUMPY>=1.0,<2.0", "numpy", false)]
    #[case("package; if __osx", "package", true)]
    fn test_normalized_name_from_matchspec_str(
        #[case] spec: &str,
        #[case] expected: &str,
        #[case] is_borrowed: bool,
    ) {
        let name = PackageName::normalized_name_from_matchspec_str(spec);
        assert_eq!(&*name, expected);
        assert_eq!(matches!(name, Cow::Borrowed(_)), is_borrowed);
    }
}
