use crate::channel::PyChannel;
use crate::match_spec::PyMatchSpec;
use crate::version::PyVersion;
use crate::{error::PyRattlerError, platform::PyPlatform, record::PyRecord};
use pep508_rs::Requirement;
use pyo3::{pyclass, pymethods, types::PyBytes, Bound, PyResult, Python};
use rattler_conda_types::RepoDataRecord;
use rattler_lock::{
    Channel, CondaPackageData, Environment, LockFile, LockedPackage, OwnedEnvironment,
    PackageHashes, PypiPackageData, PypiPackageEnvironmentData, DEFAULT_ENVIRONMENT_NAME,
};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    str::FromStr,
};

/// Represents a lock-file for both Conda packages and Pypi packages.
///
/// Lock-files can store information for multiple platforms and for multiple
/// environments.
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
                .as_ref()
                .conda_repodata_records_by_platform()
                .map_err(PyRattlerError::from)?
            {
                for record in records {
                    lock.add_conda_package(&name, platform, record.into());
                }
            }

            for (platform, records) in env.as_ref().pypi_packages_by_platform() {
                for (pkg_data, pkg_env_data) in records {
                    lock.add_pypi_package(&name, platform, pkg_data.clone(), pkg_env_data.clone());
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
        PyEnvironment::from_lock_file_and_name(self.inner.clone(), name).ok()
    }

    /// Returns the environment with the default name as defined by
    /// [`DEFAULT_ENVIRONMENT_NAME`].
    pub fn default_environment(&self) -> Option<PyEnvironment> {
        PyEnvironment::from_lock_file_and_name(self.inner.clone(), DEFAULT_ENVIRONMENT_NAME).ok()
    }

    /// Returns an iterator over all environments defined in the lock-file.
    pub fn environments(&self) -> Vec<(String, PyEnvironment)> {
        self.inner
            .environments()
            .map(|(name, _)| {
                (
                    name.to_string(),
                    PyEnvironment::from_lock_file_and_name(self.inner.clone(), name).unwrap(),
                )
            })
            .collect()
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyEnvironment {
    environment: OwnedEnvironment,
}

impl PyEnvironment {
    fn as_ref(&self) -> Environment<'_> {
        self.environment.as_ref()
    }

    pub fn from_lock_file_and_name(lock: LockFile, name: &str) -> PyResult<Self> {
        let environment = lock
            .environment(name)
            .ok_or(PyRattlerError::EnvironmentCreationError(
                "Environment creation failed.".into(),
            ))?
            .to_owned();
        Ok(Self { environment })
    }
}

#[pymethods]
impl PyEnvironment {
    #[new]
    pub fn new(
        name: String,
        records: HashMap<PyPlatform, Vec<PyRecord>>,
        channels: Vec<PyChannel>,
    ) -> PyResult<Self> {
        let mut lock = LockFile::builder();

        lock.set_channels(
            &name,
            channels.into_iter().map(|c| {
                rattler_lock::Channel::from(
                    c.inner.base_url.as_str().trim_end_matches('/').to_string(),
                )
            }),
        );

        for (platform, records) in records {
            for record in records {
                lock.add_conda_package(
                    &name,
                    platform.inner,
                    record.try_as_repodata_record()?.clone().into(),
                );
            }
        }

        Self::from_lock_file_and_name(lock.finish(), &name)
    }

    /// Returns all the platforms for which we have a locked-down environment.
    pub fn platforms(&self) -> Vec<PyPlatform> {
        self.as_ref()
            .platforms()
            .map(Into::into)
            .collect::<Vec<_>>()
    }

    /// Returns the channels that are used by this environment.
    ///
    /// Note that the order of the channels is significant. The first channel is
    /// the highest priority channel.
    pub fn channels(&self) -> Vec<PyLockChannel> {
        self.as_ref()
            .channels()
            .iter()
            .map(|p| p.clone().into())
            .collect()
    }

    /// Returns all the packages for a specific platform in this environment.
    pub fn packages(&self, platform: PyPlatform) -> Option<Vec<PyLockedPackage>> {
        if let Some(packages) = self.as_ref().packages(platform.inner) {
            return Some(packages.map(LockedPackage::from).map(Into::into).collect());
        }
        None
    }

    /// Returns a list of all packages and platforms defined for this
    /// environment
    pub fn packages_by_platform(&self) -> Vec<(PyPlatform, Vec<PyLockedPackage>)> {
        self.as_ref()
            .packages_by_platform()
            .map(|(platform, pkgs)| {
                (
                    platform.into(),
                    pkgs.map(|pkg| LockedPackage::from(pkg).into())
                        .collect::<Vec<_>>(),
                )
            })
            .collect()
    }

    /// Returns all pypi packages for all platforms
    pub fn pypi_packages(&self) -> HashMap<PyPlatform, Vec<PyLockedPackage>> {
        self.as_ref()
            .pypi_packages_by_platform()
            .map(|(platform, data_vec)| {
                let data = data_vec
                    .map(|(pkg_data, pkg_env_data)| {
                        PyLockedPackage::from(LockedPackage::Pypi(
                            pkg_data.clone(),
                            pkg_env_data.clone(),
                        ))
                    })
                    .collect::<Vec<_>>();
                (platform.into(), data)
            })
            .collect()
    }

    /// Returns all conda packages for all platforms and converts them to
    /// [`PyRecord`].
    pub fn conda_repodata_records(&self) -> PyResult<HashMap<PyPlatform, Vec<PyRecord>>> {
        Ok(self
            .as_ref()
            .conda_repodata_records_by_platform()
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

    /// Takes all the conda packages, converts them to [`PyRecord`] and returns
    /// them or returns an error if the conversion failed. Returns `None` if
    /// the specified platform is not defined for this environment.
    pub fn conda_repodata_records_for_platform(
        &self,
        platform: PyPlatform,
    ) -> PyResult<Option<Vec<PyRecord>>> {
        if let Some(records) = self
            .as_ref()
            .conda_repodata_records(platform.inner)
            .map_err(PyRattlerError::from)?
        {
            return Ok(Some(records.into_iter().map(Into::into).collect()));
        }
        Ok(None)
    }

    /// Returns all the pypi packages and their associated environment data for
    /// the specified platform. Returns `None` if the platform is not
    /// defined for this environment.
    pub fn pypi_packages_for_platform(&self, platform: PyPlatform) -> Option<Vec<PyLockedPackage>> {
        if let Some(data) = self.as_ref().pypi_packages(platform.inner) {
            return Some(
                data.map(|(pkg, env)| {
                    PyLockedPackage::from(LockedPackage::Pypi(pkg.clone(), env.clone()))
                })
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
            inner: Channel::from(value.base_url.to_string()),
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
    pub(crate) inner: LockedPackage,
}

impl From<LockedPackage> for PyLockedPackage {
    fn from(value: LockedPackage) -> Self {
        Self { inner: value }
    }
}

impl From<PyLockedPackage> for LockedPackage {
    fn from(value: PyLockedPackage) -> Self {
        value.inner
    }
}

impl PyLockedPackage {
    fn as_conda(&self) -> &CondaPackageData {
        self.inner.as_conda().expect("must be conda")
    }

    fn as_pypi(&self) -> &PypiPackageData {
        self.inner.as_pypi().expect("must be pypi").0
    }

    fn as_pypi_env(&self) -> &PypiPackageEnvironmentData {
        self.inner.as_pypi().expect("must be pypi").1
    }
}

#[pymethods]
impl PyLockedPackage {
    #[getter]
    pub fn repo_data_record(&self) -> PyResult<PyRecord> {
        Ok(
            RepoDataRecord::try_from(self.as_conda().as_binary().expect("must be binary"))
                .map_err(PyRattlerError::from)
                .map(Into::into)?,
        )
    }

    #[getter]
    pub fn package_record(&self) -> PyRecord {
        self.as_conda().record().clone().into()
    }

    #[getter]
    pub fn name(&self) -> String {
        match &self.inner {
            LockedPackage::Conda(data) => data.record().name.as_source().to_string(),
            LockedPackage::Pypi(data, _) => data.name.to_string(),
        }
    }

    #[getter]
    pub fn location(&self) -> String {
        match &self.inner {
            LockedPackage::Conda(data) => data.location().to_string(),
            LockedPackage::Pypi(data, _) => data.location.to_string(),
        }
    }

    #[getter]
    pub fn conda_version(&self) -> PyVersion {
        self.as_conda().record().version.version().clone().into()
    }

    #[getter]
    pub fn pypi_version(&self) -> String {
        self.as_pypi().version.to_string()
    }

    /// Whether the package is installed in editable mode or not.
    #[getter]
    pub fn pypi_is_editable(&self) -> bool {
        self.as_pypi().editable
    }

    // Hashes of the file pointed to by `url`.
    #[getter]
    pub fn hashes(&self) -> Option<PyPackageHashes> {
        let hash = match &self.inner {
            LockedPackage::Conda(pkg) => {
                let record = pkg.record();
                match (record.md5, record.sha256) {
                    (Some(md5), Some(sha256)) => Some(PackageHashes::Md5Sha256(md5, sha256)),
                    (Some(md5), None) => Some(PackageHashes::Md5(md5)),
                    (None, Some(sha256)) => Some(PackageHashes::Sha256(sha256)),
                    (None, None) => None,
                }
            }
            LockedPackage::Pypi(data, _) => data.hash.clone(),
        };
        hash.map(Into::into)
    }

    /// A list of dependencies on other packages.
    #[getter]
    pub fn pypi_requires_dist(&self) -> Vec<String> {
        self.as_pypi()
            .requires_dist
            .clone()
            .into_iter()
            .map(|req| req.to_string())
            .collect()
    }

    /// The python version that this package requires.
    #[getter]
    pub fn pypi_requires_python(&self) -> Option<String> {
        if let Some(specifier) = self.as_pypi().requires_python.clone() {
            return Some(specifier.to_string());
        }
        None
    }

    #[getter]
    pub fn pypi_extras(&self) -> BTreeSet<String> {
        self.as_pypi_env()
            .extras
            .iter()
            .map(std::string::ToString::to_string)
            .collect()
    }

    pub fn pypi_satisfies(&self, spec: &str) -> PyResult<bool> {
        let req = Requirement::from_str(spec)
            .map_err(|e| PyRattlerError::RequirementError(e.to_string()))?;
        Ok(self.as_pypi().satisfies(&req))
    }

    pub fn conda_satisfies(&self, spec: &PyMatchSpec) -> bool {
        self.as_conda().satisfies(&spec.inner)
    }

    #[getter]
    pub fn is_conda_source(&self) -> bool {
        matches!(
            &self.inner,
            LockedPackage::Conda(CondaPackageData::Source(_))
        )
    }

    #[getter]
    pub fn is_conda_binary(&self) -> bool {
        matches!(
            &self.inner,
            LockedPackage::Conda(CondaPackageData::Binary(_))
        )
    }

    #[getter]
    pub fn is_pypi(&self) -> bool {
        matches!(&self.inner, LockedPackage::Pypi(..))
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
    pub fn location(&self) -> String {
        self.inner.location.to_string()
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
    //     let sha256_digest =
    // parse_digest_from_hex::<rattler_digest::Sha256>(sha256);

    //     PackageHashes::from_hashes(md5_digest, sha256_digest)
    //         .expect("this should never happen since both the hashes were
    // provided")         .into()
    // }

    #[getter]
    pub fn sha256<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.inner.sha256().map(|sha256| PyBytes::new(py, sha256))
    }

    #[getter]
    pub fn md5<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.inner.md5().map(|md5| PyBytes::new(py, md5))
    }
}
