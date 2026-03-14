use std::{path::PathBuf, str::FromStr};

use pyo3::{pyclass, pymethods, types::PyBytes, Bound, PyResult, Python};
use rattler_conda_types::{ExplicitEnvironmentEntry, ExplicitEnvironmentSpec, PackageArchiveHash};
use url::Url;

use crate::{
    error::PyRattlerError,
    platform::PyPlatform,
    utils::{md5_from_pybytes, sha256_from_pybytes},
};

/// The explicit environment (e.g. env.txt) file that contains a list of
/// all URLs in a environment
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyExplicitEnvironmentSpec {
    pub(crate) inner: ExplicitEnvironmentSpec,
}

impl From<ExplicitEnvironmentSpec> for PyExplicitEnvironmentSpec {
    fn from(value: ExplicitEnvironmentSpec) -> Self {
        Self { inner: value }
    }
}

impl From<PyExplicitEnvironmentSpec> for ExplicitEnvironmentSpec {
    fn from(value: PyExplicitEnvironmentSpec) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyExplicitEnvironmentSpec {
    #[new]
    pub fn __init__(
        packages: Vec<PyExplicitEnvironmentEntry>,
        platform: Option<PyPlatform>,
    ) -> Self {
        Self {
            inner: ExplicitEnvironmentSpec {
                packages: packages.into_iter().map(Into::into).collect(),
                platform: platform.map(Into::into),
            },
        }
    }

    /// Parses the object from a file specified by a `path`, using a format appropriate for the file
    /// type.
    ///
    /// For example, if the file is in text format, this function reads the data from the file at
    /// the specified path, parses the text and returns the resulting object. If the file is
    /// not in a parse-able format or if the file could not be read, this function returns an error.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(ExplicitEnvironmentSpec::from_path(&path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object from a string containing the explicit environment specification
    #[staticmethod]
    pub fn from_str(content: &str) -> PyResult<Self> {
        Ok(ExplicitEnvironmentSpec::from_str(content)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns the platform specified in the explicit environment specification
    pub fn platform(&self) -> Option<PyPlatform> {
        self.inner.platform.map(PyPlatform::from)
    }

    /// Returns the environment entries (URLs) specified in the explicit environment specification
    pub fn packages(&self) -> Vec<PyExplicitEnvironmentEntry> {
        self.inner
            .packages
            .iter()
            .cloned()
            .map(PyExplicitEnvironmentEntry)
            .collect()
    }

    /// Converts an [`ExplicitEnvironmentSpec`] to a string representing a valid explicit
    /// environment file
    pub fn to_spec_string(&self) -> String {
        self.inner.to_spec_string()
    }

    /// Writes an explicit environment spec to file
    pub fn to_path(&self, path: PathBuf) -> PyResult<()> {
        self.inner.to_path(path).map_err(PyRattlerError::from)?;
        Ok(())
    }
}

/// A Python wrapper around an explicit environment entry which represents a URL to a package
#[pyclass]
#[derive(Clone)]
pub struct PyExplicitEnvironmentEntry(pub(crate) ExplicitEnvironmentEntry);

#[pymethods]
impl PyExplicitEnvironmentEntry {
    #[new]
    pub fn __init__(url: String) -> PyResult<Self> {
        Ok(Self(ExplicitEnvironmentEntry {
            url: Url::parse(&url).map_err(PyRattlerError::from)?,
        }))
    }

    /// Returns the URL of the package
    pub fn url(&self) -> String {
        self.0.url.to_string()
    }

    /// If the url contains a hash section, that hash refers to the hash of the package archive.
    pub fn package_archive_hash(&self) -> PyResult<Option<PyPackageArchiveHash>> {
        Ok(self
            .0
            .package_archive_hash()
            .map(|hash| hash.map(Into::into))
            .map_err(PyRattlerError::from)?)
    }
}

impl From<ExplicitEnvironmentEntry> for PyExplicitEnvironmentEntry {
    fn from(value: ExplicitEnvironmentEntry) -> Self {
        Self(value)
    }
}

impl From<PyExplicitEnvironmentEntry> for ExplicitEnvironmentEntry {
    fn from(value: PyExplicitEnvironmentEntry) -> Self {
        value.0
    }
}

/// Package urls in explicit environments can have an optional hash that signifies a hash of the
/// package archive.
#[pyclass]
#[derive(Clone)]
pub enum PyPackageArchiveHash {
    /// An MD5 hash for a given package
    Md5 { hash: PyExplicitMd5Hash },
    /// A SHA256 hash for a given package
    Sha256 { hash: PyExplicitSha256Hash },
}

#[pyclass]
#[derive(Clone)]
pub struct PyExplicitMd5Hash(pub(crate) rattler_digest::Md5Hash);

#[pymethods]
impl PyExplicitMd5Hash {
    #[new]
    pub fn __init__(hash: Bound<'_, PyBytes>) -> PyResult<Self> {
        Ok(Self(md5_from_pybytes(hash)?))
    }

    pub fn __bytes__<'a>(&self, py: Python<'a>) -> Bound<'a, PyBytes> {
        PyBytes::new(py, &self.0)
    }

    pub fn __repr__(&self) -> String {
        format!("{:x}", self.0)
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyExplicitSha256Hash(pub(crate) rattler_digest::Sha256Hash);

#[pymethods]
impl PyExplicitSha256Hash {
    #[new]
    pub fn __init__(hash: Bound<'_, PyBytes>) -> PyResult<Self> {
        Ok(Self(sha256_from_pybytes(hash)?))
    }

    pub fn __bytes__<'a>(&self, py: Python<'a>) -> Bound<'a, PyBytes> {
        PyBytes::new(py, &self.0)
    }

    pub fn __repr__(&self) -> String {
        format!("{:x}", self.0)
    }
}

impl From<PackageArchiveHash> for PyPackageArchiveHash {
    fn from(value: PackageArchiveHash) -> Self {
        match value {
            PackageArchiveHash::Md5(hash) => PyPackageArchiveHash::Md5 {
                hash: PyExplicitMd5Hash(hash),
            },
            PackageArchiveHash::Sha256(hash) => PyPackageArchiveHash::Sha256 {
                hash: PyExplicitSha256Hash(hash),
            },
        }
    }
}
