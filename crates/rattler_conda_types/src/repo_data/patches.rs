use fxhash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none, OneOrMany};
use std::io;
use std::path::Path;

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

#[cfg(test)]
mod test {
    #[test]
    fn test_null_values() {
        let record_patch: super::PackageRecordPatch =
            serde_json::from_str(r#"{"features": null, "license": null, "license_family": null, "depends": [], "constrains": [], "track_features": []}"#).unwrap();
        insta::assert_yaml_snapshot!(record_patch);
    }
}
