//! Configuration of the package cache: additional cache layers that are
//! queried before the tool's own package cache.
//!
//! ```toml
//! [package-cache]
//! validation = "fast"
//!
//! # A shared cache, e.g. on CVMFS or NFS, that is never written to.
//! [[package-cache.layers]]
//! path = "/cvmfs/software/conda-pkgs"
//! read-only = true
//!
//! # A team cache that only serves (and stores) a subset of packages.
//! [[package-cache.layers]]
//! path = "/shared/team/pkgs"
//! include = ["pytorch", "cuda-toolkit"]
//! exclude = ["pytorch *cpu*"]
//! validation = "full"
//! ```
//!
//! Layers are queried in the order they are listed, followed by the tool's
//! default package cache. A package that is not found is written to the first
//! layer that accepts it and is writable.

use std::path::PathBuf;

use rattler_conda_types::MatchSpec;
use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, serde_as};

use crate::config::{Config, MergeError, ValidationError};

/// How thoroughly the contents of a cached package are checked before it is
/// used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageCacheValidation {
    /// Only check that the package directory exists and contains a valid
    /// `index.json`.
    Skip,
    /// Check that all files of the package exist.
    Fast,
    /// Check that all files of the package exist and their hashes match.
    Full,
}

/// One additional layer of the package cache.
#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PackageCacheLayerConfig {
    /// The root directory of the layer.
    pub path: PathBuf,

    /// Never write to this layer, even when the filesystem permits it.
    /// Layers on a read-only filesystem are detected automatically.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,

    /// Only packages matching one of these specs are read from or written to
    /// this layer. Empty means every package.
    #[serde_as(as = "Vec<DisplayFromStr>")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<MatchSpec>,

    /// Packages matching one of these specs are never read from or written
    /// to this layer. Takes precedence over `include`.
    #[serde_as(as = "Vec<DisplayFromStr>")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<MatchSpec>,

    /// Overrides the validation of the `package-cache` section for this
    /// layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<PackageCacheValidation>,
}

/// The `package-cache` section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PackageCacheConfig {
    /// The validation used for every layer that does not set its own,
    /// including the tool's default package cache. Unset means the tool's
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<PackageCacheValidation>,

    /// Layers queried, in order, before the tool's default package cache.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<PackageCacheLayerConfig>,
}

impl PackageCacheConfig {
    /// Whether nothing is configured.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

impl Config for PackageCacheConfig {
    /// `other` wins per key. Its `layers`, when non-empty, replace these
    /// layers as a whole, because the order of the layers matters.
    fn merge_config(self, other: &Self) -> Result<Self, MergeError> {
        Ok(Self {
            validation: other.validation.or(self.validation),
            layers: if other.layers.is_empty() {
                self.layers
            } else {
                other.layers.clone()
            },
        })
    }

    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(layer) = self
            .layers
            .iter()
            .find(|layer| layer.path.as_os_str().is_empty())
        {
            return Err(ValidationError::InvalidValue(
                "package-cache.layers.path".to_string(),
                format!("a layer must have a non-empty path ({layer:?})"),
            ));
        }
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        vec!["validation".to_string(), "layers".to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> PackageCacheConfig {
        toml::from_str(toml).unwrap()
    }

    #[test]
    fn parses_layers() {
        let config = parse(
            r#"
validation = "fast"

[[layers]]
path = "/cvmfs/pkgs"
read-only = true

[[layers]]
path = "/team/pkgs"
include = ["pytorch >=2", "cuda-toolkit"]
exclude = ["pytorch *cpu*"]
validation = "full"
"#,
        );
        assert_eq!(config.validation, Some(PackageCacheValidation::Fast));
        let [shared, team] = config.layers.as_slice() else {
            panic!("expected two layers");
        };
        assert_eq!(shared.path, PathBuf::from("/cvmfs/pkgs"));
        assert!(shared.read_only);
        assert!(shared.include.is_empty());
        assert_eq!(shared.validation, None);
        assert!(!team.read_only);
        assert_eq!(
            team.include,
            ["pytorch >=2", "cuda-toolkit"].map(|s| s.parse::<MatchSpec>().unwrap())
        );
        assert_eq!(team.exclude.len(), 1);
        assert_eq!(team.validation, Some(PackageCacheValidation::Full));

        let rendered = toml::to_string(&config).unwrap();
        assert_eq!(parse(&rendered), config);
    }

    #[test]
    fn rejects_invalid_specs_and_modes() {
        assert!(toml::from_str::<PackageCacheConfig>("validation = \"thorough\"").is_err());
        assert!(
            toml::from_str::<PackageCacheConfig>(
                r#"
[[layers]]
path = "/pkgs"
include = ["python >=>3"]
"#
            )
            .is_err()
        );
    }

    #[test]
    fn merge_replaces_layers_as_a_whole() {
        let system = parse(
            r#"
validation = "full"
[[layers]]
path = "/a"
[[layers]]
path = "/b"
"#,
        );
        let user = parse(
            r#"
[[layers]]
path = "/c"
"#,
        );
        let merged = system.clone().merge_config(&user).unwrap();
        assert_eq!(merged.validation, Some(PackageCacheValidation::Full));
        assert_eq!(merged.layers, user.layers);

        let merged = system
            .clone()
            .merge_config(&parse("validation = \"skip\""))
            .unwrap();
        assert_eq!(merged.validation, Some(PackageCacheValidation::Skip));
        assert_eq!(merged.layers, system.layers);
    }
}
