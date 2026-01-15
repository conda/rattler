//! Intermediate types for legacy lock file deserialization (V1-V6).
//!
//! These types are used during lock file parsing for versions 1 through 6 to
//! preserve all fields needed for package disambiguation. After environment
//! resolution is complete, they are converted to the final `CondaPackageData`
//! type.
//!
//! This intermediate layer allows `CondaPackageData` to evolve for newer
//! versions (V7+) without breaking backward compatibility with older lock
//! files.
//!
//! # Why merging exists
//!
//! In lock file versions before V6, multiple packages could share the same URL
//! but differ in metadata (e.g., different subdirs for the same package). When
//! resolving environment references, these packages needed to be merged to
//! combine optional fields like `purls`, `run_exports`, and hashes that might
//! only be present in one of the duplicates.
//!
//! V6 introduced disambiguating fields (name, version, build, subdir) in
//! environment references, making exact matching possible. However, the merging
//! logic is retained here for parsing older format versions.

use std::{borrow::Cow, collections::BTreeMap};

use rattler_conda_types::{ChannelUrl, PackageRecord};

use crate::{
    conda::{CondaBinaryData, CondaSourceData, PackageBuildSource, VariantValue},
    source::SourceLocation,
    CondaPackageData, UrlOrPath,
};

/// Intermediate representation of a conda package during legacy deserialization.
///
/// This type preserves all fields from legacy lock file formats (V1-V6) that
/// are needed for package disambiguation (matching environment references to
/// package data). After resolution, it is converted to `CondaPackageData`.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub(crate) enum LegacyCondaPackageData {
    /// A binary package.
    Binary(LegacyCondaBinaryData),
    /// A source package.
    Source(LegacyCondaSourceData),
}

/// Intermediate binary package data for legacy deserialization.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub(crate) struct LegacyCondaBinaryData {
    /// The package record with all metadata.
    pub package_record: PackageRecord,
    /// The location of the package (URL or path).
    pub location: UrlOrPath,
    /// The filename of the package.
    pub file_name: String,
    /// The channel of the package.
    pub channel: Option<ChannelUrl>,
}

/// Intermediate source package data for legacy deserialization.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub(crate) struct LegacyCondaSourceData {
    /// The package record with all metadata.
    pub package_record: PackageRecord,
    /// The location of the package (URL or path).
    pub location: UrlOrPath,
    /// Conda-build variants for disambiguation.
    pub variants: BTreeMap<String, VariantValue>,
    /// Package build source location.
    pub package_build_source: Option<PackageBuildSource>,
    /// Source locations for packages built from source.
    pub sources: BTreeMap<String, SourceLocation>,
}

impl LegacyCondaPackageData {
    /// Returns the location of the package.
    pub fn location(&self) -> &UrlOrPath {
        match self {
            Self::Binary(data) => &data.location,
            Self::Source(data) => &data.location,
        }
    }

    /// Returns the package record.
    pub fn record(&self) -> &PackageRecord {
        match self {
            Self::Binary(data) => &data.package_record,
            Self::Source(data) => &data.package_record,
        }
    }

    /// Performs best-effort merge of two packages with the same location.
    ///
    /// This is needed for lock file versions before V6, where multiple packages
    /// could share the same URL but have different metadata. Optional fields
    /// like `purls`, `run_exports`, `md5`, and `sha256` are merged from the
    /// second package if they are missing in the first.
    ///
    /// For V6 and later, this method is typically not called because packages
    /// are matched exactly using name/version/build/subdir fields.
    pub fn merge(&self, other: &Self) -> Cow<'_, Self> {
        match (self, other) {
            (Self::Binary(left), Self::Binary(right)) => {
                if left.location == right.location {
                    if let Cow::Owned(merged) =
                        merge_package_record(&left.package_record, &right.package_record)
                    {
                        return Cow::Owned(Self::Binary(LegacyCondaBinaryData {
                            package_record: merged,
                            location: left.location.clone(),
                            file_name: left.file_name.clone(),
                            channel: left.channel.clone(),
                        }));
                    }
                }
            }
            (Self::Source(left), Self::Source(right)) => {
                if left.location == right.location {
                    let record_merge =
                        merge_package_record(&left.package_record, &right.package_record);
                    let build_source_merge = merge_package_build_source(
                        &left.package_build_source,
                        &right.package_build_source,
                    );

                    if matches!(record_merge, Cow::Owned(_))
                        || matches!(build_source_merge, Cow::Owned(_))
                    {
                        return Cow::Owned(Self::Source(LegacyCondaSourceData {
                            package_record: record_merge.into_owned(),
                            location: left.location.clone(),
                            variants: left.variants.clone(),
                            package_build_source: build_source_merge.into_owned(),
                            sources: left.sources.clone(),
                        }));
                    }
                }
            }
            _ => {}
        }
        Cow::Borrowed(self)
    }
}

/// Convert `LegacyCondaPackageData` to the final `CondaPackageData` type.
///
/// Currently this is a 1:1 conversion. When `CondaPackageData` is updated for
/// V7 (e.g., source packages use hash instead of full `PackageRecord`), this
/// conversion will compute the hash from the legacy data.
impl From<LegacyCondaPackageData> for CondaPackageData {
    fn from(value: LegacyCondaPackageData) -> Self {
        match value {
            LegacyCondaPackageData::Binary(data) => CondaPackageData::Binary(CondaBinaryData {
                package_record: data.package_record,
                location: data.location,
                file_name: data.file_name,
                channel: data.channel,
            }),
            LegacyCondaPackageData::Source(data) => CondaPackageData::Source(CondaSourceData {
                package_record: data.package_record,
                location: data.location,
                variants: data.variants,
                package_build_source: data.package_build_source,
                sources: data.sources,
            }),
        }
    }
}

/// Merge optional fields from `right` into `left` if they are missing.
fn merge_package_record<'a>(
    left: &'a PackageRecord,
    right: &PackageRecord,
) -> Cow<'a, PackageRecord> {
    // Check which fields need to be merged
    let needs_purls = left.purls.is_none() && right.purls.is_some();
    let needs_run_exports = left.run_exports.is_none() && right.run_exports.is_some();
    let needs_md5 = left.md5.is_none() && right.md5.is_some();
    let needs_sha256 = left.sha256.is_none() && right.sha256.is_some();

    // Return borrowed if no merging needed
    if !needs_purls && !needs_run_exports && !needs_md5 && !needs_sha256 {
        return Cow::Borrowed(left);
    }

    // Clone once and update only the fields that need merging
    let mut merged = left.clone();
    if needs_purls {
        merged.purls.clone_from(&right.purls);
    }
    if needs_run_exports {
        merged.run_exports.clone_from(&right.run_exports);
    }
    if needs_md5 {
        merged.md5 = right.md5;
    }
    if needs_sha256 {
        merged.sha256 = right.sha256;
    }
    Cow::Owned(merged)
}

/// Merge package build source, preferring the right value if left is None.
fn merge_package_build_source<'a>(
    left: &'a Option<PackageBuildSource>,
    right: &Option<PackageBuildSource>,
) -> Cow<'a, Option<PackageBuildSource>> {
    if left == right {
        Cow::Borrowed(left)
    } else if let Some(right_source) = right {
        // New data takes precedence
        Cow::Owned(Some(right_source.clone()))
    } else {
        Cow::Borrowed(left)
    }
}
