use rattler_conda_types::PackageRecord;

use pyo3::{pyclass, pymethods};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPackageRecord {
    inner: PackageRecord,
}

impl From<PackageRecord> for PyPackageRecord {
    fn from(value: PackageRecord) -> Self {
        Self { inner: value }
    }
}

impl Into<PackageRecord> for PyPackageRecord {
    fn into(self) -> PackageRecord {
        self.inner
    }
}

#[pymethods]
impl PyPackageRecord {
    /// Returns a string representation of PyPackageRecord
    fn as_str(&self) -> String {
        format!("{}", self.inner)
    }
}
