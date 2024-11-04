use std::path::PathBuf;

use pyo3::{exceptions::PyValueError, pyclass, pymethods, types::PyBytes, Bound, PyResult, Python};
use rattler_conda_types::package::{
    FileMode, PackageFile, PathType, PathsEntry, PathsJson, PrefixPlaceholder,
};
use rattler_package_streaming::seek::read_package_file;

use crate::error::PyRattlerError;

/// A representation of the `paths.json` file found in package archives.
///
/// The `paths.json` file contains information about every file included with the package.
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPathsJson {
    pub(crate) inner: PathsJson,
}

impl From<PathsJson> for PyPathsJson {
    fn from(value: PathsJson) -> Self {
        Self { inner: value }
    }
}

impl From<PyPathsJson> for PathsJson {
    fn from(value: PyPathsJson) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPathsJson {
    /// Parses the package file from archive.
    /// Note: If you want to extract multiple `info/*` files then this will be slightly
    ///       slower than manually iterating over the archive entries with
    ///       custom logic as this skips over the rest of the archive
    #[staticmethod]
    pub fn from_package_archive(path: PathBuf) -> PyResult<Self> {
        Ok(read_package_file::<PathsJson>(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object from a file specified by a `path`, using a format appropriate for the file
    /// type.
    ///
    /// For example, if the file is in JSON format, this function reads the data from the file at
    /// the specified path, parse the JSON string and return the resulting object. If the file is
    /// not in a parsable format or if the file could not read, this function returns an error.
    #[staticmethod]
    pub fn from_path(path: PathBuf) -> PyResult<Self> {
        Ok(PathsJson::from_path(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object by looking up the appropriate file from the root of the specified Conda
    /// archive directory, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function reads the appropriate file from
    /// the archive, parse the JSON string and return the resulting object. If the file is not in a
    /// parsable format or if the file could not be read, this function returns an error.
    #[staticmethod]
    pub fn from_package_directory(path: PathBuf) -> PyResult<Self> {
        Ok(PathsJson::from_package_directory(path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Parses the object from a string, using a format appropriate for the file type.
    ///
    /// For example, if the file is in JSON format, this function parses the JSON string and returns
    /// the resulting object. If the file is not in a parsable format, this function returns an
    /// error.
    #[staticmethod]
    pub fn from_str(str: &str) -> PyResult<Self> {
        Ok(PathsJson::from_str(str)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Returns the path to the file within the Conda archive.
    ///
    /// The path is relative to the root of the archive and include any necessary directories.
    #[staticmethod]
    pub fn package_path() -> PathBuf {
        PathsJson::package_path().to_owned()
    }

    /// Constructs a new instance by reading older (deprecated) files from a package directory.
    ///
    /// In older package archives the `paths.json` file does not exist. These packages contain the
    /// information normally present in the `paths.json` file spread over different files in the
    /// archive.
    ///
    /// This function reads the different files and tries to reconstruct a `paths.json` from it.
    #[staticmethod]
    pub fn from_deprecated_package_directory(path: PathBuf) -> PyResult<Self> {
        Ok(PathsJson::from_deprecated_package_directory(&path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Reads the file from a package archive directory. If the `paths.json` file could not be found
    /// use the `from_deprecated_package_directory` method as a fallback.
    #[staticmethod]
    pub fn from_package_directory_with_deprecated_fallback(path: PathBuf) -> PyResult<Self> {
        Ok(
            PathsJson::from_package_directory_with_deprecated_fallback(&path)
                .map(Into::into)
                .map_err(PyRattlerError::from)?,
        )
    }

    /// All entries included in the package.
    #[getter]
    pub fn paths(&self) -> Vec<PyPathsEntry> {
        self.inner
            .paths
            .clone()
            .into_iter()
            .map(Into::into)
            .collect()
    }

    /// The version of the file.
    #[getter]
    pub fn paths_version(&self) -> u64 {
        self.inner.paths_version
    }
}

/// A single entry in the `paths.json` file.
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPathsEntry {
    pub(crate) inner: PathsEntry,
}

impl From<PathsEntry> for PyPathsEntry {
    fn from(value: PathsEntry) -> Self {
        Self { inner: value }
    }
}

impl From<PyPathsEntry> for PathsEntry {
    fn from(value: PyPathsEntry) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPathsEntry {
    /// The relative path from the root of the package
    #[getter]
    pub fn relative_path(&self) -> PathBuf {
        self.inner.relative_path.clone()
    }

    /// Whether or not this file should be linked or not when installing the package.
    #[getter]
    pub fn no_link(&self) -> bool {
        self.inner.no_link
    }

    /// Determines how to include the file when installing the package
    #[getter]
    pub fn path_type(&self) -> PyPathType {
        self.inner.path_type.into()
    }

    /// Optionally the placeholder prefix used in the file. If this value is `None` the prefix is not
    /// present in the file.
    #[getter]
    pub fn prefix_placeholder(&self) -> Option<PyPrefixPlaceholder> {
        if let Some(placeholder) = self.inner.prefix_placeholder.clone() {
            return Some(placeholder.into());
        }

        None
    }

    /// A hex representation of the SHA256 hash of the contents of the file.
    /// This entry is only present in version 1 of the paths.json file.
    #[getter]
    pub fn sha256<'a>(&self, py: Python<'a>) -> Option<Bound<'a, PyBytes>> {
        self.inner.sha256.map(|sha| PyBytes::new_bound(py, &sha))
    }

    /// The size of the file in bytes
    /// This entry is only present in version 1 of the paths.json file.
    #[getter]
    pub fn size_in_bytes(&self) -> Option<u64> {
        self.inner.size_in_bytes
    }
}

/// The path type of the path entry
// TODO: Expose this properly later.
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPathType {
    pub(crate) inner: PathType,
}

impl From<PathType> for PyPathType {
    fn from(value: PathType) -> Self {
        Self { inner: value }
    }
}

impl From<PyPathType> for PathType {
    fn from(value: PyPathType) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPathType {
    /// The path should be hard linked (the default)
    #[getter]
    pub fn hardlink(&self) -> bool {
        matches!(&self.inner, PathType::HardLink)
    }

    /// The path should be soft linked
    #[getter]
    pub fn softlink(&self) -> bool {
        matches!(&self.inner, PathType::SoftLink)
    }

    /// This should explicitly create an empty directory
    #[getter]
    pub fn directory(&self) -> bool {
        matches!(&self.inner, PathType::Directory)
    }
}

/// Description off a placeholder text found in a file that must be replaced when installing the
/// file into the prefix.
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPrefixPlaceholder {
    pub(crate) inner: PrefixPlaceholder,
}

impl From<PrefixPlaceholder> for PyPrefixPlaceholder {
    fn from(value: PrefixPlaceholder) -> Self {
        Self { inner: value }
    }
}

impl From<PyPrefixPlaceholder> for PrefixPlaceholder {
    fn from(value: PyPrefixPlaceholder) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPrefixPlaceholder {
    /// The type of the file, either binary or text. Depending on the type of file either text
    /// replacement is performed or CString replacement.
    #[getter]
    pub fn file_mode(&self) -> PyFileMode {
        self.inner.file_mode.into()
    }

    /// The placeholder prefix used in the file. This is the path of the prefix when the package
    /// was build.
    #[getter]
    pub fn placeholder(&self) -> String {
        self.inner.placeholder.clone()
    }
}

/// The file mode of the entry
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyFileMode {
    pub(crate) inner: FileMode,
}

impl From<FileMode> for PyFileMode {
    fn from(value: FileMode) -> Self {
        Self { inner: value }
    }
}

impl From<PyFileMode> for FileMode {
    fn from(value: PyFileMode) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyFileMode {
    #[new]
    pub fn new(file_mode: &str) -> PyResult<Self> {
        match file_mode {
            "binary" => Ok(Self {
                inner: FileMode::Binary,
            }),
            "text" => Ok(Self {
                inner: FileMode::Text,
            }),
            _ => Err(PyValueError::new_err("Invalid file mode")),
        }
    }

    /// The file is a binary file (needs binary prefix replacement)
    #[getter]
    pub fn binary(&self) -> bool {
        matches!(&self.inner, FileMode::Binary)
    }

    /// The file is a text file (needs text prefix replacement)
    #[getter]
    pub fn text(&self) -> bool {
        matches!(&self.inner, FileMode::Text)
    }
}
