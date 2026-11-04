//! Low-level functions to detect the `LibC` family and version. See
//! [`libc_family_and_version`].

use once_cell::sync::OnceCell;
use rattler_conda_types::{ParseVersionError, Version};

/// Returns the `LibC` version and family of the current platform.
///
/// Returns an error if determining the `LibC` family and version resulted in an
/// error. Returns `None` if the current platform does not provide a version of
/// `LibC`.
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

/// Attempts to detect the glibc version by loading the `gnu_get_libc_version()`
/// function from `libc.so.6`.
///
/// Returns `Ok(Some(version))` if glibc is detected and the version can be parsed.
/// Returns `Ok(None)` if:
/// - `libc.so.6` cannot be loaded (e.g., on musl systems)
/// - The `gnu_get_libc_version` symbol is not found
/// - The function returns a null pointer
/// - The version string cannot be parsed
#[cfg(target_os = "linux")]
fn try_detect_libc_version_via_symbol() -> Result<Option<Version>, DetectLibCError> {
    unsafe {
        // Try to load libc.so.6
        let lib = match libloading::Library::new("libc.so.6") {
            Ok(lib) => lib,
            Err(e) => {
                tracing::debug!("failed to load libc.so.6: {e}");
                return Ok(None);
            }
        };

        // Try to get the gnu_get_libc_version symbol
        let gnu_get_libc_version: libloading::Symbol<
            '_,
            unsafe extern "C" fn() -> *const std::os::raw::c_char,
        > = match lib.get(b"gnu_get_libc_version") {
            Ok(sym) => sym,
            Err(e) => {
                tracing::debug!("failed to load gnu_get_libc_version symbol: {e}");
                return Ok(None);
            }
        };

        // Call the function to get the version string
        let version_ptr = gnu_get_libc_version();
        if version_ptr.is_null() {
            tracing::debug!("gnu_get_libc_version returned null");
            return Ok(None);
        }

        // Convert the C string to a Rust string
        let version_cstr = std::ffi::CStr::from_ptr(version_ptr);
        let version_str = match version_cstr.to_str() {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("failed to convert version string to UTF-8: {e}");
                return Ok(None);
            }
        };

        // Parse the version string
        let version = std::str::FromStr::from_str(version_str)?;
        tracing::debug!("detected glibc version via symbol: {version}");
        Ok(Some(version))
    }
}

/// Tries to detected the libc family and version that is available on the
/// system.
///
/// Note that this may differ from the libc version against which this binary
/// was build. For instance when compiling against musl libc the resulting
/// binary can still run on a glibc based system. For environments we are
/// interested in the libc family that is available on the *system*.
///
/// Currently this code is only able to detect glibc properly. We can add more
/// detection methods in the future.
#[cfg(unix)]
fn try_detect_libc_version() -> Result<Option<(String, Version)>, DetectLibCError> {
    #[cfg(target_os = "linux")]
    {
        // First, try to detect glibc version by loading gnu_get_libc_version() from libc.so.6
        if let Some(version) = try_detect_libc_version_via_symbol()? {
            return Ok(Some((String::from("glibc"), version)));
        }

        // Try the system utilities before probing the dynamic loader directly.
        // Minimal Linux distributions may ship glibc without ldd.
        if let Ok(output) = std::process::Command::new("ldd").arg("--version").output() {
            if output.status.success() {
                if let Some(version) =
                    parse_glibc_ldd_version(&String::from_utf8_lossy(&output.stdout))?
                {
                    return Ok(Some((String::from("glibc"), version)));
                }
            }
        }

        if let Ok(output) = std::process::Command::new("getconf")
            .arg("GNU_LIBC_VERSION")
            .output()
        {
            if output.status.success() {
                if let Some(version) =
                    parse_getconf_glibc_version(&String::from_utf8_lossy(&output.stdout))?
                {
                    return Ok(Some((String::from("glibc"), version)));
                }
            }
        }

        // glibc's dynamic loader is itself executable. Unlike ldd/getconf, it
        // is normally present even in minimal Yocto images. Use known ABI
        // paths rather than searching the filesystem or executing arbitrary files.
        for path in glibc_loader_paths() {
            if let Ok(output) = std::process::Command::new(path).arg("--version").output() {
                if output.status.success() {
                    if let Some(version) =
                        parse_glibc_loader_version(&String::from_utf8_lossy(&output.stdout))?
                    {
                        return Ok(Some((String::from("glibc"), version)));
                    }
                }
            }
        }

        Ok(None)
    }

    #[cfg(not(target_os = "linux"))]
    {
        Ok(None)
    }
}

/// Common glibc interpreter paths for the current Linux target ABI.
#[cfg(target_os = "linux")]
fn glibc_loader_paths() -> &'static [&'static str] {
    #[cfg(target_arch = "aarch64")]
    { &["/lib/ld-linux-aarch64.so.1", "/lib64/ld-linux-aarch64.so.1"] }
    #[cfg(target_arch = "x86_64")]
    { &["/lib64/ld-linux-x86-64.so.2", "/lib/ld-linux-x86-64.so.2"] }
    #[cfg(target_arch = "x86")]
    { &["/lib/ld-linux.so.2", "/lib32/ld-linux.so.2"] }
    #[cfg(target_arch = "arm")]
    { &["/lib/ld-linux-armhf.so.3", "/lib/ld-linux.so.3"] }
    #[cfg(target_arch = "riscv64")]
    { &["/lib/ld-linux-riscv64-lp64d.so.1"] }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        target_arch = "x86",
        target_arch = "arm",
        target_arch = "riscv64"
    )))]
    { &[] }
}

#[cfg(any(test, target_os = "linux"))]
fn parse_getconf_glibc_version(input: &str) -> Result<Option<Version>, DetectLibCError> {
    let Some(raw) = input.trim().strip_prefix("glibc ") else {
        return Ok(None);
    };
    Ok(Some(raw.parse()?))
}

#[cfg(any(test, target_os = "linux"))]
fn parse_glibc_loader_version(input: &str) -> Result<Option<Version>, DetectLibCError> {
    static LOADER_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"^ld\.so \(GNU libc\).*?version ([0-9]+\.[0-9]+)\.").unwrap()
    });
    let version = input
        .lines()
        .next()
        .and_then(|line| LOADER_RE.captures(line))
        .and_then(|captures| captures.get(1));
    version.map(|m| m.as_str().parse().map_err(Into::into)).transpose()
}

#[cfg(any(test, unix))]
#[allow(dead_code)] // not used on macOS
fn parse_glibc_ldd_version(input: &str) -> Result<Option<Version>, DetectLibCError> {
    static GNU_LIBC_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new("(?mi)(?:glibc|gentoo|gnu libc|solus).*?([0-9]+(:?.[0-9]+)*)$").unwrap()
    });

    if let Some(version_match) = GNU_LIBC_RE
        .captures(input)
        .and_then(|captures| captures.get(1))
        .map(|version_match| version_match.as_str())
    {
        let version = std::str::FromStr::from_str(version_match)?;
        return Ok(Some(version));
    }

    Ok(None)
}

#[cfg(not(unix))]
const fn try_detect_libc_version() -> Result<Option<(String, Version)>, DetectLibCError> {
    Ok(None)
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use super::*;

    #[test]
    #[cfg(unix)]
    pub fn doesnt_crash() {
        let version = super::try_detect_libc_version().unwrap();
        println!("LibC {version:?}");
    }

    #[test]
    pub fn test_parse_glibc_ldd_version() {
        assert_eq!(
            parse_glibc_ldd_version("ldd (Ubuntu GLIBC 2.35-0ubuntu3.1) 2.35").unwrap(),
            Some(Version::from_str("2.35").unwrap())
        );
        assert_eq!(
            parse_glibc_ldd_version("ldd (Gentoo 2.39-r9 (patchset 9)) 2.39").unwrap(),
            Some(Version::from_str("2.39").unwrap())
        );
        assert_eq!(
            parse_glibc_ldd_version("ldd (GNU libc) 2.31").unwrap(),
            Some(Version::from_str("2.31").unwrap())
        );
        assert_eq!(
            parse_glibc_ldd_version("ldd (Solus) 2.39").unwrap(),
            Some(Version::from_str("2.39").unwrap())
        );
    }
    #[test]
    fn test_getconf_glibc_version() {
        assert_eq!(
            parse_getconf_glibc_version("glibc 2.39\n").unwrap(),
            Some(Version::from_str("2.39").unwrap())
        );
        assert_eq!(parse_getconf_glibc_version("musl 1.2.5\n").unwrap(), None);
    }

    #[test]
    fn test_glibc_loader_version() {
        let output = "ld.so (GNU libc) stable release version 2.39.\nCopyright (C) 2024 Free Software Foundation, Inc.\n";
        assert_eq!(
            parse_glibc_loader_version(output).unwrap(),
            Some(Version::from_str("2.39").unwrap())
        );
        assert_eq!(parse_glibc_loader_version("musl libc\n").unwrap(), None);
    }

}
