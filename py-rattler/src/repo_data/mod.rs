use std::path::PathBuf;

use pyo3::{pyclass, pymethods, PyResult};
use rattler_conda_types::RepoData;

use crate::{channel::PyChannel, error::PyRattlerError, record::PyRecord};

use patch_instructions::PyPatchInstructions;

pub mod gateway;
pub mod patch_instructions;
pub mod sparse;

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
    /// Apply a patch to a repodata file Note that we currently do not handle revoked instructions.
    pub fn apply_patches(&mut self, instructions: &PyPatchInstructions) {
        self.inner.apply_patches(&instructions.inner);
    }

    /// Gets the string representation of the `PyRepoData`.
    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }
}

#[pymethods]
impl PyRepoData {
    /// Parses RepoData from a file.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(RepoData::from_path(path)
            .map(Into::into)
            .map_err(PyRattlerError::IoError)?)
    }

    /// Builds a `Vec<PyRecord>` from the packages in a `PyRepoData` given the source of the data.
    #[staticmethod]
    pub fn repo_data_to_records(repo_data: Self, channel: &PyChannel) -> Vec<PyRecord> {
        repo_data
            .inner
            .into_repo_data_records(&channel.inner)
            .into_iter()
            .map(Into::into)
            .collect()
    }
}
