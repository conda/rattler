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
/// **Performance:** Custom sources are slower than channels because data must be
/// marshaled between Python and Rust for each request. For performance-critical
/// applications, channels should be preferred when possible.
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
        let future = Python::with_gil(|py| {
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

        // Extract the records from the Python list
        Python::with_gil(|py| {
            // Get the result as a Python list
            let py_list: Vec<Bound<'_, PyAny>> = result
                .extract(py)
                .map_err(|e| GatewayError::Generic(e.to_string()))?;

            // Extract each element, handling both direct PyRecord and wrapped RepoDataRecord
            let mut rust_records = Vec::new();
            for item in py_list {
                // Try to extract PyRecord using the TryFrom implementation
                // which handles the _record attribute lookup
                let py_record: PyRecord = item
                    .try_into()
                    .map_err(|e: PyErr| GatewayError::Generic(e.to_string()))?;

                let record: RepoDataRecord = py_record
                    .try_into()
                    .map_err(|e: PyErr| GatewayError::Generic(e.to_string()))?;

                rust_records.push(Arc::new(record));
            }

            Ok(rust_records)
        })
    }

    fn package_names(&self, platform: Platform) -> Vec<String> {
        Python::with_gil(|py| {
            let py_platform = PyPlatform::from(platform);

            self.inner
                .call_method1(py, "package_names", (py_platform,))
                .and_then(|result| result.extract(py))
                .unwrap_or_default()
        })
    }
}
