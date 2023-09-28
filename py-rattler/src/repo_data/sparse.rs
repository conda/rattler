use std::{path::PathBuf, sync::Arc};

use pyo3::{pyclass, pymethods, PyAny, PyResult, Python};

use rattler_repodata_gateway::sparse::SparseRepoData;

use crate::channel::PyChannel;
use crate::package_name::PyPackageName;
use crate::repo_data::repo_data_record::PyRepoDataRecord;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PySparseRepoData {
    pub(crate) inner: Arc<SparseRepoData>,
}

impl From<SparseRepoData> for PySparseRepoData {
    fn from(value: SparseRepoData) -> Self {
        Self {
            inner: Arc::new(value),
        }
    }
}

#[pymethods]
impl PySparseRepoData {
    #[new]
    pub fn new(
        channel: PyChannel,
        subdir: String,
        path: PathBuf,
        patch_func: Option<&PyAny>,
    ) -> PyResult<Self> {
        // TODO: implement a way to pass patch function from python
        let _ = patch_func;
        // let patch_func = if let Some(func) = patch_func {
        //     Some(move |package: &mut PackageRecord| {
        //         Python::with_gil(|py| {
        //             let args = PyTuple::new(py, []);
        //             func.to_object(py)
        //                 .call1(py, args)
        //                 .expect("Callback failed!");
        //         });
        //     })
        // } else {
        //     None
        // };
        Ok(SparseRepoData::new(channel.into(), subdir, path, None)?.into())
    }

    pub fn package_names(&self) -> Vec<String> {
        self.inner
            .package_names()
            .map(Into::into)
            .collect::<Vec<_>>()
    }

    pub fn load_records(&self, package_name: &PyPackageName) -> PyResult<Vec<PyRepoDataRecord>> {
        Ok(self
            .inner
            .load_records(&package_name.inner)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>())
    }

    #[getter]
    pub fn subdir(&self) -> String {
        self.inner.subdir().into()
    }

    #[staticmethod]
    pub fn load_records_recursive(
        py: Python<'_>,
        repo_data: Vec<PySparseRepoData>,
        package_names: Vec<PyPackageName>,
        patch_func: Option<&PyAny>,
    ) -> PyResult<Vec<Vec<PyRepoDataRecord>>> {
        let repo_data = repo_data.iter().map(|r| r.inner.as_ref());

        let package_names = package_names.into_iter().map(Into::into);
        // TODO: implement a way to pass patch function from python
        let _ = patch_func;

        // release gil to allow other threads to progress
        py.allow_threads(move || {
            Ok(
                SparseRepoData::load_records_recursive(repo_data, package_names, None)?
                    .into_iter()
                    .map(|v| v.into_iter().map(Into::into).collect::<Vec<_>>())
                    .collect::<Vec<_>>(),
            )
        })
    }
}

/*
    data = fetch(cache_path, [channel], [platform], progress_func) -> [RepoData] -> Repodata { inner: SparseRepoData }
    records = solve_from_data(data, [match_spec], [generic_virtual_packages])
    records = solve_from_path(cache_path, [match_spec], [channel], [platform])
*/
