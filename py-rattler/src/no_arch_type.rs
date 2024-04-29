use pyo3::{pyclass, pymethods};
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
    #[getter]
    pub fn none(&self) -> bool {
        self.inner.is_none()
    }

    #[getter]
    pub fn python(&self) -> bool {
        self.inner.is_python()
    }

    #[getter]
    pub fn generic(&self) -> bool {
        self.inner.is_generic()
    }
}
