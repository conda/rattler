use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    str::FromStr,
};

use pyo3::{pyclass, pymethods};
use rattler_conda_types::{PackageName, PackageNameMatcher};

use crate::{error::PyRattlerError, package_name::PyPackageName};

#[pyclass]
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
        let inner = PackageNameMatcher::from_str(source.as_str()).map_err(PyRattlerError::from)?;

        Ok(Self { inner })
    }

    fn display_inner(&self) -> String {
        match self.inner {
            PackageNameMatcher::Exact(ref name) => {
                format!("\"{}\", exact", name.as_source())
            }
            PackageNameMatcher::Glob(ref glob) => format!("\"{glob}\", glob"),
            PackageNameMatcher::Regex(ref regex) => {
                format!("\"{regex}\", regex")
            }
        }
    }

    fn as_package_name(&self) -> Option<PyPackageName> {
        Option::<PackageName>::from(self.inner.clone()).map(PyPackageName::from)
    }

    /// Compute the hash of the name.
    fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.inner.hash(&mut hasher);
        hasher.finish()
    }
}
