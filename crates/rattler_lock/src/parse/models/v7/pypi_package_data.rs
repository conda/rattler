use std::{borrow::Cow, sync::LazyLock};

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
    pub version: Option<Cow<'a, pep440_rs::Version>>,
    #[serde(default, skip_serializing_if = "Option::is_none", flatten)]
    pub hash: Cow<'a, Option<PackageHashes>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_url: Option<Cow<'a, url::Url>>,
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub requires_dist: Cow<'a, [String]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_python: Cow<'a, Option<VersionSpecifiers>>,
}

impl<'a> From<PypiPackageDataModel<'a>> for PypiPackageDataRaw {
    fn from(value: PypiPackageDataModel<'a>) -> Self {
        let index_url = match value.location.inner() {
            UrlOrPath::Url(_) => value
                .index_url
                .map(std::borrow::Cow::into_owned)
                .or_else(|| Some((*PYPI_URL).clone())),
            UrlOrPath::Path(_) => None,
        };

        Self {
            name: value.name.into_owned(),
            version: value.version.map(std::borrow::Cow::into_owned),
            location: value.location.into_owned(),
            hash: value.hash.into_owned(),
            index_url,
            requires_dist: value.requires_dist.into_owned(),
            requires_python: value.requires_python.into_owned(),
        }
    }
}

static PYPI_URL: LazyLock<url::Url> =
    LazyLock::new(|| url::Url::parse("https://pypi.org/simple").expect("Valid, hard-coded URL"));

impl<'a> From<&'a PypiPackageData> for PypiPackageDataModel<'a> {
    fn from(value: &'a PypiPackageData) -> Self {
        let requires_dist = value
            .requires_dist
            .iter()
            .map(requirement_to_string)
            .collect::<Vec<_>>();
        let index_url = value.index_url.as_ref().and_then(|i| {
            if *i == *PYPI_URL {
                None
            } else {
                Some(Cow::Borrowed(i))
            }
        });

        Self {
            name: Cow::Borrowed(&value.name),
            version: value.version.as_ref().map(Cow::Borrowed),
            location: Cow::Borrowed(&value.location),
            hash: Cow::Borrowed(&value.hash),
            index_url,
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

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use pep508_rs::PackageName;

    use typed_path::Utf8TypedPathBuf;

    use crate::{parse::deserialize::PypiPackageDataRaw, UrlOrPath, Verbatim};

    use super::{PypiPackageDataModel, PYPI_URL};

    fn url_location(url: &str) -> Verbatim<UrlOrPath> {
        Verbatim::new(UrlOrPath::Url(url::Url::parse(url).unwrap()))
    }

    fn path_location(path: &str) -> Verbatim<UrlOrPath> {
        Verbatim::new(UrlOrPath::Path(Utf8TypedPathBuf::from(path)))
    }

    fn make_model(
        location: Verbatim<UrlOrPath>,
        index_url: Option<url::Url>,
    ) -> PypiPackageDataModel<'static> {
        PypiPackageDataModel {
            location: Cow::Owned(location),
            name: Cow::Owned("test-pkg".parse::<PackageName>().unwrap()),
            version: None,
            hash: Cow::Owned(None),
            index_url: index_url.map(Cow::Owned),
            requires_dist: Cow::Owned(vec![]),
            requires_python: Cow::Owned(None),
        }
    }

    #[test]
    fn url_location_without_index_url_defaults_to_pypi() {
        let model = make_model(
            url_location("https://files.pythonhosted.org/pkg-1.0.whl"),
            None,
        );
        let raw: PypiPackageDataRaw = model.into();
        assert_eq!(raw.index_url.as_ref(), Some(&*PYPI_URL));
    }

    #[test]
    fn url_location_with_custom_index_url_preserved() {
        let custom = url::Url::parse("https://custom.index/simple").unwrap();
        let model = make_model(
            url_location("https://custom.index/packages/pkg-1.0.whl"),
            Some(custom.clone()),
        );
        let raw: PypiPackageDataRaw = model.into();
        assert_eq!(raw.index_url.as_ref(), Some(&custom));
    }

    #[test]
    fn path_location_has_no_index_url() {
        let model = make_model(path_location("./my-pkg"), None);
        let raw: PypiPackageDataRaw = model.into();
        assert!(raw.index_url.is_none());
    }

    #[test]
    fn serialization_elides_default_pypi_url() {
        let data = crate::PypiPackageData {
            name: "test-pkg".parse().unwrap(),
            version: None,
            location: url_location("https://files.pythonhosted.org/pkg-1.0.whl"),
            index_url: Some(PYPI_URL.clone()),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
        };
        let model = PypiPackageDataModel::from(&data);
        assert!(
            model.index_url.is_none(),
            "default pypi.org URL should be elided"
        );
    }

    #[test]
    fn serialization_keeps_custom_index_url() {
        let custom = url::Url::parse("https://custom.index/simple").unwrap();
        let data = crate::PypiPackageData {
            name: "test-pkg".parse().unwrap(),
            version: None,
            location: url_location("https://custom.index/packages/pkg-1.0.whl"),
            index_url: Some(custom.clone()),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
        };
        let model = PypiPackageDataModel::from(&data);
        assert_eq!(model.index_url.as_deref(), Some(&custom),);
    }

    #[test]
    fn serialization_none_index_url_stays_none() {
        let data = crate::PypiPackageData {
            name: "test-pkg".parse().unwrap(),
            version: None,
            location: path_location("./my-pkg"),
            index_url: None,
            hash: None,
            requires_dist: vec![],
            requires_python: None,
        };
        let model = PypiPackageDataModel::from(&data);
        assert!(model.index_url.is_none());
    }
}
