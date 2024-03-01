use pyo3::{pyclass, pymethods, PyResult};
use rattler_virtual_packages::VirtualPackage;

use crate::{error::PyRattlerError, generic_virtual_package::PyGenericVirtualPackage};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyVirtualPackage {
    pub(crate) inner: VirtualPackage,
}

impl From<VirtualPackage> for PyVirtualPackage {
    fn from(value: VirtualPackage) -> Self {
        Self { inner: value }
    }
}

impl From<PyVirtualPackage> for VirtualPackage {
    fn from(value: PyVirtualPackage) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyVirtualPackage {
    /// Returns virtual packages detected for the current system or an error if the versions could
    /// not be properly detected.
    #[staticmethod]
    pub fn current() -> PyResult<Vec<Self>> {
        Ok(VirtualPackage::current()
            .map(|vp| vp.iter().map(|v| v.clone().into()).collect::<Vec<_>>())
            .map_err(PyRattlerError::from)?)
    }

    pub fn as_generic(&self) -> PyGenericVirtualPackage {
        self.clone().into()
    }

    /// Returns string representation of virtual package.
    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }
}
