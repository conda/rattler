use pyo3::{pyclass, pymethods};
use rattler_conda_types::GenericVirtualPackage;

use crate::package_name::PyPackageName;
use crate::version::PyVersion;
use crate::virtual_package::PyVirtualPackage;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyGenericVirtualPackage {
    pub(crate) inner: GenericVirtualPackage,
}

impl From<PyGenericVirtualPackage> for GenericVirtualPackage {
    fn from(value: PyGenericVirtualPackage) -> Self {
        value.inner
    }
}
impl From<GenericVirtualPackage> for PyGenericVirtualPackage {
    fn from(value: GenericVirtualPackage) -> Self {
        Self { inner: value }
    }
}

impl From<PyVirtualPackage> for PyGenericVirtualPackage {
    fn from(value: PyVirtualPackage) -> Self {
        Self {
            inner: value.inner.into(),
        }
    }
}

#[pymethods]
impl PyGenericVirtualPackage {
    /// Constructs a new `GenericVirtualPackage`.
    #[new]
    pub fn new(name: PyPackageName, version: PyVersion, build_string: String) -> Self {
        Self {
            inner: GenericVirtualPackage {
                name: name.into(),
                version: version.into(),
                build_string,
            },
        }
    }

    /// Constructs a string representation.
    pub fn as_str(&self) -> String {
        format!("{}", self.inner)
    }

    /// The name of the package
    #[getter]
    pub fn name(&self) -> PyPackageName {
        self.inner.name.clone().into()
    }

    /// The version of the package
    #[getter]
    pub fn version(&self) -> PyVersion {
        self.inner.version.clone().into()
    }

    /// The build identifier of the package.
    #[getter]
    pub fn build_string(&self) -> String {
        self.inner.build_string.clone()
    }
}
