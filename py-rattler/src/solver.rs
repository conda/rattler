use pyo3::{pyfunction, PyResult};
use rattler_conda_types::RepoDataRecord;
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};

use crate::{
    error::PyRattlerError, generic_virtual_package::PyGenericVirtualPackage,
    match_spec::PyMatchSpec, repo_data::repo_data_record::PyRepoDataRecord,
};

#[pyfunction]
pub fn py_solve(
    specs: Vec<PyMatchSpec>,
    available_packages: Vec<Vec<PyRepoDataRecord>>,
    locked_packages: Vec<PyRepoDataRecord>,
    pinned_packages: Vec<PyRepoDataRecord>,
    virtual_packages: Vec<PyGenericVirtualPackage>,
) -> PyResult<Vec<PyRepoDataRecord>> {
    let available_packages = available_packages
        .into_iter()
        .map(|records| {
            records
                .into_iter()
                .map(Into::<RepoDataRecord>::into)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let task = SolverTask {
        available_packages: &available_packages,
        locked_packages: locked_packages.into_iter().map(Into::into).collect(),
        pinned_packages: pinned_packages.into_iter().map(Into::into).collect(),
        virtual_packages: virtual_packages.into_iter().map(Into::into).collect(),
        specs: specs.into_iter().map(Into::into).collect(),
    };

    Ok(Solver
        .solve(task)
        .map(|res| {
            res.into_iter()
                .map(Into::into)
                .collect::<Vec<PyRepoDataRecord>>()
        })
        .map_err(PyRattlerError::from)?)
}
