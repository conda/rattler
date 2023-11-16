use pyo3::{exceptions::PyTypeError, intern, pyclass, pymethods, FromPyObject, PyAny, PyErr};
use rattler_conda_types::RepoDataRecord;

use super::package_record::PyPackageRecord;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyRepoDataRecord {
    pub(crate) inner: RepoDataRecord,
}

impl From<PyRepoDataRecord> for RepoDataRecord {
    fn from(value: PyRepoDataRecord) -> Self {
        value.inner
    }
}

impl From<RepoDataRecord> for PyRepoDataRecord {
    fn from(value: RepoDataRecord) -> Self {
        Self { inner: value }
    }
}

impl<'a> TryFrom<&'a PyAny> for PyRepoDataRecord {
    type Error = PyErr;
    fn try_from(value: &'a PyAny) -> Result<Self, Self::Error> {
        let intern_val = intern!(value.py(), "_record");
        if !value.hasattr(intern_val)? {
            return Err(PyTypeError::new_err(
                "object is not an instance of 'RepoDataRecord'",
            ));
        }

        let inner = value.getattr(intern_val)?;
        if !inner.is_instance_of::<Self>() {
            return Err(PyTypeError::new_err("'_record' is invalid"));
        }

        PyRepoDataRecord::extract(inner)
    }
}

#[pymethods]
impl PyRepoDataRecord {
    /// The data stored in the repodata.json.
    #[getter]
    pub fn package_record(&self) -> PyPackageRecord {
        self.inner.package_record.clone().into()
    }

    /// Returns a string representation of PyRepoDataRecord.
    pub fn as_str(&self) -> String {
        format!("{:?}", self.inner)
    }
}
