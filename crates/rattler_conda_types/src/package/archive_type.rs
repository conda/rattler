use std::path::Path;

use serde::{Deserialize, Serialize};

/// Describes the type of a conda package archive.
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CondaArchiveType {
    /// A file with the `.tar.bz2` extension.
    TarBz2,

    /// A file with the `.conda` extension.
    Conda,
}

impl CondaArchiveType {
    /// Returns the file extension for this archive type.
    pub fn extension(self) -> &'static str {
        match self {
            CondaArchiveType::TarBz2 => ".tar.bz2",
            CondaArchiveType::Conda => ".conda",
        }
    }

    /// Tries to determine the type of a Conda archive from its magic bytes.
    pub fn try_from_magic_bytes<T: AsRef<[u8]>>(bytes: T) -> Option<CondaArchiveType> {
        // https://en.wikipedia.org/wiki/List_of_file_signatures
        let bytes = bytes.as_ref();
        if bytes.len() >= 4 {
            match bytes[0..4] {
                // zip magic number
                [0x50, 0x4B, 0x03, 0x04] | [0x50, 0x4B, 0x05, 0x06] | [0x50, 0x4B, 0x07, 0x08] => {
                    Some(CondaArchiveType::Conda)
                }
                // bz2 magic number
                [0x42, 0x5a, 0x68, _] => Some(CondaArchiveType::TarBz2),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Split the given string into its filename and archive type, removing the extension.
    /// Only recognizes conda package extensions.
    #[allow(clippy::manual_map)]
    pub fn split_str(path: &str) -> Option<(&str, CondaArchiveType)> {
        if let Some(path) = path.strip_suffix(".conda") {
            Some((path, CondaArchiveType::Conda))
        } else if let Some(path) = path.strip_suffix(".tar.bz2") {
            Some((path, CondaArchiveType::TarBz2))
        } else {
            None
        }
    }
}

/// Describes the type of a non-conda distributable package archive.
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistArchiveType {
    /// A Python wheel package (`.whl`)
    Whl,
}

impl DistArchiveType {
    /// Returns the file extension for this archive type.
    pub fn extension(self) -> &'static str {
        match self {
            DistArchiveType::Whl => ".whl",
        }
    }

    /// Split the given string into its filename and archive type, removing the extension.
    /// Only recognizes distribution package extensions.
    #[allow(clippy::manual_map)]
    pub fn split_str(path: &str) -> Option<(&str, DistArchiveType)> {
        if let Some(path) = path.strip_suffix(".whl") {
            Some((path, DistArchiveType::Whl))
        } else {
            None
        }
    }
}

/// Describes any type of distributable package archive (conda packages or wheels).
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", untagged)]
pub enum ArchiveType {
    /// A conda package archive (`.tar.bz2` or `.conda`)
    #[serde(rename_all = "snake_case")]
    Conda(CondaArchiveType),

    /// A distribution package archive (`.whl`, etc.)
    #[serde(rename_all = "snake_case")]
    Dist(DistArchiveType),
}

impl ArchiveType {
    /// Tries to determine the type of archive from its filename.
    pub fn try_from(path: impl AsRef<Path>) -> Option<ArchiveType> {
        Self::split_str(path.as_ref().to_string_lossy().as_ref())
            .map(|(_, archive_type)| archive_type)
    }

    /// Returns the file extension for this archive type.
    pub fn extension(self) -> &'static str {
        match self {
            ArchiveType::Conda(conda_type) => conda_type.extension(),
            ArchiveType::Dist(dist_type) => dist_type.extension(),
        }
    }

    /// Split the given string into its filename and archive type, removing the extension.
    #[allow(clippy::manual_map)]
    pub fn split_str(path: &str) -> Option<(&str, ArchiveType)> {
        if let Some((path, conda_type)) = CondaArchiveType::split_str(path) {
            Some((path, ArchiveType::Conda(conda_type)))
        } else if let Some((path, dist_type)) = DistArchiveType::split_str(path) {
            Some((path, ArchiveType::Dist(dist_type)))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_conda_archive_type() {
        assert_eq!(
            CondaArchiveType::split_str("my-package.conda"),
            Some(("my-package", CondaArchiveType::Conda))
        );
        assert_eq!(
            CondaArchiveType::split_str("my-package.tar.bz2"),
            Some(("my-package", CondaArchiveType::TarBz2))
        );
        assert_eq!(CondaArchiveType::split_str("my-package.whl"), None);
    }

    #[test]
    fn test_dist_archive_type() {
        assert_eq!(
            DistArchiveType::split_str("my-package.whl"),
            Some(("my-package", DistArchiveType::Whl))
        );
        assert_eq!(DistArchiveType::split_str("my-package.conda"), None);
        assert_eq!(DistArchiveType::split_str("my-package.tar.bz2"), None);
    }

    #[test]
    fn test_try_from() {
        assert_eq!(
            ArchiveType::Conda(CondaArchiveType::Conda),
            ArchiveType::try_from("my-package.conda").unwrap()
        );
        assert_eq!(
            ArchiveType::Conda(CondaArchiveType::TarBz2),
            ArchiveType::try_from("my-package.tar.bz2").unwrap()
        );
        assert_eq!(
            ArchiveType::Dist(DistArchiveType::Whl),
            ArchiveType::try_from("my-package.whl").unwrap()
        );
        assert_eq!(None, ArchiveType::try_from("my-package.zip"));
    }

    #[test]
    fn test_conda_try_from_magic_bytes() {
        assert_eq!(
            CondaArchiveType::Conda,
            CondaArchiveType::try_from_magic_bytes([0x50, 0x4B, 0x03, 0x04, 0x01]).unwrap()
        );
        assert_eq!(
            CondaArchiveType::TarBz2,
            CondaArchiveType::try_from_magic_bytes([0x42, 0x5a, 0x68, 0x12]).unwrap()
        );
        assert_eq!(
            None,
            CondaArchiveType::try_from_magic_bytes([0x11, 0x11, 0x11, 0x11])
        );
        assert_eq!(None, CondaArchiveType::try_from_magic_bytes([]));
    }

    #[test]
    fn test_is_conda() {
        assert!(matches!(
            ArchiveType::Conda(CondaArchiveType::Conda),
            ArchiveType::Conda(_)
        ));
        assert!(matches!(
            ArchiveType::Conda(CondaArchiveType::TarBz2),
            ArchiveType::Conda(_)
        ));
        assert!(!matches!(
            ArchiveType::Dist(DistArchiveType::Whl),
            ArchiveType::Conda(_)
        ));
    }

    #[test]
    fn test_is_wheel() {
        assert!(!matches!(
            ArchiveType::Conda(CondaArchiveType::Conda),
            ArchiveType::Dist(DistArchiveType::Whl)
        ));
        assert!(!matches!(
            ArchiveType::Conda(CondaArchiveType::TarBz2),
            ArchiveType::Dist(DistArchiveType::Whl)
        ));
        assert!(matches!(
            ArchiveType::Dist(DistArchiveType::Whl),
            ArchiveType::Dist(DistArchiveType::Whl)
        ));
    }

    #[test]
    fn test_extension() {
        assert_eq!(
            ArchiveType::Conda(CondaArchiveType::Conda).extension(),
            ".conda"
        );
        assert_eq!(
            ArchiveType::Conda(CondaArchiveType::TarBz2).extension(),
            ".tar.bz2"
        );
        assert_eq!(ArchiveType::Dist(DistArchiveType::Whl).extension(), ".whl");
    }
}
