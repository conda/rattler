use super::ArchiveType;
use itertools::Itertools;
use std::fmt::{Display, Formatter};
use std::path::Path;
use url::Url;

/// A package archive identifier contains the `name`, `version`, `build_string` and `archive_type`
/// of a package archive. This information can be derived from the filename of a package archive using
/// the [`ArchiveIdentifier::try_from_filename`] and [`ArchiveIdentifier::try_from_url`] functions.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct CondaArchiveIdentifier {
    /// The name of the package.
    pub name: String,
    /// The version of the package.
    pub version: String,
    /// The build string of the package.
    pub build_string: String,
    /// The archive type of the package (tar.bz2, conda or whl)
    pub archive_type: ArchiveType,
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
        // Strip the suffix from the filename
        let (filename_without_ext, archive_type) = ArchiveType::split_str(filename)?;

        let version;
        let name;
        let build_string;

        if archive_type == ArchiveType::Whl {
            // Filename is in the form of: {distribution}-{version}(-{build tag})?-{python tag}-{abi tag}-{platform tag}
            // Build string is intentionally ignored
            (name, version) = filename_without_ext.split('-').next_tuple()?;
            build_string = "0";
        } else {
            // Filename is in the form of: <name>-<version>-<build>
            (build_string, version, name) = filename_without_ext.rsplitn(3, '-').next_tuple()?;
        }

        Some(Self {
            name: name.to_owned(),
            version: version.to_owned(),
            build_string: build_string.to_owned(),
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

/// A package archive identifier contains the `name`, `version`, `build_string` and `archive_type`
/// of a package archive. This information can be derived from the filename of a package archive using
/// the [`WheelArchiveIdentifier::try_from_filename`] and [`WheelArchiveIdentifier::try_from_url`] functions.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct WheelArchiveIdentifier {
    /// The name of the package.
    pub name: String,
    /// The version of the package.
    pub version: String,
    /// The build string of the package.
    pub build_string: String,
    /// The distribution of the package.
    pub distribution: String,
    /// The build tag of the package
    pub build_tag: String,
    /// The python tag of the package
    pub python_tag: String,
    /// The ABI tag of the package
    pub abi_tag: String,
    /// The platform tag of the package
    pub platform_tag: String,
    /// The archive type of the package (tar.bz2, conda or whl)
    pub archive_type: ArchiveType,
}

impl WheelArchiveIdentifier {
    /// Converts the archive identifier into a filename for a Wheel package.
    pub fn to_file_name(&self) -> String {
        self.to_string()
    }

    /// Tries to convert the specified filename into an [`WheelArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_filename(filename: &str) -> Option<Self> {
        // Strip the suffix from the filename
        let (filename_without_ext, archive_type) = ArchiveType::split_str(filename)?;

        let name;
        let version;
        let build_string = "py0";
        let distribution;
        let mut build_tag = "";
        let python_tag;
        let abi_tag;
        let platform_tag;

        let parts = filename_without_ext.split('-').collect_vec();

        // If a build tag is present
        if parts.len() > 5 {
            name = parts[0];
            distribution = parts[0];
            version = parts[1];
            build_tag = parts[2];
            python_tag = parts[3];
            abi_tag = parts[4];
            platform_tag = parts[5];
        } else {
            name = parts[0];
            distribution = parts[0];
            version = parts[1];
            python_tag = parts[2];
            abi_tag = parts[3];
            platform_tag = parts[4];
        }


        Some(Self {
            name: name.to_owned(),
            version: version.to_owned(),
            build_string: build_string.to_owned(),
            distribution: distribution.to_owned(),
            build_tag: build_tag.to_owned(),
            python_tag: python_tag.to_owned(),
            abi_tag: abi_tag.to_owned(),
            platform_tag: platform_tag.to_owned(),
            archive_type,
        })
    }

    /// Tries to convert the specified path into an [`WheelArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_path(path: impl AsRef<Path>) -> Option<Self> {
        Self::try_from_filename(path.as_ref().file_name()?.to_str()?)
    }

    /// Tries to convert a [`Url`] into an [`WheelArchiveIdentifier`].
    ///
    /// Since Conda archives have a format for file names (see [`Self::to_file_name`]) we can
    /// reverse engineer the information that went into it. This function tries to do just that.
    pub fn try_from_url(url: &Url) -> Option<Self> {
        let filename = url.path_segments().and_then(Iterator::last)?;
        Self::try_from_filename(filename)
    }
}

impl Display for WheelArchiveIdentifier {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // Filename is in the form of: {distribution}-{version}(-{build tag})?-{python tag}-{abi tag}-{platform tag}
        if self.build_tag.is_empty() {
            write!(
                f,
                "{}-{}-{}-{}-{}{}",
                &self.distribution,
                &self.version,
                &self.python_tag,
                &self.abi_tag,
                &self.platform_tag,
                &self.archive_type.extension()
            )
        } else {
            write!(
                f,
                "{}-{}-{}-{}-{}-{}{}",
                &self.distribution,
                &self.version,
                &self.build_tag,
                &self.python_tag,
                &self.abi_tag,
                &self.platform_tag,
                &self.archive_type.extension()
            )
        }
    }
}
#[cfg(test)]
mod test {
    use super::CondaArchiveIdentifier;
    use super::WheelArchiveIdentifier;
    use crate::package::ArchiveType;

    #[test]
    pub fn test_from_filename() {
        assert_eq!(
            CondaArchiveIdentifier::try_from_filename(
                "ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2"
            ),
            Some(CondaArchiveIdentifier {
                name: String::from("ros-noetic-rosbridge-suite"),
                version: String::from("0.11.14"),
                build_string: String::from("py39h6fdeb60_14"),
                archive_type: ArchiveType::TarBz2
            })
        );

        assert_eq!(
            CondaArchiveIdentifier::try_from_filename("clangdev-9.0.1-cling_v0.9_hd1e6b3a_3.conda"),
            Some(CondaArchiveIdentifier {
                name: String::from("clangdev"),
                version: String::from("9.0.1"),
                build_string: String::from("cling_v0.9_hd1e6b3a_3"),
                archive_type: ArchiveType::Conda
            })
        );

        // Pure Python wheel
        assert_eq!(
            WheelArchiveIdentifier::try_from_filename("flask-3.1.1-py3-none-any.whl"),
            Some(WheelArchiveIdentifier {
                name: String::from("flask"),
                version: String::from("3.1.1"),
                build_string: String::from("py0"),
                distribution: String::from("flask"),
                build_tag: String::from(""),
                python_tag: String::from("py3"),
                abi_tag: String::from("none"),
                platform_tag: String::from("any"),
                archive_type: ArchiveType::Whl
            })
        );

        // Platform specific wheel
        assert_eq!(
            WheelArchiveIdentifier::try_from_filename(
                "numpy-2.4.1-cp314-cp314-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl"
            ),
            Some(WheelArchiveIdentifier {
                name: String::from("numpy"),
                version: String::from("2.4.1"),
                build_string: String::from("py0"),
                distribution: String::from("numpy"),
                build_tag: String::from(""),
                python_tag: String::from("cp314"),
                abi_tag: String::from("cp314"),
                platform_tag: String::from("manylinux_2_27_x86_64.manylinux_2_28_x86_64"),
                archive_type: ArchiveType::Whl
            })
        );

        // Wheel with build tag
        assert_eq!(
            WheelArchiveIdentifier::try_from_filename(
                "pyproj-3.4.0-1-cp39-cp39-win_amd64.whl"
            ),
            Some(WheelArchiveIdentifier {
                name: String::from("pyproj"),
                version: String::from("3.4.0"),
                build_string: String::from("py0"),
                distribution: String::from("pyproj"),
                build_tag: String::from("1"),
                python_tag: String::from("cp39"),
                abi_tag: String::from("cp39"),
                platform_tag: String::from("win_amd64"),
                archive_type: ArchiveType::Whl
            })
        );

        // Filename reconstruction
        assert_eq!(
            CondaArchiveIdentifier::try_from_filename("clangdev-9.0.1-cling_v0.9_hd1e6b3a_3.conda")
                .unwrap()
                .to_file_name(),
            "clangdev-9.0.1-cling_v0.9_hd1e6b3a_3.conda"
        );

        // Without build_tag
        assert_eq!(
            WheelArchiveIdentifier::try_from_filename("flask-3.1.1-py3-none-any.whl")
                .unwrap()
                .to_file_name(),
            "flask-3.1.1-py3-none-any.whl"
        );

        // With build_tag
        assert_eq!(
            WheelArchiveIdentifier::try_from_filename("pyproj-3.4.0-1-cp39-cp39-win_amd64.whl")
                .unwrap()
                .to_file_name(),
            "pyproj-3.4.0-1-cp39-cp39-win_amd64.whl"
        );
    }
}
