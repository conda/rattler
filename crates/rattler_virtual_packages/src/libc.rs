//! Low-level functions to detect the `LibC` family and version. See [`libc_family_and_version`].

use once_cell::sync::OnceCell;
use rattler_conda_types::{ParseVersionError, Version};

/// Returns the `LibC` version and family of the current platform.
///
/// Returns an error if determining the `LibC` family and version resulted in an error. Returns
/// `None` if the current platform does not provide a version of `LibC`.
pub fn libc_family_and_version() -> Result<Option<(String, Version)>, DetectLibCError> {
    static DETECTED_LIBC_VERSION: OnceCell<Option<(String, Version)>> = OnceCell::new();
    DETECTED_LIBC_VERSION
        .get_or_try_init(try_detect_libc_version)
        .cloned()
}

/// An error that could occur when trying to detect to libc version
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum DetectLibCError {
    #[error("failed to parse libc version returned by the system")]
    ParseLibCVersion(#[from] ParseVersionError),
}

/// Tries to detected the libc family and version that is available on the system.
///
/// Note that this may differ from the libc version against which this binary was build. For
/// instance when compiling against musl libc the resulting binary can still run on a glibc based
/// system. For environments we are interested in the libc family that is available on the *system*.
///
/// Currently this code is only able to detect glibc properly. We can add more detection methods in
/// the future.
#[cfg(unix)]
fn try_detect_libc_version() -> Result<Option<(String, Version)>, DetectLibCError> {
    // GNU libc writes to stdout
    static GNU_LIBC_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new("(?mi)(?:glibc|gnu libc).*?([0-9]+(:?.[0-9]+)*)$").unwrap()
    });

    // Run `ldd --version` to detect the libc version and family on the system. `ldd` is shipped
    // with libc so if an error occured during its execution we can assume no libc is available on
    // the system.
    let output = match std::process::Command::new("ldd").arg("--version").output() {
        Err(e) => {
            tracing::info!(
                "failed to execute `ldd --version`: {e}. Assuming libc is not available."
            );
            return Ok(None);
        }
        Ok(output) => output,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);

    if let Some(version_match) = GNU_LIBC_RE
        .captures(&stdout)
        .and_then(|captures| captures.get(1))
        .map(|version_match| version_match.as_str())
    {
        let version = std::str::FromStr::from_str(version_match)?;
        return Ok(Some((String::from("glibc"), version)));
    }

    Ok(None)
}

#[cfg(not(unix))]
const fn try_detect_libc_version() -> Result<Option<(String, Version)>, DetectLibCError> {
    Ok(None)
}

#[cfg(test)]
mod test {
    #[test]
    #[cfg(unix)]
    pub fn doesnt_crash() {
        let version = super::try_detect_libc_version().unwrap();
        println!("LibC {version:?}");
    }
}
