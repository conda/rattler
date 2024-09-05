use std::path::PathBuf;

use pyo3::{
    exceptions::PyTypeError, intern, pyclass, pymethods, types::PyBytes, FromPyObject, PyAny,
    PyErr, PyResult, Python,
};
use rattler_conda_types::{
    package::{IndexJson, PackageFile},
    PackageRecord, PrefixRecord, RepoDataRecord,
};

use rattler_digest::{parse_digest_from_hex, Md5, Sha256};

use crate::{
    error::PyRattlerError, no_arch_type::PyNoArchType, package_name::PyPackageName,
    prefix_paths::PyPrefixPaths, version::PyVersion,
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

    pub fn try_as_repodata_record(&self) -> PyResult<&RepoDataRecord> {
        match &self.inner {
            RecordInner::Prefix(r) => Ok(&r.repodata_record),
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
    #[getter]
    pub fn arch(&self) -> Option<String> {
        self.as_package_record().arch.clone()
    }

    /// The build string of the package.
    #[getter]
    pub fn build(&self) -> String {
        self.as_package_record().build.clone()
    }

    /// The build number of the package.
    #[getter]
    pub fn build_number(&self) -> u64 {
        self.as_package_record().build_number
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

    /// Specification of packages this package depends on.
    #[getter]
    pub fn depends(&self) -> Vec<String> {
        self.as_package_record().depends.clone()
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

    /// A deprecated md5 hash.
    #[getter]
    pub fn legacy_bz2_md5<'a>(&self, py: Python<'a>) -> Option<&'a PyBytes> {
        self.as_package_record()
            .legacy_bz2_md5
            .map(|md5| PyBytes::new(py, &md5))
    }

    /// A deprecated package archive size.
    #[getter]
    pub fn legacy_bz2_size(&self) -> Option<u64> {
        self.as_package_record().legacy_bz2_size
    }

    /// The specific license of the package.
    #[getter]
    pub fn license(&self) -> Option<String> {
        self.as_package_record().license.clone()
    }

    /// The license family.
    #[getter]
    pub fn license_family(&self) -> Option<String> {
        self.as_package_record().license_family.clone()
    }

    /// Optionally a MD5 hash of the package archive.
    #[getter]
    pub fn md5<'a>(&self, py: Python<'a>) -> Option<&'a PyBytes> {
        self.as_package_record()
            .md5
            .map(|md5| PyBytes::new(py, &md5))
    }

    /// Package name of the Record.
    #[getter]
    pub fn name(&self) -> PyPackageName {
        self.as_package_record().name.clone().into()
    }

    /// Optionally the platform the package supports.
    #[getter]
    pub fn platform(&self) -> Option<String> {
        self.as_package_record().platform.clone()
    }

    /// Optionally a SHA256 hash of the package archive.
    #[getter]
    pub fn sha256<'a>(&self, py: Python<'a>) -> Option<&'a PyBytes> {
        self.as_package_record()
            .sha256
            .map(|sha| PyBytes::new(py, &sha))
    }

    /// Optionally the size of the package archive in bytes.
    #[getter]
    pub fn size(&self) -> Option<u64> {
        self.as_package_record().size
    }

    /// The subdirectory where the package can be found.
    #[getter]
    pub fn subdir(&self) -> String {
        self.as_package_record().subdir.clone()
    }

    /// The noarch type this package implements, if any.
    #[getter]
    pub fn noarch(&self) -> PyNoArchType {
        self.as_package_record().noarch.into()
    }

    /// The date this entry was created.
    #[getter]
    pub fn timestamp(&self) -> Option<i64> {
        self.as_package_record()
            .timestamp
            .map(|time| time.timestamp_millis())
    }

    /// Track features are nowadays only used to downweight packages
    /// (ie. give them less priority). To that effect, the number of track
    /// features is counted (number of commas) and the package is downweighted
    /// by the number of track_features.
    #[getter]
    pub fn track_features(&self) -> Vec<String> {
        self.as_package_record().track_features.clone()
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

    /// The filename of the package.
    #[getter]
    pub fn file_name(&self) -> PyResult<String> {
        Ok(self.try_as_repodata_record()?.file_name.clone())
    }

    /// The canonical URL from where to get this package.
    #[getter]
    pub fn url(&self) -> PyResult<String> {
        Ok(self.try_as_repodata_record()?.url.to_string())
    }

    /// String representation of the channel where the
    /// package comes from. This could be a URL but it
    /// could also be a channel name.
    #[getter]
    pub fn channel(&self) -> PyResult<String> {
        Ok(self.try_as_repodata_record()?.channel.clone())
    }

    /// The path to where the archive of the package was stored on disk.
    #[getter]
    pub fn package_tarball_full_path(&self) -> PyResult<Option<PathBuf>> {
        Ok(self
            .try_as_prefix_record()?
            .package_tarball_full_path
            .clone())
    }

    /// The path that contains the extracted package content.
    #[getter]
    pub fn extracted_package_dir(&self) -> PyResult<Option<PathBuf>> {
        Ok(self.try_as_prefix_record()?.extracted_package_dir.clone())
    }

    /// A sorted list of all files included in this package
    #[getter]
    pub fn files(&self) -> PyResult<Vec<PathBuf>> {
        Ok(self.try_as_prefix_record()?.files.clone())
    }

    /// Information about how files have been linked when installing the package.
    #[getter]
    pub fn paths_data(&self) -> PyResult<PyPrefixPaths> {
        Ok(self.try_as_prefix_record()?.paths_data.clone().into())
    }

    /// The spec that was used when this package was installed. Note that this field is not updated if the
    /// currently another spec was used.
    #[getter]
    pub fn requested_spec(&self) -> PyResult<Option<String>> {
        Ok(self.try_as_prefix_record()?.requested_spec.clone())
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
