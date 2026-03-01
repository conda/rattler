use std::path::PathBuf;

use pyo3::prelude::*;
use rattler::package_cache::PackageCache;
use rattler_cache::validation::ValidationMode;

#[pyclass(eq, eq_int)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum PyValidationMode {
    Skip,
    Fast,
    Full,
}

impl From<PyValidationMode> for ValidationMode {
    fn from(mode: PyValidationMode) -> Self {
        match mode {
            PyValidationMode::Skip => ValidationMode::Skip,
            PyValidationMode::Fast => ValidationMode::Fast,
            PyValidationMode::Full => ValidationMode::Full,
        }
    }
}

impl From<ValidationMode> for PyValidationMode {
    fn from(mode: ValidationMode) -> Self {
        match mode {
            ValidationMode::Skip => PyValidationMode::Skip,
            ValidationMode::Fast => PyValidationMode::Fast,
            ValidationMode::Full => PyValidationMode::Full,
        }
    }
}

#[pyclass]
#[repr(transparent)]
pub struct PyPackageCache {
    pub(crate) inner: PackageCache,
}

#[pymethods]
impl PyPackageCache {
    #[new]
    #[pyo3(signature = (path, cache_origin=false, validation_mode=PyValidationMode::Skip))]
    pub fn new(path: PathBuf, cache_origin: bool, validation_mode: PyValidationMode) -> Self {
        let inner = PackageCache::new_layered(
            std::iter::once(path),
            cache_origin,
            validation_mode.into(),
        );
        Self { inner }
    }

    #[staticmethod]
    #[pyo3(signature = (paths, cache_origin=false, validation_mode=PyValidationMode::Skip))]
    pub fn new_layered(
        paths: Vec<PathBuf>,
        cache_origin: bool,
        validation_mode: PyValidationMode,
    ) -> Self {
        Self {
            inner: PackageCache::new_layered(paths, cache_origin, validation_mode.into()),
        }
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.inner
            .layers()
            .iter()
            .map(|l| l.path().to_path_buf())
            .collect()
    }

    pub fn writable_paths(&self) -> Vec<PathBuf> {
        let (_, writable) = self.inner.split_layers();
        writable.iter().map(|l| l.path().to_path_buf()).collect()
    }

    pub fn readonly_paths(&self) -> Vec<PathBuf> {
        let (readonly, _) = self.inner.split_layers();
        readonly.iter().map(|l| l.path().to_path_buf()).collect()
    }

    pub fn __repr__(&self) -> String {
        let paths: Vec<String> = self
            .inner
            .layers()
            .iter()
            .map(|l| format!("{:?}", l.path()))
            .collect();
        format!("PackageCache(paths=[{}])", paths.join(", "))
    }
}
