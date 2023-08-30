use pyo3::{pyclass, pymethods, PyClassInitializer};
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
}
