use pyo3::{pyclass, pymethods};
use rattler_conda_types::RepoDataRecord;

use super::package_record::PyPackageRecord;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyRepoDataRecord {
    pub(crate) inner: RepoDataRecord,
}

impl From<PyRepoDataRecord> for RepoDataRecord {
    fn from(value: PyRepoDataRecord) -> Self {
        value.inner
    }
}

impl From<RepoDataRecord> for PyRepoDataRecord {
    fn from(value: RepoDataRecord) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyRepoDataRecord {
    /// The data stored in the repodata.json.
    #[getter]
    pub fn package_record(&self) -> PyPackageRecord {
        self.inner.package_record.clone().into()
    }

    /// The filename of the package.
    #[getter]
    pub fn file_name(&self) -> String {
        self.inner.file_name.clone()
    }

    /// The canonical URL from where to get this package.
    #[getter]
    pub fn url(&self) -> String {
        self.inner.url.to_string()
    }

    /// String representation of the channel where the
    /// package comes from. This could be a URL but it
    /// could also be a channel name.
    #[getter]
    pub fn channel(&self) -> String {
        self.inner.channel.clone()
    }

    /// Returns a string representation of PyRepoDataRecord.
    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }
}
