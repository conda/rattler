use pyo3::{pyclass, pymethods, PyResult};
use rattler_conda_types::package::{PackageFile, RunExportsJson};
use rattler_package_streaming::seek::read_package_file;
use std::path::PathBuf;

use crate::error::PyRattlerError;

/// A representation of the `run_exports.json` file found in package archives.
///
/// The `run_exports.json` file contains information about the run exports of a package
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyRunExportsJson {
    pub(crate) inner: RunExportsJson,
}

impl From<RunExportsJson> for PyRunExportsJson {
    fn from(value: RunExportsJson) -> Self {
        Self { inner: value }
    }
}

impl From<PyRunExportsJson> for RunExportsJson {
    fn from(value: PyRunExportsJson) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyRunExportsJson {
    /// Parses the package file from archive.
    /// Note: If you want to extract multiple `info/*` files then this will be slightly
    ///       slower than manually iterating over the archive entries with
    ///       custom logic as this skips over the rest of the archive
    #[staticmethod]
    pub fn from_package_archive(path: PathBuf) -> PyResult<Self> {
        Ok(read_package_file::<RunExportsJson>(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object from a file specified by a `path`, using a format appropriate for the file
    /// type.
    ///
    /// For example, if the file is in JSON format, this function reads the data from the file at
    /// the specified path, parse the JSON string and return the resulting object. If the file is
    /// not in a parsable format or if the file could not read, this function returns an error.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(RunExportsJson::from_path(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object by looking up the appropriate file from the root of the specified Conda
    /// archive directory, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function reads the appropriate file from
    /// the archive, parse the JSON string and return the resulting object. If the file is not in a
    /// parsable format or if the file could not be read, this function returns an error.
    #[staticmethod]
    pub fn from_package_directory(path: PathBuf) -> PyResult<Self> {
        Ok(RunExportsJson::from_package_directory(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object from a string, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function parses the JSON string and returns
    /// the resulting object. If the file is not in a parsable format, this function returns an
    /// error.
    #[staticmethod]
    pub fn from_str(str: &str) -> PyResult<Self> {
        Ok(RunExportsJson::from_str(str)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns the path to the file within the Conda archive.
    ///
    /// The path is relative to the root of the archive and include any necessary directories.
    #[staticmethod]
    pub fn package_path() -> PathBuf {
        RunExportsJson::package_path().to_owned()
    }

    /// Weak run exports apply a dependency from host to run.
    #[getter]
    pub fn weak(&self) -> Vec<String> {
        self.inner.weak.clone()
    }

    /// Strong run exports apply a dependency from build to host and run.
    #[getter]
    pub fn strong(&self) -> Vec<String> {
        self.inner.strong.clone()
    }

    /// NoArch run exports apply a run export only to noarch packages (other run exports are ignored).
    /// For example, python uses this to apply a dependency on python to all noarch packages, but not to
    /// the python_abi package.
    #[getter]
    pub fn noarch(&self) -> Vec<String> {
        self.inner.noarch.clone()
    }

    /// Weak constrains apply a constrain dependency from host to build, or run to host.
    #[getter]
    pub fn weak_constrains(&self) -> Vec<String> {
        self.inner.weak_constrains.clone()
    }

    /// Strong constrains apply a constrain dependency from build to host and run.
    #[getter]
    pub fn strong_constrains(&self) -> Vec<String> {
        self.inner.strong_constrains.clone()
    }
}
