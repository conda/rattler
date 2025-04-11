use std::str::FromStr;

use pyo3::{pyclass, pymethods};
use rattler_conda_types::{Arch, Platform};

use crate::error::PyRattlerError;

///////////////////////////
/// Arch                ///
///////////////////////////

#[pyclass]
#[derive(Clone)]
pub struct PyArch {
    pub inner: Arch,
}

impl From<Arch> for PyArch {
    fn from(value: Arch) -> Self {
        PyArch { inner: value }
    }
}

impl FromStr for PyArch {
    type Err = PyRattlerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let arch = Arch::from_str(s).map_err(PyRattlerError::from)?;
        Ok(arch.into())
    }
}

#[pymethods]
impl PyArch {
    #[new]
    pub fn __init__(arch: &str) -> Result<Self, PyRattlerError> {
        let arch = Arch::from_str(arch).map_err(PyRattlerError::from)?;
        Ok(arch.into())
    }

    #[staticmethod]
    pub fn current() -> Self {
        Arch::current().into()
    }

    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }
}

///////////////////////////
/// Platform            ///
///////////////////////////

#[pyclass]
#[repr(transparent)]
#[derive(Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct PyPlatform {
    pub inner: Platform,
}

impl From<Platform> for PyPlatform {
    fn from(value: Platform) -> Self {
        PyPlatform { inner: value }
    }
}

impl From<PyPlatform> for Platform {
    fn from(value: PyPlatform) -> Self {
        value.inner
    }
}

impl FromStr for PyPlatform {
    type Err = PyRattlerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let platform = Platform::from_str(s).map_err(PyRattlerError::from)?;
        Ok(platform.into())
    }
}

#[pymethods]
impl PyPlatform {
    #[new]
    pub fn __init__(platform: &str) -> Result<Self, PyRattlerError> {
        let platform = Platform::from_str(platform).map_err(PyRattlerError::from)?;
        Ok(platform.into())
    }

    #[staticmethod]
    pub fn current() -> Self {
        Platform::current().into()
    }

    #[getter]
    pub fn name(&self) -> String {
        self.inner.to_string()
    }

    #[getter]
    pub fn is_windows(&self) -> bool {
        self.inner.is_windows()
    }

    #[getter]
    pub fn is_linux(&self) -> bool {
        self.inner.is_linux()
    }

    #[getter]
    pub fn is_osx(&self) -> bool {
        self.inner.is_osx()
    }

    #[getter]
    pub fn is_unix(&self) -> bool {
        self.inner.is_unix()
    }

    pub fn arch(&self) -> Option<PyArch> {
        self.inner.arch().map(Into::into)
    }

    #[getter]
    pub fn only_platform(&self) -> Option<&str> {
        self.inner.only_platform()
    }
}
