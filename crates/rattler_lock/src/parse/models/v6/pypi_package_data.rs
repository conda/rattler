use std::borrow::Cow;

use pep440_rs::VersionSpecifiers;
use pep508_rs::{PackageName, Requirement};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};

use crate::{PackageHashes, PypiPackageData, UrlOrPath};

/// A helper struct that wraps all fields of a [`crate::PypiPackageData`] and
/// allows for easy conversion between the two.
///
/// This type provides full control over the order of the fields when
/// serializing. This is important because one of the design goals is that it
/// should be easy to read the lock file. A [`PackageRecord`] is serialized in
/// alphabetic order which might not be the most readable. This type instead
/// puts the "most important" fields at the top followed by more detailed ones.
///
/// So note that for reproducibility the order of these fields should not change
/// or should be reflected in a version change.
///
/// The complexity with `Cow<_>` types is introduced to allow both efficient
/// deserialization and serialization without requiring all data to be cloned
/// when serializing. We want to be able to use the same type of both
/// serialization and deserialization to ensure that when any of the
/// types involved change we are forced to update this struct as well.
#[serde_as]
#[skip_serializing_none]
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, Hash)]
pub(crate) struct PypiPackageDataModel<'a> {
    #[serde(rename = "pypi")]
    pub location: Cow<'a, UrlOrPath>,
    pub name: Cow<'a, PackageName>,
    pub version: Cow<'a, pep440_rs::Version>,
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
            location: value.location.into_owned(),
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
            location: Cow::Borrowed(&value.location),
            hash: Cow::Borrowed(&value.hash),
            requires_dist: Cow::Borrowed(&value.requires_dist),
            requires_python: Cow::Borrowed(&value.requires_python),
            editable: value.editable,
        }
    }
}
