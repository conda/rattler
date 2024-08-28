use pyo3::{pyclass, pymethods, PyResult};
use rattler_virtual_packages::{Override, VirtualPackage, VirtualPackageOverrides};

use crate::{error::PyRattlerError, generic_virtual_package::PyGenericVirtualPackage};

#[pyclass]
#[repr(transparent)]
#[derive(Clone, Default)]
pub struct PyOverride {
    pub(crate) inner: Override,
}

impl From<Override> for PyOverride {
    fn from(value: Override) -> Self {
        Self { inner: value }
    }
}

impl From<PyOverride> for Override {
    fn from(value: PyOverride) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyOverride {
    #[staticmethod]
    pub fn none() -> Self {
        Self {
            inner: Override::None,
        }
    }

    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone, Default)]
pub struct PyVirtualPackageOverrides {
    pub(crate) inner: VirtualPackageOverrides,
}

impl From<VirtualPackageOverrides> for PyVirtualPackageOverrides {
    fn from(value: VirtualPackageOverrides) -> Self {
        Self { inner: value }
    }
}

impl From<PyVirtualPackageOverrides> for VirtualPackageOverrides {
    fn from(value: PyVirtualPackageOverrides) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyVirtualPackageOverrides {
    #[staticmethod]
    pub fn none() -> Self {
        Self {
            inner: VirtualPackageOverrides::none(),
        }
    }

    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }
}

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
        Self::current_with_overrides(&PyVirtualPackageOverrides::default())
    }

    #[staticmethod]
    pub fn current_with_overrides(overrides: &PyVirtualPackageOverrides) -> PyResult<Vec<Self>> {
        Ok(
            VirtualPackage::current_with_overrides(&overrides.clone().into())
                .map(|vp| vp.iter().map(|v| v.clone().into()).collect::<Vec<_>>())
                .map_err(PyRattlerError::from)?,
        )
    }

    pub fn as_generic(&self) -> PyGenericVirtualPackage {
        self.clone().into()
    }

    /// Returns string representation of virtual package.
    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }
}
