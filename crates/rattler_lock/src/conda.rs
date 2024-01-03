use crate::{
    LockedDependency, LockedDependencyKind, PackageHashes,
    PackageHashes::{Md5, Md5Sha256, Sha256},
};
use rattler_conda_types::{
    InvalidPackageNameError, NoArchType, PackageName, PackageRecord, PackageUrl,
    ParseMatchSpecError, ParseVersionError, RepoDataRecord,
};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, OneOrMany};
use url::Url;

/// A locked conda dependency. This represents a [`rattler_conda_types::RepoDataRecord`].
#[serde_as]
#[skip_serializing_none]
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct CondaLockedDependency {
    /// What are its own dependencies mapping name to version constraint
    #[serde(default)]
    #[serde_as(deserialize_as = "crate::utils::serde::MatchSpecMapOrVec")]
    pub dependencies: Vec<String>,
    /// URL to find it at
    pub url: Url,
    /// Hashes of the package
    pub hash: PackageHashes,
    /// ???
    pub source: Option<Url>,

    /// Build string
    pub build: Option<String>,

    /// Experimental: architecture field
    pub arch: Option<String>,

    /// Experimental: the subdir where the package can be found
    pub subdir: Option<String>,

    /// Experimental: conda build number of the package
    pub build_number: Option<u64>,

    /// Experimental: see: [Constrains](rattler_conda_types::PackageRecord::constrains)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constrains: Vec<String>,

    /// Experimental: see: [Features](rattler_conda_types::PackageRecord::features)
    pub features: Option<String>,

    /// Experimental: see: [Track features](rattler_conda_types::PackageRecord::track_features)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "OneOrMany<_>")]
    pub track_features: Vec<String>,

    /// Experimental: the specific license of the package
    pub license: Option<String>,

    /// Experimental: the license family of the package
    pub license_family: Option<String>,

    /// Experimental: If this package is independent of architecture this field specifies in what way. See
    /// [`NoArchType`] for more information.
    #[serde(skip_serializing_if = "NoArchType::is_none")]
    pub noarch: NoArchType,

    /// Experimental: The size of the package archive in bytes
    pub size: Option<u64>,

    /// Experimental: The date this entry was created.
    #[serde_as(as = "Option<crate::utils::serde::Timestamp>")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Experimental: Defines that the package is an alias for a package from another ecosystem.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub purls: Vec<PackageUrl>,
}

impl TryFrom<&LockedDependency> for RepoDataRecord {
    type Error = ConversionError;

    fn try_from(value: &LockedDependency) -> Result<Self, Self::Error> {
        Self::try_from(value.clone())
    }
}

impl TryFrom<LockedDependency> for RepoDataRecord {
    type Error = ConversionError;

    fn try_from(value: LockedDependency) -> Result<Self, Self::Error> {
        let LockedDependency {
            name,
            version,
            kind: specific,
            platform,
            ..
        } = value;
        let LockedDependencyKind::Conda(value) = specific else {
            return Err(ConversionError::NotACondaRecord);
        };

        let version = version.parse()?;
        let md5 = match value.hash {
            Md5(md5) | Md5Sha256(md5, _) => Some(md5),
            Sha256(_) => None,
        };
        let sha256 = match value.hash {
            Sha256(sha256) | Md5Sha256(_, sha256) => Some(sha256),
            Md5(_) => None,
        };
        let channel = channel_from_url(&value.url)
            .ok_or_else(|| ConversionError::Missing("channel in url".to_string()))?
            .to_string();
        let file_name = file_name_from_url(&value.url)
            .ok_or_else(|| ConversionError::Missing("filename in url".to_string()))?
            .to_owned();

        let build = value
            .build
            .ok_or_else(|| ConversionError::Missing("build".to_string()))?;

        Ok(Self {
            package_record: PackageRecord {
                arch: value.arch,
                build,
                build_number: value.build_number.unwrap_or(0),
                constrains: value.constrains,
                depends: value.dependencies,
                features: value.features,
                legacy_bz2_md5: None,
                legacy_bz2_size: None,
                license: value.license,
                license_family: value.license_family,
                md5,
                name: PackageName::try_from(name)?,
                noarch: value.noarch,
                platform: platform.only_platform().map(ToString::to_string),
                sha256,
                size: value.size,
                subdir: value.subdir.unwrap_or(platform.to_string()),
                timestamp: value.timestamp,
                track_features: value.track_features,
                version,
                purls: value.purls,
            },
            file_name,
            url: value.url,
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
    #[error("the record is not a conda record")]
    NotACondaRecord,
    /// Parse error when converting [`MatchSpec`]
    #[error(transparent)]
    MatchSpecConversion(#[from] ParseMatchSpecError),
    /// Error when version parsing fails
    #[error(transparent)]
    VersionConversion(#[from] ParseVersionError),
    #[error(transparent)]
    InvalidCondaPackageName(#[from] InvalidPackageNameError),
}

/// Package filename from the url
fn file_name_from_url(url: &Url) -> Option<&str> {
    let path = url.path_segments()?;
    path.last()
}

/// Channel from url, this is everything before the filename and the subdir
/// So for example: <https://conda.anaconda.org/conda-forge/> is a channel name
/// that we parse from something like: <https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda>
fn channel_from_url(url: &Url) -> Option<Url> {
    let mut result = url.clone();

    // Strip the last two path segments. We assume the first one contains the file_name, and the
    // other the subdirectory.
    result.path_segments_mut().ok()?.pop().pop();

    Some(result)
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

    #[test]
    fn test_file_name_from_url() {
        assert_eq!(file_name_from_url(&Url::parse("https://conda.anaconda.org/conda-forge/osx-64/python-3.11.0-h4150a38_1_cpython.conda").unwrap()), Some("python-3.11.0-h4150a38_1_cpython.conda"));
        assert_eq!(
            file_name_from_url(
                &Url::parse(
                    "file:///C:/Users/someone/AppData/Local/Temp/.tmpsasJ7b/noarch/foo-1-0.conda"
                )
                .unwrap()
            ),
            Some("foo-1-0.conda")
        );
    }
}
