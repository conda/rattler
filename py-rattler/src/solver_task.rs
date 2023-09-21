// use pyo3::{pyclass, pymethods};
// use rattler_conda_types::RepoDataRecord;
// use rattler_solve::SolverTask;

// use crate::{generic_virtual_package::PyGenericVirtualPackage, match_spec::PyMatchSpec};

// #[pyclass]
// #[repr(transparent)]
// #[derive(Clone)]
// pub struct PySolverTask {
//     pub(crate) inner: SolverTask,
// }

// impl From<SolverTask> for PySolverTask {
//     fn from(value: SolverTask) -> Self {
//         Self { inner: value }
//     }
// }

// impl From<PySolverTask> for SolverTask {
//     fn from(value: PySolverTask) -> Self {
//         value.inner
//     }
// }

// #[pymethods]
// impl PySolverTask {
//     #[new]
//     pub fn new(
//         locked_packages: Vec<PyRepoDataRecord>,
//         pinned_packages: Vec<PyRepoDataRecord>,
//         virtual_packages: Vec<PyGenericVirtualPackage>,
//         specs: Vec<PyMatchSpec>,
//     ) -> Self {
//         Self {
//             inner: SolverTask {
//                 available_packages: Default::default(),
//                 locked_packages: locked_packages
//                     .into_iter()
//                     .map(Into::into)
//                     .collect::<Vec<_>>(),
//                 pinned_packages: pinned_packages
//                     .into_iter()
//                     .map(Into::into)
//                     .collect::<Vec<_>>(),
//                 virtual_packages: virtual_packages
//                     .into_iter()
//                     .map(Into::into)
//                     .collect::<Vec<_>>(),
//                 specs: specs.into_iter().map(Into::into).collect::<Vec<_>>(),
//             },
//         }
//     }
// }
