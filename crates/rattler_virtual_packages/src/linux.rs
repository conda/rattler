//! Low-level functions to dect the linux version on the system. See [`linux_version`].

use once_cell::sync::OnceCell;
use rattler_conda_types::{ParseVersionError, Version};
use std::str::FromStr;

/// Returns the Linux version of the current platform.
///
/// Returns an error if determining the Linux version resulted in an error. Returns `None` if
/// the current platform is not a Linux platform.
pub fn linux_version() -> Result<Option<Version>, ParseLinuxVersionError> {
    static DETECTED_LINUX_VERSION: OnceCell<Option<Version>> = OnceCell::new();
    DETECTED_LINUX_VERSION
        .get_or_try_init(try_detect_linux_version)
        .cloned()
}

/// Detects the current linux version.
#[cfg(target_os = "linux")]
fn try_detect_linux_version() -> Result<Option<Version>, ParseLinuxVersionError> {
    use std::{ffi::CStr, mem::MaybeUninit};

    mod ffi {
        use std::os::raw::{c_char, c_int};

        extern "C" {
            pub fn uname(buf: *mut utsname) -> c_int;
        }

        #[repr(C)]
        pub struct utsname {
            pub sysname: [c_char; 65],
            pub nodename: [c_char; 65],
            pub release: [c_char; 65],
            pub version: [c_char; 65],
            pub machine: [c_char; 65],
            pub domainname: [c_char; 65],
        }
    }

    // Run the uname function to determine platform information
    let mut info = MaybeUninit::uninit();
    if unsafe { ffi::uname(info.as_mut_ptr()) } != 0 {
        return Ok(None);
    }
    let info: ffi::utsname = unsafe { info.assume_init() };

    // Get the version string
    let release_str = unsafe { CStr::from_ptr(info.release.as_ptr()) }.to_string_lossy();

    // Parse the version string
    parse_linux_version(release_str.as_ref()).map(Some)
}

#[cfg(not(target_os = "linux"))]
const fn try_detect_linux_version() -> Result<Option<Version>, ParseLinuxVersionError> {
    Ok(None)
}

#[derive(Debug, Clone, thiserror::Error)]
#[allow(missing_docs)]
pub enum ParseLinuxVersionError {
    #[error("error parsing linux version")]
    ParseError,

    #[error("invalid version")]
    InvalidVersion(#[from] ParseVersionError),
}

/// Returns the parsed version of the linux uname string.
#[allow(dead_code)]
fn parse_linux_version(version_str: &str) -> Result<Version, ParseLinuxVersionError> {
    Ok(Version::from_str(
        extract_linux_version_part(version_str).ok_or(ParseLinuxVersionError::ParseError)?,
    )?)
}

/// Takes the first 2, 3, or 4 digits of the linux uname version.
#[allow(dead_code)]
fn extract_linux_version_part(version_str: &str) -> Option<&str> {
    use nom::character::complete::{char, digit1};
    use nom::combinator::{opt, recognize};
    use nom::sequence::{pair, tuple};
    let result: Result<_, nom::Err<nom::error::Error<_>>> = recognize(tuple((
        digit1,
        char('.'),
        digit1,
        opt(pair(char('.'), digit1)),
        opt(pair(char('.'), digit1)),
    )))(version_str);
    let (_rest, version_part) = result.ok()?;

    Some(version_part)
}

#[cfg(test)]
mod test {
    use super::extract_linux_version_part;

    #[test]
    pub fn test_extract_linux_version_part() {
        assert_eq!(
            extract_linux_version_part("5.10.102.1-microsoft-standard-WSL2"),
            Some("5.10.102.1")
        );
        assert_eq!(
            extract_linux_version_part("2.6.32-220.17.1.el6.i686"),
            Some("2.6.32")
        );
        assert_eq!(
            extract_linux_version_part("5.4.72-microsoft-standard-WSL2"),
            Some("5.4.72")
        );
        assert_eq!(
            extract_linux_version_part("4.9.43-1-MANJARO"),
            Some("4.9.43")
        );
        assert_eq!(
            extract_linux_version_part("3.16.0-31-generic"),
            Some("3.16.0")
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    pub fn doesnt_crash() {
        let version = super::try_detect_linux_version();
        println!("Linux {version:?}");
    }
}
