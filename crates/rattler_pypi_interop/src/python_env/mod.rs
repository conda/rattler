//! Module for working with python environments.
//!
//! Contains functionality for querying python environments, including collecting metadata
//! about packages installed in a python environment.
//!
//! Example of querying a python environment for installed packages (i.e. distributions):
//!
//! ```rust
//! use std::path::Path;
//! use rattler_pypi_interop::types::InstallPaths;
//! use rattler_pypi_interop::python_env::{
//!     find_distributions_in_venv,
//!     Distribution,
//! };
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let venv_path = Path::new(env!("CARGO_MANIFEST_DIR"))
//!         .join(Path::new("test-data/find_distributions"));
//!     let windows = false;
//!     let install_paths = InstallPaths::for_venv((3, 8, 5), windows);
//!
//!     let distributions = find_distributions_in_venv(&venv_path, &install_paths)?;
//!
//!     /// Print all distributions found in the virtual environment.
//!     println!("{:?}", distributions);
//!
//!     Ok(())
//! }
//! ```

mod byte_code_compiler;
mod distribution_finder;
mod env_markers;
mod system_python;
mod tags;
mod uninstall;
mod venv;

pub use tags::{WheelTag, WheelTags};

pub use byte_code_compiler::{ByteCodeCompiler, CompilationError, SpawnCompilerError};
pub use distribution_finder::{
    find_distributions_in_directory, find_distributions_in_venv, Distribution,
    FindDistributionError,
};
pub use env_markers::Pep508EnvMakers;
pub(crate) use system_python::{system_python_executable, FindPythonError};
pub use system_python::{ParsePythonInterpreterVersionError, PythonInterpreterVersion};
pub use uninstall::{uninstall_distribution, UninstallDistributionError};
pub use venv::PythonLocation;
