use pyo3::{pyclass, pymethods, PyResult};
use rattler_conda_types::{prefix_record::PrefixPaths, PrefixRecord};
use std::{path::PathBuf, str::FromStr};

use crate::error::PyRattlerError;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPrefixRecord {
    pub(crate) inner: PrefixRecord,
}

impl From<PrefixRecord> for PyPrefixRecord {
    fn from(value: PrefixRecord) -> Self {
        Self { inner: value }
    }
}

impl From<PyPrefixRecord> for PrefixRecord {
    fn from(value: PyPrefixRecord) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPrefixRecord {
    /// Creates a new `PrefixRecord` from string.
    #[new]
    pub fn new(source: String) -> PyResult<Self> {
        Ok(PrefixRecord::from_str(source.as_ref())
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses a `paths.json` file from a file.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(PrefixRecord::from_path(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Writes the contents of this instance to the file at the specified location.
    pub fn write_to_path(&self, path: PathBuf, pretty: bool) -> PyResult<()> {
        Ok(self
            .inner
            .to_owned()
            .write_to_path(path, pretty)
            .map_err(PyRattlerError::from)?)
    }

    // TODO: uncomment after merging fetch_repo_data
    // /// The data from the repodata
    // #[getter]
    // pub fn repodata_record(&self) -> PyRepoDataRecord {
    //     self.inner.repodata_record.clone().into()
    // }

    /// The path to where the archive of the package was stored on disk.
    #[getter]
    pub fn package_tarball_full_path(&self) -> Option<PathBuf> {
        self.inner.package_tarball_full_path.clone()
    }

    /// The path that contains the extracted package content.
    #[getter]
    pub fn extracted_package_dir(&self) -> Option<PathBuf> {
        self.inner.extracted_package_dir.clone()
    }

    /// A sorted list of all files included in this package
    #[getter]
    pub fn files(&self) -> Vec<PathBuf> {
        self.inner.files.clone()
    }

    /// Information about how files have been linked when installing the package.
    #[getter]
    pub fn paths_data(&self) -> PyPrefixPaths {
        self.inner.paths_data.clone().into()
    }

    /// The spec that was used when this package was installed. Note that this field is not updated if the
    /// currently another spec was used.
    #[getter]
    pub fn requested_spec(&self) -> Option<String> {
        self.inner.requested_spec.clone()
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPrefixPaths {
    pub(crate) inner: PrefixPaths,
}

impl From<PyPrefixPaths> for PrefixPaths {
    fn from(value: PyPrefixPaths) -> Self {
        value.inner
    }
}

impl From<PrefixPaths> for PyPrefixPaths {
    fn from(value: PrefixPaths) -> Self {
        Self { inner: value }
    }
}
