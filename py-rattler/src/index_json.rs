use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    str::FromStr,
};

use pyo3::{
    Bound, Py, PyAny, PyErr, PyResult, Python, exceptions::PyValueError, pyclass, pymethods,
};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_conda_types::{
    Flag, PackageUrl, RepodataRevision, VersionWithSource,
    package::{IndexJson, PackageFile},
    utils::TimestampMs,
};
use rattler_package_streaming::seek::read_package_file;
use url::Url;

use crate::{
    error::PyRattlerError, networking::client::PyClientWithMiddleware, no_arch_type::PyNoArchType,
    package_name::PyPackageName, version::PyVersion,
};

#[pyclass(from_py_object)]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyIndexJson {
    pub(crate) inner: IndexJson,
}

impl From<IndexJson> for PyIndexJson {
    fn from(value: IndexJson) -> Self {
        Self { inner: value }
    }
}

impl From<PyIndexJson> for IndexJson {
    fn from(value: PyIndexJson) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyIndexJson {
    /// Parses the package file from archive.
    /// Note: If you want to extract multiple `info/*` files then this will be slightly
    ///       slower than manually iterating over the archive entries with
    ///       custom logic as this skips over the rest of the archive
    #[staticmethod]
    pub fn from_package_archive(path: PathBuf) -> PyResult<Self> {
        Ok(read_package_file::<IndexJson>(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the package file from a path.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(IndexJson::from_path(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object by looking up the appropriate file from the root of the specified Conda
    /// archive directory, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function reads the appropriate file from
    /// the archive, parse the JSON string and return the resulting object. If the file is not in a
    /// parse-able format or if the file could not be read, this function returns an error.
    #[staticmethod]
    pub fn from_package_directory(path: PathBuf) -> PyResult<Self> {
        Ok(IndexJson::from_package_directory(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object from a string, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function parses the JSON string and returns
    /// the resulting object. If the file is not in a parse-able format, this function returns an
    /// error.
    #[staticmethod]
    pub fn from_str(str: &str) -> PyResult<Self> {
        Ok(IndexJson::from_str(str)
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
            let index_json =
                rattler_package_streaming::reqwest::fetch::fetch_package_file_from_remote_url::<
                    IndexJson,
                >(client.into(), url)
                .await;

            Python::attach(|py| match index_json {
                Ok(r) => Ok(Some(Py::new(py, PyIndexJson::from(r))?.into_any())),
                Err(rattler_package_streaming::ExtractError::MissingComponent) => Ok(None),
                Err(e) => Err(PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string())),
            })
        })
    }

    /// Returns the path to the file within the Conda archive.
    ///
    /// The path is relative to the root of the archive and include any necessary directories.
    #[staticmethod]
    pub fn package_path() -> PathBuf {
        IndexJson::package_path().to_owned()
    }

    /// Optionally, the architecture the package is build for.
    #[getter]
    pub fn arch(&self) -> Option<String> {
        self.inner.arch.clone()
    }

    #[setter]
    pub fn set_arch(&mut self, arch: Option<String>) {
        self.inner.arch = arch;
    }

    /// The build string of the package.
    #[getter]
    pub fn build(&self) -> String {
        self.inner.build.clone()
    }

    #[setter]
    pub fn set_build(&mut self, build: String) {
        self.inner.build = build;
    }

    /// The build number of the package. This is also included in the build string.
    #[getter]
    pub fn build_number(&self) -> u64 {
        self.inner.build_number
    }

    #[setter]
    pub fn set_build_number(&mut self, build_number: u64) {
        self.inner.build_number = build_number;
    }

    /// The package constraints of the package
    #[getter]
    pub fn constrains(&self) -> Vec<String> {
        self.inner.constrains.clone()
    }

    #[setter]
    pub fn set_constrains(&mut self, constrains: Vec<String>) {
        self.inner.constrains = constrains;
    }

    /// The dependencies of the package
    #[getter]
    pub fn depends(&self) -> Vec<String> {
        self.inner.depends.clone()
    }

    #[setter]
    pub fn set_depends(&mut self, depends: Vec<String>) {
        self.inner.depends = depends;
    }

    /// Extra dependency groups that can be selected using `foobar[extras=["scientific"]]`.
    /// The implementation is specified in this CEP: <https://github.com/conda/ceps/pull/111>
    #[getter]
    pub fn extra_depends(&self) -> BTreeMap<String, Vec<String>> {
        self.inner.extra_depends.clone()
    }

    #[setter]
    pub fn set_extra_depends(&mut self, extra_depends: BTreeMap<String, Vec<String>>) {
        self.inner.extra_depends = extra_depends;
    }

    /// Features are a deprecated way to specify different feature sets for the conda solver. This is not
    /// supported anymore and should not be used. Instead, `mutex` packages should be used to specify
    /// mutually exclusive features.
    #[getter]
    pub fn features(&self) -> Option<String> {
        self.inner.features.clone()
    }

    #[setter]
    pub fn set_features(&mut self, features: Option<String>) {
        self.inner.features = features;
    }

    /// Plain string flags used to select package variants.
    #[getter]
    pub fn flags(&self) -> Vec<String> {
        self.inner.flags.iter().map(ToString::to_string).collect()
    }

    #[setter]
    pub fn set_flags(&mut self, flags: Vec<String>) {
        self.inner.flags = flags.into_iter().map(Flag::new_unchecked).collect();
    }

    /// Optionally, the license
    #[getter]
    pub fn license(&self) -> Option<String> {
        self.inner.license.clone()
    }

    #[setter]
    pub fn set_license(&mut self, license: Option<String>) {
        self.inner.license = license;
    }

    /// Optionally, the license family
    #[getter]
    pub fn license_family(&self) -> Option<String> {
        self.inner.license_family.clone()
    }

    #[setter]
    pub fn set_license_family(&mut self, license_family: Option<String>) {
        self.inner.license_family = license_family;
    }

    /// The lowercase name of the package
    #[getter]
    pub fn name(&self) -> PyPackageName {
        self.inner.name.clone().into()
    }

    #[setter]
    pub fn set_name(&mut self, name: PyPackageName) {
        self.inner.name = name.into();
    }

    /// If this package is independent of architecture this field specifies in what way.
    #[getter]
    pub fn noarch(&self) -> PyNoArchType {
        self.inner.noarch.into()
    }

    #[setter]
    pub fn set_noarch(&mut self, noarch: PyNoArchType) {
        self.inner.noarch = noarch.into();
    }

    /// Optionally, the OS the package is build for.
    #[getter]
    pub fn platform(&self) -> Option<String> {
        self.inner.platform.clone()
    }

    #[setter]
    pub fn set_platform(&mut self, platform: Option<String>) {
        self.inner.platform = platform;
    }

    /// A list of Package URLs identifying this package.
    /// See this CEP: <https://github.com/conda/ceps/pull/63>
    #[getter]
    pub fn purls(&self) -> Option<Vec<String>> {
        self.inner
            .purls
            .as_ref()
            .map(|purls| purls.iter().map(ToString::to_string).collect())
    }

    #[setter]
    pub fn set_purls(&mut self, purls: Option<Vec<String>>) -> PyResult<()> {
        self.inner.purls = purls
            .map(|purls| {
                purls
                    .iter()
                    .map(|purl| {
                        PackageUrl::from_str(purl).map_err(|err| {
                            PyValueError::new_err(format!("Invalid package URL '{purl}': {err}"))
                        })
                    })
                    .collect::<PyResult<BTreeSet<_>>>()
            })
            .transpose()?;
        Ok(())
    }

    /// Optionally a path within the environment of the site-packages directory.
    /// This field is only present for python interpreter packages.
    /// This field was introduced with <https://github.com/conda/ceps/blob/main/cep-17.md>.
    #[getter]
    pub fn python_site_packages_path(&self) -> Option<String> {
        self.inner.python_site_packages_path.clone()
    }

    #[setter]
    pub fn set_python_site_packages_path(&mut self, python_site_packages_path: Option<String>) {
        self.inner.python_site_packages_path = python_site_packages_path;
    }

    /// The repodata revision required by this package record, formatted as `vN`.
    #[getter]
    pub fn repodata_revision(&self) -> Option<String> {
        self.inner
            .repodata_revision
            .map(|revision| revision.to_string())
    }

    #[setter]
    pub fn set_repodata_revision(&mut self, repodata_revision: Option<String>) -> PyResult<()> {
        self.inner.repodata_revision = repodata_revision
            .map(|revision| {
                RepodataRevision::from_str(&revision).map_err(|err| {
                    PyValueError::new_err(format!("Invalid repodata revision '{revision}': {err}"))
                })
            })
            .transpose()?;
        Ok(())
    }

    /// The subdirectory that contains this package
    #[getter]
    pub fn subdir(&self) -> Option<String> {
        self.inner.subdir.clone()
    }

    #[setter]
    pub fn set_subdir(&mut self, subdir: Option<String>) {
        self.inner.subdir = subdir;
    }

    /// The timestamp when this package was created
    #[getter]
    pub fn timestamp(&self) -> Option<i64> {
        self.inner.timestamp.map(|time| time.timestamp_millis())
    }

    #[setter]
    pub fn set_timestamp(&mut self, timestamp: Option<i64>) -> PyResult<()> {
        if let Some(ts) = timestamp {
            self.inner.timestamp = Some(TimestampMs::from_timestamp_millis(
                jiff::Timestamp::from_millisecond(ts)
                    .map_err(|err| PyValueError::new_err(format!("Invalid timestamp: {err}")))?,
            ));
        } else {
            self.inner.timestamp = None;
        }
        Ok(())
    }

    /// Track features are nowadays only used to down-weigh packages (ie. give them less priority). To
    /// that effect, the number of track features is counted (number of commas) and the package is downweighted
    /// by the number of `track_features`.
    #[getter]
    pub fn track_features(&self) -> Vec<String> {
        self.inner.track_features.clone()
    }

    #[setter]
    pub fn set_track_features(&mut self, track_features: Vec<String>) {
        self.inner.track_features = track_features;
    }

    /// The version of the package
    #[getter]
    pub fn version(&self) -> (PyVersion, String) {
        (
            self.inner.version.version().clone().into(),
            self.inner.version.as_str().to_string(),
        )
    }

    #[setter]
    pub fn set_version(&mut self, version_and_source: (PyVersion, String)) {
        self.inner.version =
            VersionWithSource::new(version_and_source.0.inner, version_and_source.1);
    }
}
