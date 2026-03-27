use chrono::DateTime;
use pyo3::{
    exceptions::PyValueError, pybacked::PyBackedStr, pyfunction, types::PyAnyMethods, Bound,
    FromPyObject, PyAny, PyErr, PyResult, Python,
};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{
    resolvo::Solver, ExcludeNewer, RepoDataIter, SolveStrategy, SolverImpl, SolverTask,
};
use tokio::task::JoinError;

use crate::{
    channel::PyChannelPriority,
    error::PyRattlerError,
    generic_virtual_package::PyGenericVirtualPackage,
    match_spec::PyMatchSpec,
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

fn parse_exclude_newer(
    exclude_newer_timestamp_ms: Option<i64>,
    exclude_newer_duration_seconds: Option<u64>,
) -> PyResult<Option<ExcludeNewer>> {
    match (
        exclude_newer_timestamp_ms.and_then(DateTime::from_timestamp_millis),
        exclude_newer_duration_seconds,
    ) {
        (Some(_), Some(_)) => Err(PyValueError::new_err(
            "exclude_newer timestamp and duration are mutually exclusive",
        )),
        (Some(timestamp), None) => Ok(Some(timestamp.into())),
        (None, Some(seconds)) => Ok(Some(ExcludeNewer::from_duration(
            std::time::Duration::from_secs(seconds),
        ))),
        (None, None) => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
#[pyfunction]
#[pyo3(signature = (sources, platforms, specs, constraints, gateway, locked_packages, pinned_packages, virtual_packages, channel_priority, timeout=None, exclude_newer_timestamp_ms=None, exclude_newer_duration_seconds=None, strategy=None)
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
    exclude_newer_duration_seconds: Option<u64>,
    strategy: Option<Wrap<SolveStrategy>>,
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

        let exclude_newer =
            parse_exclude_newer(exclude_newer_timestamp_ms, exclude_newer_duration_seconds)?;

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
                strategy: strategy.map_or_else(Default::default, |v| v.0),
                dependency_overrides: Vec::new(),
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
#[pyo3(signature = (specs, sparse_repodata, constraints, locked_packages, pinned_packages, virtual_packages, channel_priority, package_format_selection, timeout=None, exclude_newer_timestamp_ms=None, exclude_newer_duration_seconds=None, strategy=None)
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
    exclude_newer_duration_seconds: Option<u64>,
    strategy: Option<Wrap<SolveStrategy>>,
) -> PyResult<Bound<'py, PyAny>> {
    // Acquire read locks on the SparseRepoData instances. This allows us to safely access the
    // object in another thread.
    let repo_data_locks = sparse_repodata
        .into_iter()
        .map(|s| s.borrow().inner.read_arc())
        .collect::<Vec<_>>();

    future_into_py(py, async move {
        let exclude_newer =
            parse_exclude_newer(exclude_newer_timestamp_ms, exclude_newer_duration_seconds)?;

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
                strategy: strategy.map_or_else(Default::default, |v| v.0),
                dependency_overrides: Vec::new(),
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
