#![allow(clippy::option_option)]

use fxhash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, OneOrMany};
use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use crate::{package::ArchiveType, PackageRecord, PackageUrl, RepoData, Shard};

/// Represents a Conda repodata patch.
///
/// This struct contains information about a patch to a Conda repodata file,
/// which is used to update the metadata for Conda packages. The patch contains
/// information about changes to the repodata, such as removed (yanked) packages and updated package
/// metadata.
#[derive(Debug, Default, Clone)]
pub struct RepoDataPatch {
    /// The patches to apply for each subdir
    pub subdirs: FxHashMap<String, PatchInstructions>,
}

impl RepoDataPatch {
    /// Load repodata patches from an extracted repodata patches package archive.
    pub fn from_package(package: impl AsRef<Path>) -> io::Result<Self> {
        let mut subdirs = FxHashMap::default();

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

/// Contains items that overwrite metadata values stored for a single [`super::PackageRecord`].
///
/// Not all entries of a [`super::PackageRecord`] can be overwritten because that would cause
/// difficult to solve inconsistencies. For instance, changing the sha256 hash of a file could
/// break local caches because usually the data is cached by filename. With this struct we
/// explicitly define which fields of a [`super::PackageRecord`] can be modified through repodata
/// patches.
#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct PackageRecordPatch {
    /// Specification of packages this package depends on
    pub depends: Option<Vec<String>>,

    /// Additional constraints on packages. `constrains` are different from `depends` in that packages
    /// specified in `depends` must be installed next to this package, whereas packages specified in
    /// `constrains` are not required to be installed, but if they are installed they must follow these
    /// constraints.
    pub constrains: Option<Vec<String>>,

    /// Track features are nowadays only used to downweight packages (ie. give them less priority). To
    /// that effect, the number of track features is counted (number of commas) and the package is downweighted
    /// by the number of track_features.
    #[serde_as(as = "Option<OneOrMany<_>>")]
    pub track_features: Option<Vec<String>>,

    /// Features are a deprecated way to specify different feature sets for the conda solver. This is not
    /// supported anymore and should not be used. Instead, `mutex` packages should be used to specify
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

    /// Package identifiers of packages that are equivalent to this package but from other
    /// ecosystems.
    pub purls: Option<BTreeSet<PackageUrl>>,
}

/// Repodata patch instructions for a single subdirectory. See [`RepoDataPatch`] for more
/// information.
#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone)]
pub struct PatchInstructions {
    /// Filenames that have been removed from the subdirectory
    #[serde(default, skip_serializing_if = "FxHashSet::is_empty")]
    pub remove: FxHashSet<String>,

    /// Patches for package records
    #[serde(default, skip_serializing_if = "FxHashMap::is_empty")]
    pub packages: FxHashMap<String, PackageRecordPatch>,

    /// Patches for package records
    #[serde(
        default,
        rename = "packages.conda",
        skip_serializing_if = "FxHashMap::is_empty"
    )]
    pub conda_packages: FxHashMap<String, PackageRecordPatch>,
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
            self.track_features = track_features.clone();
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
            self.purls = Some(package_urls.clone());
        }
    }
}

/// Apply a patch to a repodata file
/// Note that we currently do not handle `revoked` instructions
pub fn apply_patches_impl(
    packages: &mut FxHashMap<String, PackageRecord>,
    conda_packages: &mut FxHashMap<String, PackageRecord>,
    removed: &mut FxHashSet<String>,
    instructions: &PatchInstructions,
) {
    for (pkg, patch) in instructions.packages.iter() {
        if let Some(record) = packages.get_mut(pkg) {
            record.apply_patch(patch);
        }

        // also apply the patch to the conda packages
        if let Some((pkg_name, archive_type)) = ArchiveType::split_str(pkg) {
            assert!(archive_type == ArchiveType::TarBz2);
            if let Some(record) = conda_packages.get_mut(&format!("{pkg_name}.conda")) {
                record.apply_patch(patch);
            }
        }
    }

    for (pkg, patch) in instructions.conda_packages.iter() {
        if let Some(record) = conda_packages.get_mut(pkg) {
            record.apply_patch(patch);
        }
    }

    // remove packages that have been removed
    for pkg in instructions.remove.iter() {
        if let Some((pkg_name, archive_type)) = ArchiveType::split_str(pkg) {
            match archive_type {
                ArchiveType::TarBz2 => {
                    if packages.remove_entry(pkg).is_some() {
                        removed.insert(pkg.clone());
                    }

                    // also remove equivalent .conda package if it exists
                    let conda_pkg_name = format!("{pkg_name}.conda");
                    if conda_packages.remove_entry(&conda_pkg_name).is_some() {
                        removed.insert(conda_pkg_name);
                    }
                }
                ArchiveType::Conda => {
                    if conda_packages.remove_entry(pkg).is_some() {
                        removed.insert(pkg.clone());
                    }
                }
            }
        }
    }
}

impl RepoData {
    /// Apply a patch to a repodata file
    /// Note that we currently do not handle `revoked` instructions
    pub fn apply_patches(&mut self, instructions: &PatchInstructions) {
        apply_patches_impl(
            &mut self.packages,
            &mut self.conda_packages,
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
            serde_json::from_str(r#"{"features": null, "license": null, "license_family": null, "depends": [], "constrains": [], "track_features": []}"#).unwrap();
        insta::assert_yaml_snapshot!(record_patch);
    }

    fn test_data_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/channels/patch")
    }

    fn load_test_repodata() -> RepoData {
        let repodata_path = test_data_path().join("linux-64/repodata_from_packages.json");
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
    fn test_patching() {
        // test data
        let mut repodata = load_test_repodata();
        let patch_instructions = load_patch_instructions("patch_instructions.json");

        // apply patch
        repodata.apply_patches(&patch_instructions);

        // check result
        insta::assert_yaml_snapshot!(repodata);
    }

    #[test]
    fn test_removing_1() {
        // test data
        let mut repodata = load_test_repodata();
        let patch_instructions = load_patch_instructions("patch_instructions_2.json");

        // apply patch
        repodata.apply_patches(&patch_instructions);

        // check result
        insta::assert_yaml_snapshot!(repodata);
    }

    #[test]
    fn test_removing_2() {
        // test data
        let mut repodata = load_test_repodata();
        let patch_instructions = load_patch_instructions("patch_instructions_3.json");

        // apply patch
        repodata.apply_patches(&patch_instructions);

        // check result
        insta::assert_yaml_snapshot!(repodata);
    }

    #[test]
    fn test_patch_purl() {
        // test data
        let mut repodata = load_test_repodata();
        let patch_instructions = load_patch_instructions("patch_instructions_4.json");

        // apply patch
        repodata.apply_patches(&patch_instructions);

        // check result
        insta::assert_yaml_snapshot!(repodata);
    }
}
