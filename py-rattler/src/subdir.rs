use std::str::FromStr;

use pyo3::{PyResult, exceptions::PyRuntimeError, pyclass, pymethods};
use rattler_conda_types::{Arch, Subdir};

use crate::error::PyRattlerError;

const UNKNOWN_HOST_PLATFORM: &str = "the current host is not a known conda platform";

///////////////////////////
/// Arch                ///
///////////////////////////

#[pyclass(from_py_object)]
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
    pub fn current() -> PyResult<Self> {
        Arch::current()
            .map(Into::into)
            .ok_or_else(|| PyRuntimeError::new_err(UNKNOWN_HOST_PLATFORM))
    }

    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }
}

///////////////////////////
/// Subdir            ///
///////////////////////////

#[pyclass(from_py_object)]
#[repr(transparent)]
#[derive(Clone, Copy, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct PySubdir {
    pub inner: Subdir,
}

impl From<Subdir> for PySubdir {
    fn from(value: Subdir) -> Self {
        PySubdir { inner: value }
    }
}

impl From<PySubdir> for Subdir {
    fn from(value: PySubdir) -> Self {
        value.inner
    }
}

impl FromStr for PySubdir {
    type Err = PyRattlerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let platform = Subdir::from_str(s).map_err(PyRattlerError::from)?;
        Ok(platform.into())
    }
}

#[pymethods]
impl PySubdir {
    #[new]
    pub fn __init__(platform: &str) -> Result<Self, PyRattlerError> {
        let platform = Subdir::from_str(platform).map_err(PyRattlerError::from)?;
        Ok(platform.into())
    }

    #[staticmethod]
    pub fn current() -> PyResult<Self> {
        Subdir::current()
            .map(Into::into)
            .ok_or_else(|| PyRuntimeError::new_err(UNKNOWN_HOST_PLATFORM))
    }

    #[staticmethod]
    pub fn all() -> Vec<Self> {
        Subdir::all().map(Into::into).collect()
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
