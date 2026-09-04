use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use rattler_macros::sorted;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, skip_serializing_none};
use thiserror::Error;

use super::{BuildString, PackageFile};
use crate::{
    CanonicalMatchSpecError, Flag, MatchSpec, NoArchType, PackageName, PackageUrl,
    ParseMatchSpecError, ParseMatchSpecOptions, RepodataRevision, VersionWithSource,
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
    pub build: BuildString,

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
    pub extra_depends: BTreeMap<String, Vec<String>>,

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

        if !self.flags.is_empty() {
            return RepodataRevision::V3;
        }

        let parse_options =
            ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3);
        if self
            .depends
            .iter()
            .chain(self.constrains.iter())
            .chain(self.extra_depends.values().flatten())
            .any(|spec| matchspec_requires_v3(spec, parse_options))
        {
            RepodataRevision::V3
        } else {
            RepodataRevision::Legacy
        }
    }

    /// Validates that the fields in this `index.json` are representable by its
    /// required repodata revision.
    ///
    /// Prefer [`Self::into_validated`] when the parsed `MatchSpecs` are needed
    /// afterwards, such as while indexing a package.
    pub fn validate(&self) -> Result<(), ValidateIndexJsonError> {
        self.clone().into_validated().map(|_| ())
    }

    /// Validates this `index.json` and retains its parsed `MatchSpecs`.
    ///
    /// This avoids parsing dependency specifications again when an indexer must
    /// render them for the effective repodata revision.
    pub fn into_validated(self) -> Result<ValidatedIndexJson, ValidateIndexJsonError> {
        let parse_options =
            ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3);
        let depends = parse_matchspecs("depends", &self.depends, parse_options)?;
        let constrains = parse_matchspecs("constrains", &self.constrains, parse_options)?;
        let extra_depends = self
            .extra_depends
            .iter()
            .map(|(group, specs)| {
                parse_matchspecs(format!("extra_depends.{group}"), specs, parse_options)
                    .map(|parsed| (group.clone(), parsed))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;

        let required_revision = self.repodata_revision.unwrap_or_else(|| {
            if !self.flags.is_empty()
                || depends
                    .iter()
                    .chain(constrains.iter())
                    .chain(extra_depends.values().flatten())
                    .any(|spec| spec.required_repodata_revision() == RepodataRevision::V3)
            {
                RepodataRevision::V3
            } else {
                RepodataRevision::Legacy
            }
        });

        if required_revision.uses_legacy_package_layout() && !self.flags.is_empty() {
            return Err(ValidateIndexJsonError::LegacyFlags);
        }
        for flag in &self.flags {
            if flag.validate().is_err() {
                return Err(ValidateIndexJsonError::InvalidFlag {
                    flag: flag.as_str().to_string(),
                });
            }
        }

        if required_revision.uses_legacy_package_layout() {
            for (field, specs) in std::iter::once(("depends".to_string(), &depends))
                .chain(std::iter::once(("constrains".to_string(), &constrains)))
                .chain(
                    extra_depends
                        .iter()
                        .map(|(group, specs)| (format!("extra_depends.{group}"), specs)),
                )
            {
                for spec in specs {
                    validate_legacy_matchspec(&field, spec)?;
                }
            }
        }

        Ok(ValidatedIndexJson {
            index: self,
            required_revision,
            matchspecs: ValidatedMatchSpecs {
                depends,
                constrains,
                extra_depends,
            },
        })
    }
}

/// An `index.json` whose dependency `MatchSpecs` have been parsed and validated.
#[derive(Debug, Clone)]
pub struct ValidatedIndexJson {
    index: IndexJson,
    required_revision: RepodataRevision,
    matchspecs: ValidatedMatchSpecs,
}

impl ValidatedIndexJson {
    /// Returns the repodata revision required by this package.
    pub fn required_repodata_revision(&self) -> RepodataRevision {
        self.required_revision
    }

    /// Splits the validated metadata into the original index record and its
    /// parsed dependency specifications.
    pub fn into_parts(self) -> (IndexJson, ValidatedMatchSpecs) {
        (self.index, self.matchspecs)
    }
}

/// Parsed dependency `MatchSpecs` retained after validating an `index.json`.
#[derive(Debug, Clone)]
pub struct ValidatedMatchSpecs {
    depends: Vec<MatchSpec>,
    constrains: Vec<MatchSpec>,
    extra_depends: BTreeMap<String, Vec<MatchSpec>>,
}

impl ValidatedMatchSpecs {
    /// Renders the validated `MatchSpecs` for a repodata revision.
    pub fn render_for_revision(
        self,
        revision: RepodataRevision,
    ) -> Result<RenderedMatchSpecs, CanonicalMatchSpecError> {
        let render = |spec: MatchSpec| {
            if revision.as_u64() >= RepodataRevision::V3.as_u64() {
                spec.to_canonical_string()
            } else {
                Ok(spec.to_string())
            }
        };

        Ok(RenderedMatchSpecs {
            depends: self
                .depends
                .into_iter()
                .map(&render)
                .collect::<Result<_, _>>()?,
            constrains: self
                .constrains
                .into_iter()
                .map(&render)
                .collect::<Result<_, _>>()?,
            extra_depends: self
                .extra_depends
                .into_iter()
                .map(|(group, specs)| {
                    specs
                        .into_iter()
                        .map(&render)
                        .collect::<Result<_, _>>()
                        .map(|rendered| (group, rendered))
                })
                .collect::<Result<_, _>>()?,
        })
    }
}

/// Dependency `MatchSpecs` rendered for a repodata record.
#[derive(Debug, Clone)]
pub struct RenderedMatchSpecs {
    /// Rendered package dependencies.
    pub depends: Vec<String>,
    /// Rendered package constraints.
    pub constrains: Vec<String>,
    /// Rendered optional dependency groups.
    pub extra_depends: BTreeMap<String, Vec<String>>,
}

fn parse_matchspecs(
    field: impl Into<String>,
    specs: &[String],
    parse_options: ParseMatchSpecOptions,
) -> Result<Vec<MatchSpec>, ValidateIndexJsonError> {
    let field = field.into();
    specs
        .iter()
        .map(|spec| {
            MatchSpec::from_str(spec, parse_options).map_err(|source| {
                ValidateIndexJsonError::InvalidMatchSpec {
                    field: field.clone(),
                    spec: spec.clone(),
                    source,
                }
            })
        })
        .collect()
}

fn validate_legacy_matchspec(
    field: &str,
    matchspec: &MatchSpec,
) -> Result<(), ValidateIndexJsonError> {
    if matchspec.extras.is_some() {
        return Err(ValidateIndexJsonError::LegacyMatchSpecExtras {
            field: field.to_string(),
            spec: matchspec.to_string(),
        });
    }
    if matchspec.condition.is_some() {
        return Err(ValidateIndexJsonError::LegacyMatchSpecCondition {
            field: field.to_string(),
            spec: matchspec.to_string(),
        });
    }
    if matchspec.flags.is_some() {
        return Err(ValidateIndexJsonError::LegacyMatchSpecFlags {
            field: field.to_string(),
            spec: matchspec.to_string(),
        });
    }
    Ok(())
}

fn matchspec_requires_v3(spec: &str, parse_options: ParseMatchSpecOptions) -> bool {
    MatchSpec::from_str(spec, parse_options)
        .is_ok_and(|matchspec| matchspec.required_repodata_revision() == RepodataRevision::V3)
}

/// An error when validating an [`IndexJson`] value.
#[derive(Debug, Error)]
pub enum ValidateIndexJsonError {
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
            RepodataRevision::Legacy
        );
        inferred_revision.validate().unwrap();

        let inferred_revision: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "extra_depends": {
                    "test": ["pytest[when=\"python >=3.10\"]"]
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
        inferred_revision.validate().unwrap();

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
    pub fn test_validated_matchspecs_render_for_revision() {
        let index: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "constrains": ["python >=3.10"],
                "depends": ["python >=3.10"],
                "extra_depends": { "test": ["pytest >=8"] },
                "name": "demo",
                "version": "1.0"
            }"#,
        )
        .unwrap();
        let (_, matchspecs) = index.into_validated().unwrap().into_parts();

        let v3 = matchspecs
            .clone()
            .render_for_revision(RepodataRevision::V3)
            .unwrap();
        assert_eq!(v3.depends, ["python[version=\">=3.10\"]"]);
        assert_eq!(v3.constrains, ["python[version=\">=3.10\"]"]);
        assert_eq!(v3.extra_depends["test"], ["pytest[version=\">=8\"]"]);

        let revision_4 = matchspecs
            .clone()
            .render_for_revision(RepodataRevision::Unknown(4))
            .unwrap();
        assert_eq!(revision_4.depends, v3.depends);
        assert_eq!(revision_4.constrains, v3.constrains);
        assert_eq!(revision_4.extra_depends, v3.extra_depends);

        let legacy = matchspecs
            .clone()
            .render_for_revision(RepodataRevision::Legacy)
            .unwrap();
        assert_eq!(legacy.depends, ["python >=3.10"]);
        assert_eq!(legacy.constrains, ["python >=3.10"]);
        assert_eq!(legacy.extra_depends["test"], ["pytest >=8"]);

        let revision_2 = matchspecs
            .render_for_revision(RepodataRevision::Unknown(2))
            .unwrap();
        assert_eq!(revision_2.depends, legacy.depends);
        assert_eq!(revision_2.constrains, legacy.constrains);
        assert_eq!(revision_2.extra_depends, legacy.extra_depends);
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
        extra_depends.validate().unwrap();

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

        let conditional_extra_dependency: IndexJson = serde_json::from_str(
            r#"{
                "build": "0",
                "build_number": 0,
                "extra_depends": {
                    "test": ["pytest[when=\"python >=3.10\"]"]
                },
                "name": "demo",
                "repodata_revision": 0,
                "version": "1.0"
            }"#,
        )
        .unwrap();
        assert!(matches!(
            conditional_extra_dependency.validate(),
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

        insta::assert_yaml_snapshot!(
            IndexJson::from_package_directory(package_dir.path()).unwrap()
        );
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
