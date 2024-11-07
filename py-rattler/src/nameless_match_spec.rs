use std::sync::Arc;

use pyo3::{pyclass, pymethods, types::PyBytes, Bound, PyResult, Python};
use rattler_conda_types::{Channel, MatchSpec, Matches, NamelessMatchSpec, ParseStrictness};

use crate::{channel::PyChannel, error::PyRattlerError, match_spec::PyMatchSpec, record::PyRecord};

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
    pub fn __init__(spec: &str, strict: bool) -> PyResult<Self> {
        Ok(NamelessMatchSpec::from_str(
            spec,
            if strict {
                ParseStrictness::Strict
            } else {
                ParseStrictness::Lenient
            },
        )
        .map(Into::into)
        .map_err(PyRattlerError::from)?)
    }

    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    #[getter]
    pub fn version(&self) -> Option<String> {
        self.inner
            .version
            .clone()
            .map(|version| version.to_string())
    }

    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    #[getter]
    pub fn build(&self) -> Option<String> {
        self.inner.build.clone().map(|build| build.to_string())
    }

    /// The build number of the package
    #[getter]
    pub fn build_number(&self) -> Option<String> {
        self.inner
            .build_number
            .clone()
            .map(|build_number| build_number.to_string())
    }

    /// Match the specific filename of the package
    #[getter]
    pub fn file_name(&self) -> Option<String> {
        self.inner.file_name.clone()
    }

    /// The channel of the package
    #[getter]
    pub fn channel(&self) -> Option<PyChannel> {
        self.inner
            .channel
            .clone()
            .map(|mut channel| Arc::<Channel>::make_mut(&mut channel).clone().into())
    }

    /// The subdir of the channel
    #[getter]
    pub fn subdir(&self) -> Option<String> {
        self.inner.subdir.clone()
    }

    /// The namespace of the package (currently not used)
    #[getter]
    pub fn namespace(&self) -> Option<String> {
        self.inner.namespace.clone()
    }

    /// The md5 hash of the package
    #[getter]
    pub fn md5<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.inner.md5.map(|md5| PyBytes::new_bound(py, &md5))
    }

    /// The sha256 hash of the package
    #[getter]
    pub fn sha256<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.inner
            .sha256
            .map(|sha256| PyBytes::new_bound(py, &sha256))
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
