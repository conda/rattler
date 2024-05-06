use crate::ParseCondaLockError;
use serde::de::Error;
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::fmt::{Display, Formatter};

/// The different version of the lock-file format.
#[derive(
    Copy, Clone, PartialEq, Eq, Hash, Debug, Ord, PartialOrd, Serialize_repr, Deserialize_repr,
)]
#[repr(u16)]
pub enum FileFormatVersion {
    /// Initial version
    V1 = 1,

    /// Dependencies are now arrays instead of maps
    V2 = 2,

    /// Pip has been renamed to pypi
    V3 = 3,

    /// Complete overhaul of the lock-file with support for multienv.
    V4 = 4,

    /// pypi indexes should be part of the file now.
    V5 = 5,
}

impl Display for FileFormatVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u16)
    }
}

impl FileFormatVersion {
    /// The latest version this crate supports.
    pub const LATEST: Self = FileFormatVersion::V5;

    /// Returns true if the pypi indexes should be present in the lock file if
    /// there are pypi packages present.
    pub fn should_pypi_indexes_be_present(self) -> bool {
        self >= FileFormatVersion::V5
    }
}

impl Default for FileFormatVersion {
    fn default() -> Self {
        Self::LATEST
    }
}

impl TryFrom<u64> for FileFormatVersion {
    type Error = ParseCondaLockError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => {
                return Err(ParseCondaLockError::ParseError(serde_yaml::Error::custom(
                    "`version` field in lock file is not an integer",
                )))
            }
            1 => Self::V1,
            2 => Self::V2,
            3 => Self::V3,
            4 => Self::V4,
            5 => Self::V5,
            _ => {
                return Err(ParseCondaLockError::IncompatibleVersion {
                    lock_file_version: value,
                    max_supported_version: FileFormatVersion::LATEST,
                })
            }
        })
    }
}
