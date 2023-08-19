use crate::PyRattlerError;
use pyo3::{pyclass, pymethods};
use rattler_conda_types::Version;
use std::str::FromStr;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyVersion {
    inner: Version,
}

impl From<Version> for PyVersion {
    fn from(value: Version) -> Self {
        PyVersion { inner: value }
    }
}

#[pymethods]
impl PyVersion {
    #[new]
    pub fn __init__(version: &str) -> pyo3::PyResult<Self> {
        Ok(Version::from_str(version)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns a string representation of the version.
    pub fn as_str(&self) -> String {
        format!("{}", self.inner)
    }

    /// Returns the epoch of the version
    pub fn epoch(&self) -> Option<u64> {
        self.inner.epoch_opt()
    }

    /// Returns a new version where the last numerical segment of this version has been bumped.
    pub fn bump(&self) -> Self {
        Self {
            inner: self.inner.bump(),
        }
    }

    pub fn equal(&self, other: &Self) -> bool {
        self.inner == other.inner
    }

    pub fn not_equal(&self, other: &Self) -> bool {
        self.inner != other.inner
    }

    pub fn less_than(&self, other: &Self) -> bool {
        self.inner < other.inner
    }

    pub fn less_than_equals(&self, other: &Self) -> bool {
        self.inner <= other.inner
    }

    pub fn equals(&self, other: &Self) -> bool {
        self.inner == other.inner
    }

    pub fn greater_than_equals(&self, other: &Self) -> bool {
        self.inner >= other.inner
    }

    pub fn greater_than(&self, other: &Self) -> bool {
        self.inner > other.inner
    }
}
