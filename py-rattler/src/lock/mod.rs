use crate::channel::PyChannel;
use crate::match_spec::PyMatchSpec;
use crate::platform::PyPlatform;
use crate::version::PyVersion;
use crate::{error::PyRattlerError, record::PyRecord};
use pep508_rs::Requirement;
use pyo3::{pyclass, pymethods, types::PyBytes, Bound, PyResult, Python};
use rattler_conda_types::RepoDataRecord;
use rattler_lock::{
    Channel, CondaPackageData, Environment, LockFile, LockedPackage, OwnedEnvironment,
    OwnedPlatform, PackageHashes, PlatformData, PlatformName, PypiPackageData,
    PypiPackageEnvironmentData, UrlOrPath, Verbatim, DEFAULT_ENVIRONMENT_NAME,
};
use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    str::FromStr,
    sync::Mutex,
};

/// State for building a lock file incrementally.
#[derive(Clone, Default)]
struct LockFileBuildState {
    platforms: Vec<PlatformData>,
    conda_packages: Vec<(String, String, CondaPackageData)>, // (env, platform_name, data)
    pypi_packages: Vec<(String, String, PypiPackageData, PypiPackageEnvironmentData)>, // (env, platform_name, data, env_data)
    channels: HashMap<String, Vec<Channel>>, // env -> channels
}

impl LockFileBuildState {
    fn build(&self) -> PyResult<LockFile> {
        let mut builder = LockFile::builder();
        builder = builder
            .with_platforms(self.platforms.clone())
            .map_err(|e| PyRattlerError::LockFileError(e.to_string()))?;

        for (env, channels) in &self.channels {
            builder.set_channels(env, channels.iter().cloned());
        }

        for (env, platform_name, pkg) in &self.conda_packages {
            builder
                .add_conda_package(env, platform_name, pkg.clone())
                .map_err(|e| PyRattlerError::LockFileError(e.to_string()))?;
        }

        for (env, platform_name, pkg, env_data) in &self.pypi_packages {
            builder
                .add_pypi_package(env, platform_name, pkg.clone(), env_data.clone())
                .map_err(|e| PyRattlerError::LockFileError(e.to_string()))?;
        }

        Ok(builder.finish())
    }
}

/// Internal state of a `PyLockFile` - either loaded from disk or being built.
#[derive(Clone)]
enum LockFileState {
    /// A lock file loaded from disk (read-only).
    Loaded(LockFile),
    /// A lock file being built incrementally.
    Building(LockFileBuildState),
}

/// Represents a lock-file for both Conda packages and Pypi packages.
///
/// Lock-files can store information for multiple platforms and for multiple
/// environments.
#[pyclass]
pub struct PyLockFile {
    state: Mutex<LockFileState>,
}

impl Clone for PyLockFile {
    fn clone(&self) -> Self {
        Self {
            state: Mutex::new(self.state.lock().unwrap().clone()),
        }
    }
}

impl PyLockFile {
    /// Gets the `LockFile`, building it if necessary.
    fn get_lock_file(&self) -> PyResult<LockFile> {
        let state = self.state.lock().unwrap();
        match &*state {
            LockFileState::Loaded(lock_file) => Ok(lock_file.clone()),
            LockFileState::Building(build_state) => build_state.build(),
        }
    }

    /// Gets the build state, returning an error if this is a loaded lock file.
    fn with_build_state_mut<F, R>(&self, f: F) -> PyResult<R>
    where
        F: FnOnce(&mut LockFileBuildState) -> PyResult<R>,
    {
        let mut state = self.state.lock().unwrap();
        match &mut *state {
            LockFileState::Loaded(_) => Err(PyRattlerError::LockFileError(
                "Cannot modify a lock file loaded from disk".into(),
            )
            .into()),
            LockFileState::Building(build_state) => f(build_state),
        }
    }
}

impl From<LockFile> for PyLockFile {
    fn from(value: LockFile) -> Self {
        Self {
            state: Mutex::new(LockFileState::Loaded(value)),
        }
    }
}

impl From<PyLockFile> for LockFile {
    fn from(value: PyLockFile) -> Self {
        value.get_lock_file().expect("failed to build lock file")
    }
}

#[pymethods]
impl PyLockFile {
    /// Creates a new lock file with the given platforms.
    ///
    /// Packages can be added using `add_conda_package` and `add_pypi_package`.
    /// Channels can be set using `set_channels`.
    #[new]
    pub fn new(platforms: Vec<PyLockPlatform>) -> PyResult<Self> {
        let platform_data: Vec<PlatformData> = platforms
            .into_iter()
            .map(|p| p.to_platform_data())
            .collect();

        Ok(Self {
            state: Mutex::new(LockFileState::Building(LockFileBuildState {
                platforms: platform_data,
                ..Default::default()
            })),
        })
    }

    /// Sets the channels for an environment.
    pub fn set_channels(&self, environment: String, channels: Vec<PyLockChannel>) -> PyResult<()> {
        self.with_build_state_mut(|state| {
            state
                .channels
                .insert(environment, channels.into_iter().map(|c| c.inner).collect());
            Ok(())
        })
    }

    /// Adds a conda package to the lock file.
    ///
    /// The platform must be one of the platforms specified when creating the lock file.
    pub fn add_conda_package(
        &self,
        environment: String,
        platform: PyLockPlatform,
        record: PyRecord,
    ) -> PyResult<()> {
        let platform_name = platform.name();
        let repo_data_record = record.try_as_repodata_record()?.clone();

        self.with_build_state_mut(|state| {
            // Validate that the platform is known
            if !state
                .platforms
                .iter()
                .any(|p| p.name.as_str() == platform_name)
            {
                return Err(PyRattlerError::LockFileError(format!(
                    "Platform '{platform_name}' is not in the list of platforms for this lock file"
                ))
                .into());
            }

            state
                .conda_packages
                .push((environment, platform_name, repo_data_record.into()));
            Ok(())
        })
    }

    /// Adds a pypi package to the lock file.
    ///
    /// The platform must be one of the platforms specified when creating the lock file.
    pub fn add_pypi_package(
        &self,
        environment: String,
        platform: PyLockPlatform,
        name: String,
        version: String,
        location: String,
    ) -> PyResult<()> {
        let platform_name = platform.name();

        self.with_build_state_mut(|state| {
            // Validate that the platform is known
            if !state
                .platforms
                .iter()
                .any(|p| p.name.as_str() == platform_name)
            {
                return Err(PyRattlerError::LockFileError(format!(
                    "Platform '{platform_name}' is not in the list of platforms for this lock file"
                ))
                .into());
            }

            let pkg_data = PypiPackageData {
                name: pep508_rs::PackageName::from_str(&name)
                    .map_err(|e| PyRattlerError::LockFileError(e.to_string()))?,
                version: pep440_rs::Version::from_str(&version)
                    .map_err(|e| PyRattlerError::LockFileError(e.to_string()))?,
                location: Verbatim::<UrlOrPath>::from_str(&location)
                    .map_err(|e| PyRattlerError::LockFileError(e.to_string()))?,
                hash: None,
                requires_dist: Vec::new(),
                requires_python: None,
            };
            let env_data = PypiPackageEnvironmentData::default();

            state
                .pypi_packages
                .push((environment, platform_name, pkg_data, env_data));
            Ok(())
        })
    }

    /// Writes the conda lock to a file
    pub fn to_path(&self, path: PathBuf) -> PyResult<()> {
        let lock_file = self.get_lock_file()?;
        Ok(lock_file.to_path(&path).map_err(PyRattlerError::from)?)
    }

    /// Parses an rattler-lock file from a file.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(LockFile::from_path(&path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns the environment with the given name.
    pub fn environment(&self, name: &str) -> PyResult<Option<PyEnvironment>> {
        let lock_file = self.get_lock_file()?;
        Ok(PyEnvironment::from_lock_file_and_name(lock_file, name).ok())
    }

    /// Returns the environment with the default name as defined by
    /// [`DEFAULT_ENVIRONMENT_NAME`].
    pub fn default_environment(&self) -> PyResult<Option<PyEnvironment>> {
        let lock_file = self.get_lock_file()?;
        Ok(PyEnvironment::from_lock_file_and_name(lock_file, DEFAULT_ENVIRONMENT_NAME).ok())
    }

    /// Returns an iterator over all environments defined in the lock-file.
    pub fn environments(&self) -> PyResult<Vec<(String, PyEnvironment)>> {
        let lock_file = self.get_lock_file()?;
        Ok(lock_file
            .environments()
            .map(|(name, _)| {
                (
                    name.to_string(),
                    PyEnvironment::from_lock_file_and_name(lock_file.clone(), name).unwrap(),
                )
            })
            .collect())
    }

    /// Returns the platform with the given name.
    pub fn platform(&self, name: &str) -> PyResult<Option<PyLockPlatform>> {
        let lock_file = self.get_lock_file()?;
        Ok(lock_file
            .platform(name)
            .map(|p| p.to_owned(&lock_file).into()))
    }

    /// Returns all platforms defined in the lock-file.
    pub fn platforms(&self) -> PyResult<Vec<PyLockPlatform>> {
        let lock_file = self.get_lock_file()?;
        Ok(lock_file
            .platforms()
            .map(|p| p.to_owned(&lock_file).into())
            .collect())
    }
}

/// Internal representation of a lock platform - either owned from an existing
/// lock file or standalone data.
#[derive(Clone)]
enum LockPlatformInner {
    /// Platform from an existing lock file.
    Owned(OwnedPlatform),
    /// Standalone platform data (not yet part of a lock file).
    Standalone(PlatformData),
}

/// Represents a platform in a lock file.
///
/// This provides access to the platform name, the underlying conda subdir,
/// and any virtual packages associated with the platform.
#[pyclass]
#[derive(Clone)]
pub struct PyLockPlatform {
    inner: LockPlatformInner,
}

impl PyLockPlatform {
    /// Returns the name of the platform (e.g., "linux-64", "osx-arm64").
    pub fn name(&self) -> String {
        match &self.inner {
            LockPlatformInner::Owned(owned) => owned.as_ref().name().to_string(),
            LockPlatformInner::Standalone(data) => data.name.to_string(),
        }
    }

    /// Returns the underlying conda subdir/platform.
    pub fn subdir(&self) -> rattler_conda_types::Platform {
        match &self.inner {
            LockPlatformInner::Owned(owned) => owned.as_ref().subdir(),
            LockPlatformInner::Standalone(data) => data.subdir,
        }
    }

    /// Returns the list of virtual packages for this platform.
    pub fn virtual_packages(&self) -> Vec<String> {
        match &self.inner {
            LockPlatformInner::Owned(owned) => owned.as_ref().virtual_packages().to_vec(),
            LockPlatformInner::Standalone(data) => data.virtual_packages.clone(),
        }
    }

    /// Returns the platform data for use in building a lock file.
    pub(crate) fn to_platform_data(&self) -> PlatformData {
        match &self.inner {
            LockPlatformInner::Owned(owned) => {
                let platform = owned.as_ref();
                PlatformData {
                    name: platform.name().clone(),
                    subdir: platform.subdir(),
                    virtual_packages: platform.virtual_packages().to_vec(),
                }
            }
            LockPlatformInner::Standalone(data) => data.clone(),
        }
    }

    /// Returns a Platform reference for use with lock file methods.
    /// For owned platforms, returns the underlying reference directly.
    /// For standalone platforms, looks up the platform by name in the given lock file.
    pub(crate) fn platform<'a>(&'a self) -> rattler_lock::Platform<'a> {
        match &self.inner {
            LockPlatformInner::Owned(owned) => owned.as_ref(),
            LockPlatformInner::Standalone(_) => {
                panic!("Cannot get platform reference from standalone platform - use an owned platform from the lock file")
            }
        }
    }
}

impl From<OwnedPlatform> for PyLockPlatform {
    fn from(inner: OwnedPlatform) -> Self {
        Self {
            inner: LockPlatformInner::Owned(inner),
        }
    }
}

#[pymethods]
impl PyLockPlatform {
    /// Creates a new platform with the given name.
    ///
    /// The subdir is automatically determined from the name if it matches a known platform.
    /// Virtual packages default to an empty list.
    #[new]
    #[pyo3(signature = (name, subdir=None, virtual_packages=None))]
    pub fn new(
        name: String,
        subdir: Option<PyPlatform>,
        virtual_packages: Option<Vec<String>>,
    ) -> PyResult<Self> {
        let platform_name = PlatformName::try_from(name.clone())
            .map_err(|e| PyRattlerError::LockFileError(e.to_string()))?;

        // Try to determine the subdir from the name if not provided
        let subdir = match subdir {
            Some(p) => p.inner,
            None => rattler_conda_types::Platform::from_str(&name)
                .map_err(|e| PyRattlerError::LockFileError(e.to_string()))?,
        };

        Ok(Self {
            inner: LockPlatformInner::Standalone(PlatformData {
                name: platform_name,
                subdir,
                virtual_packages: virtual_packages.unwrap_or_default(),
            }),
        })
    }

    /// The name of the platform (e.g., "linux-64", "osx-arm64").
    #[getter(name)]
    fn py_name(&self) -> String {
        self.name()
    }

    /// The underlying conda subdir/platform.
    #[getter(subdir)]
    fn py_subdir(&self) -> PyPlatform {
        self.subdir().into()
    }

    /// The list of virtual packages for this platform.
    #[getter(virtual_packages)]
    fn py_virtual_packages(&self) -> Vec<String> {
        self.virtual_packages()
    }

    fn __repr__(&self) -> String {
        format!("LockPlatform(name='{}')", self.name())
    }

    fn __str__(&self) -> String {
        self.name()
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

        // Collect all unique platforms
        let platforms: Vec<PlatformData> = records
            .keys()
            .map(|p| PlatformData {
                name: PlatformName::try_from(p.inner.to_string())
                    .expect("platform name should be valid"),
                subdir: p.inner,
                virtual_packages: Vec::new(),
            })
            .collect();
        lock = lock
            .with_platforms(platforms)
            .map_err(|e| PyRattlerError::LockFileError(e.to_string()))?;

        lock.set_channels(
            &name,
            channels.into_iter().map(|c| {
                rattler_lock::Channel::from(
                    c.inner.base_url.as_str().trim_end_matches('/').to_string(),
                )
            }),
        );

        for (platform, records) in records {
            let platform_name = platform.inner.to_string();
            for record in records {
                lock.add_conda_package(
                    &name,
                    &platform_name,
                    record.try_as_repodata_record()?.clone().into(),
                )
                .map_err(|e| PyRattlerError::LockFileError(e.to_string()))?;
            }
        }

        Self::from_lock_file_and_name(lock.finish(), &name)
    }

    /// Returns all the platforms for which we have a locked-down environment.
    pub fn platforms(&self) -> Vec<PyLockPlatform> {
        let lock_file = self.environment.lock_file();
        self.as_ref()
            .platforms()
            .map(|p| p.to_owned(&lock_file).into())
            .collect()
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
    pub fn packages(&self, platform: PyLockPlatform) -> Option<Vec<PyLockedPackage>> {
        self.as_ref()
            .packages(platform.platform())
            .map(|packages| packages.map(LockedPackage::from).map(Into::into).collect())
    }

    /// Returns a list of all packages and platforms defined for this
    /// environment
    pub fn packages_by_platform(&self) -> Vec<(PyLockPlatform, Vec<PyLockedPackage>)> {
        let lock_file = self.environment.lock_file();
        self.as_ref()
            .packages_by_platform()
            .map(|(platform, pkgs)| {
                (
                    platform.to_owned(&lock_file).into(),
                    pkgs.map(|pkg| LockedPackage::from(pkg).into()).collect(),
                )
            })
            .collect()
    }

    /// Returns all pypi packages for all platforms
    pub fn pypi_packages(&self) -> HashMap<String, Vec<PyLockedPackage>> {
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
                (platform.name().to_string(), data)
            })
            .collect()
    }

    /// Returns all conda packages for all platforms and converts them to
    /// [`PyRecord`].
    pub fn conda_repodata_records(&self) -> PyResult<HashMap<String, Vec<PyRecord>>> {
        Ok(self
            .as_ref()
            .conda_repodata_records_by_platform()
            .map_err(PyRattlerError::from)?
            .into_iter()
            .map(|(platform, record_vec)| {
                (
                    platform.name().to_string(),
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
        platform: PyLockPlatform,
    ) -> PyResult<Option<Vec<PyRecord>>> {
        if let Some(records) = self
            .as_ref()
            .conda_repodata_records(platform.platform())
            .map_err(PyRattlerError::from)?
        {
            return Ok(Some(records.into_iter().map(Into::into).collect()));
        }
        Ok(None)
    }

    /// Returns all the pypi packages and their associated environment data for
    /// the specified platform. Returns `None` if the platform is not
    /// defined for this environment.
    pub fn pypi_packages_for_platform(
        &self,
        platform: PyLockPlatform,
    ) -> Option<Vec<PyLockedPackage>> {
        self.as_ref()
            .pypi_packages(platform.platform())
            .map(|data| {
                data.map(|(pkg, env)| {
                    PyLockedPackage::from(LockedPackage::Pypi(pkg.clone(), env.clone()))
                })
                .collect()
            })
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
