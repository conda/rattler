//! Python `RepoDataSource` adapter implementation.

use std::sync::Arc;

use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::into_future;
use rattler_conda_types::{PackageName, Platform, RepoDataRecord};
use rattler_repodata_gateway::{GatewayError, RepoDataSource};

use crate::package_name::PyPackageName;
use crate::platform::PyPlatform;
use crate::record::PyRecord;

/// Wraps a Python object implementing the `RepoDataSource` protocol.
///
/// This adapter allows Python objects with `fetch_package_records` (async)
/// and `package_names` (sync) methods to be used as repodata sources
/// in the gateway.
///
/// **Note:** Custom sources are not cached by the gateway. The gateway's internal
/// caching mechanisms only apply to channel data. If caching is needed for a custom
/// source, it must be implemented within the source itself.
///
/// **Performance:** Records are shared with Python by `Arc`, so calls cost roughly
/// one Arc bump per record plus the unavoidable Python coroutine round-trip — not
/// a deep copy. The remaining gap versus the all-Rust path is dominated by
/// `package_names` being synchronous and by Python coroutine scheduling, not by
/// per-record marshalling. If a Python source needs to do meaningful work to
/// answer `package_names`, consider caching the result in the source itself so
/// the gateway can reuse it across queries.
pub struct PyRepoDataSource {
    inner: Py<PyAny>,
}

impl PyRepoDataSource {
    /// Create a new adapter wrapping the given Python object.
    ///
    /// The object should implement the `RepoDataSource` protocol:
    /// - `async def fetch_package_records(self, platform, name) -> List[RepoDataRecord]`
    /// - `def package_names(self, platform) -> List[str]`
    pub fn new(obj: Py<PyAny>) -> Self {
        Self { inner: obj }
    }
}

// SAFETY: The Python GIL ensures thread-safety when accessing Python objects.
// We only access self.inner while holding the GIL.
unsafe impl Send for PyRepoDataSource {}
unsafe impl Sync for PyRepoDataSource {}

#[async_trait::async_trait]
impl RepoDataSource for PyRepoDataSource {
    async fn fetch_package_records(
        &self,
        platform: Platform,
        name: &PackageName,
    ) -> Result<Vec<Arc<RepoDataRecord>>, GatewayError> {
        // Clone what we need before the async block
        let name_clone = name.clone();

        // Get the Python coroutine and convert to future
        let future = Python::attach(|py| {
            let py_platform = PyPlatform::from(platform);
            let py_name = PyPackageName::from(name_clone);

            // Call the async method - this returns a coroutine object
            let coro = self
                .inner
                .call_method1(py, "fetch_package_records", (py_platform, py_name))
                .map_err(|e| GatewayError::Generic(e.to_string()))?;

            // Convert Python coroutine to Rust future
            into_future(coro.into_bound(py)).map_err(|e| GatewayError::Generic(e.to_string()))
        })?;

        // Await the future outside the GIL
        let result = future
            .await
            .map_err(|e| GatewayError::Generic(e.to_string()))?;

        // Extract the records from the Python iterable.
        //
        // We pull the existing `Arc<RepoDataRecord>` straight out of each `PyRecord`
        // instead of deep-cloning the record into a fresh Arc — that's the whole point
        // of wrapping records in Arc on both sides. See `TryFrom<PyRecord> for
        // Arc<RepoDataRecord>`.
        //
        // Iterating via `try_iter` avoids materializing an intermediate
        // `Vec<Bound<PyAny>>` first; we hit each Python item once.
        Python::attach(|py| {
            let bound = result.bind(py);
            let mut rust_records: Vec<Arc<RepoDataRecord>> =
                Vec::with_capacity(bound.len().unwrap_or(0));
            let iter = bound
                .try_iter()
                .map_err(|e| GatewayError::Generic(e.to_string()))?;
            for item in iter {
                let item = item.map_err(|e| GatewayError::Generic(e.to_string()))?;
                let py_record: PyRecord = item
                    .try_into()
                    .map_err(|e: PyErr| GatewayError::Generic(e.to_string()))?;
                let arc: Arc<RepoDataRecord> = py_record
                    .try_into()
                    .map_err(|e: PyErr| GatewayError::Generic(e.to_string()))?;
                rust_records.push(arc);
            }
            Ok(rust_records)
        })
    }

    fn package_names(&self, platform: Platform) -> Vec<String> {
        Python::attach(|py| {
            let py_platform = PyPlatform::from(platform);

            self.inner
                .call_method1(py, "package_names", (py_platform,))
                .and_then(|result| result.extract(py))
                .unwrap_or_default()
        })
    }
}
