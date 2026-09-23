use pyo3::{exceptions::PyValueError, prelude::*};
use rattler_conda_types::package::BuildString;

#[pyclass(from_py_object, frozen)]
#[derive(Clone)]
pub struct PyBuildString {
    pub(crate) inner: BuildString,
}

#[pymethods]
impl PyBuildString {
    #[new]
    pub fn new(value: String) -> PyResult<Self> {
        Ok(Self {
            inner: BuildString::new(value).map_err(|err| PyValueError::new_err(err.to_string()))?,
        })
    }

    #[staticmethod]
    pub fn new_unchecked(value: String) -> Self {
        Self {
            inner: BuildString::new_unchecked(value),
        }
    }

    fn __str__(&self) -> &str {
        self.inner.as_str()
    }
}
