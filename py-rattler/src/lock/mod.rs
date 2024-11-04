use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    str::FromStr,
};

use pep508_rs::Requirement;
use pyo3::{pyclass, pymethods, types::PyBytes, Bound, PyResult, Python};
use rattler_conda_types::{MatchSpec, ParseStrictness, RepoDataRecord};
use rattler_lock::{
    Channel, Environment, LockFile, Package, PackageHashes, PypiPackageData,
    PypiPackageEnvironmentData,
};

use crate::{
    error::PyRattlerError,
    platform::PyPlatform,
    record::{self, PyRecord},
};

/// Represents a lock-file for both Conda packages and Pypi packages.
///
/// Lock-files can store information for multiple platforms and for multiple environments.
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyLockFile {
    pub(crate) inner: LockFile,
}

impl From<LockFile> for PyLockFile {
    fn from(value: LockFile) -> Self {
        Self { inner: value }
    }
}

impl From<PyLockFile> for LockFile {
    fn from(value: PyLockFile) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyLockFile {
    #[new]
    pub fn new(envs: HashMap<String, PyEnvironment>) -> PyResult<Self> {
        let mut lock = LockFile::builder();
        for (name, env) in envs {
            lock.set_channels(&name, env.channels());
            for (platform, records) in env
                .inner
                .conda_repodata_records()
                .map_err(PyRattlerError::from)?
            {
                for record in records {
                    lock.add_conda_package(&name, platform, record.into());
                }
            }

            for (platform, records) in env.inner.pypi_packages() {
                for (pkg_data, pkg_env_data) in records {
                    lock.add_pypi_package(&name, platform, pkg_data, pkg_env_data);
                }
            }
        }

        Ok(lock.finish().into())
    }

    /// Writes the conda lock to a file
    pub fn to_path(&self, path: PathBuf) -> PyResult<()> {
        Ok(self.inner.to_path(&path).map_err(PyRattlerError::from)?)
    }

    /// Parses an rattler-lock file from a file.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(LockFile::from_path(&path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns the environment with the given name.
    pub fn environment(&self, name: &str) -> Option<PyEnvironment> {
        self.inner.environment(name).map(Into::into)
    }

    /// Returns the environment with the default name as defined by [`DEFAULT_ENVIRONMENT_NAME`].
    pub fn default_environment(&self) -> Option<PyEnvironment> {
        self.inner.default_environment().map(Into::into)
    }

    /// Returns an iterator over all environments defined in the lock-file.
    pub fn environments(&self) -> Vec<(String, PyEnvironment)> {
        self.inner
            .environments()
            .map(|(name, env)| (name.to_owned(), env.into()))
            .collect()
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyEnvironment {
    pub(crate) inner: Environment,
}

impl From<Environment> for PyEnvironment {
    fn from(value: Environment) -> Self {
        Self { inner: value }
    }
}

impl From<PyEnvironment> for Environment {
    fn from(value: PyEnvironment) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyEnvironment {
    #[new]
    pub fn new(name: String, req: HashMap<PyPlatform, Vec<PyRecord>>) -> PyResult<Self> {
        let mut lock = LockFile::builder();
        let channels = req
            .values()
            .flat_map(|records| {
                records
                    .iter()
                    .map(record::PyRecord::channel)
                    .collect::<Vec<PyResult<_>>>()
            })
            .collect::<PyResult<HashSet<_>>>()?;

        lock.set_channels(&name, channels);

        for (platform, records) in req {
            for record in records {
                lock.add_conda_package(
                    &name,
                    platform.inner,
                    record.try_as_repodata_record()?.clone().into(),
                );
            }
        }

        Ok(lock
            .finish()
            .environment(&name)
            .ok_or(PyRattlerError::EnvironmentCreationError(
                "Environment creation failed.".into(),
            ))?
            .into())
    }

    /// Returns all the platforms for which we have a locked-down environment.
    pub fn platforms(&self) -> Vec<PyPlatform> {
        self.inner.platforms().map(Into::into).collect::<Vec<_>>()
    }

    /// Returns the channels that are used by this environment.
    ///
    /// Note that the order of the channels is significant. The first channel is the highest
    /// priority channel.
    pub fn channels(&self) -> Vec<PyLockChannel> {
        self.inner
            .channels()
            .iter()
            .map(|p| p.clone().into())
            .collect()
    }

    /// Returns all the packages for a specific platform in this environment.
    pub fn packages(&self, platform: PyPlatform) -> Option<Vec<PyLockedPackage>> {
        if let Some(packages) = self.inner.packages(platform.inner) {
            return Some(packages.map(std::convert::Into::into).collect());
        }
        None
    }

    /// Returns a list of all packages and platforms defined for this environment
    pub fn packages_by_platform(&self) -> Vec<(PyPlatform, Vec<PyLockedPackage>)> {
        self.inner
            .packages_by_platform()
            .map(|(platform, pkgs)| (platform.into(), pkgs.map(Into::into).collect::<Vec<_>>()))
            .collect()
    }

    /// Returns all pypi packages for all platforms
    pub fn pypi_packages(
        &self,
    ) -> HashMap<PyPlatform, Vec<(PyPypiPackageData, PyPypiPackageEnvironmentData)>> {
        self.inner
            .pypi_packages()
            .into_iter()
            .map(|(platform, data_vec)| {
                let data = data_vec
                    .into_iter()
                    .map(|(pkg_data, pkg_env_data)| (pkg_data.into(), pkg_env_data.into()))
                    .collect::<Vec<(PyPypiPackageData, PyPypiPackageEnvironmentData)>>();
                (platform.into(), data)
            })
            .collect()
    }

    /// Returns all conda packages for all platforms and converts them to [`PyRecord`].
    pub fn conda_repodata_records(&self) -> PyResult<HashMap<PyPlatform, Vec<PyRecord>>> {
        Ok(self
            .inner
            .conda_repodata_records()
            .map_err(PyRattlerError::from)?
            .into_iter()
            .map(|(platform, record_vec)| {
                (
                    platform.into(),
                    record_vec.into_iter().map(Into::into).collect(),
                )
            })
            .collect())
    }

    /// Takes all the conda packages, converts them to [`PyRecord`] and returns them or
    /// returns an error if the conversion failed. Returns `None` if the specified platform is not
    /// defined for this environment.
    pub fn conda_repodata_records_for_platform(
        &self,
        platform: PyPlatform,
    ) -> PyResult<Option<Vec<PyRecord>>> {
        if let Some(records) = self
            .inner
            .conda_repodata_records_for_platform(platform.inner)
            .map_err(PyRattlerError::from)?
        {
            return Ok(Some(records.into_iter().map(Into::into).collect()));
        }
        Ok(None)
    }

    /// Returns all the pypi packages and their associated environment data for the specified
    /// platform. Returns `None` if the platform is not defined for this environment.
    pub fn pypi_packages_for_platform(
        &self,
        platform: PyPlatform,
    ) -> Option<Vec<(PyPypiPackageData, PyPypiPackageEnvironmentData)>> {
        if let Some(data) = self.inner.pypi_packages_for_platform(platform.inner) {
            return Some(
                data.into_iter()
                    .map(|(pkg_data, env_data)| (pkg_data.into(), env_data.into()))
                    .collect(),
            );
        }
        None
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyLockChannel {
    pub(crate) inner: Channel,
}

impl From<Channel> for PyLockChannel {
    fn from(value: Channel) -> Self {
        Self { inner: value }
    }
}

impl From<rattler_conda_types::Channel> for PyLockChannel {
    fn from(value: rattler_conda_types::Channel) -> Self {
        Self {
            inner: Channel::from(value.base_url().to_string()),
        }
    }
}

impl From<PyLockChannel> for Channel {
    fn from(value: PyLockChannel) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyLockChannel {
    #[new]
    pub fn new(url: String) -> Self {
        Self {
            inner: Channel::from(url),
        }
    }

    pub fn as_str(&self) -> String {
        self.inner.url.clone()
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyLockedPackage {
    pub(crate) inner: Package,
}

impl From<Package> for PyLockedPackage {
    fn from(value: Package) -> Self {
        Self { inner: value }
    }
}

impl From<PyLockedPackage> for Package {
    fn from(value: PyLockedPackage) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyLockedPackage {
    #[getter]
    pub fn is_conda(&self) -> bool {
        self.inner.is_conda()
    }

    #[getter]
    pub fn is_pypi(&self) -> bool {
        self.inner.is_pypi()
    }

    #[getter]
    pub fn name(&self) -> String {
        self.inner.name().to_string()
    }

    #[getter]
    pub fn version(&self) -> String {
        self.inner.version().to_string()
    }

    #[getter]
    pub fn url_or_path(&self) -> String {
        self.inner.url_or_path().to_string()
    }

    pub fn as_conda(&self) -> Option<PyRecord> {
        if let Some(pkg) = self.inner.as_conda() {
            return Some(Into::into(RepoDataRecord {
                package_record: pkg.package_record().clone(),
                file_name: pkg.file_name().unwrap_or("").into(),
                channel: pkg.channel().map_or("".to_string(), |c| c.to_string()),
                url: pkg.url().clone(),
            }));
        }
        None
    }

    pub fn as_pypi(&self) -> Option<(PyPypiPackageData, PyPypiPackageEnvironmentData)> {
        if let Some(pkg) = self.inner.as_pypi() {
            let pkg = pkg.data();
            return Some((pkg.package.clone().into(), pkg.environment.clone().into()));
        }
        None
    }

    pub fn satisfies(&self, spec: &str) -> PyResult<bool> {
        match &self.inner {
            Package::Conda(pkg) => Ok(pkg.satisfies(
                &MatchSpec::from_str(spec, ParseStrictness::Lenient)
                    .map_err(PyRattlerError::from)?,
            )),
            Package::Pypi(pkg) => Ok(pkg.satisfies(
                &Requirement::from_str(spec)
                    .map_err(|e| PyRattlerError::RequirementError(e.to_string()))?,
            )),
        }
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPypiPackageData {
    pub(crate) inner: PypiPackageData,
}

impl From<PypiPackageData> for PyPypiPackageData {
    fn from(value: PypiPackageData) -> Self {
        Self { inner: value }
    }
}

impl From<PyPypiPackageData> for PypiPackageData {
    fn from(value: PyPypiPackageData) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPypiPackageData {
    /// Returns true if this package satisfies the given `spec`.
    pub fn satisfies(&self, spec: String) -> PyResult<bool> {
        Ok(self.inner.satisfies(
            &Requirement::from_str(&spec)
                .map_err(|e| PyRattlerError::RequirementError(e.to_string()))?,
        ))
    }

    /// The name of the package.
    #[getter]
    pub fn name(&self) -> String {
        self.inner.name.to_string()
    }

    /// The version of the package.
    #[getter]
    pub fn version(&self) -> String {
        self.inner.version.clone().to_string()
    }

    /// The URL that points to where the artifact can be downloaded from.
    #[getter]
    pub fn url_or_path(&self) -> String {
        self.inner.url_or_path.to_string()
    }

    /// Whether the package is installed in editable mode or not.
    #[getter]
    pub fn is_editable(&self) -> bool {
        self.inner.editable
    }

    /// Hashes of the file pointed to by `url`.
    #[getter]
    pub fn hash(&self) -> Option<PyPackageHashes> {
        if let Some(hash) = self.inner.hash.clone() {
            return Some(hash.into());
        }
        None
    }

    /// A list of dependencies on other packages.
    #[getter]
    pub fn requires_dist(&self) -> Vec<String> {
        self.inner
            .requires_dist
            .clone()
            .into_iter()
            .map(|req| req.to_string())
            .collect()
    }

    /// The python version that this package requires.
    #[getter]
    pub fn requires_python(&self) -> Option<String> {
        if let Some(specifier) = self.inner.requires_python.clone() {
            return Some(specifier.to_string());
        }
        None
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPypiPackageEnvironmentData {
    pub(crate) inner: PypiPackageEnvironmentData,
}

impl From<PypiPackageEnvironmentData> for PyPypiPackageEnvironmentData {
    fn from(value: PypiPackageEnvironmentData) -> Self {
        Self { inner: value }
    }
}

impl From<PyPypiPackageEnvironmentData> for PypiPackageEnvironmentData {
    fn from(value: PyPypiPackageEnvironmentData) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPypiPackageEnvironmentData {
    /// The extras enabled for the package. Note that the order doesn't matter.
    #[getter]
    pub fn extras(&self) -> BTreeSet<String> {
        self.inner
            .extras
            .iter()
            .map(std::string::ToString::to_string)
            .collect()
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPackageHashes {
    pub(crate) inner: PackageHashes,
}

impl From<PackageHashes> for PyPackageHashes {
    fn from(value: PackageHashes) -> Self {
        Self { inner: value }
    }
}

impl From<PyPackageHashes> for PackageHashes {
    fn from(value: PyPackageHashes) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPackageHashes {
    // #[new]
    // pub fn new(md5: &str, sha256: &str) -> Self {
    //     let md5_digest = parse_digest_from_hex::<rattler_digest::Md5>(md5);
    //     let sha256_digest = parse_digest_from_hex::<rattler_digest::Sha256>(sha256);

    //     PackageHashes::from_hashes(md5_digest, sha256_digest)
    //         .expect("this should never happen since both the hashes were provided")
    //         .into()
    // }

    #[getter]
    pub fn sha256<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.inner
            .sha256()
            .map(|sha256| PyBytes::new_bound(py, sha256))
    }

    #[getter]
    pub fn md5<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.inner.md5().map(|md5| PyBytes::new_bound(py, md5))
    }
}
