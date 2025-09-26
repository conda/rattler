use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher}, str::FromStr,
};

use pyo3::{pyclass, pymethods};
use rattler_conda_types::{PackageName, PackageNameMatcher};

use crate::error::PyRattlerError;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPackageNameMatcher {
    pub(crate) inner: PackageNameMatcher,
}

impl From<PyPackageNameMatcher> for PackageNameMatcher {
    fn from(value: PyPackageNameMatcher) -> Self {
        value.inner
    }
}

impl From<PackageNameMatcher> for PyPackageNameMatcher {
    fn from(value: PackageNameMatcher) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyPackageNameMatcher {
    /// Constructs a new `PackageNameMatcher` from a string, checking if the string is actually a
    /// valid or normalized conda package name.
    #[new]
    pub fn new(source: String) -> pyo3::PyResult<Self> {
        Ok(PackageNameMatcher::from_str(&source)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Constructs a new exact `PackageNameMatcher` from a string without checking if the string is actually a
    /// valid or normalized conda package name. This should only be used if you are sure that the
    /// input string is valid.
    #[staticmethod]
    pub fn new_unchecked(normalized: String) -> Self {
        PackageNameMatcher::Exact(PackageName::new_unchecked(normalized)).into()
    }

    /// Compute the hash of the name.
    fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.inner.hash(&mut hasher);
        hasher.finish()
    }
}
