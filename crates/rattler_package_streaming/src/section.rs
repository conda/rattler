//! Package sections and package-relative path handling shared by the archive
//! readers.

use std::path::Path;

use crate::ExtractError;

/// Cap for upfront buffer allocations based on (untrusted) tar header sizes.
pub(crate) const MAX_PREALLOC: u64 = 4 * 1024 * 1024;

/// The two sections of a conda package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    /// Package metadata: everything under `info/`. Stored in the
    /// `info-*.tar.zst` member of a `.conda` archive.
    Info,
    /// The package payload. Stored in the `pkg-*.tar.zst` member of a
    /// `.conda` archive.
    Content,
}

impl Section {
    /// Returns the section a path inside the package belongs to.
    pub(crate) fn containing(path: &Path) -> Section {
        let first = path
            .components()
            .find(|c| !matches!(c, std::path::Component::CurDir));
        match first {
            Some(std::path::Component::Normal(first)) if first == "info" => Section::Info,
            _ => Section::Content,
        }
    }

    /// The file name prefix of the ZIP member holding this section.
    pub(crate) fn zip_prefix(self) -> &'static str {
        match self {
            Section::Info => "info-",
            Section::Content => "pkg-",
        }
    }
}

/// Validates a package-relative path and strips `.` components.
///
/// Package paths may not be empty, absolute, or contain parent components.
pub(crate) fn normalize(path: &Path) -> Result<std::borrow::Cow<'_, Path>, ExtractError> {
    let mut needs_normalization = false;
    let mut has_component = false;
    for component in path.components() {
        match component {
            std::path::Component::Normal(_) => has_component = true,
            std::path::Component::CurDir => needs_normalization = true,
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(ExtractError::InvalidArchivePath(path.to_owned()));
            }
        }
    }
    if !has_component {
        return Err(ExtractError::InvalidArchivePath(path.to_owned()));
    }
    if needs_normalization {
        Ok(std::borrow::Cow::Owned(
            path.components()
                .filter(|component| !matches!(component, std::path::Component::CurDir))
                .collect(),
        ))
    } else {
        Ok(std::borrow::Cow::Borrowed(path))
    }
}
