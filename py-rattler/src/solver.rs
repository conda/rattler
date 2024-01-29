use pyo3::{pyclass, pyfunction, pymethods, PyResult, Python};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo::Solver, SolverImpl, SolverOptions, SolverTask};

use crate::{
    error::PyRattlerError, generic_virtual_package::PyGenericVirtualPackage,
    match_spec::PyMatchSpec, record::PyRecord, repo_data::sparse::PySparseRepoData,
};

#[pyclass]
#[derive(Clone)]
pub struct PySolverOptions {
    pub timeout: Option<std::time::Duration>,
}

#[pymethods]
impl PySolverOptions {
    #[new]
    pub fn __init__(timeout: Option<u64>) -> Self {
        Self {
            timeout: timeout.map(std::time::Duration::from_micros),
        }
    }
}

impl From<PySolverOptions> for SolverOptions {
    fn from(py_solver_options: PySolverOptions) -> Self {
        Self {
            timeout: py_solver_options.timeout,
        }
    }
}

#[pyfunction]
pub fn py_solve(
    py: Python<'_>,
    specs: Vec<PyMatchSpec>,
    available_packages: Vec<PySparseRepoData>,
    locked_packages: Vec<PyRecord>,
    pinned_packages: Vec<PyRecord>,
    virtual_packages: Vec<PyGenericVirtualPackage>,
    solver_options: Option<PySolverOptions>,
) -> PyResult<Vec<PyRecord>> {
    py.allow_threads(move || {
        let package_names = specs
            .iter()
            .filter_map(|match_spec| match_spec.inner.name.clone());

        let available_packages = SparseRepoData::load_records_recursive(
            available_packages.iter().map(Into::into),
            package_names,
            None,
        )?;

        let task = SolverTask {
            available_packages: &available_packages,
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
        };

        let solver_options = solver_options.map(Into::into).unwrap_or_default();

        Ok(Solver
            .solve(task, &solver_options)
            .map(|res| res.into_iter().map(Into::into).collect::<Vec<PyRecord>>())
            .map_err(PyRattlerError::from)?)
    })
}
