use std::{borrow::Cow, collections::BTreeSet};

use super::super::legacy::{LegacyCondaBinaryData, LegacyCondaPackageData};
use crate::{
    utils::derived_fields::{
        derive_arch_and_platform, derive_build_number_from_build, derive_channel_from_location,
    },
    UrlOrPath,
};
use rattler_conda_types::package::{
    ArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier, DistArchiveType,
};
use rattler_conda_types::{
    BuildNumber, ChannelUrl, NoArchType, PackageName, PackageRecord, PackageUrl, VersionWithSource,
};
use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
use serde::Deserialize;
use serde_with::serde_as;
use url::Url;

/// This struct is similar to [`crate::parse::models::v6::CondaPackageData`] but
/// used for the V5 version of the lock file format.
#[serde_as]
#[derive(Deserialize, Eq, PartialEq)]
pub(crate) struct CondaPackageDataModel<'a> {
    // Unique identifiers go to the top
    pub name: Cow<'a, PackageName>,
    pub version: Cow<'a, VersionWithSource>,
    #[serde(default)]
    pub build: Cow<'a, str>,
    #[serde(default)]
    pub build_number: BuildNumber,
    #[serde(default)]
    pub subdir: Cow<'a, str>,
    pub noarch: Cow<'a, NoArchType>,

    // Followed by the URL of the package
    pub url: Cow<'a, Url>,

    // Then the hashes
    #[serde(default)]
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,
    #[serde(default)]
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,
    #[serde(default)]
    #[serde_as(as = "Option<SerializableHash::<rattler_digest::Md5>>")]
    pub legacy_bz2_md5: Option<Md5Hash>,

    // Dependencies
    #[serde(default)]
    pub depends: Cow<'a, [String]>,
    #[serde(default)]
    pub constrains: Cow<'a, [String]>,

    // Additional properties (in semi alphabetic order but grouped by commonality)
    #[serde(default)]
    pub arch: Cow<'a, Option<String>>,
    #[serde(default)]
    pub platform: Cow<'a, Option<String>>,

    #[serde(default)]
    pub channel: Cow<'a, Option<Url>>,

    #[serde(default)]
    pub features: Cow<'a, Option<String>>,
    #[serde(default)]
    pub track_features: Cow<'a, [String]>,

    #[serde(default)]
    pub file_name: Cow<'a, Option<DistArchiveIdentifier>>,

    #[serde(default)]
    pub python_site_packages_path: Cow<'a, Option<String>>,

    #[serde(default)]
    pub license: Cow<'a, Option<String>>,
    #[serde(default)]
    pub license_family: Cow<'a, Option<String>>,
    #[serde(default)]
    pub purls: Cow<'a, Option<BTreeSet<PackageUrl>>>,

    #[serde(default)]
    pub size: Cow<'a, Option<u64>>,
    #[serde(default)]
    pub legacy_bz2_size: Cow<'a, Option<u64>>,

    #[serde(default)]
    #[serde_as(as = "Option<crate::utils::serde::Timestamp>")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

fn find_build_and_build_number(
    read_build: &str,
    read_build_number: u64,
    file_id: &DistArchiveIdentifier,
) -> (String, u64) {
    let build = if read_build.is_empty() {
        file_id.identifier.build_string.clone()
    } else {
        read_build.to_string()
    };
    let build_number = if read_build_number == 0 {
        derive_build_number_from_build(&build).unwrap_or(read_build_number)
    } else {
        read_build_number
    };

    (build, build_number)
}

impl<'a> From<CondaPackageDataModel<'a>> for LegacyCondaPackageData {
    fn from(value: CondaPackageDataModel<'a>) -> Self {
        let location = UrlOrPath::Url(value.url.into_owned());
        let subdir = value.subdir.into_owned();
        let (derived_arch, derived_platform) = derive_arch_and_platform(&subdir);

        let file_id = value
            .file_name
            .into_owned()
            .or_else(|| location.file_name().and_then(|f| f.parse().ok()))
            .unwrap_or_else(|| DistArchiveIdentifier {
                identifier: ArchiveIdentifier {
                    name: value.name.as_normalized().to_owned(),
                    version: value.version.to_string(),
                    build_string: value.build.to_string(),
                },
                archive_type: DistArchiveType::Conda(CondaArchiveType::Conda),
            });
        let file_name = file_id.to_file_name();

        let (build, build_number) =
            find_build_and_build_number(&value.build, value.build_number, &file_id);

        Self::Binary(LegacyCondaBinaryData {
            package_record: PackageRecord {
                build,
                build_number,
                constrains: value.constrains.into_owned(),
                depends: value.depends.into_owned(),
                experimental_extra_depends: std::collections::BTreeMap::new(),
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
                timestamp: value.timestamp.map(Into::into),
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
