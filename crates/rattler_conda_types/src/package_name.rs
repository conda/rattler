use crate::package::ArchiveIdentifier;
use crate::utils::serde::DeserializeFromStrUnchecked;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, DeserializeFromStr};
use std::borrow::Borrow;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use thiserror::Error;

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
}

/// An error that is returned when conversion from a string to a [`PackageName`] fails.
#[derive(Clone, Debug, Error, PartialEq)]
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

impl TryFrom<ArchiveIdentifier> for PackageName {
    type Error = InvalidPackageNameError;

    fn try_from(value: ArchiveIdentifier) -> Result<Self, Self::Error> {
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
}
