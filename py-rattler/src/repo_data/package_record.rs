use std::path::PathBuf;

use rattler_conda_types::{
    package::{IndexJson, PackageFile},
    PackageRecord,
};

use pyo3::{pyclass, pymethods, PyResult};

use crate::{error::PyRattlerError, package_name::PyPackageName, version::PyVersion};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPackageRecord {
    pub(crate) inner: PackageRecord,
}

impl From<PackageRecord> for PyPackageRecord {
    fn from(value: PackageRecord) -> Self {
        Self { inner: value }
    }
}

impl From<PyPackageRecord> for PackageRecord {
    fn from(val: PyPackageRecord) -> Self {
        val.inner
    }
}

impl AsRef<PackageRecord> for PyPackageRecord {
    fn as_ref(&self) -> &PackageRecord {
        &self.inner
    }
}

#[pymethods]
impl PyPackageRecord {
    /// A simple helper method that constructs a PackageRecord with the bare minimum values.
    #[new]
    fn new(name: PyPackageName, version: PyVersion, build: String) -> Self {
        PackageRecord::new(name.inner, version.inner, build).into()
    }

    /// Builds a `PyPackageRecord` from path to an `index.json` and optionally a size.
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

    #[getter]
    pub fn arch(&self) -> Option<String> {
        self.inner.arch.clone()
    }

    #[getter]
    pub fn build(&self) -> String {
        self.inner.build.clone()
    }

    #[getter]
    pub fn build_number(&self) -> u64 {
        self.inner.build_number
    }

    #[getter]
    pub fn constrains(&self) -> Vec<String> {
        self.inner.constrains.clone()
    }

    #[getter]
    pub fn depends(&self) -> Vec<String> {
        self.inner.depends.clone()
    }

    #[getter]
    pub fn features(&self) -> Option<String> {
        self.inner.features.clone()
    }

    #[getter]
    pub fn legacy_bz2_md5(&self) -> Option<String> {
        self.inner.legacy_bz2_md5.clone()
    }

    #[getter]
    pub fn legacy_bz2_size(&self) -> Option<u64> {
        self.inner.legacy_bz2_size
    }

    #[getter]
    pub fn license(&self) -> Option<String> {
        self.inner.license.clone()
    }

    #[getter]
    pub fn license_family(&self) -> Option<String> {
        self.inner.license_family.clone()
    }

    #[getter]
    pub fn md5(&self) -> Option<String> {
        if let Some(md5) = self.inner.md5 {
            Some(format!("{md5:X}"))
        } else {
            None
        }
    }

    #[getter]
    pub fn name(&self) -> PyPackageName {
        self.inner.name.clone().into()
    }

    // #[getter]
    // pub fn noarch(&self) -> Option<String> {
    //     self.inner.noarch
    // }

    #[getter]
    pub fn platform(&self) -> Option<String> {
        self.inner.platform.clone()
    }

    #[getter]
    pub fn sha256(&self) -> Option<String> {
        if let Some(sha) = self.inner.sha256 {
            Some(format!("{sha:X}"))
        } else {
            None
        }
    }

    #[getter]
    pub fn size(&self) -> Option<u64> {
        self.inner.size
    }

    #[getter]
    pub fn subdir(&self) -> String {
        self.inner.subdir.clone()
    }

    #[getter]
    pub fn timestamp(&self) -> Option<i64> {
        if let Some(time) = self.inner.timestamp {
            Some(time.timestamp())
        } else {
            None
        }
    }

    #[getter]
    pub fn track_features(&self) -> Vec<String> {
        self.inner.track_features.clone()
    }

    #[getter]
    pub fn version(&self) -> PyVersion {
        self.inner.version.clone().into_version().into()
    }

    /// Returns a string representation of PyPackageRecord
    fn as_str(&self) -> String {
        format!("{}", self.inner)
    }
}
