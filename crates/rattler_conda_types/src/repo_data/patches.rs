#![allow(clippy::option_option)]

use std::{collections::BTreeSet, io, path::Path};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};

use crate::{
    PackageRecord, PackageUrl, RepoData, Shard, V3Extensions,
    package::{ArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier, DistArchiveType},
};

/// Represents a Conda repodata patch.
///
/// This struct contains information about a patch to a Conda repodata file,
/// which is used to update the metadata for Conda packages. The patch contains
/// information about changes to the repodata, such as removed (yanked) packages
/// and updated package metadata.
#[derive(Debug, Default, Clone)]
pub struct RepoDataPatch {
    /// The patches to apply for each subdir
    pub subdirs: ahash::HashMap<String, PatchInstructions>,
}

impl RepoDataPatch {
    /// Load repodata patches from an extracted repodata patches package
    /// archive.
    pub fn from_package(package: impl AsRef<Path>) -> io::Result<Self> {
        let mut subdirs = ahash::HashMap::default();

        // Iterate over all directories in the package
        for entry in std::fs::read_dir(package)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            let path = entry.path();
            let patch_instructions_path = path.join("patch_instructions.json");
            if patch_instructions_path.is_file() {
                let contents = std::fs::read_to_string(&patch_instructions_path)?;
                let instructions = serde_json::from_str(&contents)?;
                subdirs.insert(name.to_string(), instructions);
            }
        }

        Ok(Self { subdirs })
    }
}

/// Contains items that overwrite metadata values stored for a single
/// [`super::PackageRecord`].
///
/// Not all entries of a [`super::PackageRecord`] can be overwritten because
/// that would cause difficult to solve inconsistencies. For instance, changing
/// the sha256 hash of a file could break local caches because usually the data
/// is cached by filename. With this struct we explicitly define which fields of
/// a [`super::PackageRecord`] can be modified through repodata patches.
#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct PackageRecordPatch {
    /// Specification of packages this package depends on
    pub depends: Option<Vec<String>>,

    /// Additional constraints on packages. `constrains` are different from
    /// `depends` in that packages specified in `depends` must be installed
    /// next to this package, whereas packages specified in `constrains` are
    /// not required to be installed, but if they are installed they must follow
    /// these constraints.
    pub constrains: Option<Vec<String>>,

    /// Track features are nowadays only used to downweight packages (ie. give
    /// them less priority). To that effect, the number of track features is
    /// counted (number of commas) and the package is downweighted
    /// by the number of `track_features`.
    ///
    /// This field is double wrapped to be able to express the value `null`
    /// separate from it being missing from the JSON.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "self::tracked_features"
    )]
    pub track_features: Option<Option<Vec<String>>>,

    /// Features are a deprecated way to specify different feature sets for the
    /// conda solver. This is not supported anymore and should not be used.
    /// Instead, `mutex` packages should be used to specify
    /// mutually exclusive features.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "::serde_with::rust::double_option"
    )]
    pub features: Option<Option<String>>,

    /// The specific license of the package
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "::serde_with::rust::double_option"
    )]
    pub license: Option<Option<String>>,

    /// The license family
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "::serde_with::rust::double_option"
    )]
    pub license_family: Option<Option<String>>,

    /// Package identifiers of packages that are equivalent to this package but
    /// from other ecosystems.
    /// See this CEP: <https://github.com/conda/ceps/pull/63>
    ///
    /// This field is double wrapped to be able to express the value `null`
    /// separate from it being missing from the JSON.
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub purls: Option<Option<BTreeSet<PackageUrl>>>,
}

mod tracked_features {
    //! Helper module to serialize and deserialize the `track_features` field
    //! and also allow it to be set to `null`.

    use serde::{Deserializer, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    use crate::utils::serde::Features;

    /// Deserialize potentially non-existing optional value
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Option<Vec<String>>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<Features>::deserialize_as(deserializer).map(Some)
    }

    /// Serialize optional value
    pub fn serialize<S>(
        value: &Option<Option<Vec<String>>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            None => serializer.serialize_unit(),
            Some(v) => Option::<Features>::serialize_as(v, serializer),
        }
    }
}

/// Patches for packages stored under the `v3` top-level key.
/// Mirrors [`V3Packages`] but with [`PackageRecordPatch`] values.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone, Default)]
pub struct V3PackagePatches {
    /// Patches for v3 tar.bz2 package records
    #[serde(
        default,
        rename = "tar.bz2",
        skip_serializing_if = "ahash::HashMap::is_empty"
    )]
    pub tar_bz2: ahash::HashMap<ArchiveIdentifier, PackageRecordPatch>,

    /// Patches for v3 conda package records
    #[serde(default, skip_serializing_if = "ahash::HashMap::is_empty")]
    pub conda: ahash::HashMap<ArchiveIdentifier, PackageRecordPatch>,

    /// Patches for v3 whl package records
    #[serde(default, skip_serializing_if = "ahash::HashMap::is_empty")]
    pub whl: ahash::HashMap<ArchiveIdentifier, PackageRecordPatch>,

    /// Patches for archive extensions not yet modeled by rattler.
    ///
    /// These are preserved without interpretation so patch files do not lose
    /// data for newer repodata revisions.
    #[serde(flatten, default, skip_serializing_if = "V3Extensions::is_empty")]
    pub extensions: V3Extensions,
}

impl V3PackagePatches {
    /// Returns true if all sub-maps are empty.
    pub fn is_empty(&self) -> bool {
        self.tar_bz2.is_empty()
            && self.conda.is_empty()
            && self.whl.is_empty()
            && self.extensions.is_empty()
    }
}

/// Repodata patch instructions for a single subdirectory. See [`RepoDataPatch`]
/// for more information.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct PatchInstructions {
    /// Filenames that have been removed from the subdirectory
    #[serde(default, skip_serializing_if = "ahash::HashSet::is_empty")]
    pub remove: ahash::HashSet<DistArchiveIdentifier>,

    /// Patches for package records
    #[serde(default, skip_serializing_if = "ahash::HashMap::is_empty")]
    pub packages: ahash::HashMap<DistArchiveIdentifier, PackageRecordPatch>,

    /// Patches for conda package records
    #[serde(
        default,
        rename = "packages.conda",
        skip_serializing_if = "ahash::HashMap::is_empty"
    )]
    pub conda_packages: ahash::HashMap<DistArchiveIdentifier, PackageRecordPatch>,

    /// Patches for v3 packages
    #[serde(default, skip_serializing_if = "V3PackagePatches::is_empty")]
    pub v3: V3PackagePatches,
}

impl PackageRecord {
    /// Apply a patch to a single package record
    pub fn apply_patch(&mut self, patch: &PackageRecordPatch) {
        if let Some(depends) = &patch.depends {
            self.depends = depends.clone();
        }
        if let Some(constrains) = &patch.constrains {
            self.constrains = constrains.clone();
        }
        if let Some(track_features) = &patch.track_features {
            self.track_features = track_features.clone().unwrap_or_default();
        }
        if let Some(features) = &patch.features {
            self.features = features.clone();
        }
        if let Some(license) = &patch.license {
            self.license = license.clone();
        }
        if let Some(license_family) = &patch.license_family {
            self.license_family = license_family.clone();
        }
        if let Some(package_urls) = &patch.purls {
            self.purls = package_urls.clone();
        }
    }
}

/// Apply a patch to a repodata file
/// Note that we currently do not handle `revoked` instructions
pub fn apply_patches_impl(
    packages: &mut IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,
    conda_packages: &mut IndexMap<DistArchiveIdentifier, PackageRecord, ahash::RandomState>,
    v3: &mut super::V3Packages,
    removed: &mut ahash::HashSet<DistArchiveIdentifier>,
    instructions: &PatchInstructions,
) {
    for (identifier, patch) in instructions.packages.iter() {
        if let Some(record) = packages.get_mut(identifier) {
            record.apply_patch(patch);
        }

        let conda_identifier = identifier.with_archive_type(CondaArchiveType::Conda.into());
        if let Some(record) = conda_packages.get_mut(&conda_identifier) {
            record.apply_patch(patch);
        }
    }

    for (identifier, patch) in instructions.conda_packages.iter() {
        if let Some(record) = conda_packages.get_mut(identifier) {
            record.apply_patch(patch);
        }
    }

    // Apply patches to v3 packages
    for (identifier, patch) in instructions.v3.tar_bz2.iter() {
        if let Some(record) = v3.tar_bz2.get_mut(identifier) {
            record.apply_patch(patch);
        }
    }
    for (identifier, patch) in instructions.v3.conda.iter() {
        if let Some(record) = v3.conda.get_mut(identifier) {
            record.apply_patch(patch);
        }
    }
    for (identifier, patch) in instructions.v3.whl.iter() {
        if let Some(record) = v3.whl.get_mut(identifier) {
            record.package_record.apply_patch(patch);
        }
    }
    for (extension, patch) in instructions.v3.extensions.iter() {
        if patch.is_null() {
            v3.extensions.remove(extension);
        } else if let Some(bucket) = v3.extensions.get_mut(extension) {
            apply_extension_patch(bucket, patch);
        } else {
            // `V3Extensions` rejects reserved names on construction, so an
            // extension deserialized from patch instructions is safe to add.
            let mut bucket = serde_json::Value::Null;
            apply_extension_patch(&mut bucket, patch);
            v3.extensions
                .insert(extension.clone(), bucket)
                .expect("v3 extension patch used a reserved bucket name");
        }
    }

    // Mark packages as removed. Note: we only add them to the `removed` set
    // and do NOT remove them from the package maps. The `removed` key signals
    // that a package has been yanked, but its metadata should remain available.
    for identifier in instructions.remove.iter() {
        match identifier.archive_type {
            DistArchiveType::Conda(CondaArchiveType::TarBz2) => {
                removed.insert(identifier.clone());

                // also mark equivalent .conda package as removed if it exists
                let conda_identifier = identifier.with_archive_type(CondaArchiveType::Conda.into());
                if conda_packages.contains_key(&conda_identifier) {
                    removed.insert(conda_identifier);
                }
            }
            DistArchiveType::Conda(CondaArchiveType::Conda) => {
                removed.insert(identifier.clone());
            }
            DistArchiveType::Wheel(_) => {
                // Wheel packages are stored in v3.whl; removal by
                // DistArchiveIdentifier is not currently supported for v3.
            }
        }
    }
}

/// Applies an unknown extension patch using JSON Merge Patch semantics.
///
/// Extension schemas are intentionally opaque, so recursively merging JSON
/// objects preserves untouched data while letting patch instructions update
/// individual fields. As specified by RFC 7396, an object patch treats a
/// non-object or absent target as an empty object. Non-object values are
/// replaced wholesale.
fn apply_extension_patch(target: &mut serde_json::Value, patch: &serde_json::Value) {
    if let Some(patch) = patch.as_object() {
        if !target.is_object() {
            *target = serde_json::Value::Object(serde_json::Map::new());
        }

        let target = target
            .as_object_mut()
            .expect("target was made into an object");
        for (key, patch_value) in patch {
            if patch_value.is_null() {
                target.remove(key);
            } else {
                let target_value = target.entry(key.clone()).or_insert(serde_json::Value::Null);
                apply_extension_patch(target_value, patch_value);
            }
        }
    } else {
        *target = patch.clone();
    }
}

impl RepoData {
    /// Apply a patch to a repodata file
    /// Note that we currently do not handle `revoked` instructions
    pub fn apply_patches(&mut self, instructions: &PatchInstructions) {
        apply_patches_impl(
            &mut self.packages,
            &mut self.conda_packages,
            &mut self.v3,
            &mut self.removed,
            instructions,
        );
    }
}

impl Shard {
    /// Apply a patch to a shard
    /// Note that we currently do not handle `revoked` instructions
    pub fn apply_patches(&mut self, instructions: &PatchInstructions) {
        apply_patches_impl(
            &mut self.packages,
            &mut self.conda_packages,
            &mut self.v3,
            &mut self.removed,
            instructions,
        );
    }
}

#[cfg(test)]
mod test {
    use crate::{PatchInstructions, RepoData};

    #[test]
    fn test_null_values() {
        let record_patch: super::PackageRecordPatch =
            serde_json::from_str(r#"{"features": null, "license": null, "license_family": null, "depends": [], "constrains": [], "track_features": null}"#).unwrap();
        insta::assert_yaml_snapshot!(record_patch);
    }

    fn test_data_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/channels/patch")
    }

    fn load_test_repodata(name: &str) -> RepoData {
        let repodata_path = test_data_path().join("linux-64").join(name);
        let repodata: RepoData =
            serde_json::from_str(&std::fs::read_to_string(repodata_path).unwrap()).unwrap();
        repodata
    }

    fn load_patch_instructions(name: &str) -> PatchInstructions {
        let patch_instructions_path = test_data_path().join("linux-64").join(name);
        let patch_instructions = std::fs::read_to_string(patch_instructions_path).unwrap();
        let patch_instructions: PatchInstructions =
            serde_json::from_str(&patch_instructions).unwrap();
        patch_instructions
    }

    #[test]
    fn test_v3_extension_patches_are_applied() {
        let raw = serde_json::json!({
            "packages": {},
            "packages.conda": {},
            "v3": {
                "conda": {
                    "demo-1.0-0": {
                        "build": "0",
                        "build_number": 0,
                        "depends": ["original"],
                        "name": "demo",
                        "subdir": "noarch",
                        "version": "1.0"
                    }
                },
                "zip": {
                    "demo-1.0-0": {
                        "future_field": "original",
                        "remove_me": true,
                        "nested": { "keep": true, "remove_me": true }
                    }
                },
                "removed-extension": { "old": true }
            },
            "repodata_version": 1
        });
        let mut repodata: RepoData = serde_json::from_value(raw).unwrap();
        let patch = serde_json::json!({
            "v3": {
                "conda": {
                    "demo-1.0-0": { "depends": ["patched"] }
                },
                "zip": {
                    "demo-1.0-0": {
                        "future_field": "patched",
                        "remove_me": null,
                        "nested": { "added": true, "remove_me": null }
                    }
                },
                "new-extension": { "new": true },
                "removed-extension": null
            }
        });
        let instructions: PatchInstructions = serde_json::from_value(patch.clone()).unwrap();

        assert_eq!(
            instructions.v3.extensions.get("zip"),
            Some(&patch["v3"]["zip"])
        );
        assert_eq!(serde_json::to_value(&instructions).unwrap(), patch);

        repodata.apply_patches(&instructions);
        assert_eq!(
            repodata.v3.conda.values().next().unwrap().depends,
            vec!["patched"]
        );
        assert_eq!(
            repodata.v3.extensions.get("zip"),
            Some(&serde_json::json!({
                "demo-1.0-0": {
                    "future_field": "patched",
                    "nested": { "keep": true, "added": true }
                }
            }))
        );
        assert_eq!(
            repodata.v3.extensions.get("new-extension"),
            Some(&serde_json::json!({ "new": true }))
        );
        assert!(repodata.v3.extensions.get("removed-extension").is_none());
    }

    #[test]
    fn test_v3_extension_patches_apply_merge_patch_to_absent_values() {
        let mut repodata: RepoData = serde_json::from_value(serde_json::json!({
            "packages": {},
            "packages.conda": {},
            "v3": {},
            "repodata_version": 1
        }))
        .unwrap();
        let instructions: PatchInstructions = serde_json::from_value(serde_json::json!({
            "v3": {
                "zip": {
                    "discard_from_new_bucket": null,
                    "metadata": {
                        "discard_from_new_object": null,
                        "preserve": true
                    }
                }
            }
        }))
        .unwrap();

        repodata.apply_patches(&instructions);

        // RFC 7396 treats a missing target as an empty object before applying
        // an object patch, so null values must not be retained in new buckets.
        assert_eq!(
            repodata.v3.extensions.get("zip"),
            Some(&serde_json::json!({
                "metadata": { "preserve": true }
            }))
        );
    }

    #[test]
    fn test_v3_extension_patches_replace_scalar_and_object_buckets() {
        let mut repodata: RepoData = serde_json::from_value(serde_json::json!({
            "packages": {},
            "packages.conda": {},
            "v3": {
                "scalar-target": "before",
                "object-target": {
                    "keep": true,
                    "nested": { "keep": true, "remove": true }
                }
            },
            "repodata_version": 1
        }))
        .unwrap();
        let instructions: PatchInstructions = serde_json::from_value(serde_json::json!({
            "v3": {
                "scalar-target": {
                    "discard_from_new_object": null,
                    "nested": {
                        "discard_from_new_nested_object": null,
                        "keep": true
                    }
                },
                "object-target": ["replacement"],
                "absent-scalar": false
            }
        }))
        .unwrap();

        repodata.apply_patches(&instructions);

        // Object patches turn scalar and absent targets into empty objects,
        // and nulls remove keys at every depth. Scalar patches replace an
        // existing object wholesale.
        assert_eq!(
            repodata.v3.extensions.get("scalar-target"),
            Some(&serde_json::json!({ "nested": { "keep": true } }))
        );
        assert_eq!(
            repodata.v3.extensions.get("object-target"),
            Some(&serde_json::json!(["replacement"]))
        );
        assert_eq!(
            repodata.v3.extensions.get("absent-scalar"),
            Some(&serde_json::json!(false))
        );
    }

    #[test]
    fn test_patching() {
        // test data
        let mut repodata = load_test_repodata("repodata_from_packages.json");
        let patch_instructions = load_patch_instructions("patch_instructions.json");

        // apply patch
        repodata.apply_patches(&patch_instructions);

        // check result
        insta::assert_yaml_snapshot!(repodata);
    }

    #[test]
    fn test_removing_1() {
        // test data
        let mut repodata = load_test_repodata("repodata_from_packages.json");
        let patch_instructions = load_patch_instructions("patch_instructions_2.json");

        // apply patch
        repodata.apply_patches(&patch_instructions);

        // check result
        insta::assert_yaml_snapshot!(repodata);
    }

    #[test]
    fn test_removing_2() {
        // test data
        let mut repodata = load_test_repodata("repodata_from_packages.json");
        let patch_instructions = load_patch_instructions("patch_instructions_3.json");

        // apply patch
        repodata.apply_patches(&patch_instructions);

        // check result
        insta::assert_yaml_snapshot!(repodata);
    }

    #[test]
    fn test_patch_purl() {
        // test data
        let mut repodata = load_test_repodata("repodata_from_packages.json");
        let patch_instructions = load_patch_instructions("patch_instructions_4.json");

        // apply patch
        repodata.apply_patches(&patch_instructions);

        // check result
        insta::assert_yaml_snapshot!(repodata);
    }

    #[test]
    fn test_patch_tracked_features() {
        // test data
        let mut repodata = load_test_repodata("repodata_from_packages_5.json");
        let patch_instructions = load_patch_instructions("patch_instructions_5.json");

        // apply patch
        repodata.apply_patches(&patch_instructions);

        // check result
        insta::assert_yaml_snapshot!(repodata);
    }
}
