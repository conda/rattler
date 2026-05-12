//! Contains models of files that are found in the `info/` directory of a package.

mod about;
mod archive_identifier;
mod archive_type;
mod entry_point;
mod files;
mod has_prefix;
mod index;
mod link;
mod no_link;
mod no_softlink;
mod package_metadata;
mod paths;
mod run_exports;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::io::Read;
use std::path::Path;

pub use {
    about::AboutJson,
    archive_identifier::{ArchiveIdentifier, CondaArchiveIdentifier, DistArchiveIdentifier},
    archive_type::{CondaArchiveType, DistArchiveType, WheelArchiveType},
    entry_point::{EntryPoint, EntryPointDottedField, ParseEntryPointError},
    files::Files,
    has_prefix::HasPrefix,
    has_prefix::HasPrefixEntry,
    index::IndexJson,
    link::{LinkJson, NoArchLinks, PythonEntryPoints},
    no_link::NoLink,
    no_softlink::NoSoftlink,
    package_metadata::PackageMetadata,
    paths::{FileMode, PathType, PathsEntry, PathsJson, PrefixPlaceholder},
    run_exports::RunExportsJson,
};

/// Errors that can occur when constructing or modifying a [`BuildString`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildStringError {
    /// The value contains a character that is not allowed by CEP26. Only ASCII
    /// letters, ASCII digits and the characters `_`, `.`, `+` are allowed.
    #[error(
        "invalid character {character:?} in build string: CEP26 only allows ASCII letters, ASCII digits and the characters '_', '.', '+'"
    )]
    InvalidCharacter {
        /// The offending character.
        character: char,
    },

    /// The value exceeds the byte length CEP26 allows for a build string.
    #[error("build string is too long: CEP26 allows at most {max} bytes, got {actual}")]
    TooLong {
        /// The actual byte length of the offending value.
        actual: usize,
        /// The maximum byte length CEP26 allows.
        max: usize,
    },
}

/// A conda build string.
///
/// `BuildString` is an opaque newtype around a `String`. The validating
/// constructor [`BuildString::new`] enforces the CEP26 character set (ASCII
/// alphanumeric plus `_`, `.`, `+`) and a 64-byte maximum length;
/// [`BuildString::new_unchecked`] accepts any value as-is.
///
/// The internal structure of the build string (prefix, hash, build number) is
/// intentionally not exposed -- callers should treat the value as a single
/// opaque token. Use [`BuildString::append`] / [`BuildString::prepend`]
/// (validating) or their `_unchecked` siblings to build composite values.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BuildString(String);

impl BuildString {
    /// Maximum byte length of a build string allowed by CEP26.
    pub const MAX_LEN: usize = 64;

    /// Construct a `BuildString` with CEP26 validation. Returns an error when
    /// `value` contains a disallowed character or exceeds the maximum length.
    pub fn new(value: impl Into<String>) -> Result<Self, BuildStringError> {
        let value = value.into();
        Self::validate(&value)?;
        Ok(Self(value))
    }

    /// Construct a `BuildString` without validation. Any input is accepted;
    /// the call is infallible.
    pub fn new_unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the build string as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns `true` if the build string is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The byte length of the build string.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Append `other` to this build string and validate the result against
    /// CEP26. The value is left unchanged if validation fails.
    pub fn append(&mut self, other: &BuildString) -> Result<(), BuildStringError> {
        let combined_len = self.0.len() + other.0.len();
        if combined_len > Self::MAX_LEN {
            return Err(BuildStringError::TooLong {
                actual: combined_len,
                max: Self::MAX_LEN,
            });
        }
        // Both halves may have been constructed unchecked, so validate them
        // independently before joining.
        Self::check_invalid_chars(&self.0)?;
        Self::check_invalid_chars(&other.0)?;
        self.0.push_str(&other.0);
        Ok(())
    }

    /// Append `other` to this build string without validation.
    pub fn append_unchecked(&mut self, other: &BuildString) {
        self.0.push_str(&other.0);
    }

    /// Prepend `other` to this build string and validate the result against
    /// CEP26. The value is left unchanged if validation fails.
    pub fn prepend(&mut self, other: &BuildString) -> Result<(), BuildStringError> {
        let combined_len = self.0.len() + other.0.len();
        if combined_len > Self::MAX_LEN {
            return Err(BuildStringError::TooLong {
                actual: combined_len,
                max: Self::MAX_LEN,
            });
        }
        Self::check_invalid_chars(&self.0)?;
        Self::check_invalid_chars(&other.0)?;
        self.0.insert_str(0, &other.0);
        Ok(())
    }

    /// Prepend `other` to this build string without validation.
    pub fn prepend_unchecked(&mut self, other: &BuildString) {
        self.0.insert_str(0, &other.0);
    }

    fn validate(value: &str) -> Result<(), BuildStringError> {
        if value.len() > Self::MAX_LEN {
            return Err(BuildStringError::TooLong {
                actual: value.len(),
                max: Self::MAX_LEN,
            });
        }
        Self::check_invalid_chars(value)
    }

    fn check_invalid_chars(value: &str) -> Result<(), BuildStringError> {
        if let Some(character) = value
            .chars()
            .find(|c| !c.is_ascii_alphanumeric() && !['_', '.', '+'].contains(c))
        {
            Err(BuildStringError::InvalidCharacter { character })
        } else {
            Ok(())
        }
    }
}

impl std::fmt::Display for BuildString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for BuildString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for BuildString {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for BuildString {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<String> for BuildString {
    fn eq(&self, other: &String) -> bool {
        &self.0 == other
    }
}

impl PartialEq<BuildString> for str {
    fn eq(&self, other: &BuildString) -> bool {
        self == other.0
    }
}

impl PartialEq<BuildString> for &str {
    fn eq(&self, other: &BuildString) -> bool {
        *self == other.0
    }
}

impl PartialEq<BuildString> for String {
    fn eq(&self, other: &BuildString) -> bool {
        self == &other.0
    }
}

impl Serialize for BuildString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for BuildString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self::new_unchecked(value))
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod build_string_tests {
    use super::*;

    #[test]
    fn new_rejects_invalid_character() {
        let err = BuildString::new("py-37_0").unwrap_err();
        assert!(matches!(
            err,
            BuildStringError::InvalidCharacter { character: '-' }
        ));
    }

    #[test]
    fn new_rejects_too_long() {
        let input = "a".repeat(65);
        let err = BuildString::new(&input).unwrap_err();
        assert!(matches!(
            err,
            BuildStringError::TooLong {
                actual: 65,
                max: 64
            }
        ));
    }

    #[test]
    fn new_accepts_max_length() {
        let input = "a".repeat(64);
        let bs = BuildString::new(&input).unwrap();
        assert_eq!(bs.len(), 64);
    }

    #[test]
    fn new_unchecked_accepts_anything() {
        let bs = BuildString::new_unchecked("not-valid!");
        assert_eq!(bs.as_str(), "not-valid!");
    }

    #[test]
    fn append_concatenates_and_validates_length() {
        let mut bs = BuildString::new("py").unwrap();
        let suffix = BuildString::new("h12345ab_0").unwrap();
        bs.append(&suffix).unwrap();
        assert_eq!(bs.as_str(), "pyh12345ab_0");
    }

    #[test]
    fn append_rejects_overflow() {
        let mut bs = BuildString::new("a".repeat(60)).unwrap();
        let suffix = BuildString::new("h12345").unwrap();
        let err = bs.append(&suffix).unwrap_err();
        assert!(matches!(err, BuildStringError::TooLong { .. }));
        assert_eq!(bs.len(), 60, "value must be unchanged after failure");
    }

    #[test]
    fn append_rejects_invalid_chars_in_other() {
        let mut bs = BuildString::new("py").unwrap();
        let suffix = BuildString::new_unchecked("-bad");
        let err = bs.append(&suffix).unwrap_err();
        assert!(matches!(
            err,
            BuildStringError::InvalidCharacter { character: '-' }
        ));
        assert_eq!(bs.as_str(), "py");
    }

    #[test]
    fn append_unchecked_concatenates_anything() {
        let mut bs = BuildString::new_unchecked("py");
        bs.append_unchecked(&BuildString::new_unchecked("-weird"));
        assert_eq!(bs.as_str(), "py-weird");
    }

    #[test]
    fn prepend_concatenates_in_order() {
        let mut bs = BuildString::new("h12345ab_0").unwrap();
        let prefix = BuildString::new("py").unwrap();
        bs.prepend(&prefix).unwrap();
        assert_eq!(bs.as_str(), "pyh12345ab_0");
    }

    #[test]
    fn prepend_unchecked_concatenates_anything() {
        let mut bs = BuildString::new_unchecked("py");
        bs.prepend_unchecked(&BuildString::new_unchecked("-weird"));
        assert_eq!(bs.as_str(), "-weirdpy");
    }

    #[test]
    fn equality_against_strings() {
        let bs = BuildString::new("pyhd8ed1ab_0").unwrap();
        assert_eq!(bs, "pyhd8ed1ab_0");
        assert_eq!(bs, String::from("pyhd8ed1ab_0"));
        assert_eq!("pyhd8ed1ab_0", bs);
        assert_ne!(bs, "py_0");
    }

    #[test]
    fn serde_roundtrip() {
        let bs = BuildString::new("py36h1af98f8_2").unwrap();
        let json = serde_json::to_string(&bs).unwrap();
        assert_eq!(json, "\"py36h1af98f8_2\"");
        let parsed: BuildString = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, bs);
    }

    #[test]
    fn deserialize_does_not_validate() {
        let parsed: BuildString = serde_json::from_str("\"not-valid!\"").unwrap();
        assert_eq!(parsed.as_str(), "not-valid!");
    }
}

/// A trait implemented for structs that represent specific files in a Conda archive.
///
/// This trait provides a standardized interface for accessing the contents of known files in a
/// Conda package, such as the `index.json` (see [`IndexJson`]) or `about.json` (see [`AboutJson`])
/// files. Structs that represent these files should implement this trait in order to ensure that
/// they can be easily accessed and manipulated by other code that expects a consistent interface.
pub trait PackageFile: Sized {
    /// Returns the path to the file within the Conda archive.
    ///
    /// The path is relative to the root of the archive and include any necessary directories.
    fn package_path() -> &'static Path;

    /// Parses the object from a string, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function parses the JSON string and returns
    /// the resulting object. If the file is not in a parse-able format, this function returns an
    /// error.
    fn from_str(str: &str) -> Result<Self, std::io::Error>;

    /// Parses the object from a byte slice, using a format appropriate for the file type.
    fn from_slice(slice: &[u8]) -> Result<Self, std::io::Error> {
        Self::from_str(&String::from_utf8_lossy(slice))
    }

    /// Parses the object from a `Read` trait object, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function reads the data from the `Read`
    /// object, parse the JSON string and return the resulting object. If the file is not in a
    /// parse-able format, this function returns an error.
    fn from_reader(mut reader: impl Read) -> Result<Self, std::io::Error> {
        let mut str = String::new();
        reader.read_to_string(&mut str)?;
        Self::from_str(&str)
    }

    /// Parses the object from a file specified by a `path`, using a format appropriate for the file
    /// type.
    ///
    /// For example, if the file is in JSON format, this function reads the data from the file at
    /// the specified path, parse the JSON string and return the resulting object. If the file is
    /// not in a parse-able format or if the file could not read, this function returns an error.
    fn from_path(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        Self::from_str(&fs_err::read_to_string(path)?)
    }

    /// Parses the object by looking up the appropriate file from the root of the specified Conda
    /// archive directory, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function reads the appropriate file from
    /// the archive, parse the JSON string and return the resulting object. If the file is not in a
    /// parse-able format or if the file could not be read, this function returns an error.
    fn from_package_directory(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        Self::from_path(path.as_ref().join(Self::package_path()))
    }
}
