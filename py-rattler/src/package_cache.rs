use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use pyo3::basic::CompareOp;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_cache::package_cache::{CacheIndex, PackageCache, PackageCacheLayer};
use rattler_cache::validation::ValidationMode;
use url::Url;

use crate::error::PyRattlerError;
use crate::match_spec::PyMatchSpec;
use crate::record::PyRecord;

#[pyclass(from_py_object)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PyValidationMode {
    pub(crate) inner: ValidationMode,
}

impl From<ValidationMode> for PyValidationMode {
    fn from(value: ValidationMode) -> Self {
        Self { inner: value }
    }
}

impl From<PyValidationMode> for ValidationMode {
    fn from(value: PyValidationMode) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyValidationMode {
    /// Skip validation.
    #[staticmethod]
    pub fn skip() -> Self {
        ValidationMode::Skip.into()
    }

    /// Fast validation (only check if files exist).
    #[staticmethod]
    pub fn fast() -> Self {
        ValidationMode::Fast.into()
    }

    /// Full validation (check files and hashes).
    #[staticmethod]
    pub fn full() -> Self {
        ValidationMode::Full.into()
    }

    /// Returns true if this is skip validation mode.
    #[getter]
    pub fn is_skip(&self) -> bool {
        self.inner == ValidationMode::Skip
    }

    /// Returns true if this is fast validation mode.
    #[getter]
    pub fn is_fast(&self) -> bool {
        self.inner == ValidationMode::Fast
    }

    /// Returns true if this is full validation mode.
    #[getter]
    pub fn is_full(&self) -> bool {
        self.inner == ValidationMode::Full
    }

    fn __richcmp__(&self, other: &Self, op: CompareOp) -> PyResult<bool> {
        match op {
            CompareOp::Eq => Ok(self.inner == other.inner),
            CompareOp::Ne => Ok(self.inner != other.inner),
            _ => Err(PyTypeError::new_err("Comparison not supported")),
        }
    }

    fn __hash__(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.inner.hash(&mut hasher);
        hasher.finish()
    }

    fn __repr__(&self) -> String {
        format!("ValidationMode.{:?}", self.inner)
    }
}

#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyPackageCacheLayer {
    pub(crate) inner: PackageCacheLayer,
}

impl From<PackageCacheLayer> for PyPackageCacheLayer {
    fn from(value: PackageCacheLayer) -> Self {
        Self { inner: value }
    }
}

impl From<PyPackageCacheLayer> for PackageCacheLayer {
    fn from(value: PyPackageCacheLayer) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPackageCacheLayer {
    /// Creates an unfiltered package-cache layer.
    #[new]
    pub fn new(path: PathBuf) -> Self {
        PackageCacheLayer::new(path).into()
    }

    /// Returns the root directory of this layer.
    #[getter]
    pub fn path(&self) -> PathBuf {
        self.inner.path().to_path_buf()
    }

    /// Determine if the layer is read-only in the filesystem.
    pub fn is_readonly(&self) -> bool {
        self.inner.is_readonly()
    }

    /// Sets the validation mode used by this layer.
    pub fn with_validation_mode(&self, validation_mode: PyValidationMode) -> Self {
        self.inner
            .clone()
            .with_validation_mode(validation_mode.into())
            .into()
    }

    /// Adds an inclusion `MatchSpec` to this layer.
    pub fn with_filter(&self, spec: PyMatchSpec) -> Self {
        self.inner.clone().with_filter(spec.into()).into()
    }

    /// Excludes packages matching `spec` from this layer.
    pub fn excluding(&self, spec: PyMatchSpec) -> Self {
        self.inner.clone().excluding(spec.into()).into()
    }

    fn __repr__(&self) -> String {
        format!(
            "PackageCacheLayer(path=\"{}\")",
            self.inner.path().display()
        )
    }
}

#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyCacheIndex {
    pub(crate) inner: CacheIndex,
}

impl From<CacheIndex> for PyCacheIndex {
    fn from(value: CacheIndex) -> Self {
        Self { inner: value }
    }
}

impl From<PyCacheIndex> for CacheIndex {
    fn from(value: PyCacheIndex) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyCacheIndex {
    /// Returns whether the package described by a `RepoDataRecord` is present in the cache.
    pub fn contains_record(&self, record: &PyRecord) -> PyResult<bool> {
        let repo_data = record.try_as_repodata_record()?;
        Ok(self.inner.contains_record(repo_data))
    }

    /// Returns whether a package fetched from `url` is present in the cache.
    pub fn contains_url(&self, record: &PyRecord, url: String) -> PyResult<bool> {
        let pkg = record.as_package_record();
        let parsed_url = Url::parse(&url).map_err(|e| PyValueError::new_err(e.to_string()))?;
        Ok(self.inner.contains_url(pkg, &parsed_url))
    }

    /// Returns whether a package fetched from `path` is present in the cache.
    pub fn contains_path(&self, record: &PyRecord, path: PathBuf) -> PyResult<bool> {
        let pkg = record.as_package_record();
        Ok(self.inner.contains_path(pkg, &path))
    }

    /// Returns true if the snapshot contains no packages.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn __len__(&self) -> usize {
        self.inner.len()
    }

    fn __repr__(&self) -> String {
        format!("CacheIndex(len={})", self.inner.len())
    }
}

#[pyclass(from_py_object)]
#[derive(Clone)]
pub struct PyPackageCache {
    pub(crate) inner: PackageCache,
}

impl From<PackageCache> for PyPackageCache {
    fn from(value: PackageCache) -> Self {
        Self { inner: value }
    }
}

impl From<PyPackageCache> for PackageCache {
    fn from(value: PyPackageCache) -> Self {
        value.inner
    }
}

#[pymethods]
impl PyPackageCache {
    /// Creates a new `PackageCache` with a single layer pointing to the given path.
    #[new]
    pub fn new(path: PathBuf) -> Self {
        PackageCache::new(path).into()
    }

    /// Creates a `PackageCache` from layered paths.
    #[staticmethod]
    #[pyo3(signature = (paths, cache_origin=false, validation_mode=None))]
    pub fn new_layered(
        paths: Vec<PathBuf>,
        cache_origin: bool,
        validation_mode: Option<PyValidationMode>,
    ) -> Self {
        let val_mode = validation_mode.map(Into::into).unwrap_or_default();
        PackageCache::new_layered(paths, cache_origin, val_mode).into()
    }

    /// Creates a `PackageCache` from multiple layers.
    #[staticmethod]
    #[pyo3(signature = (layers, cache_origin=false))]
    pub fn from_layers(layers: Vec<PyPackageCacheLayer>, cache_origin: bool) -> Self {
        let rust_layers: Vec<PackageCacheLayer> = layers.into_iter().map(Into::into).collect();
        PackageCache::from_layers(rust_layers, cache_origin).into()
    }

    /// Adds the origin (url or path) to the cache key.
    pub fn with_cached_origin(&self) -> Self {
        self.inner.clone().with_cached_origin().into()
    }

    /// Prepends a configured layer to this cache.
    pub fn with_prepended_layer(&self, layer: PyPackageCacheLayer) -> Self {
        self.inner.clone().with_prepended_layer(layer.into()).into()
    }

    /// Excludes packages matching `spec` from every existing layer.
    pub fn excluding_from_all_layers(&self, spec: PyMatchSpec) -> Self {
        self.inner
            .clone()
            .excluding_from_all_layers(spec.into())
            .into()
    }

    /// Returns the first writable directory in the package cache, if any.
    /// Note: Checks whether the layer directory is read-only.
    pub fn first_writable(&self) -> Option<PathBuf> {
        let (_, writable_layers) = self.inner.split_layers();
        writable_layers.first().map(|l| l.path().to_path_buf())
    }

    /// Takes a snapshot of the packages present in the cache.
    pub fn index<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let cache = self.inner.clone();
        future_into_py(py, async move {
            let index = cache.index().await.map_err(PyRattlerError::from)?;
            Ok(PyCacheIndex::from(index))
        })
    }

    /// Returns the layers in this package cache in lookup order.
    pub fn layers(&self) -> Vec<PyPackageCacheLayer> {
        self.inner
            .layers()
            .iter()
            .cloned()
            .map(Into::into)
            .collect()
    }

    fn __repr__(&self) -> String {
        let all_layers: Vec<_> = self
            .inner
            .layers()
            .iter()
            .map(PackageCacheLayer::path)
            .collect();
        format!("PackageCache(layers={all_layers:?})")
    }
}
