//! Module that helps with allowing in the creation of python virtual environments.
//! Now just use the python venv command to create the virtual environment.
//! Later on we can look into actually creating the environment by linking to the python library,
//! and creating the necessary files. See: [VEnv](https://packaging.python.org/en/latest/specifications/virtual-environments/#declaring-installation-environments-as-python-virtual-environments)
use crate::python_env::{
    system_python_executable, FindPythonError, ParsePythonInterpreterVersionError,
    PythonInterpreterVersion,
};
use std::fmt::Debug;
use std::path::PathBuf;

/// Specifies where to find the python executable
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PythonLocation {
    /// Use system interpreter
    #[default]
    System,
    /// Use custom interpreter
    Custom(PathBuf),

    /// Use custom interpreter with version
    CustomWithVersion(PathBuf, PythonInterpreterVersion),
}

impl PythonLocation {
    /// Location of python executable
    pub fn executable(&self) -> Result<PathBuf, FindPythonError> {
        match self {
            PythonLocation::System => system_python_executable().cloned(),
            PythonLocation::Custom(path) | PythonLocation::CustomWithVersion(path, _) => {
                Ok(path.clone())
            }
        }
    }

    /// Get python version from executable
    pub fn version(&self) -> Result<PythonInterpreterVersion, ParsePythonInterpreterVersionError> {
        match self {
            PythonLocation::System => PythonInterpreterVersion::from_system(),
            PythonLocation::CustomWithVersion(_, version) => Ok(version.clone()),
            PythonLocation::Custom(path) => PythonInterpreterVersion::from_path(path),
        }
    }
}
