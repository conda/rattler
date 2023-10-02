use pyo3::{pyfunction, PyAny, PyResult, Python};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};

use crate::{
    error::PyRattlerError,
    generic_virtual_package::PyGenericVirtualPackage,
    match_spec::PyMatchSpec,
    repo_data::{repo_data_record::PyRepoDataRecord, sparse::PySparseRepoData},
};

#[pyfunction]
pub fn py_solve(
    py: Python<'_>,
    specs: Vec<PyMatchSpec>,
    available_packages: Vec<&'_ PyAny>,
    locked_packages: Vec<PyRepoDataRecord>,
    pinned_packages: Vec<PyRepoDataRecord>,
    virtual_packages: Vec<PyGenericVirtualPackage>,
) -> PyResult<Vec<PyRepoDataRecord>> {
    let packages = available_packages
        .iter()
        .map(|&pkg| TryInto::<PySparseRepoData>::try_into(pkg))
        .collect::<PyResult<Vec<_>>>()?;

    let package_names = specs
        .iter()
        .filter_map(|match_spec| match_spec.inner.name.clone());

    // pure rust operation, release gil to allow python threads to do other work
    let available_packages = py.allow_threads(move || {
        SparseRepoData::load_records_recursive(packages.iter().map(Into::into), package_names, None)
    })?;

    let task = SolverTask {
        available_packages: &available_packages,
        locked_packages: locked_packages.into_iter().map(Into::into).collect(),
        pinned_packages: pinned_packages.into_iter().map(Into::into).collect(),
        virtual_packages: virtual_packages.into_iter().map(Into::into).collect(),
        specs: specs.into_iter().map(Into::into).collect(),
    };

    // pure rust operation, release gil to allow python threads to do other work
    py.allow_threads(move || {
        Ok(Solver
            .solve(task)
            .map(|res| {
                res.into_iter()
                    .map(Into::into)
                    .collect::<Vec<PyRepoDataRecord>>()
            })
            .map_err(PyRattlerError::from)?)
    })
}
