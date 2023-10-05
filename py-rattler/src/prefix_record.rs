use pyo3::{
    exceptions::PyTypeError, intern, pyclass, pymethods, FromPyObject, PyAny, PyErr, PyResult,
};
use rattler_conda_types::{prefix_record::PrefixPaths, PrefixRecord};
use std::{path::PathBuf, str::FromStr};

use crate::{
    error::PyRattlerError, package_name::PyPackageName,
    repo_data::repo_data_record::PyRepoDataRecord,
};

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

impl<'a> TryFrom<&'a PyAny> for PyPrefixRecord {
    type Error = PyErr;
    fn try_from(value: &'a PyAny) -> Result<Self, Self::Error> {
        let intern_val = intern!(value.py(), "_record");
        if !value.hasattr(intern_val)? {
            return Err(PyTypeError::new_err(
                "object is not an instance of 'PrefixRecord'",
            ));
        }

        let inner = value.getattr(intern_val)?;
        if !inner.is_instance_of::<Self>() {
            return Err(PyTypeError::new_err("'_record' is invalid"));
        }

        PyPrefixRecord::extract(inner)
    }
}

#[pymethods]
impl PyPrefixRecord {
    /// Creates a new `PrefixRecord` from string.
    #[new]
    pub fn new(source: &str) -> PyResult<Self> {
        Ok(PrefixRecord::from_str(source)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses a PrefixRecord from a file.
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
    /// The data from the repodata
    #[getter]
    pub fn repodata_record(&self) -> PyRepoDataRecord {
        self.inner.repodata_record.clone().into()
    }

    /// Package name of the PrefixRecord.
    #[getter]
    pub fn name(&self) -> PyPackageName {
        self.inner
            .repodata_record
            .package_record
            .name
            .clone()
            .into()
    }

    /// Version of the PrefixRecord.
    #[getter]
    pub fn version(&self) -> String {
        format!("{}", self.inner.repodata_record.package_record.version)
    }

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

    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
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

#[pymethods]
impl PyPrefixPaths {
    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }

    /// The version of the file
    #[getter]
    pub fn paths_version(&self) -> u64 {
        self.inner.paths_version
    }

    /// All entries included in the package.
    #[getter]
    pub fn paths(&self) -> Vec<PathBuf> {
        self.inner
            .paths
            .clone()
            .into_iter()
            .map(|pe| pe.relative_path)
            .collect::<Vec<_>>()
    }
}
