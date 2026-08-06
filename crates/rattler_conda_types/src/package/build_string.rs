use serde::{Deserialize, Serialize};

/// Errors that can occur when constructing or modifying a [`BuildString`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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

    /// The value exceeds the length CEP26 allows for a build string.
    #[error("build string is too long: CEP26 allows at most {max} characters, got {actual}")]
    TooLong {
        /// The actual length of the offending value.
        actual: usize,
        /// The maximum length CEP26 allows.
        max: usize,
    },

    /// The value is empty. CEP26 requires a build string to contain at least
    /// one character. Packages without a meaningful build string should use
    /// `"0"` instead.
    #[error("build string is empty: CEP26 requires at least one character")]
    Empty,
}

/// A conda build string.
///
/// `BuildString` is an opaque newtype around a `String`. [`BuildString::new`]
/// performs strict CEP26 validation (allowed characters, length, non-empty);
/// [`BuildString::new_unchecked`] skips validation. Packages without a
/// meaningful build string (e.g. virtual packages without a build identifier)
/// should use `"0"`. An empty value can still be encountered when reading
/// existing data (deserialization does not validate) but cannot be
/// constructed through the validating API.
///
/// The internal structure of the build string (prefix, hash, build number) is
/// intentionally not exposed -- callers should treat the value as a single
/// opaque token. Use [`BuildString::append`] / [`BuildString::prepend`] to
/// build composite values; both validate the combined result.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BuildString(String);

impl BuildString {
    /// Maximum byte length of a build string allowed by CEP26.
    pub const MAX_LEN: usize = 64;

    /// Construct a `BuildString` with CEP26 validation.
    ///
    /// Returns `Err(...)` if `value` is empty, contains a disallowed
    /// character, or exceeds the maximum length. Packages without a
    /// meaningful build string should use `"0"` instead.
    pub fn new(value: impl Into<String>) -> Result<Self, BuildStringError> {
        let value = value.into();
        Self::validate(&value)?;
        Ok(Self(value))
    }

    /// Construct a `BuildString` without validation.
    ///
    /// The resulting value may violate CEP26 if the caller passes invalid
    /// input.
    pub fn new_unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the build string as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The byte length of the build string.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if the build string is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Append `other` to this build string and validate the combined value
    /// against CEP26 (at most [`Self::MAX_LEN`] bytes, allowed characters
    /// only). The receiver is left unchanged if validation fails.
    pub fn append(&mut self, other: impl AsRef<str>) -> Result<(), BuildStringError> {
        let combined = format!("{}{}", self.0, other.as_ref());
        Self::validate(&combined)?;
        self.0 = combined;
        Ok(())
    }

    /// Prepend `other` to this build string and validate the combined value
    /// against CEP26 (at most [`Self::MAX_LEN`] bytes, allowed characters
    /// only). The receiver is left unchanged if validation fails.
    pub fn prepend(&mut self, other: impl AsRef<str>) -> Result<(), BuildStringError> {
        let combined = format!("{}{}", other.as_ref(), self.0);
        Self::validate(&combined)?;
        self.0 = combined;
        Ok(())
    }

    fn validate(value: &str) -> Result<(), BuildStringError> {
        if value.is_empty() {
            return Err(BuildStringError::Empty);
        }
        Self::check_invalid_chars(value)?;
        // Valid values are ASCII only, so bytes == characters here.
        if value.len() > Self::MAX_LEN {
            return Err(BuildStringError::TooLong {
                actual: value.len(),
                max: Self::MAX_LEN,
            });
        }
        Ok(())
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

impl From<BuildString> for String {
    fn from(value: BuildString) -> Self {
        value.0
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

#[cfg(test)]
mod tests {
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
    fn new_rejects_empty() {
        let err = BuildString::new("").unwrap_err();
        assert!(matches!(err, BuildStringError::Empty));
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
        bs.append(BuildString::new("h12345ab_0").unwrap()).unwrap();
        assert_eq!(bs.as_str(), "pyh12345ab_0");
    }

    #[test]
    fn append_accepts_str() {
        let mut bs = BuildString::new("py").unwrap();
        bs.append("h12345ab_0").unwrap();
        assert_eq!(bs.as_str(), "pyh12345ab_0");
    }

    #[test]
    fn append_empty_is_noop() {
        let mut bs = BuildString::new("py").unwrap();
        bs.append("").unwrap();
        assert_eq!(bs.as_str(), "py");
    }

    #[test]
    fn append_rejects_overflow() {
        let mut bs = BuildString::new("a".repeat(60)).unwrap();
        let err = bs.append("h12345").unwrap_err();
        assert!(matches!(err, BuildStringError::TooLong { .. }));
        assert_eq!(bs.len(), 60, "value must be unchanged after failure");
    }

    #[test]
    fn append_rejects_invalid_chars_in_other() {
        let mut bs = BuildString::new("py").unwrap();
        let err = bs.append("-bad").unwrap_err();
        assert!(matches!(
            err,
            BuildStringError::InvalidCharacter { character: '-' }
        ));
        assert_eq!(bs.as_str(), "py");
    }

    #[test]
    fn prepend_concatenates_in_order() {
        let mut bs = BuildString::new("h12345ab_0").unwrap();
        bs.prepend("py").unwrap();
        assert_eq!(bs.as_str(), "pyh12345ab_0");
    }

    #[test]
    fn prepend_empty_is_noop() {
        let mut bs = BuildString::new("py").unwrap();
        bs.prepend("").unwrap();
        assert_eq!(bs.as_str(), "py");
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
    fn into_string() {
        let bs = BuildString::new("pyhd8ed1ab_0").unwrap();
        let s: String = bs.into();
        assert_eq!(s, "pyhd8ed1ab_0");
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
