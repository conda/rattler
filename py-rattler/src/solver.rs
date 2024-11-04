use chrono::DateTime;
use pyo3::{
    exceptions::PyValueError, pyfunction, Bound, FromPyObject, PyAny, PyErr, PyResult, Python,
};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo::Solver, RepoDataIter, SolveStrategy, SolverImpl, SolverTask};
use std::sync::Arc;
use tokio::task::JoinError;

use crate::{
    channel::{PyChannel, PyChannelPriority},
    error::PyRattlerError,
    generic_virtual_package::PyGenericVirtualPackage,
    match_spec::PyMatchSpec,
    platform::PyPlatform,
    record::PyRecord,
    repo_data::gateway::PyGateway,
    PySparseRepoData, Wrap,
};

impl<'py> FromPyObject<'py> for Wrap<SolveStrategy> {
    fn extract_bound(ob: &Bound<'py, PyAny>) -> PyResult<Self> {
        let parsed = match <&'py str>::extract_bound(ob)? {
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

#[allow(clippy::too_many_arguments)]
#[pyfunction]
#[pyo3(signature = (channels, platforms, specs, constraints, gateway, locked_packages, pinned_packages, virtual_packages, channel_priority, timeout=None, exclude_newer_timestamp_ms=None, strategy=None)
)]
pub fn py_solve(
    py: Python<'_>,
    channels: Vec<PyChannel>,
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
) -> PyResult<Bound<'_, PyAny>> {
    future_into_py(py, async move {
        let available_packages = gateway
            .inner
            .query(
                channels.into_iter(),
                platforms.into_iter().map(Into::into),
                specs.clone().into_iter(),
            )
            .recursive(true)
            .execute()
            .await
            .map_err(PyRattlerError::from)?;

        let exclude_newer = exclude_newer_timestamp_ms.and_then(DateTime::from_timestamp_millis);

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
            };

            Ok::<_, PyErr>(
                Solver
                    .solve(task)
                    .map(|res| res.into_iter().map(Into::into).collect::<Vec<PyRecord>>())
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
#[pyo3(signature = (specs, sparse_repodata, constraints, locked_packages, pinned_packages, virtual_packages, channel_priority, timeout=None, exclude_newer_timestamp_ms=None, strategy=None)
)]
pub fn py_solve_with_sparse_repodata(
    py: Python<'_>,
    specs: Vec<PyMatchSpec>,
    sparse_repodata: Vec<PySparseRepoData>,
    constraints: Vec<PyMatchSpec>,
    locked_packages: Vec<PyRecord>,
    pinned_packages: Vec<PyRecord>,
    virtual_packages: Vec<PyGenericVirtualPackage>,
    channel_priority: PyChannelPriority,
    timeout: Option<u64>,
    exclude_newer_timestamp_ms: Option<i64>,
    strategy: Option<Wrap<SolveStrategy>>,
) -> PyResult<Bound<'_, PyAny>> {
    future_into_py(py, async move {
        let exclude_newer = exclude_newer_timestamp_ms.and_then(DateTime::from_timestamp_millis);

        let sparse_repodata = sparse_repodata
            .into_iter()
            .map(|s| s.inner.clone())
            .collect::<Vec<_>>();

        let solve_result = tokio::task::spawn_blocking(move || {
            let package_names = specs
                .iter()
                .filter_map(|match_spec| match_spec.inner.name.clone());

            let available_packages = SparseRepoData::load_records_recursive(
                sparse_repodata.iter().map(Arc::as_ref),
                package_names,
                None,
            )?;

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
            };

            Ok::<_, PyErr>(
                Solver
                    .solve(task)
                    .map(|res| res.into_iter().map(Into::into).collect::<Vec<PyRecord>>())
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
