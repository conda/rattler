//! A struct for a single Python entry point.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Display;
use std::path::Path;
use std::str::FromStr;
use thiserror::Error;

/// Which sub-field of an entry-point string is being validated.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum EntryPointDottedField {
    /// The module name (left of `:`).
    Module,
    /// The function name (right of `:`).
    Function,
}

impl Display for EntryPointDottedField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntryPointDottedField::Module => f.write_str("module"),
            EntryPointDottedField::Function => f.write_str("function"),
        }
    }
}

/// Errors returned by [`EntryPoint::from_str`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseEntryPointError {
    /// Missing `=` between command and module/function.
    #[error("missing entry point separator '='")]
    MissingCommandSeparator,

    /// Missing `:` between module and function.
    #[error("missing module and function separator ':'")]
    MissingFunctionSeparator,

    /// Command was empty after trimming.
    #[error("entry point command must be non-empty")]
    EmptyCommand,

    /// Command contains `/`, `\`, or NUL — would let the linker write
    /// outside the prefix.
    #[error("entry point command must not contain path separators or NUL: {0:?}")]
    CommandContainsPathSeparator(String),

    /// Command is `.`, `..`, or starts with `.`.
    #[error("entry point command must not be a relative-traversal token or hidden name: {0:?}")]
    CommandIsTraversal(String),

    /// Command is an absolute path — would discard the prefix on join.
    #[error("entry point command must be a relative simple name: {0:?}")]
    CommandIsAbsolute(String),

    /// Module or function was empty after trimming.
    #[error("entry point {0} must be non-empty")]
    EmptyDottedName(EntryPointDottedField),

    /// Module or function is not a valid Python dotted identifier.
    /// Rejected at parse time because these are interpolated verbatim
    /// into the generated script body.
    #[error("entry point {field} must be a Python dotted identifier: {name:?}")]
    InvalidDottedName {
        /// Which field failed validation.
        field: EntryPointDottedField,
        /// The offending value.
        name: String,
    },
}

/// Rejects command names that would let the linker write outside the
/// prefix when joined onto `bin_dir`.
fn validate_entry_point_command(cmd: &str) -> Result<(), ParseEntryPointError> {
    if cmd.is_empty() {
        return Err(ParseEntryPointError::EmptyCommand);
    }
    if cmd.chars().any(|c| matches!(c, '/' | '\\' | '\0')) {
        return Err(ParseEntryPointError::CommandContainsPathSeparator(
            cmd.to_string(),
        ));
    }
    if cmd == "." || cmd == ".." || cmd.starts_with('.') {
        return Err(ParseEntryPointError::CommandIsTraversal(cmd.to_string()));
    }
    if Path::new(cmd).is_absolute() {
        return Err(ParseEntryPointError::CommandIsAbsolute(cmd.to_string()));
    }
    Ok(())
}

/// Restricts `module` / `function` to Python dotted identifiers,
/// preventing code injection through the script-body interpolation.
fn validate_python_dotted_name(
    name: &str,
    field: EntryPointDottedField,
) -> Result<(), ParseEntryPointError> {
    if name.is_empty() {
        return Err(ParseEntryPointError::EmptyDottedName(field));
    }
    let is_valid_part = |part: &str| -> bool {
        let mut chars = part.chars();
        match chars.next() {
            Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
            _ => return false,
        }
        chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    };
    if !name.split('.').all(is_valid_part) {
        return Err(ParseEntryPointError::InvalidDottedName {
            field,
            name: name.to_string(),
        });
    }
    Ok(())
}

/// A struct for a single Python entry point. An entry point is a command that
/// runs a function in a Python module. For example, the entry point
/// `jlpm = jupyterlab.jlpmapp:main` will run the `main` function in the
/// `jupyterlab.jlpmapp` module when the command `jlpm` is run on the command line.
/// The main usage for entry points is in `noarch: python` packages.
///
/// The entry point is represented as a string in the format
/// `<command> = <module>:<function>`. The command is the name of the command that
/// will be run on the command line. The module is the name of the Python module
/// that contains the function. The function is the name of the function to run.
///
/// The entry point is parsed from a string using the [`FromStr`] trait. The
/// [`Display`] trait is implemented for the entry point to convert it back to a
/// string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntryPoint {
    /// The name of the command that will be available on the command line.
    pub command: String,

    /// The name of the Python module that contains the function.
    pub module: String,

    /// The name of the function to run.
    pub function: String,
}

impl FromStr for EntryPoint {
    type Err = ParseEntryPointError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (command, module_and_function) = s
            .split_once('=')
            .ok_or(ParseEntryPointError::MissingCommandSeparator)?;
        let (module, function) = module_and_function
            .split_once(':')
            .ok_or(ParseEntryPointError::MissingFunctionSeparator)?;

        let command = command.trim().to_string();
        let module = module.trim().to_string();
        let function = function.trim().to_string();

        validate_entry_point_command(&command)?;
        validate_python_dotted_name(&module, EntryPointDottedField::Module)?;
        validate_python_dotted_name(&function, EntryPointDottedField::Function)?;

        Ok(EntryPoint {
            command,
            module,
            function,
        })
    }
}

impl Display for EntryPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} = {}:{}", self.command, self.module, self.function)
    }
}

impl<'de> Deserialize<'de> for EntryPoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        FromStr::from_str(&s).map_err(de::Error::custom)
    }
}

impl Serialize for EntryPoint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod test {
    use super::EntryPoint;
    use std::str::FromStr;

    #[test]
    fn test_entry_point() {
        let entry_point = EntryPoint::from_str("jlpm = jupyterlab.jlpmapp:main").unwrap();
        assert_eq!(entry_point.command, "jlpm");
        assert_eq!(entry_point.module, "jupyterlab.jlpmapp");
        assert_eq!(entry_point.function, "main");

        let entry_point = EntryPoint::from_str("jupyter=jupyterlab.jupyterapp:main").unwrap();
        assert_eq!(entry_point.command, "jupyter");
        assert_eq!(entry_point.module, "jupyterlab.jupyterapp");
        assert_eq!(entry_point.function, "main");

        insta::assert_yaml_snapshot!(entry_point);
    }

    #[test]
    fn test_entry_point_rejects_path_traversal_in_command() {
        let cases = [
            "../bin/pip = innocuous_pkg.evil:main",
            "../../../etc/passwd = innocuous_pkg.evil:main",
            "/tmp/PWN = innocuous_pkg.evil:main",
            "..\\..\\..\\AppData\\Roaming\\evil = innocuous_pkg.evil:main",
            "foo/bar = innocuous_pkg.evil:main",
            "foo\\bar = innocuous_pkg.evil:main",
            ".hidden = innocuous_pkg.evil:main",
            ".. = innocuous_pkg.evil:main",
            ". = innocuous_pkg.evil:main",
            "\0 = innocuous_pkg.evil:main",
            " = innocuous_pkg.evil:main",
        ];
        for case in cases {
            assert!(
                EntryPoint::from_str(case).is_err(),
                "expected rejection for entry point: {case:?}",
            );
        }
    }

    #[test]
    fn test_entry_point_rejects_python_code_injection() {
        let cases = [
            "jlpm = jupyterlab.jlpmapp:main(); __import__('os').system('x')",
            "jlpm = jupyterlab.jlpmapp; __import__('os').system('x'):main",
            "jlpm = ../evil:main",
            "jlpm = jupyterlab.jlpmapp:1main",
        ];
        for case in cases {
            assert!(
                EntryPoint::from_str(case).is_err(),
                "expected rejection for entry point: {case:?}",
            );
        }
    }

    #[test]
    fn test_entry_point_accepts_legitimate_names() {
        for s in [
            "jlpm = jupyterlab.jlpmapp:main",
            "jupyter-lab = jupyterlab.labapp:main",
            "pip3.11 = pip._internal.cli.main:main",
            "_private = pkg.mod:func",
            "tool = pkg:Class.method",
        ] {
            assert!(
                EntryPoint::from_str(s).is_ok(),
                "expected acceptance for entry point: {s:?}",
            );
        }
    }
}
