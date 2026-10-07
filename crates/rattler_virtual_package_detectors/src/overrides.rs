//! The `CONDA_OVERRIDE_*` environment variables of detector-provided virtual
//! packages.
//!
//! An override replaces the whole result for its name: a nonempty value is a
//! version, optionally followed by `=` and a build string; an empty value
//! means the virtual package is absent; an invalid value is an error rather
//! than a fallback to detection. Standardized names keep the meaning their
//! own CEPs give their variables: `CONDA_OVERRIDE_ARCHSPEC` sets the build
//! string and `CONDA_OVERRIDE_UNIX` has no effect.

use std::{ffi::OsString, str::FromStr};

use rattler_conda_types::{
    ParseVersionError, Version, virtual_package_detector::VirtualPackageName,
};
use rattler_shell::environment::EnvironmentSnapshot;
use thiserror::Error;

use crate::report::DetectedVersion;

/// What an override variable says about its virtual package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverrideValue {
    /// The variable is empty: the virtual package is absent.
    Absent,
    /// The variable names a version and build string.
    Present(DetectedVersion),
}

/// Why an override variable could not be used.
#[derive(Debug, Error)]
pub enum OverrideError {
    /// The value is not valid UTF-8.
    #[error("the value of {variable} is not valid UTF-8")]
    NotUtf8 {
        /// The variable's name.
        variable: String,
    },

    /// The value's version part does not parse.
    #[error("the value of {variable} is not a version: {source}")]
    InvalidVersion {
        /// The variable's name.
        variable: String,
        /// The parse error.
        #[source]
        source: ParseVersionError,
    },

    /// The value's build string part is empty or contains invalid characters.
    #[error("the build string {build_string:?} in {variable} is invalid")]
    InvalidBuildString {
        /// The variable's name.
        variable: String,
        /// The offending build string.
        build_string: String,
    },
}

/// Reads the override for `name` from an explicit environment snapshot.
///
/// Returns `Ok(None)` when the variable is unset.
pub fn read_override(
    name: &VirtualPackageName,
    environment: &EnvironmentSnapshot,
) -> Result<Option<OverrideValue>, OverrideError> {
    let variable = name.override_variable();
    parse_override_for(
        name,
        &variable,
        environment.get(&variable).map(OsString::from),
    )
}

/// Parses the raw value of `variable` for `name`, `None` meaning unset,
/// honoring the rules of standardized names.
pub fn parse_override_for(
    name: &VirtualPackageName,
    variable: &str,
    value: Option<OsString>,
) -> Result<Option<OverrideValue>, OverrideError> {
    match name.as_normalized() {
        // CEP 30: the variable has no effect.
        "__unix" => Ok(None),
        // CEP 30: the variable names the microarchitecture, which is the build
        // string; the version is always `1`.
        "__archspec" => {
            let Some(value) = value else {
                return Ok(None);
            };
            let value = value.into_string().map_err(|_raw| OverrideError::NotUtf8 {
                variable: variable.to_string(),
            })?;
            if value.is_empty() {
                return Ok(Some(OverrideValue::Absent));
            }
            validate_build_string(variable, &value)?;
            Ok(Some(OverrideValue::Present(DetectedVersion {
                version: Version::from_str("1").expect("a literal version"),
                build_string: value,
            })))
        }
        _ => parse_override(variable, value),
    }
}

/// Parses the raw value of `variable` with the generic grammar, `None`
/// meaning unset.
pub fn parse_override(
    variable: &str,
    value: Option<OsString>,
) -> Result<Option<OverrideValue>, OverrideError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.into_string().map_err(|_raw| OverrideError::NotUtf8 {
        variable: variable.to_string(),
    })?;
    if value.is_empty() {
        return Ok(Some(OverrideValue::Absent));
    }
    let value = value.as_str();
    let (version, build_string) = match value.split_once('=') {
        Some((version, build_string)) => (version, build_string),
        None => (value, "0"),
    };
    let version = Version::from_str(version).map_err(|source| OverrideError::InvalidVersion {
        variable: variable.to_string(),
        source,
    })?;
    validate_build_string(variable, build_string)?;
    Ok(Some(OverrideValue::Present(DetectedVersion {
        version,
        build_string: build_string.to_string(),
    })))
}

fn validate_build_string(variable: &str, build_string: &str) -> Result<(), OverrideError> {
    if build_string.is_empty()
        || !build_string
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return Err(OverrideError::InvalidBuildString {
            variable: variable.to_string(),
            build_string: build_string.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    fn parse(value: Option<&str>) -> Result<Option<OverrideValue>, OverrideError> {
        parse_override("CONDA_OVERRIDE_CONDA_FORGE_MPI", value.map(OsString::from))
    }

    #[test]
    fn unset_empty_and_versions() {
        assert_eq!(parse(None).unwrap(), None);
        assert_eq!(parse(Some("")).unwrap(), Some(OverrideValue::Absent));
        // Only an empty value means absence; whitespace is not a version.
        assert!(parse(Some("  ")).is_err());
        assert_eq!(
            parse(Some("5.0.10")).unwrap(),
            Some(OverrideValue::Present(DetectedVersion {
                version: Version::from_str("5.0.10").unwrap(),
                build_string: "0".to_string(),
            }))
        );
        assert_eq!(
            parse(Some("5.0.10=h1")).unwrap(),
            Some(OverrideValue::Present(DetectedVersion {
                version: Version::from_str("5.0.10").unwrap(),
                build_string: "h1".to_string(),
            }))
        );
    }

    #[test]
    fn invalid_values_are_errors() {
        assert!(matches!(
            parse(Some("not a version!")).unwrap_err(),
            OverrideError::InvalidVersion { .. }
        ));
        assert!(matches!(
            parse(Some("=x")).unwrap_err(),
            OverrideError::InvalidVersion { .. }
        ));
        insta::assert_debug_snapshot!(
            ["1.0=", "1.0=a b"].map(|value| parse(Some(value)).unwrap_err().to_string()),
            @r#"
        [
            "the build string \"\" in CONDA_OVERRIDE_CONDA_FORGE_MPI is invalid",
            "the build string \"a b\" in CONDA_OVERRIDE_CONDA_FORGE_MPI is invalid",
        ]
        "#
        );
    }

    #[test]
    fn standardized_names_keep_their_own_rules() {
        let archspec = VirtualPackageName::try_from("__archspec").unwrap();
        assert_eq!(
            parse_override_for(&archspec, "CONDA_OVERRIDE_ARCHSPEC", Some("zen3".into())).unwrap(),
            Some(OverrideValue::Present(DetectedVersion {
                version: Version::from_str("1").unwrap(),
                build_string: "zen3".to_string(),
            }))
        );
        assert_eq!(
            parse_override_for(&archspec, "CONDA_OVERRIDE_ARCHSPEC", Some("".into())).unwrap(),
            Some(OverrideValue::Absent)
        );
        let unix = VirtualPackageName::try_from("__unix").unwrap();
        assert_eq!(
            parse_override_for(&unix, "CONDA_OVERRIDE_UNIX", Some("1".into())).unwrap(),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_overrides_keep_native_values_and_distinguish_unset_and_empty() {
        let name = VirtualPackageName::try_from("__test_native").unwrap();
        let mut environment = EnvironmentSnapshot::default();
        assert_eq!(read_override(&name, &environment).unwrap(), None);
        environment.insert(name.override_variable(), "");
        assert_eq!(
            read_override(&name, &environment).unwrap(),
            Some(OverrideValue::Absent)
        );
        environment.insert(name.override_variable(), OsString::from_vec(vec![0xff]));
        assert!(matches!(
            read_override(&name, &environment),
            Err(OverrideError::NotUtf8 { .. })
        ));
        assert_eq!(
            read_override(&name, &EnvironmentSnapshot::default()).unwrap(),
            None
        );
    }
}
