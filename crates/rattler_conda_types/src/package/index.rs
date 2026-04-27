use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use rattler_macros::sorted;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};
use thiserror::Error;

use super::PackageFile;
use crate::{
    Flag, MatchSpec, NoArchType, PackageName, PackageUrl, ParseMatchSpecError,
    ParseMatchSpecOptions, RepodataRevision, VersionWithSource,
};

/// A representation of the `index.json` file found in package archives.
///
/// The `index.json` file contains information about the package build and
/// dependencies of the package. This data makes up the repodata.json file in
/// the repository.
#[serde_as]
#[sorted]
#[skip_serializing_none]
#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct IndexJson {
    /// Optionally, the architecture the package is build for.
    pub arch: Option<String>,

    /// The build string of the package.
    pub build: String,

    /// The build number of the package. This is also included in the build
    /// string.
    pub build_number: u64,

    /// The package constraints of the package
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constrains: Vec<String>,

    /// The dependencies of the package
    #[serde(default)]
    pub depends: Vec<String>,

    /// Extra dependency groups that can be selected using `foobar[extras=["scientific"]]`
    /// The implementation is specified in this CEP: <https://github.com/conda/ceps/pull/111>
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(rename = "extra_depends")]
    pub experimental_extra_depends: BTreeMap<String, Vec<String>>,

    /// Features are a deprecated way to specify different feature sets for the
    /// conda solver. This is not supported anymore and should not be used.
    /// Instead, `mutex` packages should be used to specify
    /// mutually exclusive features.
    pub features: Option<String>,

    /// Plain string flags used to select package variants.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<Flag>,

    /// Optionally, the license
    pub license: Option<String>,

    /// Optionally, the license family
    pub license_family: Option<String>,

    /// The lowercase name of the package
    pub name: PackageName,

    /// If this package is independent of architecture this field specifies in
    /// what way. See [`NoArchType`] for more information.
    #[serde(skip_serializing_if = "NoArchType::is_none")]
    pub noarch: NoArchType,

    /// Optionally, the OS the package is build for.
    pub platform: Option<String>,

    /// A list of Package URLs identifying this package.
    /// See this CEP: <https://github.com/conda/ceps/pull/63>
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purls: Option<BTreeSet<PackageUrl>>,

    /// Optionally a path within the environment of the site-packages directory.
    /// This field is only present for python interpreter packages.
    /// This field was introduced with <https://github.com/conda/ceps/blob/main/cep-17.md>.
    pub python_site_packages_path: Option<String>,

    /// The repodata revision required by this package record.
    ///
    /// Indexers use this field to decide whether the record can be written to
    /// the legacy `packages` / `packages.conda` maps or must be written to a
    /// newer top-level `vN` map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repodata_revision: Option<RepodataRevision>,

    /// The subdirectory that contains this package
    pub subdir: Option<String>,

    /// The timestamp when this package was created
    pub timestamp: Option<crate::utils::TimestampMs>,

    /// Track features are nowadays only used to downweight packages (ie. give
    /// them less priority). To that effect, the number of track features is
    /// counted (number of commas) and the package is downweighted
    /// by the number of `track_features`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde_as(as = "crate::utils::serde::Features")]
    pub track_features: Vec<String>,

    /// The version of the package
    pub version: VersionWithSource,
}

impl PackageFile for IndexJson {
    fn package_path() -> &'static Path {
        Path::new("info/index.json")
    }

    fn from_str(str: &str) -> Result<Self, std::io::Error> {
        serde_json::from_str(str).map_err(Into::into)
    }

    fn from_slice(slice: &[u8]) -> Result<Self, std::io::Error> {
        serde_json::from_slice(slice).map_err(Into::into)
    }
}

impl IndexJson {
    /// Returns the repodata revision required by this package.
    ///
    /// If the package does not explicitly declare a revision, infer the oldest
    /// revision that can represent the currently known fields.
    pub fn required_repodata_revision(&self) -> RepodataRevision {
        if let Some(revision) = self.repodata_revision {
            return revision;
        }

        if !self.experimental_extra_depends.is_empty() || !self.flags.is_empty() {
            return RepodataRevision::V3;
        }

        let parse_options =
            ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3);
        if self
            .depends
            .iter()
            .chain(self.constrains.iter())
            .any(|spec| matchspec_requires_v3(spec, parse_options))
        {
            RepodataRevision::V3
        } else {
            RepodataRevision::Legacy
        }
    }

    /// Validates that the fields in this `index.json` are representable by its
    /// required repodata revision.
    pub fn validate(&self) -> Result<(), ValidateIndexJsonError> {
        let required_revision = self.required_repodata_revision();
        if matches!(required_revision, RepodataRevision::Legacy)
            && !self.experimental_extra_depends.is_empty()
        {
            return Err(ValidateIndexJsonError::LegacyExtraDepends);
        }

        if matches!(required_revision, RepodataRevision::Legacy) && !self.flags.is_empty() {
            return Err(ValidateIndexJsonError::LegacyFlags);
        }

        for flag in &self.flags {
            if flag.validate().is_err() {
                return Err(ValidateIndexJsonError::InvalidFlag {
                    flag: flag.as_str().to_string(),
                });
            }
        }

        let parse_options =
            ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3);

        for spec in &self.depends {
            Self::validate_matchspec(required_revision, "depends", spec, parse_options)?;
        }

        for spec in &self.constrains {
            Self::validate_matchspec(required_revision, "constrains", spec, parse_options)?;
        }

        for (group, specs) in &self.experimental_extra_depends {
            for spec in specs {
                Self::validate_matchspec(
                    required_revision,
                    format!("extra_depends.{group}"),
                    spec,
                    parse_options,
                )?;
            }
        }

        Ok(())
    }

    fn validate_matchspec(
        required_revision: RepodataRevision,
        field: impl Into<String>,
        spec: &str,
        parse_options: ParseMatchSpecOptions,
    ) -> Result<(), ValidateIndexJsonError> {
        let field = field.into();
        let matchspec = MatchSpec::from_str(spec, parse_options).map_err(|source| {
            ValidateIndexJsonError::InvalidMatchSpec {
                field: field.clone(),
                spec: spec.to_string(),
                source,
            }
        })?;

        if matches!(required_revision, RepodataRevision::Legacy) {
            if matchspec.extras.is_some() {
                return Err(ValidateIndexJsonError::LegacyMatchSpecExtras {
                    field,
                    spec: spec.to_string(),
                });
            }

            if matchspec.condition.is_some() {
                return Err(ValidateIndexJsonError::LegacyMatchSpecCondition {
                    field,
                    spec: spec.to_string(),
                });
            }

            if matchspec.flags.is_some() {
                return Err(ValidateIndexJsonError::LegacyMatchSpecFlags {
                    field,
                    spec: spec.to_string(),
                });
            }
        }

        Ok(())
    }
}

fn matchspec_requires_v3(spec: &str, parse_options: ParseMatchSpecOptions) -> bool {
    MatchSpec::from_str(spec, parse_options)
        .is_ok_and(|matchspec| matchspec.required_repodata_revision() == RepodataRevision::V3)
}

/// An error when validating an [`IndexJson`] value.
#[derive(Debug, Error)]
pub enum ValidateIndexJsonError {
    /// Legacy repodata cannot represent `extra_depends`.
    #[error("legacy repodata cannot represent extra_depends")]
    LegacyExtraDepends,

    /// Legacy repodata cannot represent package flags.
    #[error("legacy repodata cannot represent flags")]
    LegacyFlags,

    /// Legacy repodata cannot represent matchspec extras.
    #[error("legacy repodata cannot represent matchspec extras in {field}: {spec}")]
    LegacyMatchSpecExtras {
        /// The `index.json` field that contains the invalid matchspec.
        field: String,
        /// The invalid matchspec.
        spec: String,
    },

    /// Legacy repodata cannot represent conditional matchspecs.
    #[error("legacy repodata cannot represent conditional matchspecs in {field}: {spec}")]
    LegacyMatchSpecCondition {
        /// The `index.json` field that contains the invalid matchspec.
        field: String,
        /// The invalid matchspec.
        spec: String,
    },

    /// Legacy repodata cannot represent matchspec flags.
    #[error("legacy repodata cannot represent matchspec flags in {field}: {spec}")]
    LegacyMatchSpecFlags {
        /// The `index.json` field that contains the invalid matchspec.
        field: String,
        /// The invalid matchspec.
        spec: String,
    },

    /// A package flag is invalid.
    #[error("invalid package flag: {flag}")]
    InvalidFlag {
        /// The invalid flag.
        flag: String,
    },

    /// A dependency or constraint matchspec could not be parsed.
    #[error("invalid matchspec in {field}: {spec}")]
    InvalidMatchSpec {
        /// The `index.json` field that contains the invalid matchspec.
        field: String,
        /// The invalid matchspec.
        spec: String,
        /// The parse error.
        #[source]
        source: ParseMatchSpecError,
    },
}

#[cfg(test)]
mod test {
    use super::{IndexJson, PackageFile, ValidateIndexJsonError};
    use crate::RepodataRevision;

    #[test]
    pub fn test_required_repodata_revision() {
        let explicit_revision: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "name": "demo",
                "repodata_revision": 3,
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert_eq!(
            explicit_revision.required_repodata_revision(),
            RepodataRevision::V3
        );

        let inferred_revision: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "extra_depends": {
                    "test": ["pytest"]
                },
                "name": "demo",
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert_eq!(
            inferred_revision.required_repodata_revision(),
            RepodataRevision::V3
        );

        let inferred_revision: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "depends": ["foo[extras=[bar]]"],
                "name": "demo",
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert_eq!(
            inferred_revision.required_repodata_revision(),
            RepodataRevision::V3
        );

        let inferred_revision: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "constrains": ["python-tzdata[when=\"__win\"]"],
                "name": "demo",
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert_eq!(
            inferred_revision.required_repodata_revision(),
            RepodataRevision::V3
        );

        let inferred_revision: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "flags": ["cuda", "blas:mkl"],
                "name": "demo",
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert_eq!(
            inferred_revision.required_repodata_revision(),
            RepodataRevision::V3
        );
    }

    #[test]
    pub fn test_validate_legacy_repodata_revision() {
        let extra_depends: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "extra_depends": {
                    "test": ["pytest"]
                },
                "name": "demo",
                "repodata_revision": 0,
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert!(matches!(
            extra_depends.validate(),
            Err(ValidateIndexJsonError::LegacyExtraDepends)
        ));

        let extras_matchspec: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "depends": ["foo[extras=[bar]]"],
                "name": "demo",
                "repodata_revision": 0,
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert!(matches!(
            extras_matchspec.validate(),
            Err(ValidateIndexJsonError::LegacyMatchSpecExtras { .. })
        ));

        let conditional_matchspec: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "depends": ["foo[when=\"python >=3.10\"]"],
                "name": "demo",
                "repodata_revision": 0,
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert!(matches!(
            conditional_matchspec.validate(),
            Err(ValidateIndexJsonError::LegacyMatchSpecCondition { .. })
        ));

        let flags: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "flags": ["cuda"],
                "name": "demo",
                "repodata_revision": 0,
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert!(matches!(
            flags.validate(),
            Err(ValidateIndexJsonError::LegacyFlags)
        ));

        let flags_matchspec: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "depends": ["foo[flags=[cuda]]"],
                "name": "demo",
                "repodata_revision": 0,
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert!(matches!(
            flags_matchspec.validate(),
            Err(ValidateIndexJsonError::LegacyMatchSpecFlags { .. })
        ));
    }

    #[test]
    pub fn test_validate_v3_repodata_revision() {
        let index: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "depends": [
                    "foo[extras=[bar]]",
                    "python-tzdata[when=\"__win\"]",
                    "blas-provider[flags=[blas:*]]"
                ],
                "extra_depends": {
                    "test": ["pytest[when=\"python >=3.10\"]"]
                },
                "flags": ["cuda", "blas:mkl"],
                "name": "demo",
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert_eq!(index.required_repodata_revision(), RepodataRevision::V3);
        index.validate().unwrap();

        let invalid_flags: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "flags": ["CUDA"],
                "name": "demo",
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert!(matches!(
            invalid_flags.validate(),
            Err(ValidateIndexJsonError::InvalidFlag { .. })
        ));
    }

    #[test]
    pub fn test_reconstruct_index_json() {
        let package_dir = tempfile::tempdir().unwrap();
        let package_path = tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/win-64/zlib-1.2.8-vc10_0.tar.bz2"
                .parse()
                .unwrap(),
            "ee9172dbe9ebd158e8e68d6d0f7dc2060f0c8230b44d2e9a3595b7cd7336b915",
        )
        .unwrap();
        rattler_package_streaming::fs::extract(&package_path, package_dir.path()).unwrap();

        insta::assert_yaml_snapshot!(IndexJson::from_package_directory(package_dir.path()).unwrap());
    }

    #[test]
    #[cfg(unix)]
    pub fn test_reconstruct_index_json_with_symlinks() {
        let package_dir = tempfile::tempdir().unwrap();

        let package_path = tools::download_and_cache_file(
            "https://conda.anaconda.org/conda-forge/linux-64/zlib-1.2.8-3.tar.bz2"
                .parse()
                .unwrap(),
            "85fcb6906b8686fe6341db89b4e6fc2631ad69ee6eab2f4823bfd64ae0b20ac8",
        )
        .unwrap();
        rattler_package_streaming::fs::extract(&package_path, package_dir.path()).unwrap();

        let package_dir = package_dir.keep();
        println!("{}", package_dir.display());

        insta::assert_yaml_snapshot!(IndexJson::from_package_directory(&package_dir).unwrap());
    }
}
