use std::path::PathBuf;

use pyo3::{pyclass, pymethods, PyResult};
use rattler_conda_types::{RepoData, RepoDataRecord};

use crate::{channel::PyChannel, error::PyRattlerError};

pub mod package_record;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyRepoData {
    pub(crate) inner: RepoData,
}

impl From<PyRepoData> for RepoData {
    fn from(value: PyRepoData) -> Self {
        value.inner
    }
}

impl From<RepoData> for PyRepoData {
    fn from(value: RepoData) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyRepoData {
    pub fn into_repo_data_records(&self, channel: &PyChannel) -> Vec<RepoDataRecord> {
        todo!()
    }
}

#[pymethods]
impl PyRepoData {
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(RepoData::from_path(path)
            .map(Into::into)
            .map_err(PyRattlerError::IoError)?)
    }
}
