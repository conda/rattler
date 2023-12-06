use pyo3::{pyclass, pymethods};
use rattler_conda_types::prefix_record::PrefixPaths;
use std::path::PathBuf;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPrefixPaths {
    pub(crate) inner: PrefixPaths,
}

impl From<PyPrefixPaths> for PrefixPaths {
    fn from(value: PyPrefixPaths) -> Self {
        value.inner
    }
}

impl From<PrefixPaths> for PyPrefixPaths {
    fn from(value: PrefixPaths) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyPrefixPaths {
    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }

    /// The version of the file
    #[getter]
    pub fn paths_version(&self) -> u64 {
        self.inner.paths_version
    }

    /// All entries included in the package.
    #[getter]
    pub fn paths(&self) -> Vec<PathBuf> {
        self.inner
            .paths
            .clone()
            .into_iter()
            .map(|pe| pe.relative_path)
            .collect::<Vec<_>>()
    }
}
