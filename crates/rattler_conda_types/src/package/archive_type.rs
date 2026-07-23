use std::{cmp::Ordering, path::Path};

use serde::{Deserialize, Serialize};

/// Describes the type of a conda package archive.
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CondaArchiveType {
    /// A file with the `.conda` extension.
    Conda,

    /// A file with the `.tar.bz2` extension.
    TarBz2,
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

    /// Tries to determine the type of conda archive from its filename.
    pub fn try_from(path: impl AsRef<Path>) -> Option<CondaArchiveType> {
        Self::split_str(path.as_ref().to_string_lossy().as_ref())
            .map(|(_, archive_type)| archive_type)
    }

    /// Split the given string into its filename and archive type, removing the
    /// extension. Only recognizes conda package extensions.
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

/// Describes any type of distributable package archive (conda packages or
/// wheels).
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DistArchiveType {
    /// A conda package archive (`.tar.bz2` or `.conda`)
    Conda(CondaArchiveType),

    /// A Python wheel package (`.whl`)
    Wheel(WheelArchiveType),
}

impl DistArchiveType {
    /// Compares this archive type against another returning which one is
    /// preferred over another if there are two archive types that represent the
    /// same package.
    ///
    /// The order returned by this function is that `.conda` packages are
    /// preferred over all others and that `.tar.bz2` packages are preferred
    /// over `.whl` packages.
    pub fn cmp_preference(self, other: DistArchiveType) -> std::cmp::Ordering {
        match (self, other) {
            (a, b) if a == b => Ordering::Equal,
            (DistArchiveType::Conda(CondaArchiveType::Conda), _) => Ordering::Greater,
            (_, DistArchiveType::Conda(CondaArchiveType::Conda)) => Ordering::Less,
            (DistArchiveType::Conda(CondaArchiveType::TarBz2), _) => Ordering::Greater,
            (_, DistArchiveType::Conda(CondaArchiveType::TarBz2)) => Ordering::Less,
            (DistArchiveType::Wheel(WheelArchiveType::Whl), _) => Ordering::Greater,
        }
    }

    /// Tries to determine the type of archive from its filename.
    pub fn try_from(path: impl AsRef<Path>) -> Option<DistArchiveType> {
        Self::split_str(path.as_ref().to_string_lossy().as_ref())
            .map(|(_, archive_type)| archive_type)
    }

    /// Returns the file extension for this archive type.
    pub fn extension(self) -> &'static str {
        match self {
            DistArchiveType::Conda(conda_type) => conda_type.extension(),
            DistArchiveType::Wheel(wheel_type) => wheel_type.extension(),
        }
    }

    /// Split the given string into its filename and archive type, removing the
    /// extension.
    #[allow(clippy::manual_map)]
    pub fn split_str(path: &str) -> Option<(&str, DistArchiveType)> {
        if let Some((path, conda_type)) = CondaArchiveType::split_str(path) {
            Some((path, DistArchiveType::Conda(conda_type)))
        } else if let Some((path, wheel_type)) = WheelArchiveType::split_str(path) {
            Some((path, DistArchiveType::Wheel(wheel_type)))
        } else {
            None
        }
    }
}

impl From<CondaArchiveType> for DistArchiveType {
    fn from(conda_type: CondaArchiveType) -> Self {
        DistArchiveType::Conda(conda_type)
    }
}

impl From<WheelArchiveType> for DistArchiveType {
    fn from(wheel_type: WheelArchiveType) -> Self {
        DistArchiveType::Wheel(wheel_type)
    }
}

/// Describes the type of a wheel package archive.
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WheelArchiveType {
    /// A Python wheel package (`.whl`)
    Whl,
}

impl WheelArchiveType {
    /// Returns the file extension for this archive type.
    pub fn extension(self) -> &'static str {
        match self {
            WheelArchiveType::Whl => ".whl",
        }
    }

    /// Split the given string into its filename and archive type, removing the
    /// extension. Only recognizes wheel package extensions.
    #[allow(clippy::manual_map)]
    pub fn split_str(path: &str) -> Option<(&str, WheelArchiveType)> {
        if let Some(path) = path.strip_suffix(".whl") {
            Some((path, WheelArchiveType::Whl))
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
    fn test_wheel_archive_type() {
        assert_eq!(
            WheelArchiveType::split_str("my-package.whl"),
            Some(("my-package", WheelArchiveType::Whl))
        );
        assert_eq!(WheelArchiveType::split_str("my-package.conda"), None);
        assert_eq!(WheelArchiveType::split_str("my-package.tar.bz2"), None);
    }

    #[test]
    fn test_try_from() {
        assert_eq!(
            DistArchiveType::Conda(CondaArchiveType::Conda),
            DistArchiveType::try_from("my-package.conda").unwrap()
        );
        assert_eq!(
            DistArchiveType::Conda(CondaArchiveType::TarBz2),
            DistArchiveType::try_from("my-package.tar.bz2").unwrap()
        );
        assert_eq!(
            DistArchiveType::Wheel(WheelArchiveType::Whl),
            DistArchiveType::try_from("my-package.whl").unwrap()
        );
        assert_eq!(None, DistArchiveType::try_from("my-package.zip"));
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
    fn test_extension() {
        assert_eq!(
            DistArchiveType::Conda(CondaArchiveType::Conda).extension(),
            ".conda"
        );
        assert_eq!(
            DistArchiveType::Conda(CondaArchiveType::TarBz2).extension(),
            ".tar.bz2"
        );
        assert_eq!(
            DistArchiveType::Wheel(WheelArchiveType::Whl).extension(),
            ".whl"
        );
    }

    #[test]
    fn test_serialization() {
        // Test that DistArchiveType serializes using the inner type's serialization
        // (i.e., "tar_bz2", "conda", "whl" rather than {"Conda": "tar_bz2"}, etc.)

        // CondaArchiveType variants
        let conda = DistArchiveType::Conda(CondaArchiveType::Conda);
        let conda_json = serde_json::to_string(&conda).unwrap();
        assert_eq!(conda_json, r#""conda""#);

        let tar_bz2 = DistArchiveType::Conda(CondaArchiveType::TarBz2);
        let tar_bz2_json = serde_json::to_string(&tar_bz2).unwrap();
        assert_eq!(tar_bz2_json, r#""tar_bz2""#);

        // WheelArchiveType variants
        let whl = DistArchiveType::Wheel(WheelArchiveType::Whl);
        let whl_json = serde_json::to_string(&whl).unwrap();
        assert_eq!(whl_json, r#""whl""#);
    }

    #[test]
    fn test_deserialization() {
        // Test that DistArchiveType deserializes from the inner type's format

        let conda: DistArchiveType = serde_json::from_str(r#""conda""#).unwrap();
        assert_eq!(conda, DistArchiveType::Conda(CondaArchiveType::Conda));

        let tar_bz2: DistArchiveType = serde_json::from_str(r#""tar_bz2""#).unwrap();
        assert_eq!(tar_bz2, DistArchiveType::Conda(CondaArchiveType::TarBz2));

        let whl: DistArchiveType = serde_json::from_str(r#""whl""#).unwrap();
        assert_eq!(whl, DistArchiveType::Wheel(WheelArchiveType::Whl));
    }

    #[test]
    fn test_from_implementations() {
        // Test From<CondaArchiveType> for DistArchiveType
        let conda: DistArchiveType = CondaArchiveType::Conda.into();
        assert_eq!(conda, DistArchiveType::Conda(CondaArchiveType::Conda));

        let tar_bz2: DistArchiveType = CondaArchiveType::TarBz2.into();
        assert_eq!(tar_bz2, DistArchiveType::Conda(CondaArchiveType::TarBz2));

        // Test From<WheelArchiveType> for DistArchiveType
        let whl: DistArchiveType = WheelArchiveType::Whl.into();
        assert_eq!(whl, DistArchiveType::Wheel(WheelArchiveType::Whl));
    }
}
