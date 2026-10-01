//! The report a detector writes to standard output, and the contract it must
//! keep with its registration.
//!
//! A report is exactly one JSON object. It names every virtual package the
//! registration declared, and no other, mapping each to `null` for absence or
//! to an object with a version and an optional build string. An optional
//! `cache` object carries hints on how long the result stays valid. Any
//! violation makes the whole report malformed; a client then discards every
//! result of the detector.

use std::{path::PathBuf, str::FromStr};

use indexmap::IndexMap;
use rattler_conda_types::{
    GenericVirtualPackage, InvalidPackageNameError, PackageName, ParseVersionError, Version,
    virtual_package_detector::DetectorRegistration,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::limits::{MAX_WATCH_ENTRIES, MAX_WATCH_ENTRY_BYTES};

/// The report version this crate understands.
pub const PROTOCOL_VERSION: u64 = 1;

/// A present virtual package as a detector reported it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedVersion {
    /// The version of the virtual package.
    pub version: Version,
    /// The build string, `0` when the report gave none.
    pub build_string: String,
}

impl DetectedVersion {
    /// The virtual package record for `name` with this version.
    pub fn into_virtual_package(self, name: PackageName) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name,
            version: self.version,
            build_string: self.build_string,
        }
    }
}

/// How long a result stays valid, as the detector asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheLifetime {
    /// A number of seconds; `0` means the result must not be reused.
    Seconds(u64),
    /// Until the machine reboots.
    Reboot,
}

/// The optional `cache` object of a report.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheHints {
    /// The requested lifetime, if the report gave one.
    pub ttl: Option<CacheLifetime>,
    /// Absolute paths whose appearance, disappearance or modification expires
    /// the result.
    pub watch_paths: Vec<PathBuf>,
    /// Environment variables whose change of value expires the result.
    pub watch_env: Vec<String>,
}

/// A parsed and validated report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectorReport {
    /// Every registered virtual package, in report order, with `None` for
    /// absence.
    pub virtual_packages: IndexMap<PackageName, Option<DetectedVersion>>,
    /// The cache hints, empty when the report gave none.
    pub cache: CacheHints,
}

/// Why a report is malformed or breaks the registration contract.
#[derive(Debug, Error)]
pub enum ReportError {
    /// Standard output was empty or only whitespace.
    #[error("the detector wrote no report to standard output")]
    Empty,

    /// Standard output is not one JSON value with only whitespace around it.
    #[error("the report is not valid JSON: {0}")]
    NotJson(String),

    /// The JSON value is not an object.
    #[error("the report must be a JSON object")]
    NotAnObject,

    /// The `version` field is missing.
    #[error("the report has no `version` field")]
    MissingVersion,

    /// The `version` field is not a supported protocol version.
    #[error("unsupported report version {0}, expected {PROTOCOL_VERSION}")]
    UnsupportedVersion(serde_json::Value),

    /// The `virtual_packages` field is missing.
    #[error("the report has no `virtual_packages` field")]
    MissingVirtualPackages,

    /// A known field has the wrong JSON type.
    #[error("the report field `{field}` must be {expected}")]
    WrongType {
        /// The dotted path of the field.
        field: String,
        /// A description of the expected type.
        expected: &'static str,
    },

    /// A key of `virtual_packages` is not a valid package name.
    #[error("the report names {name:?}, which is not a valid package name")]
    InvalidName {
        /// The offending key.
        name: String,
        /// Why it is not a package name.
        #[source]
        source: InvalidPackageNameError,
    },

    /// A key of `virtual_packages` was not declared by the registration.
    #[error("the report names `{0}`, which the registration does not declare")]
    UndeclaredName(String),

    /// A declared virtual package is missing from `virtual_packages`.
    #[error("the report omits the registered virtual package `{0}`")]
    MissingName(String),

    /// A name appears twice after normalization.
    #[error("the report names `{0}` more than once")]
    DuplicateName(String),

    /// A result's `version` is not a valid version.
    #[error("the version of `{name}` is invalid: {source}")]
    InvalidVersion {
        /// The virtual package.
        name: String,
        /// The parse error.
        #[source]
        source: ParseVersionError,
    },

    /// A result's `build_string` contains characters a build string may not.
    #[error("the build string {build_string:?} of `{name}` is invalid")]
    InvalidBuildString {
        /// The virtual package.
        name: String,
        /// The offending build string.
        build_string: String,
    },

    /// `cache.ttl_seconds` is neither a nonnegative integer nor `"REBOOT"`.
    #[error("`cache.ttl_seconds` must be a nonnegative integer or \"REBOOT\", found {0}")]
    InvalidLifetime(serde_json::Value),

    /// A `cache.watch_paths` entry is relative.
    #[error("`cache.watch_paths` entry {0:?} is not an absolute path")]
    RelativeWatchPath(String),

    /// A watch list has more than [`MAX_WATCH_ENTRIES`] entries.
    #[error("`cache.{field}` has {count} entries, at most {MAX_WATCH_ENTRIES} are allowed")]
    TooManyWatchEntries {
        /// `watch_paths` or `watch_env`.
        field: &'static str,
        /// The number of entries.
        count: usize,
    },

    /// A watch entry is longer than [`MAX_WATCH_ENTRY_BYTES`].
    #[error("a `cache.{field}` entry is longer than {MAX_WATCH_ENTRY_BYTES} bytes")]
    WatchEntryTooLong {
        /// `watch_paths` or `watch_env`.
        field: &'static str,
    },
}

/// Parses `stdout` as a report and checks it against `registration`.
pub fn parse_report(
    stdout: &[u8],
    registration: &DetectorRegistration,
) -> Result<DetectorReport, ReportError> {
    if stdout.iter().all(u8::is_ascii_whitespace) {
        return Err(ReportError::Empty);
    }
    let value: serde_json::Value =
        serde_json::from_slice(stdout).map_err(|err| ReportError::NotJson(err.to_string()))?;
    let serde_json::Value::Object(object) = value else {
        return Err(ReportError::NotAnObject);
    };

    match object.get("version") {
        None => return Err(ReportError::MissingVersion),
        Some(version) if version.as_u64() == Some(PROTOCOL_VERSION) => {}
        Some(version) => return Err(ReportError::UnsupportedVersion(version.clone())),
    }

    let virtual_packages = match object.get("virtual_packages") {
        None => return Err(ReportError::MissingVirtualPackages),
        Some(serde_json::Value::Object(entries)) => parse_virtual_packages(entries, registration)?,
        Some(_) => {
            return Err(ReportError::WrongType {
                field: "virtual_packages".to_string(),
                expected: "an object",
            });
        }
    };

    let cache = match object.get("cache") {
        None => CacheHints::default(),
        Some(serde_json::Value::Object(hints)) => parse_cache_hints(hints)?,
        Some(_) => {
            return Err(ReportError::WrongType {
                field: "cache".to_string(),
                expected: "an object",
            });
        }
    };

    Ok(DetectorReport {
        virtual_packages,
        cache,
    })
}

fn parse_virtual_packages(
    entries: &serde_json::Map<String, serde_json::Value>,
    registration: &DetectorRegistration,
) -> Result<IndexMap<PackageName, Option<DetectedVersion>>, ReportError> {
    let mut results = IndexMap::with_capacity(entries.len());
    for (key, value) in entries {
        let name =
            PackageName::try_from(key.as_str()).map_err(|source| ReportError::InvalidName {
                name: key.clone(),
                source,
            })?;
        let Some(declared) = registration
            .virtual_packages
            .iter()
            .find(|declared| declared.as_normalized() == name.as_normalized())
        else {
            return Err(ReportError::UndeclaredName(
                name.as_normalized().to_string(),
            ));
        };
        let result = parse_result(key, value)?;
        if results.insert(declared.clone(), result).is_some() {
            return Err(ReportError::DuplicateName(name.as_normalized().to_string()));
        }
    }
    for declared in &registration.virtual_packages {
        if !results.contains_key(declared) {
            return Err(ReportError::MissingName(
                declared.as_normalized().to_string(),
            ));
        }
    }
    Ok(results)
}

fn parse_result(
    name: &str,
    value: &serde_json::Value,
) -> Result<Option<DetectedVersion>, ReportError> {
    let fields = match value {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Object(fields) => fields,
        _ => {
            return Err(ReportError::WrongType {
                field: format!("virtual_packages.{name}"),
                expected: "null or an object",
            });
        }
    };
    let version = match fields.get("version") {
        Some(serde_json::Value::String(version)) => {
            Version::from_str(version).map_err(|source| ReportError::InvalidVersion {
                name: name.to_string(),
                source,
            })?
        }
        _ => {
            return Err(ReportError::WrongType {
                field: format!("virtual_packages.{name}.version"),
                expected: "a string",
            });
        }
    };
    let build_string = match fields.get("build_string") {
        None => "0".to_string(),
        Some(serde_json::Value::String(build_string)) => {
            if build_string.is_empty()
                || !build_string
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
            {
                return Err(ReportError::InvalidBuildString {
                    name: name.to_string(),
                    build_string: build_string.clone(),
                });
            }
            build_string.clone()
        }
        Some(_) => {
            return Err(ReportError::WrongType {
                field: format!("virtual_packages.{name}.build_string"),
                expected: "a string",
            });
        }
    };
    Ok(Some(DetectedVersion {
        version,
        build_string,
    }))
}

fn parse_cache_hints(
    hints: &serde_json::Map<String, serde_json::Value>,
) -> Result<CacheHints, ReportError> {
    let ttl = match hints.get("ttl_seconds") {
        None => None,
        Some(serde_json::Value::String(reboot)) if reboot == "REBOOT" => {
            Some(CacheLifetime::Reboot)
        }
        Some(value) => match integral_seconds(value) {
            Some(seconds) => Some(CacheLifetime::Seconds(seconds)),
            None => return Err(ReportError::InvalidLifetime(value.clone())),
        },
    };
    let watch_paths = parse_watch_list(hints, "watch_paths")?
        .into_iter()
        .map(|entry| {
            let path = PathBuf::from(&entry);
            if path.is_absolute() {
                Ok(path)
            } else {
                Err(ReportError::RelativeWatchPath(entry))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let watch_env = parse_watch_list(hints, "watch_env")?;
    Ok(CacheHints {
        ttl,
        watch_paths,
        watch_env,
    })
}

/// A nonnegative integer, saturating at `u64::MAX` for values JSON can only
/// represent as floating point; the lifetime is clamped afterwards anyway.
fn integral_seconds(value: &serde_json::Value) -> Option<u64> {
    if let Some(seconds) = value.as_u64() {
        return Some(seconds);
    }
    match value.as_f64() {
        Some(seconds) if seconds >= 0.0 && seconds.fract() == 0.0 && seconds.is_finite() => {
            Some(if seconds >= u64::MAX as f64 {
                u64::MAX
            } else {
                seconds as u64
            })
        }
        _ => None,
    }
}

fn parse_watch_list(
    hints: &serde_json::Map<String, serde_json::Value>,
    field: &'static str,
) -> Result<Vec<String>, ReportError> {
    let entries = match hints.get(field) {
        None => return Ok(Vec::new()),
        Some(serde_json::Value::Array(entries)) => entries,
        Some(_) => {
            return Err(ReportError::WrongType {
                field: format!("cache.{field}"),
                expected: "an array of strings",
            });
        }
    };
    if entries.len() > MAX_WATCH_ENTRIES {
        return Err(ReportError::TooManyWatchEntries {
            field,
            count: entries.len(),
        });
    }
    entries
        .iter()
        .map(|entry| match entry {
            serde_json::Value::String(entry) if entry.len() <= MAX_WATCH_ENTRY_BYTES => {
                Ok(entry.clone())
            }
            serde_json::Value::String(_) => Err(ReportError::WatchEntryTooLong { field }),
            _ => Err(ReportError::WrongType {
                field: format!("cache.{field}"),
                expected: "an array of strings",
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use indexmap::IndexSet;

    use super::*;

    fn registration(names: &[&str]) -> DetectorRegistration {
        DetectorRegistration {
            detector: PackageName::try_from("mpi-detect").unwrap(),
            virtual_packages: names
                .iter()
                .map(|name| PackageName::try_from(*name).unwrap())
                .collect::<IndexSet<_>>(),
        }
    }

    fn parse(json: &str, names: &[&str]) -> Result<DetectorReport, ReportError> {
        parse_report(json.as_bytes(), &registration(names))
    }

    #[test]
    fn parses_the_cep_example() {
        let (watch_path_json, watch_path) = if cfg!(windows) {
            (
                r"C:\\opt\\openmpi\\bin\\ompi_info",
                r"C:\opt\openmpi\bin\ompi_info",
            )
        } else {
            ("/opt/openmpi/bin/ompi_info", "/opt/openmpi/bin/ompi_info")
        };
        let report = parse(
            &format!(
                r#"
                {{
                  "version": 1,
                  "virtual_packages": {{
                    "__conda_forge_openmpi": {{ "version": "5.0.10", "build_string": "0" }},
                    "__conda_forge_mpich": null
                  }},
                  "cache": {{
                    "ttl_seconds": 86400,
                    "watch_paths": ["{watch_path_json}"],
                    "watch_env": ["PATH"]
                  }}
                }}
                "#
            ),
            &["__conda_forge_openmpi", "__conda_forge_mpich"],
        )
        .unwrap();
        let openmpi = report.virtual_packages
            [&PackageName::try_from("__conda_forge_openmpi").unwrap()]
            .clone()
            .unwrap();
        assert_eq!(openmpi.version, Version::from_str("5.0.10").unwrap());
        assert_eq!(openmpi.build_string, "0");
        assert!(
            report.virtual_packages[&PackageName::try_from("__conda_forge_mpich").unwrap()]
                .is_none()
        );
        assert_eq!(report.cache.ttl, Some(CacheLifetime::Seconds(86400)));
        assert_eq!(report.cache.watch_paths, [PathBuf::from(watch_path)]);
        assert_eq!(report.cache.watch_env, ["PATH"]);
    }

    #[test]
    fn build_string_defaults_and_unknown_keys_are_ignored() {
        let report = parse(
            r#"{"version": 1, "extra": true, "virtual_packages": {"__a": {"version": "1.2", "note": "x"}}}"#,
            &["__a"],
        )
        .unwrap();
        let a = report.virtual_packages[&PackageName::try_from("__a").unwrap()]
            .clone()
            .unwrap();
        assert_eq!(a.build_string, "0");
        assert_eq!(report.cache, CacheHints::default());
    }

    #[test]
    fn reboot_lifetime() {
        let report = parse(
            r#"{"version": 1, "virtual_packages": {"__a": null}, "cache": {"ttl_seconds": "REBOOT"}}"#,
            &["__a"],
        )
        .unwrap();
        assert_eq!(report.cache.ttl, Some(CacheLifetime::Reboot));
    }

    #[test]
    fn malformed_reports() {
        let long_entry = "/".repeat(MAX_WATCH_ENTRY_BYTES + 1);
        let many = (0..33)
            .map(|i| format!("\"/p{i}\""))
            .collect::<Vec<_>>()
            .join(",");
        let cases: Vec<(&str, String)> = vec![
            ("empty", "   \n".to_string()),
            ("not json", "{".to_string()),
            ("two objects", "{} {}".to_string()),
            ("array", "[]".to_string()),
            ("no version", r#"{"virtual_packages": {"__a": null}}"#.to_string()),
            ("version 2", r#"{"version": 2, "virtual_packages": {"__a": null}}"#.to_string()),
            ("version string", r#"{"version": "1", "virtual_packages": {"__a": null}}"#.to_string()),
            ("no virtual packages", r#"{"version": 1}"#.to_string()),
            ("virtual packages array", r#"{"version": 1, "virtual_packages": []}"#.to_string()),
            ("undeclared", r#"{"version": 1, "virtual_packages": {"__a": null, "__b": null}}"#.to_string()),
            ("missing", r#"{"version": 1, "virtual_packages": {}}"#.to_string()),
            ("invalid name", r#"{"version": 1, "virtual_packages": {"a b": null}}"#.to_string()),
            ("result string", r#"{"version": 1, "virtual_packages": {"__a": "1.0"}}"#.to_string()),
            ("no version field", r#"{"version": 1, "virtual_packages": {"__a": {}}}"#.to_string()),
            ("bad build", r#"{"version": 1, "virtual_packages": {"__a": {"version": "1", "build_string": "a b"}}}"#.to_string()),
            ("empty build", r#"{"version": 1, "virtual_packages": {"__a": {"version": "1", "build_string": ""}}}"#.to_string()),
            ("cache array", r#"{"version": 1, "virtual_packages": {"__a": null}, "cache": []}"#.to_string()),
            ("negative ttl", r#"{"version": 1, "virtual_packages": {"__a": null}, "cache": {"ttl_seconds": -1}}"#.to_string()),
            ("float ttl", r#"{"version": 1, "virtual_packages": {"__a": null}, "cache": {"ttl_seconds": 1.5}}"#.to_string()),
            ("ttl string", r#"{"version": 1, "virtual_packages": {"__a": null}, "cache": {"ttl_seconds": "forever"}}"#.to_string()),
            ("relative path", r#"{"version": 1, "virtual_packages": {"__a": null}, "cache": {"watch_paths": ["bin/x"]}}"#.to_string()),
            ("watch env number", r#"{"version": 1, "virtual_packages": {"__a": null}, "cache": {"watch_env": [1]}}"#.to_string()),
            ("too many", format!(r#"{{"version": 1, "virtual_packages": {{"__a": null}}, "cache": {{"watch_env": [{many}]}}}}"#)),
            ("too long", format!(r#"{{"version": 1, "virtual_packages": {{"__a": null}}, "cache": {{"watch_paths": ["{long_entry}"]}}}}"#)),
        ];
        let errors: Vec<String> = cases
            .iter()
            .map(|(label, json)| format!("{label}: {}", parse(json, &["__a"]).unwrap_err()))
            .collect();
        insta::assert_debug_snapshot!(errors, @r#"
        [
            "empty: the detector wrote no report to standard output",
            "not json: the report is not valid JSON: EOF while parsing an object at line 1 column 1",
            "two objects: the report is not valid JSON: trailing characters at line 1 column 4",
            "array: the report must be a JSON object",
            "no version: the report has no `version` field",
            "version 2: unsupported report version 2, expected 1",
            "version string: unsupported report version \"1\", expected 1",
            "no virtual packages: the report has no `virtual_packages` field",
            "virtual packages array: the report field `virtual_packages` must be an object",
            "undeclared: the report names `__b`, which the registration does not declare",
            "missing: the report omits the registered virtual package `__a`",
            "invalid name: the report names \"a b\", which is not a valid package name",
            "result string: the report field `virtual_packages.__a` must be null or an object",
            "no version field: the report field `virtual_packages.__a.version` must be a string",
            "bad build: the build string \"a b\" of `__a` is invalid",
            "empty build: the build string \"\" of `__a` is invalid",
            "cache array: the report field `cache` must be an object",
            "negative ttl: `cache.ttl_seconds` must be a nonnegative integer or \"REBOOT\", found -1",
            "float ttl: `cache.ttl_seconds` must be a nonnegative integer or \"REBOOT\", found 1.5",
            "ttl string: `cache.ttl_seconds` must be a nonnegative integer or \"REBOOT\", found \"forever\"",
            "relative path: `cache.watch_paths` entry \"bin/x\" is not an absolute path",
            "watch env number: the report field `cache.watch_env` must be an array of strings",
            "too many: `cache.watch_env` has 33 entries, at most 32 are allowed",
            "too long: a `cache.watch_paths` entry is longer than 4096 bytes",
        ]
        "#);
    }

    #[test]
    fn an_invalid_version_is_malformed() {
        let err = parse(
            r#"{"version": 1, "virtual_packages": {"__a": {"version": "not a version!"}}}"#,
            &["__a"],
        )
        .unwrap_err();
        assert!(matches!(err, ReportError::InvalidVersion { name, .. } if name == "__a"));
    }

    #[test]
    fn huge_integer_lifetimes_saturate() {
        let report = parse(
            r#"{"version": 1, "virtual_packages": {"__a": null}, "cache": {"ttl_seconds": 18446744073709551616}}"#,
            &["__a"],
        )
        .unwrap();
        assert_eq!(report.cache.ttl, Some(CacheLifetime::Seconds(u64::MAX)));
    }

    #[test]
    fn names_are_compared_after_normalization() {
        // Registered names are lowercase by construction, but the report may
        // spell a name with different casing; that still refers to the same
        // virtual package and must not be counted twice.
        let err = parse(
            r#"{"version": 1, "virtual_packages": {"__a": null, "__A": null}}"#,
            &["__a"],
        )
        .unwrap_err();
        assert!(matches!(err, ReportError::DuplicateName(name) if name == "__a"));
    }
}
