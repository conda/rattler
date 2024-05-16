use std::{path::PathBuf, str::FromStr};

use chrono::{TimeZone, Utc};
use pyo3::{
    exceptions::PyTypeError, intern, pyclass, pymethods, FromPyObject, PyAny, PyErr, PyResult,
};
use rattler_conda_types::{
    package::{IndexJson, PackageFile},
    PackageRecord, PrefixRecord, RepoDataRecord,
};

use rattler_digest::{parse_digest_from_hex, Md5, Sha256};
use url::Url;

use crate::{
    error::PyRattlerError, no_arch_type::PyNoArchType, package_name::PyPackageName,
    platform::PyPlatform, prefix_paths::PyPrefixPaths, version::PyVersion,
};

/// Python bindings for `PrefixRecord`, `RepoDataRecord`, `PackageRecord`.
/// This is to expose these structs in Object Oriented manner, via a single
/// class. This class handles the conversion on its own.
/// It uses a `RecordInner` enum and (try_)as_{x}_record methods for this interface.
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

    pub fn as_mut_package_record(&mut self) -> &mut PackageRecord {
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

    pub fn try_as_mut_repodata_record(&mut self) -> PyResult<&mut RepoDataRecord> {
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

    pub fn try_as_mut_prefix_record(&mut self) -> PyResult<&mut PrefixRecord> {
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

#[pymethods]
impl PyRecord {
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
    #[getter(arch)]
    pub fn arch(&self) -> Option<String> {
        self.as_package_record().arch.clone()
    }

    /// Setter for arch.
    #[setter(arch)]
    pub fn set_arch(&mut self, arch: Option<String>) -> PyResult<()> {
        self.as_mut_package_record().arch = arch;
        Ok(())
    }

    /// The build string of the package.
    #[getter(build)]
    pub fn build(&self) -> String {
        self.as_package_record().build.clone()
    }

    /// Setter for build.
    #[setter(build)]
    pub fn set_build(&mut self, build: String) -> PyResult<()> {
        self.as_mut_package_record().build = build;
        Ok(())
    }

    /// The build number of the package.
    #[getter(build_number)]
    pub fn build_number(&self) -> u64 {
        self.as_package_record().build_number
    }

    /// Setter for build number.
    #[setter(build_number)]
    pub fn set_build_number(&mut self, build_number: u64) -> PyResult<()> {
        self.as_mut_package_record().build_number = build_number;
        Ok(())
    }

    /// Additional constraints on packages.
    /// `constrains` are different from `depends` in that packages
    /// specified in depends must be installed next to this package,
    /// whereas packages specified in `constrains` are not required
    /// to be installed, but if they are installed they must follow
    /// these constraints.
    #[getter(constrains)]
    pub fn constrains(&self) -> Vec<String> {
        self.as_package_record().constrains.clone()
    }

    /// Setter for constrains.
    #[setter(constrains)]
    pub fn set_constrains(&mut self, constrains: Vec<String>) -> PyResult<()> {
        self.as_mut_package_record().constrains = constrains;
        Ok(())
    }

    /// Specification of packages this package depends on.
    #[getter(depends)]
    pub fn depends(&self) -> Vec<String> {
        self.as_package_record().depends.clone()
    }

    /// Setter for depends.
    #[setter(depends)]
    pub fn set_depends(&mut self, depends: Vec<String>) -> PyResult<()> {
        self.as_mut_package_record().depends = depends;
        Ok(())
    }

    /// Features are a deprecated way to specify different
    /// feature sets for the conda solver. This is not
    /// supported anymore and should not be used. Instead,
    /// `mutex` packages should be used to specify
    /// mutually exclusive features.
    #[getter(features)]
    pub fn features(&self) -> Option<String> {
        self.as_package_record().features.clone()
    }

    /// Setter for features.
    #[setter(features)]
    pub fn set_features(&mut self, features: Option<String>) -> PyResult<()> {
        self.as_mut_package_record().features = features;
        Ok(())
    }

    /// A deprecated md5 hash.
    #[getter(legacy_bz2_md5)]
    pub fn legacy_bz2_md5(&self) -> Option<String> {
        self.as_package_record().legacy_bz2_md5.clone()
    }

    /// A deprecated package archive size.
    #[getter(legacy_bz2_size)]
    pub fn legacy_bz2_size(&self) -> Option<u64> {
        self.as_package_record().legacy_bz2_size
    }

    /// Setter for package archive size.
    #[setter(legacy_bz2_size)]
    pub fn set_legacy_bz2_size(&mut self, size: Option<u64>) -> PyResult<()> {
        self.as_mut_package_record().legacy_bz2_size = size;
        Ok(())
    }

    /// The specific license of the package.
    #[getter(license)]
    pub fn license(&self) -> Option<String> {
        self.as_package_record().license.clone()
    }

    /// Setter for license.
    #[setter(license)]
    pub fn set_license(&mut self, license: Option<String>) -> PyResult<()> {
        self.as_mut_package_record().license = license;
        Ok(())
    }

    /// The license family.
    #[getter(license_family)]
    pub fn license_family(&self) -> Option<String> {
        self.as_package_record().license_family.clone()
    }

    /// Setter for license family.
    #[setter(license_family)]
    pub fn set_license_family(&mut self, family: Option<String>) -> PyResult<()> {
        self.as_mut_package_record().license_family = family;
        Ok(())
    }

    /// Optionally a MD5 hash of the package archive.
    #[getter]
    pub fn md5(&self) -> Option<String> {
        self.as_package_record().md5.map(|md5| format!("{md5:X}"))
    }

    /// Package name of the Record.
    #[getter(name)]
    pub fn name(&self) -> PyPackageName {
        self.as_package_record().name.clone().into()
    }

    /// Setter for name.
    #[setter(name)]
    pub fn set_name(&mut self, name: PyPackageName) -> PyResult<()> {
        self.as_mut_package_record().name = name.inner;
        Ok(())
    }

    /// Optionally the platform the package supports.
    #[getter(platform)]
    pub fn platform(&self) -> Option<String> {
        self.as_package_record().platform.clone()
    }

    /// Setter for platform.
    #[setter(platform)]
    pub fn set_platform(&mut self, platform: PyPlatform) -> PyResult<()> {
        self.as_mut_package_record().platform = Some(platform.inner.to_string());
        Ok(())
    }

    /// Optionally a SHA256 hash of the package archive.
    #[getter]
    pub fn sha256(&self) -> Option<String> {
        self.as_package_record()
            .sha256
            .map(|sha| format!("{sha:X}"))
    }

    /// Optionally the size of the package archive in bytes.
    #[getter]
    pub fn size(&self) -> Option<u64> {
        self.as_package_record().size
    }

    /// Setter for size.
    #[setter(size)]
    pub fn set_size(&mut self, size: Option<u64>) -> PyResult<()> {
        self.as_mut_package_record().size = size;
        Ok(())
    }

    /// The subdirectory where the package can be found.
    #[getter(subdir)]
    pub fn subdir(&self) -> String {
        self.as_package_record().subdir.clone()
    }

    /// Setter for subdir.
    #[setter(subdir)]
    pub fn set_subdir(&mut self, subdir: String) -> PyResult<()> {
        self.as_mut_package_record().subdir = subdir;
        Ok(())
    }

    /// The noarch type this package implements, if any.
    #[getter(noarch)]
    pub fn noarch(&self) -> PyNoArchType {
        self.as_package_record().noarch.into()
    }

    /// Setter for noarch.
    #[setter(noarch)]
    pub fn set_noarch(&mut self, noarch: PyNoArchType) -> PyResult<()> {
        self.as_mut_package_record().noarch = noarch.into();
        Ok(())
    }

    /// The date this entry was created.
    #[getter(timestamp)]
    pub fn timestamp(&self) -> Option<i64> {
        self.as_package_record()
            .timestamp
            .map(|time| time.timestamp())
    }

    /// Setter for entry date.
    #[setter(timestamp)]
    pub fn set_timestamp(&mut self, timestamp: u64) -> PyResult<()> {
        // TODO: remove unwrap
        let time = Utc.timestamp_opt(timestamp as i64, 0).unwrap();
        self.as_mut_package_record().timestamp = Some(time);
        Ok(())
    }

    /// Track features are nowadays only used to downweight packages
    /// (ie. give them less priority). To that effect, the number of track
    /// features is counted (number of commas) and the package is downweighted
    /// by the number of track_features.
    #[getter(track_features)]
    pub fn track_features(&self) -> Vec<String> {
        self.as_package_record().track_features.clone()
    }

    /// Setter for track features.
    #[setter(track_features)]
    pub fn set_track_features(&mut self, features: Vec<String>) -> PyResult<()> {
        self.as_mut_package_record().track_features = features;
        Ok(())
    }

    /// The version of the package.
    #[getter(version)]
    pub fn version(&self) -> PyVersion {
        self.as_package_record()
            .version
            .clone()
            .into_version()
            .into()
    }

    /// Setter for version.
    #[setter(version)]
    pub fn set_version(&mut self, version: PyVersion) -> PyResult<()> {
        self.as_mut_package_record().version = version.inner.into();
        Ok(())
    }

    /// The filename of the package.
    #[getter(file_name)]
    pub fn file_name(&self) -> PyResult<String> {
        Ok(self.try_as_repodata_record()?.file_name.clone())
    }

    /// Setter for filename of the package.
    #[setter(file_name)]
    pub fn set_file_name(&mut self, file_name: String) -> PyResult<()> {
        self.try_as_mut_repodata_record()?.file_name = file_name;
        Ok(())
    }

    /// The canonical URL from where to get this package.
    #[getter(url)]
    pub fn url(&self) -> PyResult<String> {
        Ok(self.try_as_repodata_record()?.url.to_string())
    }

    /// Setter for package URL.
    #[setter(url)]
    pub fn set_url(&mut self, url: &str) -> PyResult<()> {
        self.try_as_mut_repodata_record()?.url =
            Url::from_str(url).map_err(PyRattlerError::from)?;
        Ok(())
    }

    /// String representation of the channel where the
    /// package comes from. This could be a URL but it
    /// could also be a channel name.
    #[getter(channel)]
    pub fn channel(&self) -> PyResult<String> {
        Ok(self.try_as_repodata_record()?.channel.clone())
    }

    /// Setter for package URL.
    #[setter(channel)]
    pub fn set_channel(&mut self, channel: String) -> PyResult<()> {
        self.try_as_mut_repodata_record()?.channel = channel;
        Ok(())
    }

    /// The path to where the archive of the package was stored on disk.
    #[getter(package_tarball_full_path)]
    pub fn package_tarball_full_path(&self) -> PyResult<Option<PathBuf>> {
        Ok(self
            .try_as_prefix_record()?
            .package_tarball_full_path
            .clone())
    }

    /// Setter for local package path.
    #[setter(package_tarball_full_path)]
    pub fn set_package_tarball_full_path(&mut self, path: Option<PathBuf>) -> PyResult<()> {
        self.try_as_mut_prefix_record()?.package_tarball_full_path = path;
        Ok(())
    }

    /// The path that contains the extracted package content.
    #[getter(extracted_package_dir)]
    pub fn extracted_package_dir(&self) -> PyResult<Option<PathBuf>> {
        Ok(self.try_as_prefix_record()?.extracted_package_dir.clone())
    }

    /// Setter for path to extracted package content.
    #[setter(extracted_package_dir)]
    pub fn set_extracted_package_dir(&mut self, path: Option<PathBuf>) -> PyResult<()> {
        self.try_as_mut_prefix_record()?.extracted_package_dir = path;
        Ok(())
    }

    /// A sorted list of all files included in this package
    #[getter(files)]
    pub fn files(&self) -> PyResult<Vec<PathBuf>> {
        Ok(self.try_as_prefix_record()?.files.clone())
    }

    /// Setter for files included in the package.
    #[setter(files)]
    pub fn set_file(&mut self, files: Vec<PathBuf>) -> PyResult<()> {
        self.try_as_mut_prefix_record()?.files = files;
        Ok(())
    }

    /// Information about how files have been linked when installing the package.
    #[getter(paths_data)]
    pub fn paths_data(&self) -> PyResult<PyPrefixPaths> {
        Ok(self.try_as_prefix_record()?.paths_data.clone().into())
    }

    /// Setter for paths data.
    #[setter(paths_data)]
    pub fn set_paths_data(&mut self, paths_data: PyPrefixPaths) -> PyResult<()> {
        self.try_as_mut_prefix_record()?.paths_data = paths_data.into();
        Ok(())
    }

    /// The spec that was used when this package was installed. Note that this field is not updated if the
    /// currently another spec was used.
    #[getter(requested_spec)]
    pub fn requested_spec(&self) -> PyResult<Option<String>> {
        Ok(self.try_as_prefix_record()?.requested_spec.clone())
    }

    /// Setter for requested spec.
    #[setter(requested_spec)]
    pub fn set_requested_spec(&mut self, spec: Option<String>) -> PyResult<()> {
        self.try_as_mut_prefix_record()?.requested_spec = spec;
        Ok(())
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
                "connot use object of type 'RepoDataRecord' as 'PrefixRecord'",
            )),
            RecordInner::Package(_) => Err(PyTypeError::new_err(
                "connot use object of type 'PackageRecord' as 'PrefixRecord'",
            )),
        }
    }
}

impl<'a> TryFrom<&'a PyAny> for PyRecord {
    type Error = PyErr;
    fn try_from(value: &'a PyAny) -> Result<Self, Self::Error> {
        let intern_val = intern!(value.py(), "_record");
        if !value.hasattr(intern_val)? {
            return Err(PyTypeError::new_err("object is not a record type"));
        }

        let inner = value.getattr(intern_val)?;
        if !inner.is_instance_of::<Self>() {
            return Err(PyTypeError::new_err("'_record' is invalid"));
        }

        PyRecord::extract(inner)
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
                "connot use object of type 'PackageRecord' as 'RepoDataRecord'",
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

    /// Writes the contents of this instance to the file at the specified location.
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

    /// Sorts the records topologically.
    ///
    /// This function is deterministic, meaning that it will return the same result
    /// regardless of the order of records and of the depends vector inside the records.
    ///
    /// Note that this function only works for packages with unique names.
    #[staticmethod]
    fn sort_topologically(records: Vec<&PyAny>) -> PyResult<Vec<Self>> {
        let records = records
            .into_iter()
            .map(PyRecord::try_from)
            .collect::<PyResult<Vec<_>>>()?;
        Ok(PackageRecord::sort_topologically(records))
    }
}
