use std::path::PathBuf;

use pyo3::{pyclass, pymethods, PyResult};
use rattler_conda_types::{ChannelInfo, ChannelRelations, RepoData};

use crate::{channel::PyChannel, error::PyRattlerError, record::PyRecord};

use patch_instructions::PyPatchInstructions;

pub mod gateway;
pub mod patch_instructions;
pub mod source;
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

    /// Returns the channel info contained in the repodata, if any.
    #[getter]
    pub fn info(&self) -> Option<PyChannelInfo> {
        self.inner.info.clone().map(Into::into)
    }

    /// Returns the repodata format version, if any.
    #[getter]
    pub fn version(&self) -> Option<u64> {
        self.inner.version
    }
}

#[pymethods]
impl PyRepoData {
    /// Parses `RepoData` from a file.
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

/// Python wrapper around [`ChannelInfo`] — the `info` section of a
/// `repodata.json` file.
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyChannelInfo {
    pub(crate) inner: ChannelInfo,
}

impl From<ChannelInfo> for PyChannelInfo {
    fn from(value: ChannelInfo) -> Self {
        Self { inner: value }
    }
}

impl From<PyChannelInfo> for ChannelInfo {
    fn from(value: PyChannelInfo) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyChannelInfo {
    #[getter]
    pub fn subdir(&self) -> Option<String> {
        self.inner.subdir.clone()
    }

    #[getter]
    pub fn base_url(&self) -> Option<String> {
        self.inner.base_url.clone()
    }

    /// Channel relations declared by this channel (CEP-42). `None` when the
    /// channel does not declare any relations.
    #[getter]
    pub fn channel_relations(&self) -> Option<PyChannelRelations> {
        self.inner.channel_relations.clone().map(Into::into)
    }
}

/// Python wrapper around [`ChannelRelations`] — see
/// [CEP-42](https://github.com/conda/ceps/blob/main/cep-0042.md).
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyChannelRelations {
    pub(crate) inner: ChannelRelations,
}

impl From<ChannelRelations> for PyChannelRelations {
    fn from(value: ChannelRelations) -> Self {
        Self { inner: value }
    }
}

impl From<PyChannelRelations> for ChannelRelations {
    fn from(value: PyChannelRelations) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyChannelRelations {
    #[getter]
    pub fn base(&self) -> Option<String> {
        self.inner.base.clone()
    }

    #[getter]
    pub fn overrides(&self) -> Option<String> {
        self.inner.overrides.clone()
    }
}
