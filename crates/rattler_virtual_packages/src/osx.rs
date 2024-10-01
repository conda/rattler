//! Low-level functions to detect the OSX version of the system. See [`osx_version`].

use once_cell::sync::OnceCell;
use rattler_conda_types::{ParseVersionError, Version};

/// Returns the macOS version of the current platform.
///
/// Returns an error if determining the version resulted in an error. Returns `None` if
/// the current platform is not a macOS platform.
pub fn osx_version() -> Result<Option<Version>, ParseOsxVersionError> {
    static DETECTED_OSX_VERSION: OnceCell<Option<Version>> = OnceCell::new();
    DETECTED_OSX_VERSION
        .get_or_try_init(|| try_detect_macos_version(None))
        .cloned()
}


// ```xml
// <?xml version="1.0" encoding="UTF-8"?>
// <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
// <plist version="1.0">
// <dict>
// 	<key>ProductBuildVersion</key>
// 	<string>21H1320</string>
// 	<key>ProductCopyright</key>
// 	<string>1983-2024 Apple Inc.</string>
// 	<key>ProductName</key>
// 	<string>macOS</string>
// 	<key>ProductUserVisibleVersion</key>
// 	<string>12.7.6</string>
// 	<key>ProductVersion</key>
// 	<string>12.7.6</string>
// 	<key>iOSSupportVersion</key>
// 	<string>15.7</string>
// </dict>
// </plist>
// ```


/// Detects the current macOS version.
#[cfg(target_os = "macos")]
use std::path::PathBuf;

#[cfg(target_os = "macos")]
fn try_detect_macos_version(path: Option<PathBuf>) -> Result<Option<Version>, ParseOsxVersionError> {
    use std::str::FromStr;

    let path = path.unwrap_or_else(|| "/System/Library/CoreServices/SystemVersion.plist".into());

    let file = std::fs::read_to_string(path)
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
        let version = super::try_detect_macos_version(None);
        println!("MacOS version {version:?}");
    }

    #[test]
    pub fn test_old_plist() {
        let plist = r#"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>ProductBuildVersion</key>
	<string>21H1320</string>
	<key>ProductCopyright</key>
	<string>1983-2024 Apple Inc.</string>
	<key>ProductName</key>
	<string>macOS</string>
	<key>ProductUserVisibleVersion</key>
	<string>12.7.6</string>
	<key>ProductVersion</key>
	<string>12.7.6</string>
	<key>iOSSupportVersion</key>
	<string>15.7</string>
</dict>
</plist>"#;

        let path = std::env::temp_dir().join("SystemVersion.plist");
        std::fs::write(&path, plist).unwrap();

        let version = super::try_detect_macos_version(Some(path));
        println!("MacOS version {version:?}");
        assert_eq!(version.unwrap(), Some("12.7.6".parse().unwrap()));
    }
}
