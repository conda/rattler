use rattler_conda_types::PackageRecord;

use pyo3::{pyclass, pymethods};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPackageRecord {
    pub(crate) inner: PackageRecord,
}

impl From<PackageRecord> for PyPackageRecord {
    fn from(value: PackageRecord) -> Self {
        Self { inner: value }
    }
}

impl From<PyPackageRecord> for PackageRecord {
    fn from(val: PyPackageRecord) -> Self {
        val.inner
    }
}

#[pymethods]
impl PyPackageRecord {
    /// Returns a string representation of PyPackageRecord
    fn as_str(&self) -> String {
        format!("{}", self.inner)
    }
}
