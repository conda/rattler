use std::cmp::Ordering;

use rattler_conda_types::{PackageRecord, RepoDataRecord};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};
use url::Url;
use crate::{UrlOrPath};

/// A locked conda dependency is just a [`PackageRecord`] with some additional
/// information on where it came from. It is very similar to a
/// [`RepoDataRecord`], but it does not explicitly contain the channel name.
#[serde_as]
#[skip_serializing_none]
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, Hash)]
pub struct CondaPackageData {
    /// The package record.
    #[serde(flatten)]
    pub package_record: PackageRecord,

    /// The location of the package. This can be a URL or a local path.
    #[serde(with = "crate::utils::serde::url_or_path", flatten)]
    pub location: UrlOrPath,

    /// The filename of the package if the last segment of the url does not
    /// refer to the filename.
    pub(crate) file_name: Option<String>,

    /// The channel of the package if this cannot be derived from the url.
    pub(crate) channel: Option<Url>,
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
        self.package_record
            .name
            .cmp(&other.package_record.name)
            .then_with(|| {
                self.package_record
                    .version
                    .cmp(&other.package_record.version)
            })
            .then_with(|| self.package_record.build.cmp(&other.package_record.build))
            .then_with(|| self.package_record.subdir.cmp(&other.package_record.subdir))
    }
}

impl CondaPackageData {
    /// Constructs a new [`CondaPackageData`] from a [`PackageRecord`] and
    /// metadata.
    ///
    /// The `file_name` is only stored if we cannot derive it from the location.
    /// The `channel` is only stored if we cannot derive it from the location.
    pub fn from_record(
        package_record: PackageRecord,
        file_name: Option<String>,
        channel: Option<Url>,
        location: UrlOrPath,
    ) -> Self {
        let derived_file_name = location.file_name();
        let derived_channel = channel_from_location(&location);

        Self {
            package_record,

            // The file_name is only stored if we cannot derive it from the location.
            file_name: if derived_file_name != file_name.as_deref() {
                file_name
            } else {
                None
            },

            // TODO: This is not entirely correct. If the `channel` is `None`, but it can be derived
            // then after serialization the channel will be `Some`.
            channel: if derived_channel != channel {
                channel
            } else {
                None
            },

            location,
        }
    }

    /// Returns the filename of the package.
    pub fn file_name(&self) -> Option<&str> {
        self.file_name
            .as_deref()
            .or_else(|| self.location.file_name())
    }

    /// Returns the channel of the package.
    pub fn channel(&self) -> Option<Url> {
        self.channel.clone().or_else(|| match &self.location {
            UrlOrPath::Url(url) => channel_from_url(url),
            _ => None,
        })
    }
}

impl From<RepoDataRecord> for CondaPackageData {
    fn from(value: RepoDataRecord) -> Self {
        let location = UrlOrPath::from(value.url).normalize().into_owned();
        Self::from_record(
            value.package_record,
            Some(value.file_name),
            Url::parse(&value.channel).ok(),
            location,
        )
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
            .channel()
            .map_or_else(String::default, |url| url.to_string());

        let file_name = value
            .file_name()
            .ok_or_else(|| ConversionError::Missing("file name".to_string()))?
            .to_string();

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

/// Channel from url, this is everything before the filename and the subdir
/// So for example: <https://conda.anaconda.org/conda-forge/> is a channel name
/// that we parse from something like: <https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda>
fn channel_from_url(url: &Url) -> Option<Url> {
    let mut result = url.clone();

    // Strip the last two path segments. We assume the first one contains the
    // file_name, and the other the subdirectory.
    result.path_segments_mut().ok()?.pop().pop();

    Some(result)
}

/// Returns the channel when deriving it from the location if possible.
fn channel_from_location(url: &UrlOrPath) -> Option<Url> {
    match url {
        UrlOrPath::Url(url) => channel_from_url(url),
        _ => None,
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_channel_from_url() {
        assert_eq!(channel_from_url(&Url::parse("https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda").unwrap()), Some(Url::parse("https://conda.anaconda.org/conda-forge").unwrap()));
        assert_eq!(
            channel_from_url(
                &Url::parse(
                    "file:///C:/Users/someone/AppData/Local/Temp/.tmpsasJ7b/noarch/foo-1-0.conda"
                )
                .unwrap()
            ),
            Some(Url::parse("file:///C:/Users/someone/AppData/Local/Temp/.tmpsasJ7b").unwrap())
        );
    }
}
