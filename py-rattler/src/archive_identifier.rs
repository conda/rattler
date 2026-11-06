use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::str::FromStr;

use pyo3::basic::CompareOp;
use pyo3::exceptions::PyValueError;
use pyo3::{PyResult, pyclass, pymethods};
use rattler_conda_types::package::{ArchiveIdentifier, CondaArchiveIdentifier, CondaArchiveType};
use url::Url;

/// Represents an archive identifier consisting of `name`, `version`, and `build_string`.
#[pyclass(from_py_object)]
#[repr(transparent)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyArchiveIdentifier {
    pub(crate) inner: ArchiveIdentifier,
}

impl From<ArchiveIdentifier> for PyArchiveIdentifier {
    fn from(value: ArchiveIdentifier) -> Self {
        Self { inner: value }
    }
}

impl From<PyArchiveIdentifier> for ArchiveIdentifier {
    fn from(value: PyArchiveIdentifier) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyArchiveIdentifier {
    #[new]
    pub fn new(name: String, version: String, build_string: String) -> Self {
        Self {
            inner: ArchiveIdentifier {
                name,
                version,
                build_string,
            },
        }
    }

    #[staticmethod]
    pub fn from_str(str: &str) -> PyResult<Self> {
        ArchiveIdentifier::from_str(str)
            .map(Into::into)
            .map_err(PyValueError::new_err)
    }

    #[getter]
    pub fn name(&self) -> String {
        self.inner.name.clone()
    }

    #[setter]
    pub fn set_name(&mut self, name: String) {
        self.inner.name = name;
    }

    #[getter]
    pub fn version(&self) -> String {
        self.inner.version.clone()
    }

    #[setter]
    pub fn set_version(&mut self, version: String) {
        self.inner.version = version;
    }

    #[getter]
    pub fn build_string(&self) -> String {
        self.inner.build_string.clone()
    }

    #[setter]
    pub fn set_build_string(&mut self, build_string: String) {
        self.inner.build_string = build_string;
    }

    pub fn __str__(&self) -> String {
        self.inner.to_string()
    }

    pub fn __repr__(&self) -> String {
        format!(
            "ArchiveIdentifier(name={:?}, version={:?}, build_string={:?})",
            self.inner.name, self.inner.version, self.inner.build_string
        )
    }

    pub fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.inner.hash(&mut hasher);
        hasher.finish()
    }

    pub fn __richcmp__(&self, other: &Self, op: CompareOp) -> bool {
        op.matches(self.inner.cmp(&other.inner))
    }
}

/// Represents the type of a conda package archive (`.conda` or `.tar.bz2`).
#[pyclass(from_py_object)]
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct PyCondaArchiveType {
    pub(crate) inner: CondaArchiveType,
}

impl From<CondaArchiveType> for PyCondaArchiveType {
    fn from(value: CondaArchiveType) -> Self {
        Self { inner: value }
    }
}

impl From<PyCondaArchiveType> for CondaArchiveType {
    fn from(value: PyCondaArchiveType) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyCondaArchiveType {
    #[staticmethod]
    pub fn conda() -> Self {
        CondaArchiveType::Conda.into()
    }

    #[staticmethod]
    pub fn tar_bz2() -> Self {
        CondaArchiveType::TarBz2.into()
    }

    #[getter]
    pub fn is_conda(&self) -> bool {
        matches!(self.inner, CondaArchiveType::Conda)
    }

    #[getter]
    pub fn is_tar_bz2(&self) -> bool {
        matches!(self.inner, CondaArchiveType::TarBz2)
    }

    #[getter]
    pub fn extension(&self) -> &'static str {
        self.inner.extension()
    }

    #[staticmethod]
    pub fn try_from_magic_bytes(bytes: &[u8]) -> Option<Self> {
        CondaArchiveType::try_from_magic_bytes(bytes).map(Into::into)
    }

    #[staticmethod]
    pub fn try_from_path(path: &str) -> Option<Self> {
        CondaArchiveType::try_from(Path::new(path)).map(Into::into)
    }

    #[staticmethod]
    pub fn split_str(path: &str) -> Option<(String, Self)> {
        CondaArchiveType::split_str(path).map(|(stem, ty)| (stem.to_string(), ty.into()))
    }

    pub fn __str__(&self) -> &'static str {
        self.inner.extension()
    }

    pub fn __repr__(&self) -> String {
        match self.inner {
            CondaArchiveType::Conda => "CondaArchiveType.Conda".to_string(),
            CondaArchiveType::TarBz2 => "CondaArchiveType.TarBz2".to_string(),
        }
    }

    pub fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.inner.hash(&mut hasher);
        hasher.finish()
    }

    pub fn __richcmp__(&self, other: &Self, op: CompareOp) -> bool {
        op.matches(self.inner.cmp(&other.inner))
    }
}

/// Represents a conda archive identifier containing an `ArchiveIdentifier` and `CondaArchiveType`.
#[pyclass(from_py_object)]
#[repr(transparent)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyCondaArchiveIdentifier {
    pub(crate) inner: CondaArchiveIdentifier,
}

impl From<CondaArchiveIdentifier> for PyCondaArchiveIdentifier {
    fn from(value: CondaArchiveIdentifier) -> Self {
        Self { inner: value }
    }
}

impl From<PyCondaArchiveIdentifier> for CondaArchiveIdentifier {
    fn from(value: PyCondaArchiveIdentifier) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyCondaArchiveIdentifier {
    #[new]
    pub fn new(identifier: PyArchiveIdentifier, archive_type: PyCondaArchiveType) -> Self {
        Self {
            inner: CondaArchiveIdentifier {
                identifier: identifier.inner,
                archive_type: archive_type.inner,
            },
        }
    }

    #[staticmethod]
    pub fn from_str(str: &str) -> PyResult<Self> {
        CondaArchiveIdentifier::from_str(str)
            .map(Into::into)
            .map_err(PyValueError::new_err)
    }

    #[staticmethod]
    pub fn try_from_filename(filename: &str) -> Option<Self> {
        CondaArchiveIdentifier::try_from_filename(filename).map(Into::into)
    }

    #[staticmethod]
    pub fn try_from_path(path: &str) -> Option<Self> {
        CondaArchiveIdentifier::try_from_path(Path::new(path)).map(Into::into)
    }

    #[staticmethod]
    pub fn try_from_url(url: &str) -> Option<Self> {
        let parsed = Url::parse(url).ok()?;
        CondaArchiveIdentifier::try_from_url(&parsed).map(Into::into)
    }

    #[getter]
    pub fn identifier(&self) -> PyArchiveIdentifier {
        self.inner.identifier.clone().into()
    }

    #[setter]
    pub fn set_identifier(&mut self, identifier: PyArchiveIdentifier) {
        self.inner.identifier = identifier.inner;
    }

    #[getter]
    pub fn archive_type(&self) -> PyCondaArchiveType {
        self.inner.archive_type.into()
    }

    #[setter]
    pub fn set_archive_type(&mut self, archive_type: PyCondaArchiveType) {
        self.inner.archive_type = archive_type.inner;
    }

    #[getter]
    pub fn name(&self) -> String {
        self.inner.identifier.name.clone()
    }

    #[getter]
    pub fn version(&self) -> String {
        self.inner.identifier.version.clone()
    }

    #[getter]
    pub fn build_string(&self) -> String {
        self.inner.identifier.build_string.clone()
    }

    pub fn to_file_name(&self) -> String {
        self.inner.to_file_name()
    }

    pub fn __str__(&self) -> String {
        self.inner.to_string()
    }

    pub fn __repr__(&self) -> String {
        format!(
            "CondaArchiveIdentifier(identifier={:?}, archive_type={:?})",
            self.inner.identifier, self.inner.archive_type
        )
    }

    pub fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.inner.hash(&mut hasher);
        hasher.finish()
    }

    pub fn __richcmp__(&self, other: &Self, op: CompareOp) -> bool {
        op.matches(self.inner.cmp(&other.inner))
    }
}
