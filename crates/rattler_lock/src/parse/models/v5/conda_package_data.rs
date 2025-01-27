use std::{borrow::Cow, collections::BTreeSet};

use rattler_conda_types::{
    BuildNumber, ChannelUrl, NoArchType, PackageName, PackageRecord, PackageUrl, VersionWithSource,
};
use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use url::Url;

use crate::{
    conda::CondaBinaryData,
    utils::derived_fields::{derive_arch_and_platform, derive_channel_from_location},
    CondaPackageData, UrlOrPath,
};

fn is_default<T: Default + Eq>(value: &T) -> bool {
    value == &T::default()
}

/// This struct is similar to [`crate::parse::models::v6::CondaPackageData`] but
/// used for the V5 version of the lock file format.
#[serde_as]
#[derive(Serialize, Deserialize, Eq, PartialEq)]
pub(crate) struct CondaPackageDataModel<'a> {
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

impl<'a> From<CondaPackageDataModel<'a>> for CondaPackageData {
    fn from(value: CondaPackageDataModel<'a>) -> Self {
        let location = UrlOrPath::Url(value.url.into_owned());
        let subdir = value.subdir.into_owned();
        let (derived_arch, derived_platform) = derive_arch_and_platform(&subdir);

        let file_name = value
            .file_name
            .into_owned()
            .or_else(|| location.file_name().map(ToString::to_string))
            .unwrap_or_else(|| {
                format!(
                    "{}-{}-{}.conda",
                    value.name.as_normalized(),
                    value.version,
                    value.build
                )
            });

        Self::Binary(CondaBinaryData {
            package_record: PackageRecord {
                build: value.build.into_owned(),
                build_number: value.build_number,
                constrains: value.constrains.into_owned(),
                depends: value.depends.into_owned(),
                extra_depends: std::collections::BTreeMap::new(),
                features: value.features.into_owned(),
                legacy_bz2_md5: value.legacy_bz2_md5,
                legacy_bz2_size: value.legacy_bz2_size.into_owned(),
                license: value.license.into_owned(),
                license_family: value.license_family.into_owned(),
                md5: value.md5,
                name: value.name.into_owned(),
                noarch: value.noarch.into_owned(),
                purls: value.purls.into_owned(),
                sha256: value.sha256,
                size: value.size.into_owned(),
                subdir,
                timestamp: value.timestamp,
                track_features: value.track_features.into_owned(),
                version: value.version.into_owned(),
                run_exports: None,
                // Polyfill the arch and platform fields if they are not present in the lock-file.
                arch: value.arch.into_owned().or(derived_arch),
                platform: value.platform.into_owned().or(derived_platform),
                python_site_packages_path: value.python_site_packages_path.into_owned(),
            },
            channel: value
                .channel
                .into_owned()
                .map(ChannelUrl::from)
                .or_else(|| derive_channel_from_location(&location)),
            file_name,
            location,
        })
    }
}
