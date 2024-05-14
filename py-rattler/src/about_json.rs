use std::path::PathBuf;

use pyo3::{pyclass, pymethods, PyResult};
use rattler_conda_types::package::{AboutJson, PackageFile};

use crate::error::PyRattlerError;

/// The `about.json` file contains metadata about the package
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyAboutJson {
    pub(crate) inner: AboutJson,
}

impl From<AboutJson> for PyAboutJson {
    fn from(value: AboutJson) -> Self {
        Self { inner: value }
    }
}

impl From<PyAboutJson> for AboutJson {
    fn from(value: PyAboutJson) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyAboutJson {
    /// Parses the object from a file specified by a `path`, using a format appropriate for the file
    /// type.
    ///
    /// For example, if the file is in JSON format, this function reads the data from the file at
    /// the specified path, parse the JSON string and return the resulting object. If the file is
    /// not in a parsable format or if the file could not read, this function returns an error.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(AboutJson::from_path(path)
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
        Ok(AboutJson::from_package_directory(path)
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
        Ok(AboutJson::from_str(str)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns the path to the file within the Conda archive.
    ///
    /// The path is relative to the root of the archive and include any necessary directories.
    #[staticmethod]
    pub fn package_path() -> PathBuf {
        AboutJson::package_path().to_owned()
    }

    /// A list of channels that where used during the build
    #[getter]
    pub fn channels(&self) -> Vec<String> {
        self.inner.channels.clone()
    }

    /// Description of the package
    #[getter]
    pub fn description(&self) -> Option<String> {
        self.inner.description.clone()
    }

    /// URL to the development page of the package
    #[getter]
    pub fn dev_url(&self) -> Vec<String> {
        self.inner
            .dev_url
            .clone()
            .into_iter()
            .map(|url| url.to_string())
            .collect()
    }

    /// URL to the documentation of the package
    #[getter]
    pub fn doc_url(&self) -> Vec<String> {
        self.inner
            .doc_url
            .clone()
            .into_iter()
            .map(|url| url.to_string())
            .collect()
    }

    /// URL to the homepage of the package
    #[getter]
    pub fn home(&self) -> Vec<String> {
        self.inner
            .home
            .clone()
            .into_iter()
            .map(|url| url.to_string())
            .collect()
    }

    /// Optionally, the license
    #[getter]
    pub fn license(&self) -> Option<String> {
        self.inner.license.clone()
    }

    /// Optionally, the license family
    #[getter]
    pub fn license_family(&self) -> Option<String> {
        self.inner.license_family.clone()
    }

    /// URL to the latest source code of the package
    #[getter]
    pub fn source_url(&self) -> Option<String> {
        self.inner.source_url.clone().map(|v| v.to_string())
    }

    /// Short summary description
    #[getter]
    pub fn summary(&self) -> Option<String> {
        self.inner.summary.clone()
    }
}
