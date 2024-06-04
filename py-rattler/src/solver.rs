use chrono::DateTime;
use pyo3::exceptions::PyValueError;
use pyo3::{pyfunction, FromPyObject, PyAny, PyErr, PyResult, Python};
use pyo3_asyncio::tokio::future_into_py;
use rattler_solve::{resolvo::Solver, RepoDataIter, SolveStrategy, SolverImpl, SolverTask};
use tokio::task::JoinError;

use crate::channel::PyChannel;
use crate::platform::PyPlatform;
use crate::repo_data::gateway::PyGateway;
use crate::{
    channel::PyChannelPriority, error::PyRattlerError,
    generic_virtual_package::PyGenericVirtualPackage, match_spec::PyMatchSpec, record::PyRecord,
    Wrap,
};

impl FromPyObject<'_> for Wrap<SolveStrategy> {
    fn extract(ob: &'_ PyAny) -> PyResult<Self> {
        let parsed = match &*ob.extract::<String>()? {
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
) -> PyResult<&'_ PyAny> {
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
