//! Code signing for Apple Silicon binaries

use super::LinkFileError;
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

/// Sign a binary with an ad-hoc certificate on macOS.
/// This is required for binaries to run on macOS when their signature has been invalidated
/// by prefix replacement (modifying binary content). The function preserves existing entitlements.
pub(crate) fn codesign(destination_path: &Path) -> Result<(), LinkFileError> {
    use goblin_ext::{adhoc_sign_file, AdhocSignOptions, Entitlements};

    // Get identifier from filename
    let identifier = destination_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("binary");

    // Sign with ad-hoc signature, preserving existing entitlements
    let options = AdhocSignOptions::new(identifier).with_entitlements(Entitlements::Preserve);

    adhoc_sign_file(destination_path, &options).map_err(|err| {
        LinkFileError::IoError(format!("signing {}", destination_path.display()), err)
    })
}
