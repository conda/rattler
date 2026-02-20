use std::sync::Arc;
use std::{borrow::Borrow, str::FromStr};

use pyo3::{pyclass, pymethods, types::PyBytes, Bound, PyResult, Python};
use rattler_conda_types::{Channel, MatchSpec, Matches, PackageNameMatcher, ParseMatchSpecOptions};

use crate::{
    channel::PyChannel, error::PyRattlerError, nameless_match_spec::PyNamelessMatchSpec,
    package_name_matcher::PyPackageNameMatcher, record::PyRecord,
};

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

impl Borrow<MatchSpec> for PyMatchSpec {
    fn borrow(&self) -> &MatchSpec {
        &self.inner
    }
}

#[pymethods]
impl PyMatchSpec {
    #[new]
    #[pyo3(signature = (spec, strict = false, exact_names_only = true, experimental_extras = false, experimental_conditionals = false))]
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn __init__(
        spec: &str,
        strict: bool,
        exact_names_only: bool,
        experimental_extras: bool,
        experimental_conditionals: bool,
    ) -> PyResult<Self> {
        let options = if strict {
            ParseMatchSpecOptions::strict()
        } else {
            ParseMatchSpecOptions::lenient()
        }
        .with_exact_names_only(exact_names_only)
        .with_experimental_extras(experimental_extras)
        .with_experimental_conditionals(experimental_conditionals);

        Ok(MatchSpec::from_str(spec, options)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// The name of the package
    #[getter]
    pub fn name(&self) -> Option<PyPackageNameMatcher> {
        Some(self.inner.name.clone().into())
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

    /// The extras (optional dependencies) of the package
    #[getter]
    pub fn extras(&self) -> Option<Vec<String>> {
        self.inner.extras.clone()
    }

    /// The condition under which this match spec applies
    #[getter]
    pub fn condition(&self) -> Option<String> {
        self.inner
            .condition
            .as_ref()
            .map(std::string::ToString::to_string)
    }

    /// The md5 hash of the package
    #[getter]
    pub fn md5<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.inner.md5.map(|md5| PyBytes::new(py, &md5))
    }

    /// The sha256 hash of the package
    #[getter]
    pub fn sha256<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.inner.sha256.map(|sha256| PyBytes::new(py, &sha256))
    }

    /// Returns a string representation of `MatchSpec`
    pub fn as_str(&self) -> String {
        format!("{}", self.inner)
    }

    /// Matches a `MatchSpec` against a `PackageRecord`
    pub fn matches(&self, record: &PyRecord) -> bool {
        self.inner.matches(record.as_package_record())
    }

    /// Constructs a `PyMatchSpec` from a `PyNamelessMatchSpec` and a name.
    #[staticmethod]
    pub fn from_nameless(spec: &PyNamelessMatchSpec, name: String) -> PyResult<Self> {
        Ok(Self {
            inner: MatchSpec::from_nameless(
                spec.clone().into(),
                PackageNameMatcher::from_str(&name).map_err(PyRattlerError::from)?,
            ),
        })
    }

    #[staticmethod]
    pub fn from_url(url: &str) -> PyResult<Self> {
        let url = url
            .parse::<url::Url>()
            .map_err(PyRattlerError::InvalidUrl)?;

        Ok(MatchSpec::try_from(url)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }
}
