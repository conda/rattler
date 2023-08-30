use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use pyo3::{basic::CompareOp, pyclass, pymethods, PyClassInitializer};
use rattler_conda_types::VersionWithSource;

use crate::version::PyVersion;

#[pyclass(extends=PyVersion, subclass)]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyVersionWithSource {
    pub(crate) inner: VersionWithSource,
}

impl From<VersionWithSource> for PyVersionWithSource {
    fn from(value: VersionWithSource) -> Self {
        Self { inner: value }
    }
}

impl From<PyVersionWithSource> for VersionWithSource {
    fn from(value: PyVersionWithSource) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyVersionWithSource {
    #[new]
    pub fn new(version: &PyVersion, source: String) -> pyo3::PyClassInitializer<Self> {
        PyClassInitializer::from(version.clone())
            .add_subclass(VersionWithSource::new(version.inner.clone(), source).into())
    }

    /// Returns the `PyVersion` from current object.
    pub fn version(&self) -> PyVersion {
        self.inner.version().clone().into()
    }

    /// Returns a string representation of `PyVersionWithSource`.
    pub fn as_str(&self) -> String {
        self.inner.as_str().into_owned()
    }

    /// Compute the hash of the version.
    fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.inner.hash(&mut hasher);
        hasher.finish()
    }

    /// Performs comparison between this version and another.
    pub fn __richcmp__(&self, other: &Self, op: CompareOp) -> bool {
        op.matches(self.inner.cmp(&other.inner))
    }
}
