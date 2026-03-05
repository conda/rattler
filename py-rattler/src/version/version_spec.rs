use crate::{error::PyRattlerError, version::PyVersion};
use pyo3::{pyclass, pymethods, PyResult};
use rattler_conda_types::{ParseStrictness, VersionSpec};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyVersionSpec {
    pub(crate) inner: VersionSpec,
}

impl From<VersionSpec> for PyVersionSpec {
    fn from(value: VersionSpec) -> Self {
        PyVersionSpec { inner: value }
    }
}

impl From<PyVersionSpec> for VersionSpec {
    fn from(value: PyVersionSpec) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyVersionSpec {
    /// Construct a new `VersionSpec` from a string with optional strict parsing.
    #[new]
    #[pyo3(signature = (spec, strict=false))]
    pub fn __init__(spec: &str, strict: bool) -> PyResult<Self> {
        let strictness = if strict {
            ParseStrictness::Strict
        } else {
            ParseStrictness::Lenient
        };

        Ok(VersionSpec::from_str(spec, strictness)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns a string representation of the version specification.
    pub fn as_str(&self) -> String {
        format!("{}", self.inner)
    }

    /// Check if a version matches this version specification.
    pub fn matches(&self, version: &PyVersion) -> bool {
        self.inner.matches(&version.inner)
    }

    fn __str__(&self) -> String {
        format!("{}", self.inner)
    }

    fn __repr__(&self) -> String {
        format!("PyVersionSpec(\"{}\")", self.inner)
    }

    fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.inner.hash(&mut hasher);
        hasher.finish()
    }

    fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }

    fn __ne__(&self, other: &Self) -> bool {
        self.inner != other.inner
    }
}
