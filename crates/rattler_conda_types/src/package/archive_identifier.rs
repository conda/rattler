use super::ArchiveType;
use itertools::Itertools;
use std::fmt::{Display, Formatter};
use std::path::Path;
use url::Url;

/// A package archive identifier contains the `name`, `version`, `build_string` and `archive_type`
/// of a package archive. This information can derived from the filename of a package archive using
/// the [`ArchiveIdentifier::try_from_filename`] and [`ArchiveIdentifier::try_from_url`] functions.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct ArchiveIdentifier {
    /// The name of the package.
    pub name: String,
    /// The version of the package.
    pub version: String,
    /// The build string of the package.
    pub build_string: String,
    /// The archive type of the package (tar.bz2 or conda)
    pub archive_type: ArchiveType,
}

impl ArchiveIdentifier {
    /// Converts the archive identifier into a filename for a Conda package.
    pub fn to_file_name(&self) -> String {
        self.to_string()
    }

    /// Tries to convert the specified filename into an [`ArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_filename(filename: &str) -> Option<Self> {
        // Strip the suffix from the filename
        let (filename_without_ext, archive_type) = ArchiveType::split_str(filename)?;

        // Filename is in the form of: <name>-<version>-<build>
        let (build_string, version, name) = filename_without_ext.rsplitn(3, '-').next_tuple()?;

        Some(Self {
            name: name.to_owned(),
            version: version.to_owned(),
            build_string: build_string.to_owned(),
            archive_type,
        })
    }

    /// Tries to convert the specified path into an [`ArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_path(path: impl AsRef<Path>) -> Option<Self> {
        Self::try_from_filename(path.as_ref().file_name()?.to_str()?)
    }

    /// Tries to convert a [`Url`] into an [`ArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_url(url: &Url) -> Option<Self> {
        let filename = url.path_segments().and_then(Iterator::last)?;
        Self::try_from_filename(filename)
    }
}

impl Display for ArchiveIdentifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-{}-{}{}",
            &self.name,
            &self.version,
            &self.build_string,
            self.archive_type.extension()
        )
    }
}

#[cfg(test)]
mod test {
    use super::ArchiveIdentifier;
    use crate::package::ArchiveType;

    #[test]
    pub fn test_from_filename() {
        assert_eq!(
            ArchiveIdentifier::try_from_filename(
                "ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2"
            ),
            Some(ArchiveIdentifier {
                name: String::from("ros-noetic-rosbridge-suite"),
                version: String::from("0.11.14"),
                build_string: String::from("py39h6fdeb60_14"),
                archive_type: ArchiveType::TarBz2
            })
        );

        assert_eq!(
            ArchiveIdentifier::try_from_filename("clangdev-9.0.1-cling_v0.9_hd1e6b3a_3.conda"),
            Some(ArchiveIdentifier {
                name: String::from("clangdev"),
                version: String::from("9.0.1"),
                build_string: String::from("cling_v0.9_hd1e6b3a_3"),
                archive_type: ArchiveType::Conda
            })
        );

        assert_eq!(
            ArchiveIdentifier::try_from_filename("clangdev-9.0.1-cling_v0.9_hd1e6b3a_3.conda")
                .unwrap()
                .to_file_name(),
            "clangdev-9.0.1-cling_v0.9_hd1e6b3a_3.conda"
        );
    }
}
