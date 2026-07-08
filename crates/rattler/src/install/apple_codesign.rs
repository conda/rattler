//! Code signing for macOS binaries.
//!
//! Prefix replacement modifies binary content, which invalidates any existing
//! code signature. macOS (and especially Apple Silicon, where valid
//! signatures are mandatory) then kills the binary on launch, so every
//! modified Mach-O binary must be re-signed with an ad-hoc signature.
//!
//! Signing happens in-process through the [`arwen_codesign`] crate. This is
//! orders of magnitude faster than spawning `/usr/bin/codesign` per binary
//! and also works when installing a macOS environment from a non-macOS host.
//! On macOS hosts, `/usr/bin/codesign` is kept as a fallback for anything the
//! in-process signer cannot handle.

use super::LinkFileError;
use arwen_codesign::{AdhocSignOptions, Entitlements};
use std::path::Path;

/// Controls the behavior of the [`super::link_package`] function when it encounters a binary that needs
/// to be signed on macOS (both Intel and Apple Silicon).
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

/// The signature identifier for a binary, derived from its file name (this is
/// also what `codesign --sign -` derives it from).
pub(crate) fn signing_identifier(destination_path: &Path) -> String {
    destination_path.file_name().map_or_else(
        || String::from("rattler-signed"),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Sign a binary with an ad-hoc signature, the equivalent of
/// `codesign --sign - --force --preserve-metadata=entitlements`. This is
/// required for binaries to run on macOS when their signature has been
/// invalidated by prefix replacement (modifying binary content). The function
/// preserves existing entitlements.
///
/// Signing happens in-process (thin binaries are signed in a single streaming
/// pass, fat binaries per architecture slice). If that fails and the host is
/// macOS, `/usr/bin/codesign` is used as a fallback.
pub(crate) fn codesign(destination_path: &Path) -> Result<(), LinkFileError> {
    let identifier = signing_identifier(destination_path);
    let options = AdhocSignOptions::new(&identifier).with_entitlements(Entitlements::Preserve);

    match arwen_codesign::adhoc_sign_file(destination_path, &options) {
        Ok(()) => Ok(()),
        Err(err) => {
            tracing::warn!(
                "in-process ad-hoc signing of {} failed: {err}",
                destination_path.display()
            );
            codesign_fallback(destination_path)
        }
    }
}

/// Sign a binary by invoking the `/usr/bin/codesign` tool. Only available on
/// macOS hosts.
fn codesign_fallback(destination_path: &Path) -> Result<(), LinkFileError> {
    if !cfg!(target_os = "macos") {
        return Err(LinkFileError::FailedToSignAppleBinary);
    }

    let status = std::process::Command::new("/usr/bin/codesign")
        .arg("--sign")
        // Use an ad-hoc certificate (`-`)
        .arg("-")
        // replace any existing signature
        .arg("--force")
        // preserve entitlements from the original binary
        .arg("--preserve-metadata=entitlements")
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
