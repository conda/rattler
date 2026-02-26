use std::collections::HashSet;

use chrono::DateTime;
use pyo3::{
    exceptions::PyValueError, pybacked::PyBackedStr, pyclass, pyfunction, pymethods,
    types::PyAnyMethods, Bound, FromPyObject, PyAny, PyErr, PyResult, Python,
};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{
    resolvo::Solver, MinimumAgeConfig, RepoDataIter, SolveStrategy, SolverImpl, SolverTask,
};
use tokio::task::JoinError;

use crate::{
    channel::PyChannelPriority,
    error::PyRattlerError,
    generic_virtual_package::PyGenericVirtualPackage,
    match_spec::PyMatchSpec,
    package_name::PyPackageName,
    platform::PyPlatform,
    record::PyRecord,
    repo_data::gateway::{py_object_to_source, PyGateway},
    PyPackageFormatSelection, PySparseRepoData, Wrap,
};

impl<'py> FromPyObject<'py> for Wrap<SolveStrategy> {
    fn extract_bound(ob: &Bound<'py, PyAny>) -> PyResult<Self> {
        let parsed: PyBackedStr = ob.extract()?;
        let parsed = match parsed.as_ref() {
            "highest" => SolveStrategy::Highest,
            "lowest" => SolveStrategy::LowestVersion,
            "lowest-direct" => SolveStrategy::LowestVersionDirect,
            v => {
                return Err(PyValueError::new_err(format!(
                    "cache action must be one of {{'highest', 'lowest', 'lowest-direct'}}, got {v}",
                )))
            }
        };
        Ok(Wrap(parsed))
    }
}

/// Configuration for minimum package age filtering.
///
/// This helps reduce the risk of installing compromised packages by delaying
/// the installation of newly published versions. In most cases, malicious
/// releases are discovered and removed from channels within an hour.
#[pyclass]
#[derive(Clone)]
pub struct PyMinimumAgeConfig {
    pub(crate) inner: MinimumAgeConfig,
}

#[pymethods]
impl PyMinimumAgeConfig {
    /// Create a new minimum age configuration.
    ///
    /// Args:
    ///     seconds: The minimum age in seconds that a package must have been
    ///         published before it can be installed.
    ///     exempt_packages: Optional list of package names that are exempt
    ///         from the minimum age requirement.
    ///     include_unknown_timestamp: Whether to include packages without a
    ///         timestamp. Defaults to False (exclude them).
    #[new]
    #[pyo3(signature = (seconds, exempt_packages=None, include_unknown_timestamp=false))]
    pub fn new(
        seconds: u64,
        exempt_packages: Option<Vec<PyPackageName>>,
        include_unknown_timestamp: bool,
    ) -> Self {
        let mut config = MinimumAgeConfig::new(std::time::Duration::from_secs(seconds));
        if let Some(exempt) = exempt_packages {
            let exempt_set: HashSet<PackageName> = exempt.into_iter().map(Into::into).collect();
            config = config.with_exempt_packages(exempt_set);
        }
        config = config.with_include_unknown_timestamp(include_unknown_timestamp);
        Self { inner: config }
    }

    /// The minimum age in seconds.
    #[getter]
    pub fn seconds(&self) -> u64 {
        self.inner.min_age.as_secs()
    }

    /// The list of exempt package names.
    #[getter]
    pub fn exempt_packages(&self) -> Vec<PyPackageName> {
        self.inner
            .exempt_packages
            .iter()
            .cloned()
            .map(Into::into)
            .collect()
    }

    /// Whether packages without a timestamp are included.
    #[getter]
    pub fn include_unknown_timestamp(&self) -> bool {
        self.inner.include_unknown_timestamp
    }
}

#[allow(clippy::too_many_arguments)]
#[pyfunction]
#[pyo3(signature = (sources, platforms, specs, constraints, gateway, locked_packages, pinned_packages, virtual_packages, channel_priority, timeout=None, exclude_newer_timestamp_ms=None, strategy=None, min_age=None)
)]
pub fn py_solve<'a>(
    py: Python<'a>,
    sources: Vec<Bound<'a, PyAny>>,
    platforms: Vec<PyPlatform>,
    specs: Vec<PyMatchSpec>,
    constraints: Vec<PyMatchSpec>,
    gateway: PyGateway,
    locked_packages: Vec<PyRecord>,
    pinned_packages: Vec<PyRecord>,
    virtual_packages: Vec<PyGenericVirtualPackage>,
    channel_priority: PyChannelPriority,
    timeout: Option<u64>,
    exclude_newer_timestamp_ms: Option<i64>,
    strategy: Option<Wrap<SolveStrategy>>,
    min_age: Option<PyMinimumAgeConfig>,
) -> PyResult<Bound<'a, PyAny>> {
    // Convert Python sources to Rust Source enum
    let rust_sources: Vec<rattler_repodata_gateway::Source> = sources
        .into_iter()
        .map(py_object_to_source)
        .collect::<PyResult<_>>()?;

    future_into_py(py, async move {
        let available_packages = gateway
            .inner
            .query(
                rust_sources,
                platforms.into_iter().map(Into::into),
                specs.clone().into_iter(),
            )
            .recursive(true)
            .execute()
            .await
            .map_err(PyRattlerError::from)?;

        let exclude_newer = exclude_newer_timestamp_ms.and_then(DateTime::from_timestamp_millis);
        let min_age = min_age.map(|config| config.inner);

        let solve_result = tokio::task::spawn_blocking(move || {
            let task = SolverTask {
                available_packages: available_packages
                    .iter()
                    .map(RepoDataIter)
                    .collect::<Vec<_>>(),
                locked_packages: locked_packages
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<PyResult<Vec<_>>>()?,
                pinned_packages: pinned_packages
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<PyResult<Vec<_>>>()?,
                virtual_packages: virtual_packages.into_iter().map(Into::into).collect(),
                specs: specs.into_iter().map(Into::into).collect(),
                constraints: constraints.into_iter().map(Into::into).collect(),
                timeout: timeout.map(std::time::Duration::from_micros),
                channel_priority: channel_priority.into(),
                exclude_newer,
                min_age,
                strategy: strategy.map_or_else(Default::default, |v| v.0),
            };

            Ok::<_, PyErr>(
                Solver
                    .solve(task)
                    .map(|res| {
                        res.records
                            .into_iter()
                            .map(Into::into)
                            .collect::<Vec<PyRecord>>()
                    })
                    .map_err(PyRattlerError::from)?,
            )
        })
        .await;

        match solve_result.map_err(JoinError::try_into_panic) {
            Ok(solve_result) => Ok(solve_result?),
            Err(Ok(payload)) => std::panic::resume_unwind(payload),
            Err(Err(_err)) => Err(PyRattlerError::IoError(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "solver task was cancelled",
            )))?,
        }
    })
}

#[allow(clippy::too_many_arguments)]
#[pyfunction]
#[pyo3(signature = (specs, sparse_repodata, constraints, locked_packages, pinned_packages, virtual_packages, channel_priority, package_format_selection, timeout=None, exclude_newer_timestamp_ms=None, strategy=None, min_age=None)
)]
pub fn py_solve_with_sparse_repodata<'py>(
    py: Python<'py>,
    specs: Vec<PyMatchSpec>,
    sparse_repodata: Vec<Bound<'py, PySparseRepoData>>,
    constraints: Vec<PyMatchSpec>,
    locked_packages: Vec<PyRecord>,
    pinned_packages: Vec<PyRecord>,
    virtual_packages: Vec<PyGenericVirtualPackage>,
    channel_priority: PyChannelPriority,
    package_format_selection: PyPackageFormatSelection,
    timeout: Option<u64>,
    exclude_newer_timestamp_ms: Option<i64>,
    strategy: Option<Wrap<SolveStrategy>>,
    min_age: Option<PyMinimumAgeConfig>,
) -> PyResult<Bound<'py, PyAny>> {
    // Acquire read locks on the SparseRepoData instances. This allows us to safely access the
    // object in another thread.
    let repo_data_locks = sparse_repodata
        .into_iter()
        .map(|s| s.borrow().inner.read_arc())
        .collect::<Vec<_>>();

    future_into_py(py, async move {
        let exclude_newer = exclude_newer_timestamp_ms.and_then(DateTime::from_timestamp_millis);
        let min_age = min_age.map(|config| config.inner);

        let solve_result = tokio::task::spawn_blocking(move || {
            // Ensure that all the SparseRepoData instances are still valid, e.g. not closed.
            let repo_data_refs = repo_data_locks
                .iter()
                .map(|s| {
                    s.as_ref()
                        .ok_or_else(|| PyValueError::new_err("I/O operation on closed file."))
                })
                .collect::<Result<Vec<_>, _>>()?;

            let package_names = specs
                .iter()
                .filter_map(|match_spec| match_spec.inner.name.clone().into_exact());

            let available_packages = SparseRepoData::load_records_recursive(
                repo_data_refs,
                package_names,
                None,
                package_format_selection.into(),
            )?;

            // Force drop the locks to avoid holding them longer than necessary.
            drop(repo_data_locks);

            let task = SolverTask {
                available_packages: available_packages
                    .iter()
                    .map(RepoDataIter)
                    .collect::<Vec<_>>(),
                locked_packages: locked_packages
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<PyResult<Vec<_>>>()?,
                pinned_packages: pinned_packages
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<PyResult<Vec<_>>>()?,
                virtual_packages: virtual_packages.into_iter().map(Into::into).collect(),
                specs: specs.into_iter().map(Into::into).collect(),
                constraints: constraints.into_iter().map(Into::into).collect(),
                timeout: timeout.map(std::time::Duration::from_micros),
                channel_priority: channel_priority.into(),
                exclude_newer,
                min_age,
                strategy: strategy.map_or_else(Default::default, |v| v.0),
            };

            Ok::<_, PyErr>(
                Solver
                    .solve(task)
                    .map(|res| {
                        res.records
                            .into_iter()
                            .map(Into::into)
                            .collect::<Vec<PyRecord>>()
                    })
                    .map_err(PyRattlerError::from)?,
            )
        })
        .await;

        match solve_result.map_err(JoinError::try_into_panic) {
            Ok(solve_result) => Ok(solve_result?),
            Err(Ok(payload)) => std::panic::resume_unwind(payload),
            Err(Err(_err)) => Err(PyRattlerError::IoError(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "solver task was cancelled",
            )))?,
        }
    })
}
