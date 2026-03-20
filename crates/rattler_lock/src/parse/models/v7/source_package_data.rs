use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use rattler_conda_types::{BuildNumber, NoArchType, PackageRecord, PackageUrl, VersionWithSource};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use super::source_data::{PackageBuildSourceSerializer, SourceLocationSerializer};
use crate::{
    conda::{
        CondaSourceData, FullSourceMetadata, PackageBuildSource, PartialSourceMetadata,
        SourceMetadata, VariantValue,
    },
    source::SourceLocation,
    utils::derived_fields,
    CondaPackageData, ConversionError, SourceIdentifier,
};

/// A model struct for source packages in V7 lock files.
///
/// This type is used for packages identified by the `- conda_source:` key.
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
    pub conda_source: SourceIdentifier,

    // Package record fields are optional: absent means the metadata has not
    // been evaluated yet and `CondaSourceData::package_record` will be `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<Cow<'a, VersionWithSource>>,

    #[serde(default, skip_serializing_if = "str::is_empty")]
    pub build: Cow<'a, str>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub build_number: BuildNumber,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdir: Option<Cow<'a, str>>,

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

    #[serde(default)]
    #[serde_as(as = "crate::utils::serde::Timestamp")]
    pub timestamp: chrono::DateTime<chrono::Utc>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<PackageBuildSourceSerializer>")]
    pub source: Option<PackageBuildSource>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[serde_as(as = "BTreeMap<_, SourceLocationSerializer>")]
    pub source_depends: BTreeMap<String, SourceLocation>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_site_packages_path: Cow<'a, Option<String>>,
}

fn is_zero(value: &BuildNumber) -> bool {
    *value == 0
}

impl<'a> SourcePackageDataModel<'a> {
    /// Converts into `(SourceIdentifier, CondaSourceData)`.
    ///
    /// This method preserves the deserialized `SourceIdentifier` so it can be
    /// used directly for lookups without recomputing the hash.
    ///
    /// When `version` is absent all package-record fields are ignored and
    /// `CondaSourceData::package_record` is set to `None`.
    pub fn into_parts(self) -> Result<(SourceIdentifier, CondaSourceData), ConversionError> {
        // Extract name and location from the identifier, preserving the identifier itself
        let (name, hash, location) = self.conda_source.clone().into_parts();

        let timestamp = self.timestamp;

        // Only build a PackageRecord when version (and subdir) are present.
        let metadata = if let (Some(version), Some(subdir)) = (self.version, self.subdir) {
            let subdir = subdir.into_owned();
            let build = self.build.into_owned();
            let (arch, platform) = derived_fields::derive_arch_and_platform(&subdir);
            SourceMetadata::Full(Box::new(FullSourceMetadata {
                package_record: PackageRecord {
                    name: name.clone(),
                    version: version.into_owned(),
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
                    timestamp: Some(timestamp.into()),
                    track_features: self.track_features.into_owned(),
                    run_exports: None,
                    python_site_packages_path: self.python_site_packages_path.into_owned(),
                },
                sources: self.source_depends,
            }))
        } else {
            SourceMetadata::Partial(PartialSourceMetadata {
                name,
                depends: self.depends.into_owned(),
                sources: self.source_depends,
            })
        };

        let source_data = CondaSourceData {
            location,
            variants: self.variants.map(Cow::into_owned).unwrap_or_default(),
            timestamp,
            package_build_source: self.source,
            identifier_hash: Some(hash),
            metadata,
        };

        Ok((self.conda_source, source_data))
    }
}

impl<'a> TryFrom<SourcePackageDataModel<'a>> for CondaPackageData {
    type Error = ConversionError;

    fn try_from(value: SourcePackageDataModel<'a>) -> Result<Self, Self::Error> {
        let (_identifier, source_data) = value.into_parts()?;
        Ok(CondaPackageData::Source(Box::new(source_data)))
    }
}

impl<'a> From<&'a CondaSourceData> for SourcePackageDataModel<'a> {
    fn from(value: &'a CondaSourceData) -> Self {
        let variants = (!value.variants.is_empty()).then_some(Cow::Borrowed(&value.variants));
        let identifier = SourceIdentifier::from_source_data(value);

        match &value.metadata {
            SourceMetadata::Full(full) => {
                let r = &full.package_record;
                Self {
                    conda_source: identifier,
                    version: Some(Cow::Borrowed(&r.version)),
                    subdir: Some(Cow::Borrowed(&r.subdir)),
                    build: Cow::Borrowed(&r.build),
                    build_number: r.build_number,
                    noarch: r.noarch,
                    variants,
                    purls: Cow::Borrowed(&r.purls),
                    depends: Cow::Borrowed(&r.depends),
                    constrains: Cow::Borrowed(&r.constrains),
                    experimental_extra_depends: Cow::Borrowed(&r.experimental_extra_depends),
                    size: Cow::Borrowed(&r.size),
                    timestamp: value.timestamp,
                    features: Cow::Borrowed(&r.features),
                    track_features: Cow::Borrowed(&r.track_features),
                    license: Cow::Borrowed(&r.license),
                    license_family: Cow::Borrowed(&r.license_family),
                    python_site_packages_path: Cow::Borrowed(&r.python_site_packages_path),
                    source: value.package_build_source.clone(),
                    source_depends: full.sources.clone(),
                }
            }
            SourceMetadata::Partial(partial) => Self {
                conda_source: identifier,
                version: None,
                subdir: None,
                build: Cow::Borrowed(""),
                build_number: 0,
                noarch: NoArchType::default(),
                variants,
                purls: Cow::Owned(None),
                depends: Cow::Borrowed(&partial.depends),
                constrains: Cow::Borrowed(&[]),
                experimental_extra_depends: Cow::Owned(BTreeMap::new()),
                size: Cow::Owned(None),
                timestamp: value.timestamp,
                features: Cow::Owned(None),
                track_features: Cow::Borrowed(&[]),
                license: Cow::Owned(None),
                license_family: Cow::Owned(None),
                python_site_packages_path: Cow::Owned(None),
                source: value.package_build_source.clone(),
                source_depends: partial.sources.clone(),
            },
        }
    }
}
