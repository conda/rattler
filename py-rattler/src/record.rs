use std::path::PathBuf;

use pyo3::{
    exceptions::PyTypeError, intern, pyclass, pymethods, FromPyObject, PyAny, PyErr, PyResult,
};
use rattler_conda_types::{
    package::{IndexJson, PackageFile},
    PackageRecord, PrefixRecord, RepoDataRecord,
};

use crate::{
    error::PyRattlerError, package_name::PyPackageName, prefix_record::PyPrefixPaths,
    version::PyVersion,
};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyRecord {
    pub inner: RecordInner,
}

#[derive(Clone)]
pub enum RecordInner {
    PrefixRecord(PrefixRecord),
    RepoDataRecord(RepoDataRecord),
    PackageRecord(PackageRecord),
}

impl PyRecord {
    pub fn as_package_record(&self) -> PyResult<&PackageRecord> {
        match &self.inner {
            RecordInner::PrefixRecord(r) => Ok(&r.repodata_record.package_record),

            RecordInner::RepoDataRecord(r) => Ok(&r.package_record),

            RecordInner::PackageRecord(r) => Ok(r),
        }
    }

    pub fn as_repodata_record(&self) -> PyResult<&RepoDataRecord> {
        match &self.inner {
            RecordInner::PrefixRecord(r) => Ok(&r.repodata_record),
            RecordInner::RepoDataRecord(r) => Ok(r),
            RecordInner::PackageRecord(_) => Err(PyTypeError::new_err(
                "Cannot use object of type 'PackageRecord' as 'RepoDataRecord'",
            )),
        }
    }

    pub fn as_prefix_record(&self) -> PyResult<&PrefixRecord> {
        match &self.inner {
            RecordInner::PrefixRecord(r) => Ok(r),
            RecordInner::RepoDataRecord(_) => Err(PyTypeError::new_err(
                "Cannot use object of type 'RepoDataRecord' as 'PrefixRecord'",
            )),
            RecordInner::PackageRecord(_) => Err(PyTypeError::new_err(
                "Cannot use object of type 'PackageRecord' as 'PrefixRecord'",
            )),
        }
    }
}

#[pymethods]
impl PyRecord {
    /// Returns a string representation of PackageRecord
    pub fn as_str(&self) -> PyResult<String> {
        Ok(format!("{}", self.as_package_record()?))
    }

    /// Optionally the architecture the package supports.
    #[getter]
    pub fn arch(&self) -> PyResult<Option<String>> {
        Ok(self.as_package_record()?.arch.clone())
    }

    /// The build string of the package.
    #[getter]
    pub fn build(&self) -> PyResult<String> {
        Ok(self.as_package_record()?.build.clone())
    }

    /// The build number of the package.
    #[getter]
    pub fn build_number(&self) -> PyResult<u64> {
        Ok(self.as_package_record()?.build_number.into())
    }

    /// Additional constraints on packages.
    /// `constrains` are different from `depends` in that packages
    /// specified in depends must be installed next to this package,
    /// whereas packages specified in `constrains` are not required
    /// to be installed, but if they are installed they must follow
    /// these constraints.
    #[getter]
    pub fn constrains(&self) -> PyResult<Vec<String>> {
        Ok(self.as_package_record()?.constrains.clone())
    }

    /// Specification of packages this package depends on.
    #[getter]
    pub fn depends(&self) -> PyResult<Vec<String>> {
        Ok(self.as_package_record()?.depends.clone())
    }

    /// Features are a deprecated way to specify different
    /// feature sets for the conda solver. This is not
    /// supported anymore and should not be used. Instead,
    /// `mutex` packages should be used to specify
    /// mutually exclusive features.
    #[getter]
    pub fn features(&self) -> PyResult<Option<String>> {
        Ok(self.as_package_record()?.features.clone())
    }

    /// A deprecated md5 hash.
    #[getter]
    pub fn legacy_bz2_md5(&self) -> PyResult<Option<String>> {
        Ok(self.as_package_record()?.legacy_bz2_md5.clone())
    }

    /// A deprecated package archive size.
    #[getter]
    pub fn legacy_bz2_size(&self) -> PyResult<Option<u64>> {
        Ok(self.as_package_record()?.legacy_bz2_size)
    }

    /// The specific license of the package.
    #[getter]
    pub fn license(&self) -> PyResult<Option<String>> {
        Ok(self.as_package_record()?.license.clone())
    }

    /// The license family.
    #[getter]
    pub fn license_family(&self) -> PyResult<Option<String>> {
        Ok(self.as_package_record()?.license_family.clone())
    }

    /// Optionally a MD5 hash of the package archive.
    #[getter]
    pub fn md5(&self) -> PyResult<Option<String>> {
        if let Some(md5) = self.as_package_record()?.md5 {
            Ok(Some(format!("{md5:X}")))
        } else {
            Ok(None)
        }
    }

    /// Package name of the Record.
    #[getter]
    pub fn name(&self) -> PyResult<PyPackageName> {
        Ok(self.as_package_record()?.name.clone().into())
    }

    /// Optionally the platform the package supports.
    #[getter]
    pub fn platform(&self) -> PyResult<Option<String>> {
        Ok(self.as_package_record()?.platform.clone())
    }

    /// Optionally a SHA256 hash of the package archive.
    #[getter]
    pub fn sha256(&self) -> PyResult<Option<String>> {
        if let Some(sha) = self.as_package_record()?.sha256 {
            Ok(Some(format!("{sha:X}")))
        } else {
            Ok(None)
        }
    }

    /// Optionally the size of the package archive in bytes.
    #[getter]
    pub fn size(&self) -> PyResult<Option<u64>> {
        Ok(self.as_package_record()?.size)
    }

    /// The subdirectory where the package can be found.
    #[getter]
    pub fn subdir(&self) -> PyResult<String> {
        Ok(self.as_package_record()?.subdir.clone())
    }

    /// The date this entry was created.
    #[getter]
    pub fn timestamp(&self) -> PyResult<Option<i64>> {
        if let Some(time) = self.as_package_record()?.timestamp {
            Ok(Some(time.timestamp()))
        } else {
            Ok(None)
        }
    }

    /// Track features are nowadays only used to downweight packages
    /// (ie. give them less priority). To that effect, the number of track
    /// features is counted (number of commas) and the package is downweighted
    /// by the number of track_features.
    #[getter]
    pub fn track_features(&self) -> PyResult<Vec<String>> {
        Ok(self.as_package_record()?.track_features.clone())
    }

    /// The version of the package.
    #[getter]
    pub fn version(&self) -> PyResult<PyVersion> {
        Ok(self
            .as_package_record()?
            .version
            .clone()
            .into_version()
            .into())
    }

    /// The filename of the package.
    #[getter]
    pub fn file_name(&self) -> PyResult<String> {
        Ok(self.as_repodata_record()?.file_name.clone())
    }

    /// The canonical URL from where to get this package.
    #[getter]
    pub fn url(&self) -> PyResult<String> {
        Ok(self.as_repodata_record()?.url.to_string())
    }

    /// String representation of the channel where the
    /// package comes from. This could be a URL but it
    /// could also be a channel name.
    #[getter]
    pub fn channel(&self) -> PyResult<String> {
        Ok(self.as_repodata_record()?.channel.clone())
    }

    /// The path to where the archive of the package was stored on disk.
    #[getter]
    pub fn package_tarball_full_path(&self) -> PyResult<Option<PathBuf>> {
        Ok(self.as_prefix_record()?.package_tarball_full_path.clone())
    }

    /// The path that contains the extracted package content.
    #[getter]
    pub fn extracted_package_dir(&self) -> PyResult<Option<PathBuf>> {
        Ok(self.as_prefix_record()?.extracted_package_dir.clone())
    }

    /// A sorted list of all files included in this package
    #[getter]
    pub fn files(&self) -> PyResult<Vec<PathBuf>> {
        Ok(self.as_prefix_record()?.files.clone())
    }

    /// Information about how files have been linked when installing the package.
    #[getter]
    pub fn paths_data(&self) -> PyResult<PyPrefixPaths> {
        Ok(self.as_prefix_record()?.paths_data.clone().into())
    }

    /// The spec that was used when this package was installed. Note that this field is not updated if the
    /// currently another spec was used.
    #[getter]
    pub fn requested_spec(&self) -> PyResult<Option<String>> {
        Ok(self.as_prefix_record()?.requested_spec.clone())
    }
}

impl From<PrefixRecord> for PyRecord {
    fn from(value: PrefixRecord) -> Self {
        Self {
            inner: RecordInner::PrefixRecord(value),
        }
    }
}

impl From<PyRecord> for PrefixRecord {
    fn from(value: PyRecord) -> Self {
        match value.inner {
            RecordInner::PrefixRecord(r) => r,
            _ => panic!("invalid conversion tried!"),
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
            inner: RecordInner::RepoDataRecord(value),
        }
    }
}

impl From<PyRecord> for RepoDataRecord {
    fn from(value: PyRecord) -> Self {
        match value.inner {
            RecordInner::PrefixRecord(r) => r.repodata_record,
            RecordInner::RepoDataRecord(r) => r,
            _ => panic!("invalid conversion tried!"),
        }
    }
}

impl From<PackageRecord> for PyRecord {
    fn from(value: PackageRecord) -> Self {
        Self {
            inner: RecordInner::PackageRecord(value),
        }
    }
}

impl From<PyRecord> for PackageRecord {
    fn from(value: PyRecord) -> Self {
        value.as_ref().to_owned()
    }
}

impl AsRef<PackageRecord> for PyRecord {
    fn as_ref(&self) -> &PackageRecord {
        match &self.inner {
            RecordInner::PrefixRecord(r) => &r.repodata_record.package_record,
            RecordInner::RepoDataRecord(r) => &r.package_record,
            RecordInner::PackageRecord(r) => r,
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
            .as_prefix_record()?
            .to_owned()
            .write_to_path(path, pretty)
            .map_err(PyRattlerError::from)?)
    }
}

#[pymethods]
impl PyRecord {
    /// Builds a `PyRecord` from path to an `index.json` and optionally a size.
    #[staticmethod]
    fn from_index_json(index_json: PathBuf, size: Option<u64>) -> PyResult<Self> {
        let index = IndexJson::from_path(index_json)?;
        Ok(PackageRecord::from_index_json(index, size, None, None)
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
    fn sort_topologically(records: Vec<Self>) -> Vec<Self> {
        PackageRecord::sort_topologically(records)
    }
}
