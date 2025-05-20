use std::{path::PathBuf, sync::Arc};

use pyo3::{pyclass, pymethods, PyResult, Python};

use rattler_repodata_gateway::sparse::{SparseRepoData, VariantSelection};

use crate::channel::PyChannel;
use crate::package_name::PyPackageName;
use crate::record::PyRecord;

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

#[pyclass(eq)]
#[derive(Copy, Clone, PartialEq, Default)]
pub enum PyVariantSelection {
    OnlyTarBz2,
    OnlyConda,
    #[default]
    PreferConda,
    Both,
}

impl From<PyVariantSelection> for VariantSelection {
    fn from(value: PyVariantSelection) -> Self {
        match value {
            PyVariantSelection::OnlyTarBz2 => VariantSelection::OnlyTarBz2,
            PyVariantSelection::OnlyConda => VariantSelection::OnlyConda,
            PyVariantSelection::PreferConda => VariantSelection::PreferConda,
            PyVariantSelection::Both => VariantSelection::Both,
        }
    }
}

#[pymethods]
impl PyVariantSelection {
    fn __repr__(&self) -> &'static str {
        VariantSelection::from(*self).into()
    }
}

#[pymethods]
impl PySparseRepoData {
    #[new]
    pub fn new(channel: PyChannel, subdir: String, path: PathBuf) -> PyResult<Self> {
        Ok(SparseRepoData::from_file(channel.into(), subdir, path, None)?.into())
    }

    pub fn package_names(&self) -> Vec<String> {
        self.inner
            .package_names()
            .map(Into::into)
            .collect::<Vec<_>>()
    }

    pub fn load_records(
        &self,
        package_name: &PyPackageName,
        variant_selection: Option<PyVariantSelection>,
    ) -> PyResult<Vec<PyRecord>> {
        Ok(self
            .inner
            .load_records(
                &package_name.inner,
                variant_selection.unwrap_or_default().into(),
            )?
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
        variant_selection: Option<PyVariantSelection>,
    ) -> PyResult<Vec<Vec<PyRecord>>> {
        py.allow_threads(move || {
            let repo_data = repo_data.iter().map(Into::into);
            let package_names = package_names.into_iter().map(Into::into);
            Ok(SparseRepoData::load_records_recursive(
                repo_data,
                package_names,
                None,
                variant_selection.unwrap_or_default().into(),
            )?
            .into_iter()
            .map(|v| v.into_iter().map(Into::into).collect::<Vec<_>>())
            .collect::<Vec<_>>())
        })
    }
}
