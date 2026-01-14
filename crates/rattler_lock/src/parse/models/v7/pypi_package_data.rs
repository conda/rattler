use std::borrow::Cow;

use pep440_rs::VersionSpecifiers;
use pep508_rs::{PackageName, VersionOrUrl};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};

use crate::{
    parse::deserialize::PypiPackageDataRaw, PackageHashes, PypiPackageData, UrlOrPath, Verbatim,
};

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
    pub location: Cow<'a, Verbatim<UrlOrPath>>,
    pub name: Cow<'a, PackageName>,
    pub version: Cow<'a, pep440_rs::Version>,
    #[serde(default, skip_serializing_if = "Option::is_none", flatten)]
    pub hash: Cow<'a, Option<PackageHashes>>,
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub requires_dist: Cow<'a, [String]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_python: Cow<'a, Option<VersionSpecifiers>>,
}

impl<'a> From<PypiPackageDataModel<'a>> for PypiPackageDataRaw {
    fn from(value: PypiPackageDataModel<'a>) -> Self {
        Self {
            name: value.name.into_owned(),
            version: value.version.into_owned(),
            location: value.location.into_owned(),
            hash: value.hash.into_owned(),
            requires_dist: value.requires_dist.into_owned(),
            requires_python: value.requires_python.into_owned(),
        }
    }
}

impl<'a> From<&'a PypiPackageData> for PypiPackageDataModel<'a> {
    fn from(value: &'a PypiPackageData) -> Self {
        let requires_dist = value
            .requires_dist
            .iter()
            .map(requirement_to_string)
            .collect::<Vec<_>>();
        Self {
            name: Cow::Borrowed(&value.name),
            version: Cow::Borrowed(&value.version),
            location: Cow::Borrowed(&value.location),
            hash: Cow::Borrowed(&value.hash),
            requires_dist: requires_dist.into(),
            requires_python: Cow::Borrowed(&value.requires_python),
        }
    }
}

/// This code is heavily inspired from the `Display::fmt` implementation of `pep508_rs`
/// (under Apache-2.0 or BSD-2-clause license).
///
/// This uses the `given()` of the URL if it exists though, so that we keep relative
/// paths intact.
fn requirement_to_string(req: &pep508_rs::Requirement) -> String {
    let extras = (!req.extras.is_empty())
        .then_some(format!(
            "[{}]",
            req.extras
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ))
        .unwrap_or_default();

    let version_or_url = req
        .version_or_url
        .as_ref()
        .map(|version_or_url| {
            match version_or_url {
                VersionOrUrl::VersionSpecifier(version_specifier) => {
                    let version_specifier: Vec<String> =
                        version_specifier.iter().map(ToString::to_string).collect();
                    version_specifier.join(",")
                }
                VersionOrUrl::Url(url) => {
                    if let Some(g) = url.given() {
                        format!(" @ {g}")
                    } else {
                        // We add the space for markers later if necessary
                        format!(" @ {url}")
                    }
                }
            }
        })
        .unwrap_or_default();
    let marker = req
        .marker
        .contents()
        .map(|c| format!(" ; {c}"))
        .unwrap_or_default();

    format!("{}{extras}{version_or_url}{marker}", req.name)
}
