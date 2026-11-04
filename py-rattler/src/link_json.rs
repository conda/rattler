use std::path::PathBuf;
use std::str::FromStr;

use pyo3::exceptions::PyValueError;
use pyo3::{Bound, Py, PyAny, PyErr, PyResult, Python, pyclass, pymethods};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_conda_types::package::{
    EntryPoint, LinkJson, NoArchLinks, PackageFile, PythonEntryPoints,
};
use rattler_package_streaming::seek::read_package_file;
use url::Url;

use crate::{error::PyRattlerError, networking::client::PyClientWithMiddleware};

/// A Python entry point described in `link.json`.
#[pyclass(from_py_object)]
#[repr(transparent)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyEntryPoint {
    pub(crate) inner: EntryPoint,
}

impl From<EntryPoint> for PyEntryPoint {
    fn from(value: EntryPoint) -> Self {
        Self { inner: value }
    }
}

impl From<PyEntryPoint> for EntryPoint {
    fn from(value: PyEntryPoint) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyEntryPoint {
    #[new]
    pub fn new(command: String, module: String, function: String) -> Self {
        Self {
            inner: EntryPoint {
                command,
                module,
                function,
            },
        }
    }

    #[staticmethod]
    pub fn from_str(str: &str) -> PyResult<Self> {
        EntryPoint::from_str(str)
            .map(Into::into)
            .map_err(|e| PyValueError::new_err(e.to_string()))
    }

    #[getter]
    pub fn command(&self) -> String {
        self.inner.command.clone()
    }

    #[setter]
    pub fn set_command(&mut self, command: String) {
        self.inner.command = command;
    }

    #[getter]
    pub fn module(&self) -> String {
        self.inner.module.clone()
    }

    #[setter]
    pub fn set_module(&mut self, module: String) {
        self.inner.module = module;
    }

    #[getter]
    pub fn function(&self) -> String {
        self.inner.function.clone()
    }

    #[setter]
    pub fn set_function(&mut self, function: String) {
        self.inner.function = function;
    }

    pub fn __str__(&self) -> String {
        self.inner.to_string()
    }

    pub fn __repr__(&self) -> String {
        format!(
            "EntryPoint(command={:?}, module={:?}, function={:?})",
            self.inner.command, self.inner.module, self.inner.function
        )
    }

    pub fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

/// Describes Python noarch specific entry points in `link.json`.
#[pyclass(from_py_object)]
#[repr(transparent)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyPythonEntryPoints {
    pub(crate) inner: PythonEntryPoints,
}

impl From<PythonEntryPoints> for PyPythonEntryPoints {
    fn from(value: PythonEntryPoints) -> Self {
        Self { inner: value }
    }
}

impl From<PyPythonEntryPoints> for PythonEntryPoints {
    fn from(value: PyPythonEntryPoints) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPythonEntryPoints {
    #[new]
    #[pyo3(signature = (entry_points=None))]
    pub fn new(entry_points: Option<Vec<PyEntryPoint>>) -> Self {
        Self {
            inner: PythonEntryPoints {
                entry_points: entry_points
                    .unwrap_or_default()
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            },
        }
    }

    #[getter]
    pub fn entry_points(&self) -> Vec<PyEntryPoint> {
        self.inner
            .entry_points
            .clone()
            .into_iter()
            .map(Into::into)
            .collect()
    }

    #[setter]
    pub fn set_entry_points(&mut self, entry_points: Vec<PyEntryPoint>) {
        self.inner.entry_points = entry_points.into_iter().map(Into::into).collect();
    }

    pub fn __repr__(&self) -> String {
        let eps: Vec<String> = self
            .entry_points()
            .iter()
            .map(PyEntryPoint::__repr__)
            .collect();
        format!("PythonEntryPoints(entry_points=[{}])", eps.join(", "))
    }

    pub fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

/// Links for specific types of noarch packages in `link.json`.
#[pyclass(from_py_object)]
#[repr(transparent)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyNoArchLinks {
    pub(crate) inner: NoArchLinks,
}

impl From<NoArchLinks> for PyNoArchLinks {
    fn from(value: NoArchLinks) -> Self {
        Self { inner: value }
    }
}

impl From<PyNoArchLinks> for NoArchLinks {
    fn from(value: PyNoArchLinks) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyNoArchLinks {
    #[staticmethod]
    pub fn generic() -> Self {
        Self {
            inner: NoArchLinks::Generic,
        }
    }

    #[staticmethod]
    #[pyo3(signature = (entry_points=None))]
    pub fn python(entry_points: Option<Vec<PyEntryPoint>>) -> Self {
        Self {
            inner: NoArchLinks::Python(PythonEntryPoints {
                entry_points: entry_points
                    .unwrap_or_default()
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            }),
        }
    }

    #[getter]
    pub fn is_python(&self) -> bool {
        matches!(&self.inner, NoArchLinks::Python(_))
    }

    #[getter]
    pub fn is_generic(&self) -> bool {
        matches!(&self.inner, NoArchLinks::Generic)
    }

    #[getter]
    pub fn python_entry_points(&self) -> Option<PyPythonEntryPoints> {
        match &self.inner {
            NoArchLinks::Python(pep) => Some(pep.clone().into()),
            NoArchLinks::Generic => None,
        }
    }

    #[getter]
    pub fn entry_points(&self) -> Option<Vec<PyEntryPoint>> {
        match &self.inner {
            NoArchLinks::Python(pep) => {
                Some(pep.entry_points.iter().cloned().map(Into::into).collect())
            }
            NoArchLinks::Generic => None,
        }
    }

    pub fn __repr__(&self) -> String {
        match &self.inner {
            NoArchLinks::Python(pep) => {
                let eps: Vec<String> = pep
                    .entry_points
                    .iter()
                    .map(|e| PyEntryPoint::from(e.clone()).__repr__())
                    .collect();
                format!("NoArchLinks.python(entry_points=[{}])", eps.join(", "))
            }
            NoArchLinks::Generic => "NoArchLinks.generic()".to_string(),
        }
    }

    pub fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

/// A representation of the `link.json` file found in noarch package archives.
///
/// The `link.json` file contains information about entrypoints that need to be
/// installed for the package.
#[pyclass(from_py_object)]
#[repr(transparent)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyLinkJson {
    pub(crate) inner: LinkJson,
}

impl From<LinkJson> for PyLinkJson {
    fn from(value: LinkJson) -> Self {
        Self { inner: value }
    }
}

impl From<PyLinkJson> for LinkJson {
    fn from(value: PyLinkJson) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyLinkJson {
    #[new]
    #[pyo3(signature = (noarch, package_metadata_version=1))]
    pub fn new(noarch: PyNoArchLinks, package_metadata_version: u64) -> Self {
        Self {
            inner: LinkJson {
                noarch: noarch.into(),
                package_metadata_version,
            },
        }
    }

    /// Parses the object from a file specified by a `path`, using a format
    /// appropriate for the file type.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(LinkJson::from_path(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object by looking up the appropriate file from the root of the
    /// specified Conda archive directory.
    #[staticmethod]
    pub fn from_package_directory(path: PathBuf) -> PyResult<Self> {
        Ok(LinkJson::from_package_directory(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the package file from archive.
    #[staticmethod]
    pub fn from_package_archive(path: PathBuf) -> PyResult<Self> {
        Ok(read_package_file::<LinkJson>(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object from a string, using JSON format.
    #[staticmethod]
    pub fn from_str(str: &str) -> PyResult<Self> {
        Ok(LinkJson::from_str(str)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Fetches the file from a remote package URL.
    #[staticmethod]
    pub fn from_remote_url<'a>(
        py: Python<'a>,
        client: PyClientWithMiddleware,
        url: String,
    ) -> PyResult<Bound<'a, PyAny>> {
        let url = Url::parse(&url).map_err(|e| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Invalid URL: {e}"))
        })?;

        future_into_py(py, async move {
            let link_json =
                rattler_package_streaming::reqwest::fetch::fetch_package_file_from_remote_url::<
                    LinkJson,
                >(client.into(), url)
                .await;

            Python::attach(|py| match link_json {
                Ok(r) => Ok(Some(Py::new(py, PyLinkJson::from(r))?.into_any())),
                Err(rattler_package_streaming::ExtractError::MissingComponent) => Ok(None),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string())),
            })
        })
    }

    /// Returns the path to the file within the Conda archive.
    #[staticmethod]
    pub fn package_path() -> PathBuf {
        LinkJson::package_path().to_owned()
    }

    /// Returns a JSON string representation of the object.
    pub fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(|e| {
            PyValueError::new_err(format!("Failed to serialize LinkJson to JSON: {e}"))
        })
    }

    /// The version of the package metadata file.
    #[getter]
    pub fn package_metadata_version(&self) -> u64 {
        self.inner.package_metadata_version
    }

    /// Set the package metadata version.
    #[setter]
    pub fn set_package_metadata_version(&mut self, version: u64) {
        self.inner.package_metadata_version = version;
    }

    /// Links for specific noarch packages.
    #[getter]
    pub fn noarch(&self) -> PyNoArchLinks {
        self.inner.noarch.clone().into()
    }

    /// Set links for specific noarch packages.
    #[setter]
    pub fn set_noarch(&mut self, noarch: PyNoArchLinks) {
        self.inner.noarch = noarch.into();
    }

    pub fn __repr__(&self) -> String {
        format!(
            "LinkJson(noarch={}, package_metadata_version={})",
            self.noarch().__repr__(),
            self.package_metadata_version()
        )
    }

    pub fn __eq__(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}
