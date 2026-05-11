use std::{borrow::Cow, sync::LazyLock};

use pep440_rs::VersionSpecifiers;
use pep508_rs::PackageName;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};

use super::{given_verbatim_url::GivenVerbatimUrl, package_selector::PackageSelector};
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
    pub index: Option<Cow<'a, url::Url>>,
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub requires_dist: Cow<'a, [String]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_python: Cow<'a, Option<VersionSpecifiers>>,

    /// Selectors for packages in the build environment (pypi source
    /// packages only — empty for wheel distributions).
    /// Populated at lockfile serialization time; empty for standalone package
    /// serialization. Resolved to indices after deserialization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build_packages: Vec<PackageSelector>,
    /// Selectors for packages in the host environment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_packages: Vec<PackageSelector>,
}

impl<'a> From<PypiPackageDataModel<'a>> for PypiPackageDataRaw {
    fn from(value: PypiPackageDataModel<'a>) -> Self {
        let index_url = match value.location.inner() {
            UrlOrPath::Url(_) => value.index.map(std::borrow::Cow::into_owned),
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
        match value {
            PypiPackageData::Distribution(w) => {
                let requires_dist = w
                    .requires_dist
                    .iter()
                    .map(requirement_to_string)
                    .collect::<Vec<_>>();
                let index_url = w.index_url.as_ref().and_then(|i| {
                    if *i == *PYPI_URL {
                        None
                    } else {
                        Some(Cow::Borrowed(i))
                    }
                });
                Self {
                    name: Cow::Borrowed(&w.name),
                    version: Some(Cow::Borrowed(&w.version)),
                    location: Cow::Borrowed(&w.location),
                    hash: Cow::Borrowed(&w.hash),
                    index: index_url,
                    requires_dist: requires_dist.into(),
                    requires_python: Cow::Borrowed(&w.requires_python),
                    build_packages: Vec::new(),
                    host_packages: Vec::new(),
                }
            }
            PypiPackageData::Source(s) => {
                let requires_dist = s
                    .requires_dist
                    .iter()
                    .map(requirement_to_string)
                    .collect::<Vec<_>>();
                Self {
                    name: Cow::Borrowed(&s.name),
                    version: None,
                    location: Cow::Borrowed(&s.location),
                    hash: Cow::Owned(None),
                    index: None,
                    requires_dist: requires_dist.into(),
                    requires_python: Cow::Borrowed(&s.requires_python),
                    build_packages: Vec::new(),
                    host_packages: Vec::new(),
                }
            }
        }
    }
}

/// Serialize a requirement as a string, preserving relative paths for file
/// dependencies. Delegates to pep508_rs's own [`Display`](std::fmt::Display)
/// impl via the [`GivenVerbatimUrl`] wrapper.
fn requirement_to_string(req: &pep508_rs::Requirement) -> String {
    GivenVerbatimUrl::wrap_requirement(req).to_string()
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
            index: index_url.map(Cow::Owned),
            requires_dist: Cow::Owned(vec![]),
            requires_python: Cow::Owned(None),
            build_packages: Vec::new(),
            host_packages: Vec::new(),
        }
    }

    #[test]
    fn url_location_without_index_url_is_none() {
        let model = make_model(
            url_location("https://files.pythonhosted.org/pkg-1.0.whl"),
            None,
        );
        let raw: PypiPackageDataRaw = model.into();
        assert!(
            raw.index_url.is_none(),
            "default index is applied per-environment, not at model level"
        );
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
        let data = crate::PypiDistributionData {
            name: "test-pkg".parse().unwrap(),
            version: "1.0.0".parse().unwrap(),
            location: url_location("https://files.pythonhosted.org/pkg-1.0.whl"),
            index_url: Some(PYPI_URL.clone()),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
        };
        let data = crate::PypiPackageData::from(data);
        let model = PypiPackageDataModel::from(&data);
        assert!(
            model.index.is_none(),
            "default pypi.org URL should be elided"
        );
    }

    #[test]
    fn serialization_keeps_custom_index_url() {
        let custom = url::Url::parse("https://custom.index/simple").unwrap();
        let data = crate::PypiDistributionData {
            name: "test-pkg".parse().unwrap(),
            version: "1.0.0".parse().unwrap(),
            location: url_location("https://custom.index/packages/pkg-1.0.whl"),
            index_url: Some(custom.clone()),
            hash: None,
            requires_dist: vec![],
            requires_python: None,
        };
        let data = crate::PypiPackageData::from(data);
        let model = PypiPackageDataModel::from(&data);
        assert_eq!(model.index.as_deref(), Some(&custom),);
    }

    #[test]
    fn serialization_none_index_url_stays_none() {
        let data = crate::PypiSourceData {
            name: "test-pkg".parse().unwrap(),
            location: path_location("./my-pkg"),
            requires_dist: vec![],
            requires_python: None,
            source_data: crate::SourceData::default(),
        };
        let data = crate::PypiPackageData::from(data);
        let model = PypiPackageDataModel::from(&data);
        assert!(model.index.is_none());
    }
}
