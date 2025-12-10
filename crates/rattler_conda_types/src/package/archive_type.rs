use std::path::Path;

use serde::{Deserialize, Serialize};

/// Describes the type of package archive. This can be derived from the file extension of a package.
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveType {
    /// A file with the `.tar.bz2` extension.
    TarBz2,

    /// A file with the `.conda` extension.
    Conda,

    /// A file with the `.whl` extension.
    Whl,
}

impl ArchiveType {
    /// Tries to determine the type of a Conda archive from its filename.
    pub fn try_from(path: impl AsRef<Path>) -> Option<ArchiveType> {
        Self::split_str(path.as_ref().to_string_lossy().as_ref())
            .map(|(_, archive_type)| archive_type)
    }

    /// Tries to determine the type of a Conda archive from its magic bytes.
    pub fn try_from_magic_bytes<T: AsRef<[u8]>>(bytes: T) -> Option<ArchiveType> {
        // https://en.wikipedia.org/wiki/List_of_file_signatures
        let bytes = bytes.as_ref();
        if bytes.len() >= 4 {
            match bytes[0..4] {
                // zip magic number
                [0x50, 0x4B, 0x03, 0x04] | [0x50, 0x4B, 0x05, 0x06] | [0x50, 0x4B, 0x07, 0x08] => {
                    Some(ArchiveType::Conda)
                }
                // bz2 magic number
                [0x42, 0x5a, 0x68, _] => Some(ArchiveType::TarBz2),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Returns the file extension for this archive type.
    pub fn extension(self) -> &'static str {
        match self {
            ArchiveType::TarBz2 => ".tar.bz2",
            ArchiveType::Conda => ".conda",
            ArchiveType::Whl => ".whl",
        }
    }

    /// Split the given string into its filename and archive, removing the extension.
    #[allow(clippy::manual_map)]
    pub fn split_str(path: &str) -> Option<(&str, ArchiveType)> {
        if let Some(path) = path.strip_suffix(".conda") {
            Some((path, ArchiveType::Conda))
        } else if let Some(path) = path.strip_suffix(".tar.bz2") {
            Some((path, ArchiveType::TarBz2))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_try_from() {
        assert_eq!(
            ArchiveType::Conda,
            ArchiveType::try_from("my-package.conda").unwrap()
        );
        assert_eq!(
            ArchiveType::TarBz2,
            ArchiveType::try_from("my-package.tar.bz2").unwrap()
        );
        assert_eq!(
            ArchiveType::Whl,
            ArchiveType::try_from("my-package.whl").unwrap()
        );
        assert_eq!(None, ArchiveType::try_from("my-package.zip"));
    }

    #[test]
    fn test_try_from_magic_bytes() {
        assert_eq!(
            ArchiveType::Conda,
            ArchiveType::try_from_magic_bytes([0x50, 0x4B, 0x03, 0x04, 0x01]).unwrap()
        );
        assert_eq!(
            ArchiveType::TarBz2,
            ArchiveType::try_from_magic_bytes([0x42, 0x5a, 0x68, 0x12]).unwrap()
        );
        assert_eq!(
            None,
            ArchiveType::try_from_magic_bytes([0x11, 0x11, 0x11, 0x11])
        );
        assert_eq!(None, ArchiveType::try_from_magic_bytes([]));
    }
}
