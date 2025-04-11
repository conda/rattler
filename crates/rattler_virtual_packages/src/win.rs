//! Low-level functions to detect the Windows version on the system. See
//! [`windows_version`].

use once_cell::sync::OnceCell;
use rattler_conda_types::Version;

/// Returns the Windows version of the current platform.
///
/// Returns an error if determining the Windows version resulted in an error.
/// Returns `None` if the Windows version could not be determined. Note that
/// this does not mean the current platform is not Windows.
pub fn windows_version() -> Option<Version> {
    static DETECTED_WINDOWS_VERSION: OnceCell<Option<Version>> = OnceCell::new();
    DETECTED_WINDOWS_VERSION
        .get_or_init(detect_windows_version)
        .clone()
}

#[cfg(target_os = "windows")]
fn detect_windows_version() -> Option<Version> {
    let windows_version = winver::WindowsVersion::detect()?;
    Some(
        std::str::FromStr::from_str(&windows_version.to_string())
            .expect("WindowsVersion::to_string() should always return a valid version"),
    )
}

#[cfg(not(target_os = "windows"))]
const fn detect_windows_version() -> Option<Version> {
    None
}

#[cfg(test)]
mod test {
    #[test]
    pub fn doesnt_crash() {
        let version = super::detect_windows_version();
        println!("Windows {version:?}");
    }
}
