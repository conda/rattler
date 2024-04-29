use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use pyo3::{basic::CompareOp, pyclass, pymethods};
use rattler_conda_types::NoArchType;

#[pyclass]
#[derive(Clone)]
pub struct PyNoArchType {
    pub inner: NoArchType,
}

impl From<NoArchType> for PyNoArchType {
    fn from(value: NoArchType) -> Self {
        Self { inner: value }
    }
}

impl From<PyNoArchType> for NoArchType {
    fn from(value: PyNoArchType) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyNoArchType {
    /// Constructs a new `NoArchType` of type `python`.
    #[staticmethod]
    pub fn python() -> Self {
        NoArchType::python().into()
    }

    #[getter]
    pub fn is_python(&self) -> bool {
        self.inner.is_python()
    }

    /// Constructs a new `NoArchType` of type `generic`.
    #[staticmethod]
    pub fn generic() -> Self {
        NoArchType::generic().into()
    }

    #[getter]
    pub fn is_generic(&self) -> bool {
        self.inner.is_generic()
    }

    /// Constructs a new `NoArchType` of type `none`.
    #[staticmethod]
    pub fn none() -> Self {
        NoArchType::none().into()
    }

    #[getter]
    pub fn is_none(&self) -> bool {
        self.inner.is_none()
    }

    /// Compute the hash of the noarch type.
    fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.inner.hash(&mut hasher);
        hasher.finish()
    }

    /// Performs comparison between this noarch type and another.
    pub fn __richcmp__(&self, other: &Self, op: CompareOp) -> bool {
        op.matches(self.inner.cmp(&other.inner))
    }
}
