use std::{path::PathBuf, sync::Arc};

use pyo3::{pyclass, pymethods, Bound, PyResult, Python};

use rattler_repodata_gateway::sparse::SparseRepoData;

use crate::channel::PyChannel;
use crate::package_name::PyPackageName;
use crate::record::PyRecord;
use parking_lot::RwLock;
use pyo3::exceptions::PyValueError;

#[pyclass]
pub struct PySparseRepoData {
    // `SparseRepoData` holds a memory-mapped view on a file which prevents the file from being
    // modified or deleted. We want to be able to close this view on demand. But we also want to
    // move shared ownership into different threads to unblock the GIL.
    //
    // We wrap the `SparseRepoData` in an Option to indicate whether it's "open" or not. Closing
    // simply means taking the value from the option and dropping it. This construct is wrapped
    // in a RwLock because most of the time we just want to be able to read from it. We only
    // need write access to close it.
    //
    // This whole thing is then wrapped in an Arc so we can share this with a background thread
    // without blocking the GIL.
    pub(crate) inner: Arc<RwLock<Option<SparseRepoData>>>,
    subdir: String,
}

impl From<SparseRepoData> for PySparseRepoData {
    fn from(value: SparseRepoData) -> Self {
        Self {
            subdir: value.subdir().to_owned(),
            inner: Arc::new(RwLock::new(Some(value))),
        }
    }
}

#[pymethods]
impl PySparseRepoData {
    #[new]
    pub fn new(channel: PyChannel, subdir: String, path: PathBuf) -> PyResult<Self> {
        Ok(SparseRepoData::from_file(channel.into(), subdir, path, None)?.into())
    }

    pub fn package_names(&self) -> PyResult<Vec<String>> {
        let lock = self.inner.read();
        let Some(sparse) = lock.as_ref() else {
            return Err(PyValueError::new_err("I/O operation on closed file."));
        };
        Ok(sparse.package_names().map(Into::into).collect::<Vec<_>>())
    }

    pub fn load_records(&self, package_name: &PyPackageName) -> PyResult<Vec<PyRecord>> {
        let lock = self.inner.read();
        let Some(sparse) = lock.as_ref() else {
            return Err(PyValueError::new_err("I/O operation on closed file."));
        };
        Ok(sparse
            .load_records(&package_name.inner)?
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>())
    }

    #[getter]
    pub fn subdir(&self) -> String {
        self.subdir.clone()
    }

    pub fn close(&self) {
        self.inner.write().take();
    }

    #[staticmethod]
    pub fn load_records_recursive<'py>(
        py: Python<'py>,
        repo_data: Vec<Bound<'py, PySparseRepoData>>,
        package_names: Vec<PyPackageName>,
    ) -> PyResult<Vec<Vec<PyRecord>>> {
        // Acquire read locks on the SparseRepoData instances. This allows us to safely access the
        // object in another thread.
        let repo_data_locks = repo_data
            .into_iter()
            .map(|s| s.borrow().inner.read_arc())
            .collect::<Vec<_>>();

        // Ensure that all the SparseRepoData instances are still valid, e.g. not closed.
        let repo_data_refs = repo_data_locks
            .iter()
            .map(|s| {
                s.as_ref()
                    .ok_or_else(|| PyValueError::new_err("I/O operation on closed file."))
            })
            .collect::<Result<Vec<_>, _>>()?;

        py.allow_threads(move || {
            let package_names = package_names.into_iter().map(Into::into);
            Ok(
                SparseRepoData::load_records_recursive(repo_data_refs, package_names, None)?
                    .into_iter()
                    .map(|v| v.into_iter().map(Into::into).collect::<Vec<_>>())
                    .collect::<Vec<_>>(),
            )
        })
    }
}
