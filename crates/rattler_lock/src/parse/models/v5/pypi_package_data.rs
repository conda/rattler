use std::borrow::Cow;

use pep440_rs::VersionSpecifiers;
use pep508_rs::PackageName;
use serde::{Deserialize, Serialize};

use crate::{parse::deserialize::PypiPackageDataRaw, PackageHashes, UrlOrPath, Verbatim};

/// This struct is similar to [`crate::parse::models::v6::PypiPackageDataModel`] but used for
/// the V5 version of the lock file format.
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, Hash)]
pub(crate) struct PypiPackageDataModel<'a> {
    pub name: Cow<'a, PackageName>,
    pub version: Cow<'a, pep440_rs::Version>,
    #[serde(with = "crate::utils::serde::url_or_path", flatten)]
    pub location: UrlOrPath,
    #[serde(default, skip_serializing_if = "Option::is_none", flatten)]
    pub hash: Cow<'a, Option<PackageHashes>>,
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub requires_dist: Cow<'a, [String]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_python: Cow<'a, Option<VersionSpecifiers>>,
    #[serde(default)]
    pub editable: bool,
}

impl<'a> From<PypiPackageDataModel<'a>> for PypiPackageDataRaw {
    fn from(value: PypiPackageDataModel<'a>) -> Self {
        Self {
            name: value.name.into_owned(),
            version: value.version.into_owned(),
            location: Verbatim::new(value.location),
            index: None,
            hash: value.hash.into_owned(),
            requires_dist: value.requires_dist.into_owned(),
            requires_python: value.requires_python.into_owned(),
        }
    }
}
