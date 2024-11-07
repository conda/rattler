use std::{borrow::Cow, cmp::Ordering, collections::BTreeSet};

use rattler_conda_types::{
    BuildNumber, NoArchType, PackageName, PackageRecord, PackageUrl, VersionWithSource,
};
use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use url::Url;

use crate::CondaPackageData;

fn is_default<T: Default + Eq>(value: &T) -> bool {
    value == &T::default()
}

/// A helper struct that wraps all fields of a [`CondaPackageData`] and allows
/// for easy conversion between the two.
///
/// This type provides full control over the order of the fields when
/// serializing. This is important because one of the design goals is that it
/// should be easy to read the lock file. A [`PackageRecord`] is serialized in
/// alphabetic order which might not be the most readable. This type instead
/// puts the "most important" fields at the top followed by more detailed ones.
///
/// So note that for reproducibility the order of these fields should not change
/// or should be reflected in a version change.
//
/// This type also adds more default values (e.g. for `build_number` and
/// `build_string`).
///
/// The complexity with `Cow<_>` types is introduced to allow both efficient
/// deserialization and serialization without requiring all data to be cloned
/// when serializing. We want to be able to use the same type of both
/// serialization and deserialization to ensure that when any of the
/// types involved change we are forced to update this struct as well.
#[serde_as]
#[derive(Serialize, Deserialize, Eq, PartialEq)]
pub(crate) struct RawCondaPackageData<'a> {
    // Unique identifiers go to the top
    pub name: Cow<'a, PackageName>,
    pub version: Cow<'a, VersionWithSource>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub build: Cow<'a, String>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub build_number: BuildNumber,
    #[serde(default)]
    pub subdir: Cow<'a, String>,
    #[serde(skip_serializing_if = "NoArchType::is_none")]
    pub noarch: Cow<'a, NoArchType>,

    // Followed by the URL of the package
    pub url: Cow<'a, Url>,

    // Then the hashes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Md5>>")]
    pub legacy_bz2_md5: Option<Md5Hash>,

    // Dependencies
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends: Cow<'a, Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constrains: Cow<'a, Vec<String>>,

    // Additional properties (in semi alphabetic order but grouped by commonality)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Cow<'a, Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Cow<'a, Option<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Cow<'a, Option<Url>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Cow<'a, Option<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub track_features: Cow<'a, Vec<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Cow<'a, Option<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_site_packages_path: Cow<'a, Option<String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Cow<'a, Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_family: Cow<'a, Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purls: Cow<'a, Option<BTreeSet<PackageUrl>>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Cow<'a, Option<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_bz2_size: Cow<'a, Option<u64>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<crate::utils::serde::Timestamp>")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

impl<'a> From<RawCondaPackageData<'a>> for CondaPackageData {
    fn from(value: RawCondaPackageData<'a>) -> Self {
        Self {
            package_record: PackageRecord {
                arch: value.arch.into_owned(),
                build: value.build.into_owned(),
                build_number: value.build_number,
                constrains: value.constrains.into_owned(),
                depends: value.depends.into_owned(),
                features: value.features.into_owned(),
                legacy_bz2_md5: value.legacy_bz2_md5,
                legacy_bz2_size: value.legacy_bz2_size.into_owned(),
                license: value.license.into_owned(),
                license_family: value.license_family.into_owned(),
                md5: value.md5,
                name: value.name.into_owned(),
                noarch: value.noarch.into_owned(),
                platform: value.platform.into_owned(),
                purls: value.purls.into_owned(),
                sha256: value.sha256,
                size: value.size.into_owned(),
                subdir: value.subdir.into_owned(),
                timestamp: value.timestamp,
                track_features: value.track_features.into_owned(),
                version: value.version.into_owned(),
                run_exports: None,
                python_site_packages_path: value.python_site_packages_path.into_owned(),
            },
            url: value.url.into_owned(),
            file_name: value.file_name.into_owned(),
            channel: value.channel.into_owned(),
        }
    }
}

impl<'a> From<&'a CondaPackageData> for RawCondaPackageData<'a> {
    fn from(value: &'a CondaPackageData) -> Self {
        Self {
            name: Cow::Borrowed(&value.package_record.name),
            version: Cow::Borrowed(&value.package_record.version),
            build: Cow::Borrowed(&value.package_record.build),
            build_number: value.package_record.build_number,
            subdir: Cow::Borrowed(&value.package_record.subdir),
            noarch: Cow::Borrowed(&value.package_record.noarch),
            url: Cow::Borrowed(&value.url),
            channel: Cow::Borrowed(&value.channel),
            file_name: Cow::Borrowed(&value.file_name),
            purls: Cow::Borrowed(&value.package_record.purls),
            depends: Cow::Borrowed(&value.package_record.depends),
            constrains: Cow::Borrowed(&value.package_record.constrains),
            platform: Cow::Borrowed(&value.package_record.platform),
            arch: Cow::Borrowed(&value.package_record.arch),
            md5: value.package_record.md5,
            legacy_bz2_md5: value.package_record.legacy_bz2_md5,
            sha256: value.package_record.sha256,
            size: Cow::Borrowed(&value.package_record.size),
            legacy_bz2_size: Cow::Borrowed(&value.package_record.legacy_bz2_size),
            timestamp: value.package_record.timestamp,
            features: Cow::Borrowed(&value.package_record.features),
            track_features: Cow::Borrowed(&value.package_record.track_features),
            license: Cow::Borrowed(&value.package_record.license),
            license_family: Cow::Borrowed(&value.package_record.license_family),
            python_site_packages_path: Cow::Borrowed(
                &value.package_record.python_site_packages_path,
            ),
        }
    }
}

impl<'a> PartialOrd<Self> for RawCondaPackageData<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<'a> Ord for RawCondaPackageData<'a> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name
            .cmp(&other.name)
            .then_with(|| self.version.cmp(&other.version))
            .then_with(|| self.build.cmp(&other.build))
            .then_with(|| self.subdir.cmp(&other.subdir))
    }
}
