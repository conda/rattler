use std::{path::PathBuf, str::FromStr};

use pyo3::{pyclass, pymethods, PyResult};
use rattler_conda_types::{ExplicitEnvironmentEntry, ExplicitEnvironmentSpec};

use crate::{error::PyRattlerError, platform::PyPlatform};

/// The explicit environment (e.g. env.txt) file that contains a list of
/// all URLs in a environment
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyExplicitEnvironmentSpec {
    pub(crate) inner: ExplicitEnvironmentSpec,
}

impl From<ExplicitEnvironmentSpec> for PyExplicitEnvironmentSpec {
    fn from(value: ExplicitEnvironmentSpec) -> Self {
        Self { inner: value }
    }
}

impl From<PyExplicitEnvironmentSpec> for ExplicitEnvironmentSpec {
    fn from(value: PyExplicitEnvironmentSpec) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyExplicitEnvironmentSpec {
    /// Parses the object from a file specified by a `path`, using a format appropriate for the file
    /// type.
    ///
    /// For example, if the file is in text format, this function reads the data from the file at
    /// the specified path, parses the text and returns the resulting object. If the file is
    /// not in a parsable format or if the file could not be read, this function returns an error.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(ExplicitEnvironmentSpec::from_path(&path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object from a string containing the explicit environment specification
    #[staticmethod]
    pub fn from_str(content: &str) -> PyResult<Self> {
        Ok(ExplicitEnvironmentSpec::from_str(content)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns the platform specified in the explicit environment specification
    pub fn platform(&self) -> Option<PyPlatform> {
        self.inner.platform.map(PyPlatform::from)
    }

    /// Returns the environment entries (URLs) specified in the explicit environment specification
    pub fn packages(&self) -> Vec<PyExplicitEnvironmentEntry> {
        self.inner
            .packages
            .iter()
            .cloned()
            .map(PyExplicitEnvironmentEntry)
            .collect()
    }
}

/// A Python wrapper around an explicit environment entry which represents a URL to a package
#[pyclass]
#[derive(Clone)]
pub struct PyExplicitEnvironmentEntry(pub(crate) ExplicitEnvironmentEntry);

#[pymethods]
impl PyExplicitEnvironmentEntry {
    /// Returns the URL of the package
    pub fn url(&self) -> String {
        self.0.url.to_string()
    }
}

impl From<ExplicitEnvironmentEntry> for PyExplicitEnvironmentEntry {
    fn from(value: ExplicitEnvironmentEntry) -> Self {
        Self(value)
    }
}

impl From<PyExplicitEnvironmentEntry> for ExplicitEnvironmentEntry {
    fn from(value: PyExplicitEnvironmentEntry) -> Self {
        value.0
    }
}
