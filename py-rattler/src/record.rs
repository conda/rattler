use std::path::PathBuf;

use pyo3::prelude::PyAnyMethods;
use pyo3::{
    exceptions::PyTypeError, intern, pyclass, pymethods, types::PyBytes, Bound, FromPyObject,
    PyAny, PyErr, PyResult, Python,
};
use rattler_conda_types::{
    package::{IndexJson, PackageFile},
    prefix_record::{Link, LinkType},
    NoArchType, PackageRecord, PrefixRecord, RepoDataRecord, VersionWithSource,
};
use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
use url::Url;

use crate::{
    error::PyRattlerError,
    no_arch_type::PyNoArchType,
    package_name::PyPackageName,
    prefix_paths::PyPrefixPaths,
    utils::{md5_from_pybytes, sha256_from_pybytes},
    version::PyVersion,
};

/// Python bindings for `PrefixRecord`, `RepoDataRecord`, `PackageRecord`.
/// This is to expose these structs in Object Oriented manner, via a single
/// class. This class handles the conversion on its own.
/// It uses a `RecordInner` enum and (try_)as_{x}_record methods for this
/// interface.
///
/// PyO3 cannot expose tagged enums directly, to achieve this we use the
/// `PyRecord` wrapper pyclass on top of `RecordInner`.
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyRecord {
    pub inner: RecordInner,
}

#[derive(Clone)]
pub enum RecordInner {
    Prefix(PrefixRecord),
    RepoData(RepoDataRecord),
    Package(PackageRecord),
}

impl PyRecord {
    pub fn as_package_record(&self) -> &PackageRecord {
        self.as_ref()
    }

    pub fn as_package_record_mut(&mut self) -> &mut PackageRecord {
        self.as_mut()
    }

    pub fn try_as_repodata_record(&self) -> PyResult<&RepoDataRecord> {
        match &self.inner {
            RecordInner::Prefix(r) => Ok(&r.repodata_record),
            RecordInner::RepoData(r) => Ok(r),
            RecordInner::Package(_) => Err(PyTypeError::new_err(
                "Cannot use object of type 'PackageRecord' as 'RepoDataRecord'",
            )),
        }
    }

    pub fn try_as_repodata_record_mut(&mut self) -> PyResult<&mut RepoDataRecord> {
        match &mut self.inner {
            RecordInner::Prefix(r) => Ok(&mut r.repodata_record),
            RecordInner::RepoData(r) => Ok(r),
            RecordInner::Package(_) => Err(PyTypeError::new_err(
                "Cannot use object of type 'PackageRecord' as 'RepoDataRecord'",
            )),
        }
    }

    pub fn try_as_prefix_record(&self) -> PyResult<&PrefixRecord> {
        match &self.inner {
            RecordInner::Prefix(r) => Ok(r),
            RecordInner::RepoData(_) => Err(PyTypeError::new_err(
                "Cannot use object of type 'RepoDataRecord' as 'PrefixRecord'",
            )),
            RecordInner::Package(_) => Err(PyTypeError::new_err(
                "Cannot use object of type 'PackageRecord' as 'PrefixRecord'",
            )),
        }
    }

    pub fn try_as_prefix_record_mut(&mut self) -> PyResult<&mut PrefixRecord> {
        match &mut self.inner {
            RecordInner::Prefix(r) => Ok(r),
            RecordInner::RepoData(_) => Err(PyTypeError::new_err(
                "Cannot use object of type 'RepoDataRecord' as 'PrefixRecord'",
            )),
            RecordInner::Package(_) => Err(PyTypeError::new_err(
                "Cannot use object of type 'PackageRecord' as 'PrefixRecord'",
            )),
        }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyLink {
    #[pyo3(get, set)]
    pub source: PathBuf,
    #[pyo3(get, set)]
    pub type_: String,
}

#[pymethods]
impl PyLink {
    #[new]
    pub fn new(source: PathBuf, type_: String) -> Self {
        Self { source, type_ }
    }
}

impl From<PyLink> for Link {
    fn from(value: PyLink) -> Self {
        let link_type = if value.type_.is_empty() {
            None
        } else {
            match value.type_.as_str() {
                "hardlink" => Some(LinkType::HardLink),
                "softlink" => Some(LinkType::SoftLink),
                "copy" => Some(LinkType::Copy),
                "directory" => Some(LinkType::Directory),
                _ => None,
            }
        };

        Link {
            source: value.source,
            link_type,
        }
    }
}

#[pymethods]
impl PyRecord {
    #[staticmethod]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (name, version, build, build_number, subdir, arch=None, platform=None, noarch=None, python_site_packages_path=None))]
    pub fn create(
        name: PyPackageName,
        version: (PyVersion, String),
        build: String,
        build_number: u64,
        subdir: String,
        arch: Option<String>,
        platform: Option<String>,
        noarch: Option<PyNoArchType>,
        python_site_packages_path: Option<String>,
    ) -> Self {
        let noarch = noarch.map(Into::into);
        Self {
            inner: RecordInner::Package(PackageRecord {
                name: name.into(),
                version: VersionWithSource::new(version.0.inner.clone(), version.1),
                build,
                build_number,
                arch,
                platform,
                subdir,
                constrains: Vec::new(),
                depends: Vec::new(),
                features: None,
                legacy_bz2_md5: None,
                legacy_bz2_size: None,
                license: None,
                license_family: None,
                md5: None,
                noarch: noarch.unwrap_or(NoArchType::none()),
                purls: None,
                python_site_packages_path,
                run_exports: None,
                sha256: None,
                size: None,
                timestamp: None,
                track_features: Vec::new(),
            }),
        }
    }

    #[staticmethod]
    pub fn create_repodata_record(
        package_record: PyRecord,
        file_name: PathBuf,
        url: String,
        channel: String,
    ) -> PyResult<Self> {
        if !package_record.is_package_record() {
            return Err(PyTypeError::new_err(
                "Cannot use object of type 'PackageRecord' as 'RepoDataRecord'",
            ));
        }

        Ok(Self {
            inner: RecordInner::RepoData(RepoDataRecord {
                package_record: package_record.as_package_record().clone(),
                file_name: file_name.to_string_lossy().to_string(),
                url: Url::parse(&url).unwrap(),
                channel,
            }),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (package_record, paths_data, link=None, package_tarball_full_path=None, extracted_package_dir=None, requested_spec=None, files=None))]
    pub fn create_prefix_record(
        package_record: PyRecord,
        paths_data: PyPrefixPaths,
        link: Option<PyLink>,
        package_tarball_full_path: Option<PathBuf>,
        extracted_package_dir: Option<PathBuf>,
        requested_spec: Option<String>,
        files: Option<Vec<PathBuf>>,
    ) -> PyResult<Self> {
        if !package_record.is_repodata_record() {
            return Err(PyTypeError::new_err(
                "Cannot use object of type 'PackageRecord' as 'RepoDataRecord'",
            ));
        }

        Ok(Self {
            inner: RecordInner::Prefix(PrefixRecord {
                repodata_record: package_record.try_as_repodata_record().unwrap().clone(),
                package_tarball_full_path,
                extracted_package_dir,
                files: files.unwrap_or_default(),
                paths_data: paths_data.into(),
                link: link.map(Into::into),
                requested_spec,
            }),
        })
    }

    /// Returns a string representation of `PackageRecord`.
    pub fn as_str(&self) -> String {
        format!("{}", self.as_package_record())
    }

    /// Checks whether if the current record is a `PackageRecord`.
    #[getter]
    #[allow(clippy::unused_self)]
    pub fn is_package_record(&self) -> bool {
        // always true, because all records are package records
        true
    }

    /// Checks whether if the current record is a `RepoDataRecord`.
    #[getter]
    pub fn is_repodata_record(&self) -> bool {
        self.try_as_repodata_record().is_ok()
    }

    /// Checks whether if the current record is a `PrefixRecord`.
    #[getter]
    pub fn is_prefix_record(&self) -> bool {
        self.try_as_prefix_record().is_ok()
    }

    /// Optionally the architecture the package supports.
    #[getter]
    pub fn arch(&self) -> Option<String> {
        self.as_package_record().arch.clone()
    }

    #[setter]
    pub fn set_arch(&mut self, arch: Option<String>) {
        self.as_package_record_mut().arch = arch;
    }

    /// The build string of the package.
    #[getter]
    pub fn build(&self) -> String {
        self.as_package_record().build.clone()
    }

    #[setter]
    pub fn set_build(&mut self, build: String) {
        self.as_package_record_mut().build = build;
    }

    /// The build number of the package.
    #[getter]
    pub fn build_number(&self) -> u64 {
        self.as_package_record().build_number
    }

    #[setter]
    pub fn set_build_number(&mut self, build_number: u64) {
        self.as_package_record_mut().build_number = build_number;
    }

    /// Additional constraints on packages.
    /// `constrains` are different from `depends` in that packages
    /// specified in depends must be installed next to this package,
    /// whereas packages specified in `constrains` are not required
    /// to be installed, but if they are installed they must follow
    /// these constraints.
    #[getter]
    pub fn constrains(&self) -> Vec<String> {
        self.as_package_record().constrains.clone()
    }

    #[setter]
    pub fn set_constrains(&mut self, constrains: Vec<String>) {
        self.as_package_record_mut().constrains = constrains;
    }

    /// Specification of packages this package depends on.
    #[getter]
    pub fn depends(&self) -> Vec<String> {
        self.as_package_record().depends.clone()
    }

    #[setter]
    pub fn set_depends(&mut self, depends: Vec<String>) {
        self.as_package_record_mut().depends = depends;
    }

    /// Features are a deprecated way to specify different
    /// feature sets for the conda solver. This is not
    /// supported anymore and should not be used. Instead,
    /// `mutex` packages should be used to specify
    /// mutually exclusive features.
    #[getter]
    pub fn features(&self) -> Option<String> {
        self.as_package_record().features.clone()
    }

    #[setter]
    pub fn set_features(&mut self, features: Option<String>) {
        self.as_package_record_mut().features = features;
    }

    /// A deprecated md5 hash.
    #[getter]
    pub fn legacy_bz2_md5<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.as_package_record()
            .legacy_bz2_md5
            .map(|md5| PyBytes::new_bound(py, &md5))
    }

    #[setter]
    pub fn set_legacy_bz2_md5(&mut self, md5: Option<Bound<'_, PyBytes>>) -> PyResult<()> {
        self.as_package_record_mut().legacy_bz2_md5 = md5.map(md5_from_pybytes).transpose()?;
        Ok(())
    }

    /// A deprecated package archive size.
    #[getter]
    pub fn legacy_bz2_size(&self) -> Option<u64> {
        self.as_package_record().legacy_bz2_size
    }

    #[setter]
    pub fn set_legacy_bz2_size(&mut self, size: Option<u64>) {
        self.as_package_record_mut().legacy_bz2_size = size;
    }

    /// The specific license of the package.
    #[getter]
    pub fn license(&self) -> Option<String> {
        self.as_package_record().license.clone()
    }

    #[setter]
    pub fn set_license(&mut self, license: Option<String>) {
        self.as_package_record_mut().license = license;
    }

    /// The license family.
    #[getter]
    pub fn license_family(&self) -> Option<String> {
        self.as_package_record().license_family.clone()
    }

    #[setter]
    pub fn set_license_family(&mut self, family: Option<String>) {
        self.as_package_record_mut().license_family = family;
    }

    /// Optionally a MD5 hash of the package archive.
    #[getter]
    pub fn md5<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.as_package_record()
            .md5
            .map(|md5| PyBytes::new_bound(py, &md5))
    }

    #[setter]
    pub fn set_md5(&mut self, md5: Option<Bound<'_, PyBytes>>) -> PyResult<()> {
        self.as_package_record_mut().md5 = md5.map(md5_from_pybytes).transpose()?;
        Ok(())
    }

    /// Package name of the Record.
    #[getter]
    pub fn name(&self) -> PyPackageName {
        self.as_package_record().name.clone().into()
    }

    #[setter]
    pub fn set_name(&mut self, name: PyPackageName) {
        self.as_package_record_mut().name = name.into();
    }

    /// Optionally the platform the package supports.
    #[getter]
    pub fn platform(&self) -> Option<String> {
        self.as_package_record().platform.clone()
    }

    #[setter]
    pub fn set_platform(&mut self, platform: Option<String>) {
        self.as_package_record_mut().platform = platform;
    }

    /// Optionally a SHA256 hash of the package archive.
    #[getter]
    pub fn sha256<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.as_package_record()
            .sha256
            .map(|sha| PyBytes::new_bound(py, &sha))
    }

    /// Optionally a SHA256 hash of the package archive.
    #[setter]
    pub fn set_sha256(&mut self, sha256: Option<Bound<'_, PyBytes>>) -> PyResult<()> {
        self.as_package_record_mut().sha256 = sha256.map(sha256_from_pybytes).transpose()?;
        Ok(())
    }

    /// Optionally the size of the package archive in bytes.
    #[getter]
    pub fn size(&self) -> Option<u64> {
        self.as_package_record().size
    }

    #[setter]
    pub fn set_size(&mut self, size: Option<u64>) {
        self.as_package_record_mut().size = size;
    }

    /// The subdirectory where the package can be found.
    #[getter]
    pub fn subdir(&self) -> String {
        self.as_package_record().subdir.clone()
    }

    #[setter]
    pub fn set_subdir(&mut self, subdir: String) {
        self.as_package_record_mut().subdir = subdir;
    }

    /// The noarch type this package implements, if any.
    #[getter]
    pub fn noarch(&self) -> PyNoArchType {
        self.as_package_record().noarch.into()
    }

    #[setter]
    pub fn set_noarch(&mut self, noarch: PyNoArchType) {
        self.as_package_record_mut().noarch = noarch.into();
    }

    /// The date this entry was created.
    #[getter]
    pub fn timestamp(&self) -> Option<i64> {
        self.as_package_record()
            .timestamp
            .map(|time| time.timestamp_millis())
    }

    #[setter]
    pub fn set_timestamp(&mut self, timestamp: Option<i64>) {
        self.as_package_record_mut().timestamp =
            timestamp.map(|ts| chrono::DateTime::from_timestamp_millis(ts).unwrap());
    }

    /// Track features are nowadays only used to downweight packages
    /// (ie. give them less priority). To that effect, the number of track
    /// features is counted (number of commas) and the package is downweighted
    /// by the number of track_features.
    #[getter]
    pub fn track_features(&self) -> Vec<String> {
        self.as_package_record().track_features.clone()
    }

    #[setter]
    pub fn set_track_features(&mut self, features: Vec<String>) {
        self.as_package_record_mut().track_features = features;
    }

    /// The version of the package.
    #[getter]
    pub fn version(&self) -> (PyVersion, String) {
        let version = &self.as_package_record().version;
        (
            version.version().clone().into(),
            version.as_str().into_owned(),
        )
    }

    #[setter]
    pub fn set_version(&mut self, version: (PyVersion, String)) {
        self.as_package_record_mut().version =
            VersionWithSource::new(version.0.inner.clone(), version.1);
    }

    /// Optionally a path within the environment of the site-packages directory.
    #[getter]
    pub fn python_site_packages_path(&self) -> Option<String> {
        self.as_package_record().python_site_packages_path.clone()
    }

    #[setter]
    pub fn set_python_site_packages_path(&mut self, python_site_packages_path: Option<String>) {
        self.as_package_record_mut().python_site_packages_path = python_site_packages_path;
    }

    /// The filename of the package.
    #[getter]
    pub fn file_name(&self) -> PyResult<String> {
        Ok(self.try_as_repodata_record()?.file_name.clone())
    }

    #[setter]
    pub fn set_file_name(&mut self, file_name: String) -> PyResult<()> {
        self.try_as_repodata_record_mut()?.file_name = file_name;
        Ok(())
    }

    /// The canonical URL from where to get this package.
    #[getter]
    pub fn url(&self) -> PyResult<String> {
        Ok(self.try_as_repodata_record()?.url.to_string())
    }

    #[setter]
    pub fn set_url(&mut self, url: String) -> PyResult<()> {
        self.try_as_repodata_record_mut()?.url = url.parse().unwrap();
        Ok(())
    }

    /// String representation of the channel where the
    /// package comes from. This could be a URL but it
    /// could also be a channel name.
    #[getter]
    pub fn channel(&self) -> PyResult<String> {
        Ok(self.try_as_repodata_record()?.channel.clone())
    }

    #[setter]
    pub fn set_channel(&mut self, channel: String) -> PyResult<()> {
        self.try_as_repodata_record_mut()?.channel = channel;
        Ok(())
    }

    /// The path to where the archive of the package was stored on disk.
    #[getter]
    pub fn package_tarball_full_path(&self) -> PyResult<Option<PathBuf>> {
        Ok(self
            .try_as_prefix_record()?
            .package_tarball_full_path
            .clone())
    }

    #[setter]
    pub fn set_package_tarball_full_path(&mut self, path: Option<PathBuf>) -> PyResult<()> {
        self.try_as_prefix_record_mut()?.package_tarball_full_path = path;
        Ok(())
    }

    /// The path that contains the extracted package content.
    #[getter]
    pub fn extracted_package_dir(&self) -> PyResult<Option<PathBuf>> {
        Ok(self.try_as_prefix_record()?.extracted_package_dir.clone())
    }

    #[setter]
    pub fn set_extracted_package_dir(&mut self, dir: Option<PathBuf>) -> PyResult<()> {
        self.try_as_prefix_record_mut()?.extracted_package_dir = dir;
        Ok(())
    }

    /// A sorted list of all files included in this package
    #[getter]
    pub fn files(&self) -> PyResult<Vec<PathBuf>> {
        Ok(self.try_as_prefix_record()?.files.clone())
    }

    #[setter]
    pub fn set_files(&mut self, files: Vec<PathBuf>) -> PyResult<()> {
        self.try_as_prefix_record_mut()?.files = files;
        Ok(())
    }

    /// Information about how files have been linked when installing the
    /// package.
    #[getter]
    pub fn paths_data(&self) -> PyResult<PyPrefixPaths> {
        Ok(self.try_as_prefix_record()?.paths_data.clone().into())
    }

    #[setter]
    pub fn set_paths_data(&mut self, paths: PyPrefixPaths) -> PyResult<()> {
        self.try_as_prefix_record_mut()?.paths_data = paths.into();
        Ok(())
    }

    /// The spec that was used when this package was installed. Note that this
    /// field is not updated if the currently another spec was used.
    #[getter]
    pub fn requested_spec(&self) -> PyResult<Option<String>> {
        Ok(self.try_as_prefix_record()?.requested_spec.clone())
    }

    #[setter]
    pub fn set_requested_spec(&mut self, spec: Option<String>) -> PyResult<()> {
        self.try_as_prefix_record_mut()?.requested_spec = spec;
        Ok(())
    }

    pub fn to_json(&self) -> String {
        match &self.inner {
            RecordInner::Prefix(r) => serde_json::to_string_pretty(&r),
            RecordInner::RepoData(r) => serde_json::to_string_pretty(&r),
            RecordInner::Package(r) => serde_json::to_string_pretty(&r),
        }
        .unwrap()
    }
}

impl From<PrefixRecord> for PyRecord {
    fn from(value: PrefixRecord) -> Self {
        Self {
            inner: RecordInner::Prefix(value),
        }
    }
}

impl TryFrom<PyRecord> for PrefixRecord {
    type Error = PyErr;
    fn try_from(value: PyRecord) -> Result<Self, Self::Error> {
        match value.inner {
            RecordInner::Prefix(r) => Ok(r),
            RecordInner::RepoData(_) => Err(PyTypeError::new_err(
                "cannot use object of type 'RepoDataRecord' as 'PrefixRecord'",
            )),
            RecordInner::Package(_) => Err(PyTypeError::new_err(
                "cannot use object of type 'PackageRecord' as 'PrefixRecord'",
            )),
        }
    }
}

impl<'a> TryFrom<Bound<'a, PyAny>> for PyRecord {
    type Error = PyErr;
    fn try_from(value: Bound<'a, PyAny>) -> Result<Self, Self::Error> {
        let intern_val = intern!(value.py(), "_record");
        if !value.hasattr(intern_val)? {
            return Err(PyTypeError::new_err("object is not a record type"));
        }

        let inner = value.getattr(intern_val)?;
        if !inner.is_instance_of::<Self>() {
            return Err(PyTypeError::new_err("'_record' is invalid"));
        }

        PyRecord::extract_bound(&inner)
    }
}

impl From<RepoDataRecord> for PyRecord {
    fn from(value: RepoDataRecord) -> Self {
        Self {
            inner: RecordInner::RepoData(value),
        }
    }
}

impl TryFrom<PyRecord> for RepoDataRecord {
    type Error = PyErr;
    fn try_from(value: PyRecord) -> Result<Self, Self::Error> {
        match value.inner {
            RecordInner::Prefix(r) => Ok(r.repodata_record),
            RecordInner::RepoData(r) => Ok(r),
            RecordInner::Package(_) => Err(PyTypeError::new_err(
                "cannot use object of type 'PackageRecord' as 'RepoDataRecord'",
            )),
        }
    }
}

impl From<PackageRecord> for PyRecord {
    fn from(value: PackageRecord) -> Self {
        Self {
            inner: RecordInner::Package(value),
        }
    }
}

impl From<PyRecord> for PackageRecord {
    fn from(value: PyRecord) -> Self {
        value.as_ref().clone()
    }
}

impl AsRef<PackageRecord> for PyRecord {
    fn as_ref(&self) -> &PackageRecord {
        match &self.inner {
            RecordInner::Prefix(r) => &r.repodata_record.package_record,
            RecordInner::RepoData(r) => &r.package_record,
            RecordInner::Package(r) => r,
        }
    }
}

impl AsMut<PackageRecord> for PyRecord {
    fn as_mut(&mut self) -> &mut PackageRecord {
        match &mut self.inner {
            RecordInner::Prefix(r) => &mut r.repodata_record.package_record,
            RecordInner::RepoData(r) => &mut r.package_record,
            RecordInner::Package(r) => r,
        }
    }
}

#[pymethods]
impl PyRecord {
    /// Parses a PrefixRecord from a file.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(PrefixRecord::from_path(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Writes the contents of this instance to the file at the specified
    /// location.
    pub fn write_to_path(&self, path: PathBuf, pretty: bool) -> PyResult<()> {
        Ok(self
            .try_as_prefix_record()?
            .clone()
            .write_to_path(path, pretty)
            .map_err(PyRattlerError::from)?)
    }
}

#[pymethods]
impl PyRecord {
    /// Builds a `PyRecord` from path to an `index.json` and optionally a size.
    #[staticmethod]
    #[pyo3(signature = (index_json, size=None, sha256=None, md5=None))]
    fn from_index_json(
        index_json: PathBuf,
        size: Option<u64>,
        sha256: Option<String>,
        md5: Option<String>,
    ) -> PyResult<Self> {
        let index = IndexJson::from_path(index_json)?;
        let sha256 = if let Some(hex) = sha256 {
            parse_digest_from_hex::<Sha256>(&hex)
        } else {
            None
        };

        let md5 = if let Some(hex) = md5 {
            parse_digest_from_hex::<Md5>(&hex)
        } else {
            None
        };

        Ok(PackageRecord::from_index_json(index, size, sha256, md5)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Validate that the given package records are valid w.r.t. 'depends' and
    /// 'constrains'. This function will return nothing if all records form
    /// a valid environment, i.e., all dependencies of each package are
    /// satisfied by the other packages in the list. If there is a
    /// dependency that is not satisfied, this function will raise an exception.
    #[staticmethod]
    fn validate(records: Vec<Bound<'_, PyAny>>) -> PyResult<()> {
        let records = records
            .into_iter()
            .map(PyRecord::try_from)
            .collect::<PyResult<Vec<_>>>()?;
        Ok(PackageRecord::validate(records).map_err(PyRattlerError::from)?)
    }

    /// Sorts the records topologically.
    ///
    /// This function is deterministic, meaning that it will return the same
    /// result regardless of the order of records and of the depends vector
    /// inside the records.
    ///
    /// Note that this function only works for packages with unique names.
    #[staticmethod]
    fn sort_topologically(records: Vec<Bound<'_, PyAny>>) -> PyResult<Vec<Self>> {
        let records = records
            .into_iter()
            .map(PyRecord::try_from)
            .collect::<PyResult<Vec<_>>>()?;
        Ok(PackageRecord::sort_topologically(records))
    }
}
