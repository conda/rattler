use pyo3::{pyfunction, PyResult, Python};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};

use crate::{
    error::PyRattlerError, generic_virtual_package::PyGenericVirtualPackage,
    match_spec::PyMatchSpec, record::PyRecord, repo_data::sparse::PySparseRepoData,
};

#[pyfunction]
pub fn py_solve(
    py: Python<'_>,
    specs: Vec<PyMatchSpec>,
    available_packages: Vec<PySparseRepoData>,
    locked_packages: Vec<PyRecord>,
    pinned_packages: Vec<PyRecord>,
    virtual_packages: Vec<PyGenericVirtualPackage>,
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

        Ok(Solver
            .solve(task)
            .map(|res| res.into_iter().map(Into::into).collect::<Vec<PyRecord>>())
            .map_err(PyRattlerError::from)?)
    })
}
