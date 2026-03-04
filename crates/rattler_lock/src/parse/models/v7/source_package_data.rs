use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use rattler_conda_types::{
    utils::TimestampMs, BuildNumber, NoArchType, PackageRecord, PackageUrl, VersionWithSource,
};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use super::source_data::{PackageBuildSourceSerializer, SourceLocationSerializer};
use crate::{
    conda::{CondaSourceData, PackageBuildSource, VariantValue},
    source::SourceLocation,
    utils::derived_fields,
    CondaPackageData, ConversionError, SourceIdentifier,
};

/// A model struct for source packages in V7 lock files.
///
/// This type is used for packages identified by the `- source:` key.
/// Unlike `CondaPackageDataModel` (for binary packages), this type:
/// - Always converts to `CondaPackageData::Source`
/// - Does not include binary-specific fields (`file_name`, `channel`, `hashes`)
/// - Includes source-specific fields (`variants`, `package_build_source`, `sources`)
/// - Uses `SourceIdentifier` format: `name[hash] @ location`
///
/// The `source` field contains a unique identifier that includes:
/// - Package name
/// - A short hash computed from the package record
/// - The source location (URL or path)
#[serde_as]
#[derive(Serialize, Deserialize, Eq, PartialEq)]
pub(crate) struct SourcePackageDataModel<'a> {
    /// The source identifier in the format `name[hash] @ location`.
    /// This is the discriminator key and uniquely identifies the package.
    #[serde(rename = "source")]
    pub identifier: SourceIdentifier,

    // Version is required (not embedded in identifier)
    pub version: Cow<'a, VersionWithSource>,

    // Optional identification fields
    #[serde(default, skip_serializing_if = "str::is_empty")]
    pub build: Cow<'a, str>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub build_number: BuildNumber,

    // Required subdir
    pub subdir: Cow<'a, str>,

    #[serde(default, skip_serializing_if = "NoArchType::is_none")]
    pub noarch: NoArchType,

    // Conda-build variants for source packages
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<Cow<'a, BTreeMap<String, VariantValue>>>,

    // Dependencies
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub depends: Cow<'a, [String]>,
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub constrains: Cow<'a, [String]>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(rename = "extra_depends")]
    pub experimental_extra_depends: Cow<'a, BTreeMap<String, Vec<String>>>,

    // Metadata
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Cow<'a, Option<String>>,
    #[serde(default, skip_serializing_if = "<[String]>::is_empty")]
    pub track_features: Cow<'a, [String]>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Cow<'a, Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_family: Cow<'a, Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purls: Cow<'a, Option<BTreeSet<PackageUrl>>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Cow<'a, Option<u64>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<crate::utils::serde::Timestamp>")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<PackageBuildSourceSerializer>")]
    pub package_build_source: Option<PackageBuildSource>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[serde_as(as = "BTreeMap<_, SourceLocationSerializer>")]
    pub sources: BTreeMap<String, SourceLocation>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_site_packages_path: Cow<'a, Option<String>>,
}

fn is_zero(value: &BuildNumber) -> bool {
    *value == 0
}

impl<'a> SourcePackageDataModel<'a> {
    /// Converts into `(SourceIdentifier, CondaSourceData)`.
    ///
    /// This method preserves the deserialized `SourceIdentifier` so it can be used
    /// directly for lookups without recomputing the hash.
    pub fn into_parts(self) -> Result<(SourceIdentifier, CondaSourceData), ConversionError> {
        // Extract name and location from the identifier, preserving the identifier itself
        let (name, _hash, location) = self.identifier.clone().into_parts();

        let subdir = self.subdir.into_owned();
        let build = self.build.into_owned();
        let (arch, platform) = derived_fields::derive_arch_and_platform(&subdir);

        let package_record = PackageRecord {
            name,
            version: self.version.into_owned(),
            subdir,
            build,
            build_number: self.build_number,
            noarch: self.noarch,
            arch,
            platform,
            constrains: self.constrains.into_owned(),
            depends: self.depends.into_owned(),
            experimental_extra_depends: self.experimental_extra_depends.into_owned(),
            features: self.features.into_owned(),
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: self.license.into_owned(),
            license_family: self.license_family.into_owned(),
            md5: None,
            purls: self.purls.into_owned(),
            sha256: None,
            size: self.size.into_owned(),
            timestamp: self.timestamp.map(Into::into),
            track_features: self.track_features.into_owned(),
            run_exports: None,
            python_site_packages_path: self.python_site_packages_path.into_owned(),
        };

        let source_data = CondaSourceData {
            package_record,
            location,
            variants: self.variants.map(Cow::into_owned).unwrap_or_default(),
            package_build_source: self.package_build_source,
            sources: self.sources,
        };

        Ok((self.identifier, source_data))
    }
}

impl<'a> TryFrom<SourcePackageDataModel<'a>> for CondaPackageData {
    type Error = ConversionError;

    fn try_from(value: SourcePackageDataModel<'a>) -> Result<Self, Self::Error> {
        let (_identifier, source_data) = value.into_parts()?;
        Ok(CondaPackageData::Source(source_data))
    }
}

impl<'a> From<&'a CondaSourceData> for SourcePackageDataModel<'a> {
    fn from(value: &'a CondaSourceData) -> Self {
        let package_record = &value.package_record;
        let variants = (!value.variants.is_empty()).then_some(Cow::Borrowed(&value.variants));

        // Create the source identifier with computed hash
        let identifier = SourceIdentifier::from_source_data(value);

        Self {
            identifier,
            version: Cow::Borrowed(&package_record.version),
            subdir: Cow::Borrowed(&package_record.subdir),
            build: Cow::Borrowed(&package_record.build),
            build_number: package_record.build_number,
            noarch: package_record.noarch,
            variants,
            purls: Cow::Borrowed(&package_record.purls),
            depends: Cow::Borrowed(&package_record.depends),
            constrains: Cow::Borrowed(&package_record.constrains),
            experimental_extra_depends: Cow::Borrowed(&package_record.experimental_extra_depends),
            size: Cow::Borrowed(&package_record.size),
            timestamp: package_record.timestamp.map(TimestampMs::into_datetime),
            features: Cow::Borrowed(&package_record.features),
            track_features: Cow::Borrowed(&package_record.track_features),
            license: Cow::Borrowed(&package_record.license),
            license_family: Cow::Borrowed(&package_record.license_family),
            python_site_packages_path: Cow::Borrowed(&package_record.python_site_packages_path),
            package_build_source: value.package_build_source.clone(),
            sources: value.sources.clone(),
        }
    }
}
