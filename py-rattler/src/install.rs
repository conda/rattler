use std::path::PathBuf;
use std::sync::Arc;

use pyo3::{pyfunction, Bound, PyAny, PyResult, Python};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler::install::{
    empty_trash, link_package, unlink_package, InstallDriver, InstallOptions, PythonInfo,
};
use rattler_conda_types::{Platform, PrefixRecord};
use tokio::sync::Semaphore;

use crate::{error::PyRattlerError, platform::PyPlatform, record::PyRecord};

/// Links a package into a prefix.
///
/// Args:
///     package_dir: Path to the extracted package directory
///     target_dir: Path to the environment prefix
///     python_info_version: Optional Python version for noarch packages
///     python_info_implementation: Optional Python implementation for noarch packages
///     platform: Optional target platform
///     io_concurrency_limit: Optional limit for concurrent IO operations
///     prefix_records: Optional list of prefix records in the environment
///     execute_link_scripts: Whether to execute pre/post link scripts
///
/// Returns:
///     A list of path entries that were linked into the prefix
#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (
    package_dir,
    target_dir,
    python_info_version=None,
    python_info_implementation=None,
    platform=None,
    io_concurrency_limit=None,
    prefix_records=None,
    execute_link_scripts=false
))]
pub fn py_link_package<'a>(
    py: Python<'a>,
    package_dir: PathBuf,
    target_dir: PathBuf,
    python_info_version: Option<String>,
    python_info_implementation: Option<String>,
    platform: Option<PyPlatform>,
    io_concurrency_limit: Option<usize>,
    prefix_records: Option<Vec<Bound<'a, PyAny>>>,
    execute_link_scripts: bool,
) -> PyResult<Bound<'a, PyAny>> {
    let target_platform = platform.map(|p| p.inner).unwrap_or_else(Platform::current);

    // Create python info if version is provided
    let python_info = if let Some(version) = python_info_version {
        Some(
            PythonInfo::from_version(
                &version.parse().map_err(|e| {
                    PyRattlerError::LinkError(format!("invalid Python version: {}", e))
                })?,
                python_info_implementation.as_deref(),
                target_platform,
            )
            .map_err(|e| {
                PyRattlerError::LinkError(format!("failed to create Python info: {}", e))
            })?,
        )
    } else {
        None
    };

    // Convert prefix_records if provided
    let records = match prefix_records {
        Some(records) => {
            let mut converted = Vec::new();
            for record in records {
                let pr: PrefixRecord = PyRecord::try_from(record)?.try_into()?;
                converted.push(pr);
            }
            Some(converted)
        }
        None => None,
    };

    // Store these separately, we can't move Python references into the async block
    let package_dir_clone = package_dir.clone();
    let target_dir_clone = target_dir.clone();
    let python_info_clone = python_info.clone();
    let platform_clone = platform;

    future_into_py(py, async move {
        // Create an install driver with the provided configuration
        let mut driver_builder = InstallDriver::builder();

        // Set IO concurrency if specified
        if let Some(limit) = io_concurrency_limit {
            driver_builder =
                driver_builder.with_io_concurrency_semaphore(Arc::new(Semaphore::new(limit)));
        }

        driver_builder = driver_builder.execute_link_scripts(execute_link_scripts);

        if let Some(records) = records {
            driver_builder = driver_builder.with_prefix_records(&records);
        }

        // Build the driver
        let driver = driver_builder.finish();

        // Create installation options
        let options = InstallOptions {
            platform: platform_clone.map(|p| p.inner),
            python_info: python_info_clone,
            ..InstallOptions::default()
        };

        match link_package(&package_dir_clone, &target_dir_clone, &driver, options).await {
            Ok(_paths) => {
                // Return True to indicate success
                Ok(true)
            }
            Err(err) => Err(PyRattlerError::InstallError(err).into()),
        }
    })
}

/// Unlinks a package from a prefix.
///
/// Args:
///     target_prefix: Path to the environment prefix
///     prefix_record: Prefix record for the package to unlink
///
/// Returns:
///     None
#[pyfunction]
pub fn py_unlink_package<'a>(
    py: Python<'a>,
    target_prefix: PathBuf,
    prefix_record: Bound<'a, PyAny>,
) -> PyResult<Bound<'a, PyAny>> {
    // Convert PyAny to PrefixRecord
    let record: PrefixRecord = PyRecord::try_from(prefix_record)?.try_into()?;

    let target_prefix_clone = target_prefix.clone();
    let record_clone = record.clone();

    future_into_py(py, async move {
        match unlink_package(&target_prefix_clone, &record_clone).await {
            Ok(_) => Ok(true),
            Err(err) => Err(PyRattlerError::UnlinkError(err).into()),
        }
    })
}

/// Empties the trash directory in the prefix.
///
/// Args:
///     target_prefix: Path to the environment prefix
///
/// Returns:
///     None
#[pyfunction]
pub fn py_empty_trash<'a>(py: Python<'a>, target_prefix: PathBuf) -> PyResult<Bound<'a, PyAny>> {
    let target_prefix_clone = target_prefix.clone();

    future_into_py(py, async move {
        match empty_trash(&target_prefix_clone).await {
            Ok(_) => Ok(true),
            Err(err) => Err(PyRattlerError::UnlinkError(err).into()),
        }
    })
}
