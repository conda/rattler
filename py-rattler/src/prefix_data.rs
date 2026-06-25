use pyo3::{exceptions::PyValueError, pyclass, pymethods, PyResult};
use rattler_conda_types::{PackageName, PrefixData};

use crate::{error::PyRattlerError, package_name::PyPackageName, record::PyRecord};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyPrefixData {
    pub(crate) inner: PrefixData,
}

impl From<PyPrefixData> for PrefixData {
    fn from(value: PyPrefixData) -> Self {
        value.inner
    }
}
impl From<PrefixData> for PyPrefixData {
    fn from(value: PrefixData) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyPrefixData {
    #[new]
    #[pyo3(signature = (prefix_path))]
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn __init__(prefix_path: &str) -> PyResult<Self> {
        Ok(PrefixData::new(prefix_path)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
    }

    /// Get the record for a given package name
    pub fn get(&self, package_name: PyPackageName) -> PyResult<Option<PyRecord>> {
        let pkg_name = PackageName::from(package_name);
        let record = self.inner.get(&pkg_name);

        match record {
            Some(Ok(record)) => Ok(Some(record.clone().into())),
            // TODO: Expose underlying error
            Some(Err(_)) => Err(PyValueError::new_err("Could not process record")),
            None => Ok(None),
        }
    }
}
