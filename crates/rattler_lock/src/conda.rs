use std::{cmp::Ordering, hash::Hash};

use rattler_conda_types::{PackageRecord, RepoDataRecord};
use rattler_digest::Sha256Hash;
use url::Url;

use crate::UrlOrPath;

/// A locked conda dependency is just a [`PackageRecord`] with some additional
/// information on where it came from. It is very similar to a
/// [`RepoDataRecord`], but it does not explicitly contain the channel name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CondaPackageData {
    /// The package record.
    pub package_record: PackageRecord,

    /// The location of the package. This can be a URL or a local path.
    pub location: UrlOrPath,

    /// The filename of the package.
    pub file_name: Option<String>,

    /// The channel of the package if this cannot be derived from the url.
    pub channel: Option<Url>,

    /// The input hash of the package (only valid for source packages)
    pub input: Option<InputHash>,
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
        &self.package_record
    }
}

impl PartialOrd<Self> for CondaPackageData {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CondaPackageData {
    fn cmp(&self, other: &Self) -> Ordering {
        self.location
            .cmp(&other.location)
            .then_with(|| self.package_record.name.cmp(&other.package_record.name))
            .then_with(|| {
                self.package_record
                    .version
                    .cmp(&other.package_record.version)
            })
            .then_with(|| self.package_record.build.cmp(&other.package_record.build))
            .then_with(|| self.package_record.subdir.cmp(&other.package_record.subdir))
    }
}

impl From<RepoDataRecord> for CondaPackageData {
    fn from(value: RepoDataRecord) -> Self {
        let location = UrlOrPath::from(value.url).normalize().into_owned();
        Self {
            package_record: value.package_record,
            file_name: Some(value.file_name),
            channel: Url::parse(&value.channel).ok(),
            location,
            input: None,
        }
    }
}

impl TryFrom<&CondaPackageData> for RepoDataRecord {
    type Error = ConversionError;

    fn try_from(value: &CondaPackageData) -> Result<Self, Self::Error> {
        Self::try_from(value.clone())
    }
}

impl TryFrom<CondaPackageData> for RepoDataRecord {
    type Error = ConversionError;

    fn try_from(value: CondaPackageData) -> Result<Self, Self::Error> {
        // Determine the channel and file name based on the url stored in the record.
        let channel = value
            .channel
            .map_or_else(String::default, |url| url.to_string());

        let file_name = value
            .file_name
            .ok_or_else(|| ConversionError::Missing("file name".to_string()))?;

        Ok(Self {
            package_record: value.package_record,
            file_name,
            url: value.location.try_into_url()?,
            channel,
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
