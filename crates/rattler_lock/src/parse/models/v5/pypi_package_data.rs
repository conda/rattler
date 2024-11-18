use std::borrow::Cow;

use pep440_rs::VersionSpecifiers;
use pep508_rs::{PackageName, Requirement};
use serde::{Deserialize, Serialize};

use crate::{PackageHashes, PypiPackageData, UrlOrPath};

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_dist: Cow<'a, Vec<Requirement>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_python: Cow<'a, Option<VersionSpecifiers>>,
    #[serde(default, skip_serializing_if = "should_skip_serializing_editable")]
    pub editable: bool,
}

/// Used in `skip_serializing_if` to skip serializing the `editable` field if it
/// is `false`.
fn should_skip_serializing_editable(editable: &bool) -> bool {
    !*editable
}

impl<'a> From<PypiPackageDataModel<'a>> for PypiPackageData {
    fn from(value: PypiPackageDataModel<'a>) -> Self {
        Self {
            name: value.name.into_owned(),
            version: value.version.into_owned(),
            location: value.location,
            hash: value.hash.into_owned(),
            requires_dist: value.requires_dist.into_owned(),
            requires_python: value.requires_python.into_owned(),
            editable: value.editable,
        }
    }
}

impl<'a> From<&'a PypiPackageData> for PypiPackageDataModel<'a> {
    fn from(value: &'a PypiPackageData) -> Self {
        Self {
            name: Cow::Borrowed(&value.name),
            version: Cow::Borrowed(&value.version),
            location: value.location.clone(),
            hash: Cow::Borrowed(&value.hash),
            requires_dist: Cow::Borrowed(&value.requires_dist),
            requires_python: Cow::Borrowed(&value.requires_python),
            editable: value.editable,
        }
    }
}
