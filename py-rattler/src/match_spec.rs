use pyo3::{pyclass, pymethods, PyResult};
use rattler_conda_types::{MatchSpec, PackageName};
use std::str::FromStr;

use crate::{error::PyRattlerError, nameless_match_spec::PyNamelessMatchSpec, record::PyRecord};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyMatchSpec {
    pub(crate) inner: MatchSpec,
}

impl From<MatchSpec> for PyMatchSpec {
    fn from(value: MatchSpec) -> Self {
        Self { inner: value }
    }
}

impl From<PyMatchSpec> for MatchSpec {
    fn from(value: PyMatchSpec) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyMatchSpec {
    #[new]
    pub fn __init__(spec: &str) -> PyResult<Self> {
        Ok(MatchSpec::from_str(spec)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns a string representation of MatchSpec
    pub fn as_str(&self) -> String {
        format!("{}", self.inner)
    }

    /// Matches a MatchSpec against a PackageRecord
    pub fn matches(&self, record: &PyRecord) -> bool {
        self.inner.matches(record.as_package_record())
    }

    /// Constructs a PyMatchSpec from a PyNamelessMatchSpec and a name.
    #[staticmethod]
    pub fn from_nameless(spec: &PyNamelessMatchSpec, name: String) -> PyResult<Self> {
        Ok(Self {
            inner: MatchSpec::from_nameless(
                spec.clone().into(),
                Some(PackageName::try_from(name).map_err(PyRattlerError::from)?),
            ),
        })
    }
}
