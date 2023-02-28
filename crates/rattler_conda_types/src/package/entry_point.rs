//! A struct for a single Python entry point.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Display;
use std::str::FromStr;

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
#[derive(Debug, Clone)]
pub struct EntryPoint {
    /// The name of the command that will be available on the command line.
    pub command: String,

    /// The name of the Python module that contains the function.
    pub module: String,

    /// The name of the function to run.
    pub function: String,
}

impl FromStr for EntryPoint {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (command, module_and_function) =
            s.split_once('=').ok_or("missing entry point separator")?;
        let (module, function) = module_and_function
            .split_once(':')
            .ok_or("missing module and function separator")?;

        Ok(EntryPoint {
            command: command.trim().to_string(),
            module: module.trim().to_string(),
            function: function.trim().to_string(),
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
}
