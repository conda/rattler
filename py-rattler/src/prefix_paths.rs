use crate::paths_json::PyFileMode;
use pyo3::{pyclass, pymethods, types::PyBytes, Python};
use rattler_conda_types::prefix_record::{PathType, PathsEntry, PrefixPaths};
use std::path::PathBuf;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPrefixPaths {
    pub(crate) inner: PrefixPaths,
}

impl From<PyPrefixPaths> for PrefixPaths {
    fn from(value: PyPrefixPaths) -> Self {
        value.inner
    }
}

impl From<PrefixPaths> for PyPrefixPaths {
    fn from(value: PrefixPaths) -> Self {
        Self { inner: value }
    }
}

/// An entry in the paths_data attribute of the PrefixRecord
/// This is similar to PathsEntry from paths_json but refers
/// to an entry for an installed package
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPrefixPathsEntry {
    pub(crate) inner: PathsEntry,
}

impl From<PathsEntry> for PyPrefixPathsEntry {
    fn from(value: PathsEntry) -> Self {
        Self { inner: value }
    }
}

impl From<PyPrefixPathsEntry> for PathsEntry {
    fn from(value: PyPrefixPathsEntry) -> Self {
        value.inner
    }
}

/// The path type of the path entry
/// This is similar to PathType from paths_json; however, it contains additional enum fields
/// since it represents a file that's installed
#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPrefixPathType {
    pub(crate) inner: PathType,
}

impl From<PathType> for PyPrefixPathType {
    fn from(value: PathType) -> Self {
        Self { inner: value }
    }
}

impl From<PyPrefixPathType> for PathType {
    fn from(value: PyPrefixPathType) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPrefixPathType {
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

    /// A file compiled from Python code when a noarch package was installed
    #[getter]
    pub fn pyc_file(&self) -> bool {
        matches!(&self.inner, PathType::PycFile)
    }

    /// A Windows entry point python script (a <entrypoint>-script.py Python script file)
    #[getter]
    pub fn windows_python_entrypoint_script(&self) -> bool {
        matches!(&self.inner, PathType::WindowsPythonEntryPointScript)
    }

    /// A Windows Python entry point executable (a <entrypoint>.exe file)
    #[getter]
    pub fn windows_python_entrypoint_exe(&self) -> bool {
        matches!(&self.inner, PathType::WindowsPythonEntryPointExe)
    }

    /// This file is a Python entry point executable for Unix (a `<entrypoint>` Python script file)
    /// Entry points are created in the `bin/...` directory when installing Python noarch packages
    #[getter]
    pub fn unix_python_entrypoint(&self) -> bool {
        matches!(&self.inner, PathType::UnixPythonEntryPoint)
    }
}

#[pymethods]
impl PyPrefixPathsEntry {
    /// The relative path from the root of the package
    #[getter]
    pub fn relative_path(&self) -> PathBuf {
        self.inner.relative_path.clone()
    }

    /// Whether this file should be linked when installing the package.
    #[getter]
    pub fn no_link(&self) -> bool {
        self.inner.no_link
    }

    /// Determines how to include the file when installing the package
    #[getter]
    pub fn path_type(&self) -> PyPrefixPathType {
        self.inner.path_type.into()
    }

    /// Optionally the placeholder prefix used in the file. If this value is `None` the prefix is not
    /// present in the file.
    #[getter]
    pub fn prefix_placeholder(&self) -> Option<String> {
        self.inner.prefix_placeholder.clone()
    }

    /// If a file has a placeholder, the method by which the placeholder was replaced
    #[getter]
    pub fn file_mode(&self) -> Option<PyFileMode> {
        if let Some(file_mode) = self.inner.file_mode {
            return Some(file_mode.into());
        }
        None
    }

    /// A hex representation of the SHA256 hash of the contents of the file
    /// If prefix_placeholder is present, this represents the hash of the file *before*
    /// any placeholders were replaced
    #[getter]
    pub fn sha256<'a>(&self, py: Python<'a>) -> Option<&'a PyBytes> {
        self.inner.sha256.map(|sha| PyBytes::new(py, &sha))
    }

    /// A hex representation of the SHA256 hash of the contents of the file as installed
    /// This will be present only if prefix_placeholder is defined. In this case,
    /// this is the hash of the file after the placeholder has been replaced.
    #[getter]
    pub fn sha256_in_prefix<'a>(&self, py: Python<'a>) -> Option<&'a PyBytes> {
        self.inner
            .sha256_in_prefix
            .map(|shla| PyBytes::new(py, &shla))
    }

    /// The size of the file in bytes
    /// This entry is only present in version 1 of the paths.json file.
    #[getter]
    pub fn size_in_bytes(&self) -> Option<u64> {
        self.inner.size_in_bytes
    }
}

#[pymethods]
impl PyPrefixPaths {
    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }

    /// The version of the file
    #[getter]
    pub fn paths_version(&self) -> u64 {
        self.inner.paths_version
    }

    /// All entries included in the package.
    #[getter]
    pub fn paths(&self) -> Vec<PyPrefixPathsEntry> {
        self.inner
            .paths
            .clone()
            .into_iter()
            .map(Into::into)
            .collect()
    }
}
