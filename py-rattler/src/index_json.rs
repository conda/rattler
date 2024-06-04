use std::path::PathBuf;

use pyo3::{pyclass, pymethods, PyResult};
use rattler_conda_types::package::{IndexJson, PackageFile};
use rattler_package_streaming::seek::read_package_file;

use crate::{error::PyRattlerError, package_name::PyPackageName};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyIndexJson {
    pub(crate) inner: IndexJson,
}

impl From<IndexJson> for PyIndexJson {
    fn from(value: IndexJson) -> Self {
        Self { inner: value }
    }
}

impl From<PyIndexJson> for IndexJson {
    fn from(value: PyIndexJson) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyIndexJson {
    /// Parses the package file from archive.
    /// Note: If you want to extract multiple `info/*` files then this will be slightly
    ///       slower than manually iterating over the archive entries with
    ///       custom logic as this skips over the rest of the archive
    #[staticmethod]
    pub fn from_package_archive(path: PathBuf) -> PyResult<Self> {
        Ok(read_package_file::<IndexJson>(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the package file from a path.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(IndexJson::from_path(path)
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
        Ok(IndexJson::from_package_directory(path)
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
        Ok(IndexJson::from_str(str)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns the path to the file within the Conda archive.
    ///
    /// The path is relative to the root of the archive and include any necessary directories.
    #[staticmethod]
    pub fn package_path() -> PathBuf {
        IndexJson::package_path().to_owned()
    }

    /// Optionally, the architecture the package is build for.
    #[getter]
    pub fn arch(&self) -> Option<String> {
        self.inner.arch.clone()
    }

    /// The build string of the package.
    #[getter]
    pub fn build(&self) -> String {
        self.inner.build.clone()
    }

    /// The build number of the package. This is also included in the build string.
    #[getter]
    pub fn build_number(&self) -> u64 {
        self.inner.build_number
    }

    /// The package constraints of the package
    #[getter]
    pub fn constrains(&self) -> Vec<String> {
        self.inner.constrains.clone()
    }

    /// The dependencies of the package
    #[getter]
    pub fn depends(&self) -> Vec<String> {
        self.inner.depends.clone()
    }

    /// Features are a deprecated way to specify different feature sets for the conda solver. This is not
    /// supported anymore and should not be used. Instead, `mutex` packages should be used to specify
    /// mutually exclusive features.
    #[getter]
    pub fn features(&self) -> Option<String> {
        self.inner.features.clone()
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

    /// The lowercase name of the package
    #[getter]
    pub fn name(&self) -> PyPackageName {
        self.inner.name.clone().into()
    }

    /// Optionally, the OS the package is build for.
    #[getter]
    pub fn platform(&self) -> Option<String> {
        self.inner.platform.clone()
    }

    /// The subdirectory that contains this package
    #[getter]
    pub fn subdir(&self) -> Option<String> {
        self.inner.subdir.clone()
    }

    /// The timestamp when this package was created
    #[getter]
    pub fn timestamp(&self) -> Option<i64> {
        self.inner.timestamp.map(|time| time.timestamp_millis())
    }

    /// Track features are nowadays only used to downweight packages (ie. give them less priority). To
    /// that effect, the number of track features is counted (number of commas) and the package is downweighted
    /// by the number of track_features.
    #[getter]
    pub fn track_features(&self) -> Vec<String> {
        self.inner.track_features.clone()
    }

    /// The version of the package
    #[getter]
    pub fn version(&self) -> String {
        self.inner.version.as_str().into_owned()
    }
}
