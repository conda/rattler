//! Registrations of virtual package detectors that a channel publishes in the
//! `info.virtual_package_detectors` dictionary of its repodata.
//!
//! A registration names a detector package and the virtual packages it
//! reports. Channels publish one dictionary per subdir; clients combine the
//! dictionaries of the subdirs they load into one set per channel. The value
//! is parsed as opaque JSON first, so a malformed registration never prevents
//! parsing the surrounding repodata. Validation happens in two steps:
//!
//! 1. [`SubdirDetectorRegistrations::parse`] checks the shape of one subdir's
//!    dictionary.
//! 2. [`ChannelDetectorRegistrations::combine`] merges the parsed subdirs and
//!    applies the limits that hold across the combined set.
//!
//! Any [`RegistrationError`] discards the channel's entire combined set; the
//! caller reports it and continues without the channel's detectors.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Formatter};
use std::time::Duration;

use indexmap::{IndexMap, IndexSet};
use lazy_regex::regex;
use thiserror::Error;

use crate::{InvalidPackageNameError, PackageName};

/// Deserializes an `info.virtual_package_detectors` field that is present,
/// keeping a `null` value as `Some(Value::Null)`.
///
/// Serde's default handling of `Option` maps `null` to `None`, which would make
/// `null` indistinguishable from an absent field. The CEP treats an absent field
/// as "no registrations" and `null` as a registration error, so the distinction
/// has to survive parsing. Combine with `#[serde(default)]` for the absent case.
pub(crate) fn deserialize_present<'de, D>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(deserializer).map(Some)
}

/// The most virtual package names one detector may register.
pub const MAX_VIRTUAL_PACKAGES_PER_DETECTOR: usize = 16;

/// The most detectors one channel may register across its combined subdirs.
pub const MAX_DETECTORS_PER_CHANNEL: usize = 64;

/// The longest virtual package name a detector may report, in characters.
pub const MAX_VIRTUAL_PACKAGE_NAME_LENGTH: usize = 64;

/// The prefix of every override variable.
pub const OVERRIDE_VARIABLE_PREFIX: &str = "CONDA_OVERRIDE_";

/// The default time a detector process, and separately its activation, may
/// take.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest timeout a client may grant a detector process or its
/// activation.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(300);

/// Why a string is not a valid virtual package name for a detector.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum InvalidVirtualPackageNameError {
    /// The name does not start with two underscores.
    #[error("'{0}' does not start with two underscores")]
    MissingPrefix(String),

    /// The name is longer than [`MAX_VIRTUAL_PACKAGE_NAME_LENGTH`] characters.
    #[error("'{0}' is longer than {MAX_VIRTUAL_PACKAGE_NAME_LENGTH} characters")]
    TooLong(String),

    /// The name does not match the pattern the detector protocol requires.
    #[error(
        "'{0}' is not a valid virtual package name: after the two underscores it must consist of lowercase letters, digits and single '.', '-' or '_' separators"
    )]
    InvalidPattern(String),
}

/// Validates `name` as a virtual package name a detector may report and
/// returns it as a [`PackageName`].
///
/// Valid names start with two underscores, contain at most
/// [`MAX_VIRTUAL_PACKAGE_NAME_LENGTH`] characters, and match
/// `^__[a-z0-9][._-]?([a-z0-9]+(\.|-|_|$))*$`.
pub fn parse_virtual_package_name(
    name: &str,
) -> Result<PackageName, InvalidVirtualPackageNameError> {
    if !name.starts_with("__") {
        return Err(InvalidVirtualPackageNameError::MissingPrefix(
            name.to_string(),
        ));
    }
    if name.chars().count() > MAX_VIRTUAL_PACKAGE_NAME_LENGTH {
        return Err(InvalidVirtualPackageNameError::TooLong(name.to_string()));
    }
    if !regex!(r"^__[a-z0-9][._-]?([a-z0-9]+(\.|-|_|$))*$").is_match(name) {
        return Err(InvalidVirtualPackageNameError::InvalidPattern(
            name.to_string(),
        ));
    }
    // The pattern only admits characters that are valid in a package name and
    // are already lowercase, so this cannot fail and the normalized form equals
    // the source form.
    Ok(PackageName::new_unchecked(name))
}

/// Returns the name of the `CONDA_OVERRIDE_*` environment variable for a
/// virtual package name.
///
/// The variable is [`OVERRIDE_VARIABLE_PREFIX`] followed by the name without
/// its two leading underscores, uppercased, with `-` and `.` replaced by `_`.
/// Distinct names can map to the same variable; a combined registration set
/// containing such a pair is a [`RegistrationError::OverrideVariableCollision`].
pub fn override_variable(name: &PackageName) -> String {
    let stripped = name
        .as_normalized()
        .strip_prefix("__")
        .unwrap_or(name.as_normalized());
    let mut variable = String::with_capacity(OVERRIDE_VARIABLE_PREFIX.len() + stripped.len());
    variable.push_str(OVERRIDE_VARIABLE_PREFIX);
    for c in stripped.chars() {
        match c {
            '-' | '.' => variable.push('_'),
            c => variable.extend(c.to_uppercase()),
        }
    }
    variable
}

/// Why a channel's registrations are ignored as a whole.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RegistrationError {
    /// The `virtual_package_detectors` value is not a dictionary.
    #[error("`virtual_package_detectors` must be a dictionary, found {found}")]
    NotADictionary {
        /// A short description of the JSON type that was found.
        found: JsonKind,
    },

    /// A detector key is not a valid package name.
    #[error("detector key {key:?} is not a valid package name: {source}")]
    InvalidDetectorName {
        /// The offending key.
        key: String,
        /// Why the key is not a package name.
        source: InvalidPackageNameError,
    },

    /// A detector key names a virtual package instead of an installable one.
    #[error(
        "detector key {key:?} is a virtual package name, detectors must be installable packages"
    )]
    VirtualDetectorName {
        /// The offending key.
        key: String,
    },

    /// A detector key violates CEP 26's installable-name syntax or length.
    #[error(
        "detector key {key:?} must be an installable package name of at most 64 characters with single '.', '-' or '_' separators"
    )]
    InvalidInstallableDetectorName {
        /// The offending key.
        key: String,
    },

    /// The value of a detector is not an array of strings.
    #[error("the registration of detector '{detector}' must be an array of strings, found {found}")]
    InvalidRegistration {
        /// The detector whose value is malformed.
        detector: String,
        /// A short description of the JSON type that was found.
        found: JsonKind,
    },

    /// A detector registers no virtual packages or more than the limit,
    /// counted before invalid names are dropped.
    #[error(
        "detector '{detector}' registers {count} virtual packages, expected between 1 and {MAX_VIRTUAL_PACKAGES_PER_DETECTOR}"
    )]
    VirtualPackageCountOutOfRange {
        /// The detector with the offending array.
        detector: String,
        /// How many entries the array, or the union across subdirs, has.
        count: usize,
    },

    /// One subdir's dictionary lists the same detector name twice.
    #[error("detector '{detector}' is registered twice in the same subdir")]
    DuplicateDetector {
        /// The duplicated detector name.
        detector: String,
    },

    /// The same detector registers different virtual packages in different
    /// subdirs.
    #[error("detector '{detector}' registers different virtual packages in different subdirs")]
    InconsistentAcrossSubdirs {
        /// The detector whose registrations disagree.
        detector: String,
    },

    /// The channel registers more detectors than the limit, counted before
    /// empty registrations are removed.
    #[error(
        "the channel registers {count} detectors, at most {MAX_DETECTORS_PER_CHANNEL} are allowed"
    )]
    TooManyDetectors {
        /// How many distinct detectors the combined set has.
        count: usize,
    },

    /// A virtual package name appears twice, either within one detector's
    /// array or across two detectors.
    #[error("virtual package '{name}' is registered more than once by {detectors}")]
    DuplicateVirtualPackage {
        /// The duplicated name.
        name: String,
        /// The detectors registering it, comma separated.
        detectors: String,
    },

    /// Two distinct virtual package names map to the same override variable.
    #[error("virtual packages {names} map to the same override variable {variable}")]
    OverrideVariableCollision {
        /// The environment variable both names map to.
        variable: String,
        /// The colliding names, comma separated.
        names: String,
    },
}

/// A short description of a JSON value's type, for error messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JsonKind {
    /// `null`
    Null,
    /// `true` or `false`
    Bool,
    /// A number
    Number,
    /// A string
    String,
    /// An array
    Array,
    /// An object
    Object,
}

impl JsonKind {
    fn of(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(_) => Self::Bool,
            serde_json::Value::Number(_) => Self::Number,
            serde_json::Value::String(_) => Self::String,
            serde_json::Value::Array(_) => Self::Array,
            serde_json::Value::Object(_) => Self::Object,
        }
    }
}

impl Display for JsonKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Null => "null",
            Self::Bool => "a boolean",
            Self::Number => "a number",
            Self::String => "a string",
            Self::Array => "an array",
            Self::Object => "an object",
        };
        f.write_str(name)
    }
}

/// The registrations of one subdir, validated for shape but with the virtual
/// package names still unvalidated.
///
/// Names stay raw here because the combined limits count entries before
/// invalid names are dropped.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SubdirDetectorRegistrations {
    detectors: IndexMap<PackageName, Vec<String>>,
}

impl SubdirDetectorRegistrations {
    /// Parses the raw `info.virtual_package_detectors` value of one subdir.
    ///
    /// `None` and an empty dictionary register nothing. `null` and any other
    /// non-dictionary value are errors.
    pub fn parse(value: Option<&serde_json::Value>) -> Result<Self, RegistrationError> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        let serde_json::Value::Object(entries) = value else {
            return Err(RegistrationError::NotADictionary {
                found: JsonKind::of(value),
            });
        };

        let mut detectors = IndexMap::with_capacity(entries.len());
        for (key, registration) in entries {
            if key.starts_with("__") {
                return Err(RegistrationError::VirtualDetectorName { key: key.clone() });
            }
            let detector = PackageName::try_from(key.as_str()).map_err(|source| {
                RegistrationError::InvalidDetectorName {
                    key: key.clone(),
                    source,
                }
            })?;
            if key.len() > 64
                || !regex!(r"(?i)^(?:[a-z0-9][._-]?|_)(?:[a-z0-9]+[._-]?)*$").is_match(key)
            {
                return Err(RegistrationError::InvalidInstallableDetectorName { key: key.clone() });
            }
            let serde_json::Value::Array(names) = registration else {
                return Err(RegistrationError::InvalidRegistration {
                    detector: key.clone(),
                    found: JsonKind::of(registration),
                });
            };
            let names = names
                .iter()
                .map(|name| match name {
                    serde_json::Value::String(name) => Ok(name.clone()),
                    other => Err(RegistrationError::InvalidRegistration {
                        detector: key.clone(),
                        found: JsonKind::of(other),
                    }),
                })
                .collect::<Result<Vec<_>, _>>()?;
            if detectors.insert(detector, names).is_some() {
                return Err(RegistrationError::DuplicateDetector {
                    detector: key.clone(),
                });
            }
        }
        Ok(Self { detectors })
    }

    /// Whether this subdir registers no detectors.
    pub fn is_empty(&self) -> bool {
        self.detectors.is_empty()
    }
}

/// A virtual package name a detector registered that the client dropped, with
/// the reason. Clients report these but keep the rest of the registration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DroppedVirtualPackageName {
    /// The detector that registered the name.
    pub detector: PackageName,
    /// The name as the channel wrote it.
    pub name: String,
    /// Why the name was dropped.
    pub reason: InvalidVirtualPackageNameError,
}

/// One detector a channel registered together with the valid virtual package
/// names it reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DetectorRegistration {
    /// The package that provides the detector; also the executable's name.
    pub detector: PackageName,
    /// The virtual packages the detector reports, in registration order. Never
    /// empty.
    pub virtual_packages: IndexSet<PackageName>,
}

impl DetectorRegistration {
    /// The override variables of the registered virtual packages, in the same
    /// order.
    pub fn override_variables(&self) -> impl Iterator<Item = String> + '_ {
        self.virtual_packages.iter().map(override_variable)
    }
}

/// Whether two arrays register the same virtual packages, whatever their
/// order.
fn same_names(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().collect::<BTreeSet<_>>() == b.iter().collect::<BTreeSet<_>>()
}

/// The registrations of one channel after combining the subdirs the client
/// loaded.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChannelDetectorRegistrations {
    registrations: Vec<DetectorRegistration>,
    dropped_names: Vec<DroppedVirtualPackageName>,
}

impl ChannelDetectorRegistrations {
    /// Combines the parsed registrations of the subdirs a client loaded for one
    /// channel and applies the limits of the combined set.
    ///
    /// Detectors keep the order of their first appearance. Detectors whose
    /// names are all invalid are dropped after the limits are checked and are
    /// reported through [`Self::dropped_names`].
    pub fn combine<'a>(
        subdirs: impl IntoIterator<Item = &'a SubdirDetectorRegistrations>,
    ) -> Result<Self, RegistrationError> {
        let mut raw: IndexMap<PackageName, Vec<String>> = IndexMap::new();
        for subdir in subdirs {
            for (detector, names) in &subdir.detectors {
                if names.is_empty() || names.len() > MAX_VIRTUAL_PACKAGES_PER_DETECTOR {
                    return Err(RegistrationError::VirtualPackageCountOutOfRange {
                        detector: detector.as_source().to_string(),
                        count: names.len(),
                    });
                }
                match raw.get(detector) {
                    None => {
                        raw.insert(detector.clone(), names.clone());
                    }
                    Some(existing) if same_names(existing, names) => {}
                    Some(_) => {
                        return Err(RegistrationError::InconsistentAcrossSubdirs {
                            detector: detector.as_source().to_string(),
                        });
                    }
                }
            }
        }

        if raw.len() > MAX_DETECTORS_PER_CHANNEL {
            return Err(RegistrationError::TooManyDetectors { count: raw.len() });
        }

        let mut registrations = Vec::with_capacity(raw.len());
        let mut dropped_names = Vec::new();
        let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut by_variable: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (detector, names) in raw {
            let mut virtual_packages = IndexSet::with_capacity(names.len());
            for name in names {
                match parse_virtual_package_name(&name) {
                    Ok(name) => {
                        let owners = owners.entry(name.as_normalized().to_string()).or_default();
                        owners.insert(detector.as_source().to_string());
                        if !virtual_packages.insert(name.clone()) || owners.len() > 1 {
                            return Err(RegistrationError::DuplicateVirtualPackage {
                                name: name.as_normalized().to_string(),
                                detectors: owners.iter().cloned().collect::<Vec<_>>().join(", "),
                            });
                        }
                        by_variable
                            .entry(override_variable(&name))
                            .or_default()
                            .insert(name.as_normalized().to_string());
                    }
                    Err(reason) => dropped_names.push(DroppedVirtualPackageName {
                        detector: detector.clone(),
                        name,
                        reason,
                    }),
                }
            }
            if !virtual_packages.is_empty() {
                registrations.push(DetectorRegistration {
                    detector,
                    virtual_packages,
                });
            }
        }

        if let Some((variable, names)) = by_variable.iter().find(|(_, names)| names.len() > 1) {
            return Err(RegistrationError::OverrideVariableCollision {
                variable: variable.clone(),
                names: names.iter().cloned().collect::<Vec<_>>().join(", "),
            });
        }

        Ok(Self {
            registrations,
            dropped_names,
        })
    }

    /// The accepted registrations, in order of first appearance.
    pub fn registrations(&self) -> &[DetectorRegistration] {
        &self.registrations
    }

    /// The names that were dropped for being invalid.
    pub fn dropped_names(&self) -> &[DroppedVirtualPackageName] {
        &self.dropped_names
    }

    /// Whether no detector survived combination.
    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty()
    }

    /// Consumes the set and returns the accepted registrations.
    pub fn into_registrations(self) -> Vec<DetectorRegistration> {
        self.registrations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<SubdirDetectorRegistrations, RegistrationError> {
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        SubdirDetectorRegistrations::parse(Some(&value))
    }

    fn combine(jsons: &[&str]) -> Result<ChannelDetectorRegistrations, RegistrationError> {
        let subdirs = jsons
            .iter()
            .map(|json| parse(json).unwrap())
            .collect::<Vec<_>>();
        ChannelDetectorRegistrations::combine(&subdirs)
    }

    #[test]
    fn detector_keys_obey_installable_name_boundaries() {
        for key in [
            "",
            "-bad",
            ".bad",
            "bad..name",
            "bad_-name",
            "__bad",
            &"a".repeat(65),
        ] {
            let value = serde_json::json!({key: ["__capability"]});
            assert!(
                SubdirDetectorRegistrations::parse(Some(&value)).is_err(),
                "invalid detector key {key:?} was accepted"
            );
        }
        for key in [
            "a",
            "_private",
            "detector-1.2_name",
            "detector-",
            &"a".repeat(64),
        ] {
            let value = serde_json::json!({key: ["__capability"]});
            let parsed = SubdirDetectorRegistrations::parse(Some(&value)).unwrap();
            let combined = ChannelDetectorRegistrations::combine([&parsed]).unwrap();
            assert_eq!(combined.registrations()[0].detector.as_source(), key);
        }
    }

    #[test]
    fn valid_virtual_package_names() {
        for name in [
            "__conda_forge_openmpi",
            "__cuda",
            "__a",
            "__1",
            "__conda-forge.mpi_abi",
            "__glibc",
            // The pattern admits a trailing separator.
            "__conda_forge_",
        ] {
            let parsed = parse_virtual_package_name(name).unwrap();
            assert_eq!(parsed.as_source(), name);
            assert_eq!(parsed.as_normalized(), name);
        }
    }

    #[test]
    fn invalid_virtual_package_names() {
        insta::assert_debug_snapshot!(
            [
                "openmpi",
                "_cuda",
                "__mpi/openmpi",
                "__CUDA",
                "__conda__forge",
                "__-cuda",
                "__cuda..arch",
                "__",
                &format!("__{}", "a".repeat(63)),
            ]
            .map(|name| parse_virtual_package_name(name).unwrap_err()),
            @r#"
        [
            MissingPrefix(
                "openmpi",
            ),
            MissingPrefix(
                "_cuda",
            ),
            InvalidPattern(
                "__mpi/openmpi",
            ),
            InvalidPattern(
                "__CUDA",
            ),
            InvalidPattern(
                "__conda__forge",
            ),
            InvalidPattern(
                "__-cuda",
            ),
            InvalidPattern(
                "__cuda..arch",
            ),
            InvalidPattern(
                "__",
            ),
            TooLong(
                "__aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ]
        "#
        );
    }

    #[test]
    fn override_variables() {
        let variable = |name: &str| override_variable(&parse_virtual_package_name(name).unwrap());
        assert_eq!(variable("__cuda"), "CONDA_OVERRIDE_CUDA");
        assert_eq!(
            variable("__conda_forge_mpi"),
            "CONDA_OVERRIDE_CONDA_FORGE_MPI"
        );
        assert_eq!(
            variable("__conda-forge_mpi"),
            "CONDA_OVERRIDE_CONDA_FORGE_MPI"
        );
        assert_eq!(variable("__mpi_abi.v2"), "CONDA_OVERRIDE_MPI_ABI_V2");
    }

    #[test]
    fn absent_and_empty_register_nothing() {
        assert!(SubdirDetectorRegistrations::parse(None).unwrap().is_empty());
        assert!(parse("{}").unwrap().is_empty());
        assert!(combine(&["{}", "{}"]).unwrap().is_empty());
    }

    #[test]
    fn parse_shape_errors() {
        insta::assert_debug_snapshot!(
            [
                "null",
                r#"["mpi-detect"]"#,
                r#"{"mpi-detect": {"__cuda": true}}"#,
                r#"{"mpi-detect": ["__cuda", 5]}"#,
                r#"{"__mpi-detect": ["__cuda"]}"#,
                r#"{"mpi detect": ["__cuda"]}"#,
            ]
            .map(|json| parse(json).unwrap_err()),
            @r#"
        [
            NotADictionary {
                found: Null,
            },
            NotADictionary {
                found: Array,
            },
            InvalidRegistration {
                detector: "mpi-detect",
                found: Object,
            },
            InvalidRegistration {
                detector: "mpi-detect",
                found: Number,
            },
            VirtualDetectorName {
                key: "__mpi-detect",
            },
            InvalidDetectorName {
                key: "mpi detect",
                source: InvalidCharacters(
                    "mpi detect",
                ),
            },
        ]
        "#
        );
    }

    #[test]
    fn combine_ignores_the_order_of_names_across_subdirs() {
        let combined = combine(&[
            r#"{"mpi-detect": ["__a", "__b"]}"#,
            r#"{"mpi-detect": ["__b", "__a"]}"#,
        ])
        .unwrap();
        assert_eq!(combined.registrations().len(), 1);
    }

    #[test]
    fn combine_accepts_the_cep_example() {
        let combined = combine(&[
            r#"{"mpi-detect": ["__conda_forge_openmpi", "__conda_forge_mpich"]}"#,
            r#"{"mpi-detect": ["__conda_forge_openmpi", "__conda_forge_mpich"], "cuda-detect": ["__cuda"]}"#,
        ])
        .unwrap();
        assert!(combined.dropped_names().is_empty());
        let names = combined
            .registrations()
            .iter()
            .map(|registration| {
                (
                    registration.detector.as_source(),
                    registration
                        .virtual_packages
                        .iter()
                        .map(PackageName::as_normalized)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                (
                    "mpi-detect",
                    vec!["__conda_forge_openmpi", "__conda_forge_mpich"]
                ),
                ("cuda-detect", vec!["__cuda"]),
            ]
        );
    }

    #[test]
    fn combine_drops_invalid_names_and_empty_detectors() {
        let combined = combine(&[
            r#"{"mpi-detect": ["__conda_forge_openmpi", "openmpi"], "broken-detect": ["Nope"]}"#,
        ])
        .unwrap();
        assert_eq!(combined.registrations().len(), 1);
        assert_eq!(
            combined.registrations()[0].detector.as_source(),
            "mpi-detect"
        );
        insta::assert_debug_snapshot!(combined.dropped_names(), @r#"
        [
            DroppedVirtualPackageName {
                detector: PackageName {
                    normalized: None,
                    source: "mpi-detect",
                },
                name: "openmpi",
                reason: MissingPrefix(
                    "openmpi",
                ),
            },
            DroppedVirtualPackageName {
                detector: PackageName {
                    normalized: None,
                    source: "broken-detect",
                },
                name: "Nope",
                reason: MissingPrefix(
                    "Nope",
                ),
            },
        ]
        "#);
    }

    #[test]
    fn combine_errors() {
        let seventeen = (0..17)
            .map(|i| format!("\"__vp{i}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let sixty_five = (0..65)
            .map(|i| format!("\"detect{i}\": [\"__vp{i}\"]"))
            .collect::<Vec<_>>()
            .join(", ");
        insta::assert_debug_snapshot!(
            [
                combine(&[r#"{"mpi-detect": []}"#]),
                combine(&[&format!(r#"{{"mpi-detect": [{seventeen}]}}"#)]),
                combine(&[&format!("{{{sixty_five}}}")]),
                combine(&[
                    r#"{"mpi-detect": ["__a"]}"#,
                    r#"{"mpi-detect": ["__b"]}"#,
                ]),
                combine(&[r#"{"mpi-detect": ["__a", "__a"]}"#]),
                combine(&[r#"{"mpi-detect": ["__a"], "other-detect": ["__a"]}"#]),
                combine(&[r#"{"mpi-detect": ["__conda-forge_mpi", "__conda_forge_mpi"]}"#]),
                combine(&[r#"{"mpi-detect": ["__conda-forge_mpi"], "b-detect": ["__conda.forge.mpi"]}"#]),
            ]
            .map(Result::unwrap_err),
            @r#"
        [
            VirtualPackageCountOutOfRange {
                detector: "mpi-detect",
                count: 0,
            },
            VirtualPackageCountOutOfRange {
                detector: "mpi-detect",
                count: 17,
            },
            TooManyDetectors {
                count: 65,
            },
            InconsistentAcrossSubdirs {
                detector: "mpi-detect",
            },
            DuplicateVirtualPackage {
                name: "__a",
                detectors: "mpi-detect",
            },
            DuplicateVirtualPackage {
                name: "__a",
                detectors: "mpi-detect, other-detect",
            },
            OverrideVariableCollision {
                variable: "CONDA_OVERRIDE_CONDA_FORGE_MPI",
                names: "__conda-forge_mpi, __conda_forge_mpi",
            },
            OverrideVariableCollision {
                variable: "CONDA_OVERRIDE_CONDA_FORGE_MPI",
                names: "__conda-forge_mpi, __conda.forge.mpi",
            },
        ]
        "#
        );
    }

    #[test]
    fn limits_count_before_dropping_invalid_names() {
        // 16 entries of which one is invalid: within the limit.
        let names = (0..15)
            .map(|i| format!("\"__vp{i}\""))
            .chain(std::iter::once("\"invalid\"".to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        let combined = combine(&[&format!(r#"{{"mpi-detect": [{names}]}}"#)]).unwrap();
        assert_eq!(combined.registrations()[0].virtual_packages.len(), 15);
        assert_eq!(combined.dropped_names().len(), 1);

        // A detector with only an invalid name still counts towards the detector limit.
        let sixty_four = (0..64)
            .map(|i| format!("\"detect{i}\": [\"invalid\"]"))
            .collect::<Vec<_>>()
            .join(", ");
        let combined = combine(&[&format!("{{{sixty_four}}}")]).unwrap();
        assert!(combined.is_empty());
        assert_eq!(combined.dropped_names().len(), 64);
    }
}
