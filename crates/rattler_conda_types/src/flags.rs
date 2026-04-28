use std::{fmt::Display, str::FromStr};

use itertools::Itertools;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// A package variant flag.
///
/// Flags are plain strings stored in package records and used by V3 `MatchSpecs`
/// to select package variants.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Flag(Box<str>);

impl Flag {
    /// Constructs a new [`Flag`] without validating it.
    ///
    /// This is used when deserializing package metadata so invalid repodata can
    /// still be loaded and reported through explicit validation.
    pub fn new_unchecked<S: Into<String>>(value: S) -> Self {
        Self(value.into().into_boxed_str())
    }

    /// Returns this flag as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validates that this flag satisfies the record flag syntax.
    pub fn validate(&self) -> Result<(), InvalidFlagError> {
        if is_valid_record_flag(self.as_str()) {
            Ok(())
        } else {
            Err(InvalidFlagError::InvalidFlag(self.as_str().to_string()))
        }
    }
}

impl Display for Flag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Flag {
    type Err = InvalidFlagError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl TryFrom<&str> for Flag {
    type Error = InvalidFlagError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let flag = Self::new_unchecked(value);
        flag.validate()?;
        Ok(flag)
    }
}

impl TryFrom<String> for Flag {
    type Error = InvalidFlagError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let flag = Self::new_unchecked(value);
        flag.validate()?;
        Ok(flag)
    }
}

impl Serialize for Flag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.as_str().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Flag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::new_unchecked(String::deserialize(deserializer)?))
    }
}

/// An error that is returned when a package variant flag is invalid.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum InvalidFlagError {
    /// The flag does not match the required syntax.
    #[error("'{0}' is not a valid flag. Flags must match ^[a-z0-9_]+(:[a-z0-9_]+)?$")]
    InvalidFlag(String),
}

fn is_valid_record_flag(value: &str) -> bool {
    is_valid_flag(value, false)
}

pub(crate) fn is_valid_matchspec_flag(value: &str) -> bool {
    is_valid_flag(value, true)
}

fn is_valid_flag(value: &str, allow_glob: bool) -> bool {
    let mut parts = value.split(':');
    let Some(first) = parts.next() else {
        return false;
    };

    if !is_valid_flag_part(first, allow_glob) {
        return false;
    }

    match parts.at_most_one() {
        Ok(None) => true,
        Ok(Some(second)) => is_valid_flag_part(second, allow_glob),
        Err(_) => false,
    }
}

fn is_valid_flag_part(value: &str, allow_glob: bool) -> bool {
    !value.is_empty()
        && value.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || (allow_glob && c == '*')
        })
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{is_valid_matchspec_flag, is_valid_record_flag, Flag, InvalidFlagError};

    #[test]
    fn validate_record_flags() {
        for value in ["cuda", "blas:mkl", "cuda_12"] {
            assert!(is_valid_record_flag(value));
        }

        for value in ["", "CUDA", "blas:", ":mkl", "blas:mkl:extra", "blas-*"] {
            assert!(!is_valid_record_flag(value));
        }
    }

    #[test]
    fn validate_matchspec_flags() {
        for value in ["cuda", "blas:*", "*:mkl", "*", "cuda_*"] {
            assert!(is_valid_matchspec_flag(value));
        }

        for value in ["", "CUDA", "blas:", ":mkl", "blas:mkl:extra", "blas-*"] {
            assert!(!is_valid_matchspec_flag(value));
        }
    }

    #[test]
    fn flag_parse_rejects_invalid_inputs() {
        let cases = [
            ("blas:mkl:extra", "multiple colons"),
            ("", "empty string"),
            ("blas:", "trailing colon / empty second part"),
            (":mkl", "leading colon / empty first part"),
            ("CUDA", "uppercase characters"),
            ("blas-mkl", "disallowed character"),
            ("blas:*", "glob not allowed in record flag"),
            ("*", "bare glob not allowed in record flag"),
        ];

        for (input, reason) in cases {
            let err = Flag::from_str(input).expect_err(reason);
            assert_eq!(
                err,
                InvalidFlagError::InvalidFlag(input.into()),
                "case: {reason}"
            );
        }
    }
}
