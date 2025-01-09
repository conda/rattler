use crate::package::ArchiveIdentifier;
use crate::utils::serde::DeserializeFromStrUnchecked;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::DeserializeAs;
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
#[derive(Debug, Clone, Eq)]
pub enum PackageName {
    /// A package name without a feature
    WithoutFeature(PackageNameWithoutFeature),
    /// A package name with a feature
    WithFeature(PackageNameWithFeature),
}

#[derive(Debug, Clone, Eq)]
pub struct PackageNameWithoutFeature {
    normalized: Option<String>,
    source: String,
}

#[derive(Debug, Clone, Eq)]
pub struct PackageNameWithFeature {
    normalized: Option<String>,
    source: String,
    feature: String,
}

impl PackageNameWithoutFeature {
    /// Constructs a new `PackageName` from a string without checking if the string is actually a
    /// valid or normalized conda package name. This should only be used if you are sure that the
    /// input string is valid, otherwise use the `TryFrom` implementations.
    pub fn new_unchecked(normalized: impl Into<String>) -> Self {
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

    /// Adds a feature to the package name.
    pub fn with_feature(self, feature: String) -> PackageNameWithFeature {
        let normalized = if self.source.chars().any(|c| c.is_ascii_uppercase()) {
            Some(format!("{}[{}]", self.source.to_ascii_lowercase(), feature))
        } else {
            Some(format!("{source}[{feature}]", source = self.source))
        };
        PackageNameWithFeature {
            normalized,
            source: self.source,
            feature,
        }
    }
}

impl PackageNameWithFeature {
    /// Constructs a new `PackageName` from a string without checking if the string is actually a
    /// valid or normalized conda package name. This should only be used if you are sure that the
    /// input string is valid, otherwise use the `TryFrom` implementations.
    pub fn new_unchecked<S: Into<String>>(normalized: S, feature: S) -> Self {
        let source = normalized.into();
        let feature = feature.into();
        let normalized = if source.chars().any(|c| c.is_ascii_uppercase()) {
            Some(format!("{}[{}]", source.to_ascii_lowercase(), feature))
        } else {
            Some(format!("{source}[{feature}]"))
        };
        Self {
            normalized,
            source,
            feature,
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

    /// Returns the feature of the package name.
    pub fn as_feature(&self) -> &str {
        &self.feature
    }
}

/// An error that is returned when conversion from a string to a [`PackageName`] fails.
#[derive(Clone, Debug, Error, PartialEq)]
pub enum InvalidPackageNameError {
    /// The package name contains illegal characters
    #[error("'{0}' is not a valid package name. Package names can only contain 0-9, a-z, A-Z, -, _, or .")]
    InvalidCharacters(String),
}

impl PackageName {
    /// Returns the source representation of the package name. This is the string from which this
    /// instance was created.
    pub fn as_source(&self) -> &str {
        match self {
            Self::WithoutFeature(p) => p.as_source(),
            Self::WithFeature(p) => p.as_source(),
        }
    }

    /// Returns the normalized version of the package name. The normalized string is guaranteed to
    /// be a valid conda package name.
    pub fn as_normalized(&self) -> &str {
        match self {
            Self::WithoutFeature(p) => p.as_normalized(),
            Self::WithFeature(p) => p.as_normalized(),
        }
    }

    /// Constructs a new `PackageName` from a string without checking if the string is actually a
    /// valid or normalized conda package name.
    pub fn new_unchecked<S: Into<String>>(s: S) -> Self {
        Self::WithoutFeature(PackageNameWithoutFeature::new_unchecked(s))
    }

    /// Constructs a new `PackageName` with a feature.
    /// Returns an error if the package name already has a feature.
    pub fn with_feature(self, feature: String) -> Result<Self, InvalidPackageNameError> {
        match self {
            Self::WithoutFeature(p) => Ok(Self::WithFeature(p.with_feature(feature))),
            Self::WithFeature(_) => Err(InvalidPackageNameError::InvalidCharacters(
                "Cannot add a feature to a package name that already has one".to_string(),
            )),
        }
    }
}

impl TryFrom<&String> for PackageNameWithoutFeature {
    type Error = InvalidPackageNameError;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        value.clone().try_into()
    }
}

impl TryFrom<ArchiveIdentifier> for PackageNameWithoutFeature {
    type Error = InvalidPackageNameError;

    fn try_from(value: ArchiveIdentifier) -> Result<Self, Self::Error> {
        value.name.try_into()
    }
}

impl TryFrom<ArchiveIdentifier> for PackageName {
    type Error = InvalidPackageNameError;

    fn try_from(value: ArchiveIdentifier) -> Result<Self, Self::Error> {
        Ok(Self::WithoutFeature(value.name.try_into()?))
    }
}

impl TryFrom<String> for PackageNameWithoutFeature {
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

impl<'a> TryFrom<&'a str> for PackageNameWithoutFeature {
    type Error = InvalidPackageNameError;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        value.to_owned().try_into()
    }
}

impl FromStr for PackageNameWithoutFeature {
    type Err = InvalidPackageNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.to_owned().try_into()
    }
}

impl FromStr for PackageNameWithFeature {
    type Err = InvalidPackageNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((name, feature)) = s.split_once('[') {
            if let Some(feature) = feature.strip_suffix(']') {
                let name = PackageNameWithoutFeature::from_str(name)?;
                Ok(name.with_feature(feature.to_string()))
            } else {
                Err(InvalidPackageNameError::InvalidCharacters(s.to_string()))
            }
        } else {
            Err(InvalidPackageNameError::InvalidCharacters(s.to_string()))
        }
    }
}

impl Hash for PackageNameWithoutFeature {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_normalized().hash(state);
    }
}

impl Hash for PackageNameWithFeature {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_normalized().hash(state);
        self.feature.hash(state);
    }
}

impl PartialEq for PackageNameWithoutFeature {
    fn eq(&self, other: &Self) -> bool {
        self.as_normalized().eq(other.as_normalized())
    }
}

impl PartialEq for PackageNameWithFeature {
    fn eq(&self, other: &Self) -> bool {
        self.as_normalized().eq(other.as_normalized()) && self.feature.eq(&other.feature)
    }
}

impl PartialOrd for PackageNameWithoutFeature {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialOrd for PackageNameWithFeature {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageNameWithoutFeature {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_normalized().cmp(other.as_normalized())
    }
}

impl Ord for PackageNameWithFeature {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_normalized()
            .cmp(other.as_normalized())
            .then(self.feature.cmp(&other.feature))
    }
}

impl Serialize for PackageNameWithoutFeature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.as_source().serialize(serializer)
    }
}

impl Serialize for PackageNameWithFeature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        format!("{}[{}]", self.as_source(), self.feature).serialize(serializer)
    }
}

impl Serialize for PackageName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::WithoutFeature(p) => p.serialize(serializer),
            Self::WithFeature(p) => p.serialize(serializer),
        }
    }
}

impl<'de> DeserializeAs<'de, PackageNameWithoutFeature> for DeserializeFromStrUnchecked {
    fn deserialize_as<D>(deserializer: D) -> Result<PackageNameWithoutFeature, D::Error>
    where
        D: Deserializer<'de>,
    {
        let source = String::deserialize(deserializer)?;
        Ok(PackageNameWithoutFeature::new_unchecked(source))
    }
}

impl<'de> DeserializeAs<'de, PackageNameWithFeature> for DeserializeFromStrUnchecked {
    fn deserialize_as<D>(deserializer: D) -> Result<PackageNameWithFeature, D::Error>
    where
        D: Deserializer<'de>,
    {
        let source = String::deserialize(deserializer)?;
        if let Some((name, feature)) = source.split_once('[') {
            if let Some(feature) = feature.strip_suffix(']') {
                Ok(PackageNameWithFeature::new_unchecked(name, feature))
            } else {
                Err(D::Error::custom(format!(
                    "Invalid package name with feature: {source}"
                )))
            }
        } else {
            Err(D::Error::custom(format!(
                "Invalid package name with feature: {source}"
            )))
        }
    }
}

impl<'de> DeserializeAs<'de, PackageName> for DeserializeFromStrUnchecked {
    fn deserialize_as<D>(deserializer: D) -> Result<PackageName, D::Error>
    where
        D: Deserializer<'de>,
    {
        let source = String::deserialize(deserializer)?;
        if let Some((name, feature)) = source.split_once('[') {
            if let Some(feature) = feature.strip_suffix(']') {
                Ok(PackageName::WithFeature(
                    PackageNameWithFeature::new_unchecked(name, feature),
                ))
            } else {
                Ok(PackageName::WithoutFeature(
                    PackageNameWithoutFeature::new_unchecked(source),
                ))
            }
        } else {
            Ok(PackageName::WithoutFeature(
                PackageNameWithoutFeature::new_unchecked(source),
            ))
        }
    }
}

impl Borrow<str> for PackageNameWithoutFeature {
    fn borrow(&self) -> &str {
        self.as_normalized()
    }
}

impl PartialEq for PackageName {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::WithoutFeature(a), Self::WithoutFeature(b)) => a == b,
            (Self::WithFeature(a), Self::WithFeature(b)) => a == b,
            _ => false,
        }
    }
}

impl PartialOrd for PackageName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageName {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::WithoutFeature(a), Self::WithoutFeature(b)) => a.cmp(b),
            (Self::WithFeature(a), Self::WithFeature(b)) => a.cmp(b),
            (Self::WithoutFeature(_), Self::WithFeature(_)) => Ordering::Less,
            (Self::WithFeature(_), Self::WithoutFeature(_)) => Ordering::Greater,
        }
    }
}

impl Hash for PackageName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::WithoutFeature(p) => p.hash(state),
            Self::WithFeature(p) => p.hash(state),
        }
    }
}

impl FromStr for PackageName {
    type Err = InvalidPackageNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((name, feature)) = s.split_once('[') {
            if let Some(feature) = feature.strip_suffix(']') {
                let name = PackageNameWithoutFeature::from_str(name)?;
                Ok(Self::WithFeature(name.with_feature(feature.to_string())))
            } else {
                Err(InvalidPackageNameError::InvalidCharacters(s.to_string()))
            }
        } else {
            PackageNameWithoutFeature::from_str(s).map(Self::WithoutFeature)
        }
    }
}

impl TryFrom<String> for PackageName {
    type Error = InvalidPackageNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        PackageNameWithoutFeature::try_from(value).map(Self::WithoutFeature)
    }
}

impl TryFrom<&str> for PackageName {
    type Error = InvalidPackageNameError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        PackageNameWithoutFeature::try_from(value).map(Self::WithoutFeature)
    }
}

impl<'de> Deserialize<'de> for PackageName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
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

    #[test]
    fn test_package_name_with_feature() {
        let name1 = PackageName::from_str("cuDNN[feature]").unwrap();
        if let PackageName::WithFeature(p) = name1 {
            assert_eq!(p.as_source(), "cuDNN");
            assert_eq!(p.as_normalized(), "cudnn");
            assert_eq!(p.as_feature(), "feature");
        } else {
            panic!("Expected PackageName::WithFeature");
        }

        assert!(PackageName::from_str("cuDNN[feature").is_err());
        assert!(PackageName::from_str("cuDNN]feature[").is_err());
    }
}
