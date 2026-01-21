use super::{CondaArchiveType, DistArchiveType};
use itertools::Itertools;
use serde_with::{DeserializeFromStr, SerializeDisplay};
use std::fmt::{Display, Formatter};
use std::path::Path;
use std::str::FromStr;
use url::Url;

/// A package in the conda ecosystem consists of a `name`, `version`
/// and `build_string`. This can be used to uniquely identify a package in a
/// subdirectory.
#[derive(
    Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd, SerializeDisplay, DeserializeFromStr,
)]
pub struct ArchiveIdentifier {
    /// The name of the package.
    pub name: String,
    /// The version of the package.
    pub version: String,
    /// The build string of the package.
    pub build_string: String,
}

impl FromStr for ArchiveIdentifier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (build_string, version, name) = s
            .rsplitn(3, '-')
            .next_tuple()
            .ok_or_else(|| String::from("invalid archive identifier"))?;
        Ok(Self {
            name: name.to_owned(),
            version: version.to_owned(),
            build_string: build_string.to_owned(),
        })
    }
}

impl Display for ArchiveIdentifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}-{}", self.name, self.version, self.build_string)
    }
}

/// A conda package archive identifier contains the `name`, `version`,
/// `build_string` and `archive_type`  of a conda package archive. This
/// information can be derived from the filename of a conda package
/// archive using the [`CondaArchiveIdentifier::try_from_filename`]
/// and [`CondaArchiveIdentifier::try_from_url`] functions.
#[derive(
    Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd, SerializeDisplay, DeserializeFromStr,
)]
pub struct CondaArchiveIdentifier {
    /// The identification part
    pub identifier: ArchiveIdentifier,
    /// The archive type of the package (tar.bz2 or conda)
    pub archive_type: CondaArchiveType,
}

impl CondaArchiveIdentifier {
    /// Converts the archive identifier into a filename for a Conda package.
    pub fn to_file_name(&self) -> String {
        self.to_string()
    }

    /// Tries to convert the specified filename into an [`CondaArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_filename(filename: &str) -> Option<Self> {
        let (filename_without_ext, archive_type) = CondaArchiveType::split_str(filename)?;
        let identifier = ArchiveIdentifier::from_str(filename_without_ext).ok()?;

        Some(Self {
            identifier,
            archive_type,
        })
    }

    /// Tries to convert the specified path into an [`CondaArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_path(path: impl AsRef<Path>) -> Option<Self> {
        Self::try_from_filename(path.as_ref().file_name()?.to_str()?)
    }

    /// Tries to convert a [`Url`] into an [`CondaArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_url(url: &Url) -> Option<Self> {
        let filename = url.path_segments().and_then(Iterator::last)?;
        Self::try_from_filename(filename)
    }
}

impl Display for CondaArchiveIdentifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.identifier, self.archive_type.extension())
    }
}

impl FromStr for CondaArchiveIdentifier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        CondaArchiveIdentifier::try_from_filename(s)
            .ok_or_else(|| String::from("invalid archive identifier"))
    }
}

/// A `DistArchiveIdentifier` is similar to a [`CondaArchiveIdentifier`] but
/// represents a package in a format that is not a conda native archive. A
/// `DistArchiveIdentifier` can be used to uniquely identify an archive, but it
/// does not necessarily represent the correct filename of the archive.
#[derive(
    Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd, SerializeDisplay, DeserializeFromStr,
)]
pub struct DistArchiveIdentifier {
    /// The identification part
    pub identifier: ArchiveIdentifier,

    /// The archive type of the package
    pub archive_type: DistArchiveType,
}

impl DistArchiveIdentifier {
    /// Converts the archive identifier into a filename for a Conda package.
    pub fn to_file_name(&self) -> String {
        self.to_string()
    }

    /// Tries to convert the specified filename into an [`CondaArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_filename(filename: &str) -> Option<Self> {
        let (filename_without_ext, archive_type) = DistArchiveType::split_str(filename)?;
        let identifier = ArchiveIdentifier::from_str(filename_without_ext).ok()?;

        Some(Self {
            identifier,
            archive_type,
        })
    }

    /// Tries to convert the specified path into an [`CondaArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_path(path: impl AsRef<Path>) -> Option<Self> {
        Self::try_from_filename(path.as_ref().file_name()?.to_str()?)
    }

    /// Tries to convert a [`Url`] into an [`CondaArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_url(url: &Url) -> Option<Self> {
        let filename = url.path_segments().and_then(Iterator::last)?;
        Self::try_from_filename(filename)
    }
}

impl Display for DistArchiveIdentifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.identifier, self.archive_type.extension())
    }
}

impl FromStr for DistArchiveIdentifier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        DistArchiveIdentifier::try_from_filename(s)
            .ok_or_else(|| String::from("invalid archive identifier"))
    }
}

impl From<CondaArchiveIdentifier> for DistArchiveIdentifier {
    fn from(conda_archive_identifier: CondaArchiveIdentifier) -> Self {
        Self {
            identifier: conda_archive_identifier.identifier,
            archive_type: conda_archive_identifier.archive_type.into(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    pub fn test_from_filename() {
        assert_eq!(
            CondaArchiveIdentifier::try_from_filename(
                "ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2"
            ),
            Some(CondaArchiveIdentifier {
                identifier: ArchiveIdentifier {
                    name: String::from("ros-noetic-rosbridge-suite"),
                    version: String::from("0.11.14"),
                    build_string: String::from("py39h6fdeb60_14"),
                },
                archive_type: CondaArchiveType::TarBz2
            })
        );

        assert_eq!(
            CondaArchiveIdentifier::try_from_filename("clangdev-9.0.1-cling_v0.9_hd1e6b3a_3.conda"),
            Some(CondaArchiveIdentifier {
                identifier: ArchiveIdentifier {
                    name: String::from("clangdev"),
                    version: String::from("9.0.1"),
                    build_string: String::from("cling_v0.9_hd1e6b3a_3"),
                },
                archive_type: CondaArchiveType::Conda
            })
        );

        assert_eq!(
            CondaArchiveIdentifier::try_from_filename("clangdev-9.0.1-cling_v0.9_hd1e6b3a_3.conda")
                .unwrap()
                .to_file_name(),
            "clangdev-9.0.1-cling_v0.9_hd1e6b3a_3.conda"
        );

        // Wheel packages should return None
        assert_eq!(
            CondaArchiveIdentifier::try_from_filename("numpy-1.24.0-cp39-cp39-linux_x86_64.whl"),
            None
        );
    }
}
