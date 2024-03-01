use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use pyo3::{basic::CompareOp, pyclass, pymethods};
use rattler_conda_types::PackageName;

use crate::error::PyRattlerError;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPackageName {
    pub(crate) inner: PackageName,
}

impl From<PyPackageName> for PackageName {
    fn from(value: PyPackageName) -> Self {
        value.inner
    }
}

impl From<PackageName> for PyPackageName {
    fn from(value: PackageName) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyPackageName {
    /// Constructs a new `PackageName` from a string, checking if the string is actually a
    /// valid or normalized conda package name.
    #[new]
    pub fn new(source: String) -> pyo3::PyResult<Self> {
        Ok(PackageName::try_from(source)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Constructs a new `PackageName` from a string without checking if the string is actually a
    /// valid or normalized conda package name. This should only be used if you are sure that the
    /// input string is valid.
    #[staticmethod]
    pub fn new_unchecked(normalized: String) -> Self {
        PackageName::new_unchecked(normalized).into()
    }

    /// Returns the source representation of the package name. This is the string from which this
    /// instance was created.
    #[getter]
    pub fn source(&self) -> String {
        self.inner.as_source().into()
    }

    /// Returns the normalized version of the package name. The normalized string is guaranteed to
    /// be a valid conda package name.
    #[getter]
    pub fn normalized(&self) -> String {
        self.inner.as_normalized().into()
    }

    /// Compute the hash of the name.
    fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.inner.hash(&mut hasher);
        hasher.finish()
    }

    /// Performs comparison between this name and another.
    pub fn __richcmp__(&self, other: &Self, op: CompareOp) -> bool {
        op.matches(self.inner.cmp(&other.inner))
    }
}
