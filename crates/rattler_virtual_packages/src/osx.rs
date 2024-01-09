//! Low-level functions to detect the OSX version of the system. See [`osx_version`].

use once_cell::sync::OnceCell;
use rattler_conda_types::{ParseVersionError, Version};

/// Returns the OSX version of the current platform.
///
/// Returns an error if determining the version resulted in an error. Returns `None` if
/// the current platform is not a OSX platform.
pub fn osx_version() -> Result<Option<Version>, ParseOsxVersionError> {
    static DETECTED_OSX_VERSION: OnceCell<Option<Version>> = OnceCell::new();
    DETECTED_OSX_VERSION
        .get_or_try_init(try_detect_osx_version)
        .cloned()
}

/// Detects the current linux version.
#[cfg(target_os = "macos")]
fn try_detect_osx_version() -> Result<Option<Version>, ParseOsxVersionError> {
    use std::str::FromStr;

    let file = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist")
        .map_err(ParseOsxVersionError::FailedToReadSystemVersion)?;
    let cur = std::io::Cursor::new(file.as_bytes());
    let v =
        plist::Value::from_reader(cur).map_err(|_err| ParseOsxVersionError::CorruptedDictionary)?;

    let version = v
        .as_dictionary()
        .ok_or(ParseOsxVersionError::CorruptedDictionary)?
        .get("ProductVersion")
        .ok_or(ParseOsxVersionError::MissingProductVersion)?
        .as_string()
        .ok_or(ParseOsxVersionError::ProductVersionIsNotAString)?;

    Ok(Some(Version::from_str(version)?))
}

#[cfg(not(target_os = "macos"))]
const fn try_detect_osx_version() -> Result<Option<Version>, ParseOsxVersionError> {
    Ok(None)
}

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum ParseOsxVersionError {
    #[error("failed to read `/System/Library/CoreServices/SystemVersion.plist`")]
    FailedToReadSystemVersion(#[source] std::io::Error),

    #[error("SystemVersion.plist is not a dictionary")]
    CorruptedDictionary,

    #[error("SystemVersion.plist is missing the ProductVersion string")]
    MissingProductVersion,

    #[error("SystemVersion.plist ProductVersion value is not a string")]
    ProductVersionIsNotAString,

    #[error("invalid version")]
    InvalidVersion(#[from] ParseVersionError),
}

#[cfg(test)]
mod test {
    #[test]
    #[cfg(target_os = "macos")]
    pub fn doesnt_crash() {
        let version = super::try_detect_osx_version();
        println!("MacOS version {version:?}");
    }
}
