use pyo3::{pyclass, pymethods, PyResult};
use rattler_virtual_packages::{Override, VirtualPackage, VirtualPackageOverrides};

use crate::{error::PyRattlerError, generic_virtual_package::PyGenericVirtualPackage};

#[pyclass]
#[repr(transparent)]
#[derive(Clone, Default, PartialEq)]
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

    #[staticmethod]
    pub fn default_env_var() -> Self {
        Self {
            inner: Override::DefaultEnvVar,
        }
    }

    #[staticmethod]
    pub fn env_var(name: &str) -> Self {
        Self {
            inner: Override::EnvVar(name.to_string()),
        }
    }

    #[staticmethod]
    pub fn string(value: &str) -> Self {
        Self {
            inner: Override::String(value.to_string()),
        }
    }

    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }

    pub fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
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
    pub fn default() -> Self {
        Self {
            inner: VirtualPackageOverrides::default(),
        }
    }

    #[staticmethod]
    pub fn none() -> Self {
        Self {
            inner: VirtualPackageOverrides::none(),
        }
    }

    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }

    #[getter]
    pub fn get_osx(&self) -> PyOverride {
        self.inner.osx.clone().into()
    }
    #[setter]
    pub fn set_osx(&mut self, value: PyOverride) {
        self.inner.osx = value.into();
    }
    #[getter]
    pub fn get_cuda(&self) -> PyOverride {
        self.inner.cuda.clone().into()
    }
    #[setter]
    pub fn set_cuda(&mut self, value: PyOverride) {
        self.inner.cuda = value.into();
    }
    #[getter]
    pub fn get_libc(&self) -> PyOverride {
        self.inner.libc.clone().into()
    }
    #[setter]
    pub fn set_libc(&mut self, value: PyOverride) {
        self.inner.libc = value.into();
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
    // marking this as depreacted causes a warning when building the code,
    // we just warn directly from python.
    #[staticmethod]
    pub fn current() -> PyResult<Vec<Self>> {
        Self::detect_with_overrides(&PyVirtualPackageOverrides::none())
    }

    #[staticmethod]
    pub fn detect() -> PyResult<Vec<Self>> {
        Self::detect_with_overrides(&PyVirtualPackageOverrides::default())
    }

    #[staticmethod]
    pub fn detect_with_overrides(overrides: &PyVirtualPackageOverrides) -> PyResult<Vec<Self>> {
        Ok(
            VirtualPackage::detect_with_overrides(&overrides.clone().into())
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
