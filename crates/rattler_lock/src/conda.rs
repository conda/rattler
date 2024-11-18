use std::{cmp::Ordering, hash::Hash};

use rattler_conda_types::{ChannelUrl, PackageRecord, RepoDataRecord};
use rattler_digest::Sha256Hash;
use url::Url;

use crate::UrlOrPath;

/// A locked conda dependency can be either a binary package or a source
/// package.
///
/// A binary package is a package that is already built and can be installed
/// directly.
///
/// A source package is a package that needs to be built before it can
/// be installed. Although the source package is not built, it does contain
/// dependency information through the [`PackageRecord`] struct.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CondaPackageData {
    /// A binary package. A binary package is identified by looking at the
    /// location or filename of the package file and seeing if it represents a
    /// valid binary package name.
    Binary(CondaBinaryData),

    /// A source package.
    Source(CondaSourceData),
}

impl CondaPackageData {
    /// Returns the location of the package.
    pub fn location(&self) -> &UrlOrPath {
        match self {
            Self::Binary(data) => &data.location,
            Self::Source(data) => &data.location,
        }
    }

    /// Returns the dependency information of the package.
    pub fn record(&self) -> &PackageRecord {
        match self {
            CondaPackageData::Binary(data) => &data.package_record,
            CondaPackageData::Source(data) => &data.package_record,
        }
    }

    /// Returns a reference to the binary representation of this instance if it
    /// exists.
    pub fn as_binary(&self) -> Option<&CondaBinaryData> {
        match self {
            Self::Binary(data) => Some(data),
            Self::Source(_) => None,
        }
    }

    /// Returns a reference to the source representation of this instance if it
    /// exists.
    pub fn as_source(&self) -> Option<&CondaSourceData> {
        match self {
            Self::Binary(_) => None,
            Self::Source(data) => Some(data),
        }
    }

    /// Returns the binary representation of this instance if it exists.
    pub fn into_binary(self) -> Option<CondaBinaryData> {
        match self {
            Self::Binary(data) => Some(data),
            Self::Source(_) => None,
        }
    }

    /// Returns the source representation of this instance if it exists.
    pub fn into_source(self) -> Option<CondaSourceData> {
        match self {
            Self::Binary(_) => None,
            Self::Source(data) => Some(data),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CondaBinaryData {
    /// The package record.
    pub package_record: PackageRecord,

    /// The location of the package. This can be a URL or a local path.
    pub location: UrlOrPath,

    /// The filename of the package.
    pub file_name: String,

    /// The channel of the package.
    pub channel: Option<ChannelUrl>,
}

impl From<CondaBinaryData> for CondaPackageData {
    fn from(value: CondaBinaryData) -> Self {
        Self::Binary(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CondaSourceData {
    /// The package record.
    pub package_record: PackageRecord,

    /// The location of the package. This can be a URL or a local path.
    pub location: UrlOrPath,

    /// The input hash of the package
    pub input: Option<InputHash>,
}

impl From<CondaSourceData> for CondaPackageData {
    fn from(value: CondaSourceData) -> Self {
        Self::Source(value)
    }
}

/// A record of input files that were used to define the metadata of the
/// package.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InputHash {
    /// The hash of all input files combined.
    pub hash: Sha256Hash,

    /// The globs that were used to define the input files.
    pub globs: Vec<String>,
}

impl AsRef<PackageRecord> for CondaPackageData {
    fn as_ref(&self) -> &PackageRecord {
        match self {
            Self::Binary(data) => &data.package_record,
            Self::Source(data) => &data.package_record,
        }
    }
}

impl PartialOrd<Self> for CondaPackageData {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CondaPackageData {
    fn cmp(&self, other: &Self) -> Ordering {
        let pkg_a: &PackageRecord = self.as_ref();
        let pkg_b: &PackageRecord = other.as_ref();
        let location_a = self.location();
        let location_b = other.location();

        location_a
            .cmp(location_b)
            .then_with(|| pkg_a.name.cmp(&pkg_b.name))
            .then_with(|| pkg_a.version.cmp(&pkg_b.version))
            .then_with(|| pkg_a.build.cmp(&pkg_b.build))
            .then_with(|| pkg_a.subdir.cmp(&pkg_b.subdir))
    }
}

impl From<RepoDataRecord> for CondaPackageData {
    fn from(value: RepoDataRecord) -> Self {
        let location = UrlOrPath::from(value.url).normalize().into_owned();
        Self::Binary(CondaBinaryData {
            package_record: value.package_record,
            file_name: value.file_name,
            channel: value
                .channel
                .and_then(|channel| Url::parse(&channel).ok())
                .map(Into::into),
            location,
        })
    }
}

impl TryFrom<&CondaBinaryData> for RepoDataRecord {
    type Error = ConversionError;

    fn try_from(value: &CondaBinaryData) -> Result<Self, Self::Error> {
        Self::try_from(value.clone())
    }
}

impl TryFrom<CondaBinaryData> for RepoDataRecord {
    type Error = ConversionError;

    fn try_from(value: CondaBinaryData) -> Result<Self, Self::Error> {
        Ok(Self {
            package_record: value.package_record,
            file_name: value.file_name,
            url: value.location.try_into_url()?,
            channel: value.channel.map(|channel| channel.to_string()),
        })
    }
}

/// Error used when converting from `repo_data` module to conda lock module
#[derive(thiserror::Error, Debug)]
pub enum ConversionError {
    /// This field was found missing during the conversion
    #[error("missing field/fields '{0}'")]
    Missing(String),

    /// The location of the conda package cannot be converted to a URL
    #[error(transparent)]
    LocationToUrlConversionError(#[from] file_url::FileURLParseError),
}
