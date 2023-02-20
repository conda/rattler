//! Low-level functions to detect the LibC family and version. See [`libc_family_and_version`].

use once_cell::sync::OnceCell;
use rattler_conda_types::{ParseVersionError, Version};
use std::ffi::{FromVecWithNulError, IntoStringError};

/// Returns the LibC version and family of the current platform.
///
/// Returns an error if determining the LibC family and version resulted in an error. Returns
/// `None` if the current platform does not provide a version of LibC.
pub fn libc_family_and_version() -> Result<Option<(String, Version)>, DetectLibCError> {
    static DETECTED_LIBC_VERSION: OnceCell<Option<(String, Version)>> = OnceCell::new();
    DETECTED_LIBC_VERSION
        .get_or_try_init(try_detect_libc_version)
        .cloned()
}

#[cfg(unix)]
mod ffi {
    use std::os::raw::{c_char, c_int};

    pub const CS_GNU_LIBC_VERSION: c_int = 2;
    pub const CS_GNU_LIBPTHREAD_VERSION: c_int = 3;

    extern "C" {
        /// Get configuration dependent string variables
        pub fn confstr(name: c_int, buf: *mut c_char, length: usize) -> usize;
    }
}

/// An error that could occur when trying to detect to libc version
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum DetectLibCError {
    #[error("failed to parse libc version returned by the system")]
    ParseLibCVersion(#[from] ParseVersionError),
}

/// Returns the detected libc version used by the system.
#[cfg(unix)]
fn try_detect_libc_version() -> Result<Option<(String, Version)>, DetectLibCError> {
    use std::str::FromStr;

    // Use confstr to determine the LibC family and version
    let version = match [ffi::CS_GNU_LIBC_VERSION, ffi::CS_GNU_LIBPTHREAD_VERSION]
        .into_iter()
        .find_map(|name| confstr(name).unwrap_or(None))
    {
        Some(version) => version,
        None => return Ok(None),
    };

    // Split into family and version
    let (family, version) = match version.split_once(' ') {
        Some(split) => split,
        None => return Ok(None),
    };

    // Parse the version string
    let version = Version::from_str(version)?;

    // The family might be NPTL but thats just the name of the threading library, even though the
    // version refers to that of uClibc.
    if family == "NPTL" {
        let family = String::from("uClibc");
        tracing::warn!(
            "failed to detect non-glibc family, assuming {} ({})",
            &family,
            &version
        );
        Ok(Some((family, version)))
    } else {
        Ok(Some((family.to_owned(), version)))
    }
}

#[cfg(not(unix))]
const fn try_detect_libc_version() -> Result<Option<(String, Version)>, DetectLibCError> {
    Ok(None)
}

/// A possible error returned by `confstr`.
#[derive(Debug, thiserror::Error)]
enum ConfStrError {
    #[error("invalid string returned: {0}")]
    FromVecWithNulError(#[from] FromVecWithNulError),

    #[error("invalid utf8 string: {0}")]
    InvalidUtf8String(#[from] IntoStringError),
}

/// Safe wrapper around `confstr`
#[cfg(unix)]
fn confstr(name: std::os::raw::c_int) -> Result<Option<String>, ConfStrError> {
    let len = match unsafe { ffi::confstr(name, std::ptr::null_mut(), 0) } {
        0 => return Ok(None),
        len => len,
    };
    let mut bytes = vec![0u8; len];
    if unsafe { ffi::confstr(name, bytes.as_mut_ptr() as *mut _, bytes.len()) } == 0 {
        return Ok(None);
    }
    Ok(Some(
        std::ffi::CString::from_vec_with_nul(bytes)?.into_string()?,
    ))
}

#[cfg(test)]
mod test {
    #[test]
    #[cfg(unix)]
    pub fn doesnt_crash() {
        let version = super::try_detect_libc_version().unwrap();
        println!("LibC {:?}", version);
    }
}
