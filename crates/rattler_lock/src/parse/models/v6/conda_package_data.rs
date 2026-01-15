use super::{
    super::legacy::{LegacyCondaBinaryData, LegacyCondaPackageData, LegacyCondaSourceData},
    source_data::{PackageBuildSourceSerializer, SourceLocationSerializer},
};
use crate::{
    conda::{PackageBuildSource, VariantValue},
    source::SourceLocation,
    utils::{derived_fields, derived_fields::LocationDerivedFields},
    ConversionError, UrlOrPath,
};
use rattler_conda_types::package::DistArchiveIdentifier;
use rattler_conda_types::{
    package::CondaArchiveIdentifier, BuildNumber, ChannelUrl, NoArchType, PackageName,
    PackageRecord, PackageUrl, VersionWithSource,
};
use rattler_digest::{serde::SerializableHash, Md5Hash, Sha256Hash};
use serde::Deserialize;
use serde_with::serde_as;
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};
use url::Url;

/// A helper struct that wraps all fields of a [`crate::CondaPackageData`] and
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
#[derive(Deserialize, Eq, PartialEq)]
pub(crate) struct CondaPackageDataModel<'a> {
    /// The location of the package. This can be a URL or a path.
    #[serde(rename = "conda")]
    pub location: UrlOrPath,

    // Unique identifiers go to the top
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<Cow<'a, PackageName>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<Cow<'a, VersionWithSource>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<Cow<'a, str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_number: Option<BuildNumber>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdir: Option<Cow<'a, str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noarch: Option<Cow<'a, NoArchType>>,

    // Conda-build variants for source packages (optional in V6, required in V7)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<Cow<'a, BTreeMap<String, VariantValue>>>,

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
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub depends: Cow<'a, [String]>,
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub constrains: Cow<'a, [String]>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(rename = "extra_depends")]
    pub experimental_extra_depends: Cow<'a, BTreeMap<String, Vec<String>>>,

    // Additional properties (in semi alphabetic order but grouped by commonality)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<Cow<'a, Option<Url>>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Cow<'a, Option<String>>,
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub track_features: Cow<'a, [String]>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<Cow<'a, Option<DistArchiveIdentifier>>>,

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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<InputHash<'a>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<PackageBuildSourceSerializer>")]
    pub package_build_source: Option<PackageBuildSource>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[serde_as(as = "BTreeMap<_, SourceLocationSerializer>")]
    pub sources: BTreeMap<String, SourceLocation>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_site_packages_path: Cow<'a, Option<String>>,
}

#[serde_as]
#[derive(Deserialize, Eq, PartialEq)]
pub(crate) struct InputHash<'a> {
    #[serde_as(as = "SerializableHash::<rattler_digest::Sha256>")]
    pub hash: Sha256Hash,
    pub globs: Cow<'a, [String]>,
}

impl<'a> TryFrom<CondaPackageDataModel<'a>> for LegacyCondaPackageData {
    type Error = ConversionError;

    fn try_from(value: CondaPackageDataModel<'a>) -> Result<Self, Self::Error> {
        let derived = LocationDerivedFields::new(&value.location);
        let build = value
            .build
            .map(Cow::into_owned)
            .or_else(|| derived.build.clone())
            .unwrap_or_default();
        let build_number = value
            .build_number
            .or_else(|| derived_fields::derive_build_number_from_build(&build))
            .unwrap_or(0);
        let subdir = value
            .subdir
            .map(Cow::into_owned)
            .or_else(|| derived.subdir.clone())
            .ok_or_else(|| ConversionError::Missing("subdir".to_string()))?;
        let noarch = value.noarch.map_or_else(
            || {
                derived_fields::derive_noarch_type(
                    derived.subdir.as_deref().unwrap_or(&subdir),
                    derived.build.as_deref().unwrap_or(&build),
                )
            },
            Cow::into_owned,
        );
        let (derived_arch, derived_platform) = derived_fields::derive_arch_and_platform(&subdir);

        let package_record = PackageRecord {
            build,
            build_number,
            constrains: value.constrains.into_owned(),
            depends: value.depends.into_owned(),
            experimental_extra_depends: value.experimental_extra_depends.into_owned(),
            features: value.features.into_owned(),
            legacy_bz2_md5: value.legacy_bz2_md5,
            legacy_bz2_size: value.legacy_bz2_size.into_owned(),
            license: value.license.into_owned(),
            license_family: value.license_family.into_owned(),
            md5: value.md5,
            name: value
                .name
                .map(Cow::into_owned)
                .or(derived.name)
                .ok_or_else(|| ConversionError::Missing("name".to_string()))?,
            noarch,
            arch: derived_arch,
            platform: derived_platform,
            purls: value.purls.into_owned(),
            sha256: value.sha256,
            size: value.size.into_owned(),
            subdir,
            timestamp: value.timestamp.map(Into::into),
            track_features: value.track_features.into_owned(),
            version: value
                .version
                .map(Cow::into_owned)
                .or(derived.version)
                .ok_or_else(|| ConversionError::Missing("version".to_string()))?,
            run_exports: None,
            python_site_packages_path: value.python_site_packages_path.into_owned(),
        };

        if value
            .location
            .file_name()
            .is_some_and(|name| CondaArchiveIdentifier::try_from_filename(name).is_some())
        {
            let file_name = value
                .location
                .file_name()
                .expect("if checked this")
                .to_owned();
            Ok(LegacyCondaPackageData::Binary(LegacyCondaBinaryData {
                location: value.location,
                file_name,
                channel: value
                    .channel
                    .map(Cow::into_owned)
                    .map(|m| m.map(ChannelUrl::from))
                    .unwrap_or(derived.channel),
                package_record,
            }))
        } else {
            Ok(LegacyCondaPackageData::Source(LegacyCondaSourceData {
                package_record,
                location: value.location,
                variants: value.variants.map(Cow::into_owned).unwrap_or_default(),
                package_build_source: value.package_build_source,
                sources: value.sources,
            }))
        }
    }
}
