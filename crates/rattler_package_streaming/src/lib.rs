//! This crate provides the ability to extract a package archive or specific parts of it.

use std::path::Path;

pub mod read;
pub mod seek;

#[cfg(feature = "reqwest")]
pub mod reqwest;

#[cfg(feature = "tokio")]
pub mod tokio;

/// An error that can occur when extracting a package archive.
#[derive(thiserror::Error, Debug)]
pub enum ExtractError {
    #[error("an io error occurred")]
    IoError(#[from] std::io::Error),

    #[error("could not create the destination path")]
    CouldNotCreateDestination(#[source] std::io::Error),

    #[error("invalid zip archive")]
    ZipError(#[from] zip::result::ZipError),

    #[error("a component is missing from the Conda archive")]
    MissingComponent,

    #[error("unsupported compression method")]
    UnsupportedCompressionMethod,

    #[cfg(feature = "reqwest")]
    #[error(transparent)]
    ReqwestError(::reqwest::Error),

    #[error("unsupported package archive format")]
    UnsupportedArchiveType,

    #[error("the task was cancelled")]
    Cancelled,
}

/// Describes the type of package archive. This can be derived from the file extension of a package.
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub enum ArchiveType {
    /// A file with the `.tar.bz2` extension.
    TarBz2,

    /// A file with the `.conda` extension.
    Conda,
}

impl ArchiveType {
    /// Tries to determine the type of a Conda archive from its filename.
    pub fn try_from(path: &Path) -> Option<ArchiveType> {
        let file_name = path.file_name()?.to_string_lossy();
        if file_name.ends_with(".conda") {
            Some(ArchiveType::Conda)
        } else if file_name.ends_with(".tar.bz2") {
            Some(ArchiveType::TarBz2)
        } else {
            None
        }
    }
}
