use std::{path::PathBuf, sync::Arc};

use pyo3::{intern, pyclass, pymethods, FromPyObject, PyAny, PyResult, Python};

use rattler_repodata_gateway::sparse::SparseRepoData;

use crate::channel::PyChannel;
use crate::error::PyRattlerError;
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

impl<'a> From<&'a PySparseRepoData> for &'a SparseRepoData {
    fn from(value: &'a PySparseRepoData) -> Self {
        value.inner.as_ref()
    }
}

impl<'a> TryFrom<&'a PyAny> for PySparseRepoData {
    type Error = pyo3::PyErr;
    fn try_from(value: &'a PyAny) -> Result<Self, Self::Error> {
        let intern_val = intern!(value.py(), "_sparse");
        if !value.hasattr(intern_val)? {
            return Err(PyRattlerError::from(anyhow::anyhow!(
                "TypeError: Object is not an instance of 'SparseRepoData'"
            ))
            .into());
        }

        let inner = value.getattr(intern_val)?;
        if !inner.is_instance_of::<PySparseRepoData>() {
            return Err(
                PyRattlerError::from(anyhow::anyhow!("TypeError: '_sparse' is invalid!")).into(),
            );
        }

        PySparseRepoData::extract(inner)
    }
}

#[pymethods]
impl PySparseRepoData {
    #[new]
    pub fn new(channel: PyChannel, subdir: String, path: PathBuf) -> PyResult<Self> {
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
    ) -> PyResult<Vec<Vec<PyRepoDataRecord>>> {
        let repo_data = repo_data.iter().map(Into::into);
        let package_names = package_names.into_iter().map(Into::into);

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
