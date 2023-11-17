use pyo3::{pyclass, pymethods, PyResult};
use rattler_conda_types::{MatchSpec, NamelessMatchSpec};
use std::str::FromStr;

use crate::{error::PyRattlerError, match_spec::PyMatchSpec, record::PyRecord};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyNamelessMatchSpec {
    inner: NamelessMatchSpec,
}

impl From<NamelessMatchSpec> for PyNamelessMatchSpec {
    fn from(value: NamelessMatchSpec) -> Self {
        Self { inner: value }
    }
}

impl From<PyNamelessMatchSpec> for NamelessMatchSpec {
    fn from(val: PyNamelessMatchSpec) -> Self {
        val.inner
    }
}

impl From<PyMatchSpec> for PyNamelessMatchSpec {
    fn from(value: PyMatchSpec) -> Self {
        let inner: NamelessMatchSpec = Into::<MatchSpec>::into(value).into();
        Self { inner }
    }
}

#[pymethods]
impl PyNamelessMatchSpec {
    #[new]
    pub fn __init__(spec: &str) -> PyResult<Self> {
        Ok(NamelessMatchSpec::from_str(spec)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns a string representation of MatchSpec
    pub fn as_str(&self) -> String {
        format!("{}", self.inner)
    }

    /// Match a PyNamelessMatchSpec against a PackageRecord
    pub fn matches(&self, record: &PyRecord) -> bool {
        self.inner.matches(&record.as_package_record().clone())
    }

    /// Constructs a [`PyNamelessMatchSpec`] from a [`PyMatchSpec`].
    #[staticmethod]
    pub fn from_match_spec(spec: &PyMatchSpec) -> Self {
        Into::<Self>::into(spec.clone())
    }
}
