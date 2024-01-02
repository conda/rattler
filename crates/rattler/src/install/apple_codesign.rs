//! Code signing for Apple Silicon binaries

use super::LinkFileError;
use std::path::Path;

/// Controls the behavior of the [`super::link_package`] function when it encounters a binary that needs
/// to be signed on macOS ARM64 (Apple Silicon).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AppleCodeSignBehavior {
    /// Do nothing (do not attempt to sign any binary)
    DoNothing,
    /// Ignore if the signing fails
    Ignore,
    /// Bubble up the error if the code signing fails (default)
    #[default]
    Fail,
}

/// Sign a binary using the `codesign` tool with an ad-hoc certificate on  macOS.
/// This is required for binaries to run on Apple Silicon.
pub(crate) fn codesign(destination_path: &Path) -> Result<(), LinkFileError> {
    let status = std::process::Command::new("/usr/bin/codesign")
        .arg("--sign")
        // Use an ad-hoc certificate (`-`)
        .arg("-")
        // replace any existing signature
        .arg("--force")
        .arg(destination_path)
        .stdout(std::process::Stdio::null()) // Suppress stdout
        .stderr(std::process::Stdio::null()) // Suppress stderr
        .status()
        .map_err(|err| LinkFileError::IoError(String::from("invoking /usr/bin/codesign"), err))?;

    if !status.success() {
        return Err(LinkFileError::FailedToSignAppleBinary);
    }

    Ok(())
}
