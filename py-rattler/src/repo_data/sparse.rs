use std::{path::PathBuf, sync::Arc};

use pyo3::{pyclass, pymethods, PyResult, Python};

use rattler_repodata_gateway::sparse::{PackageFormatSelection, SparseRepoData};

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
#[derive(Copy, Clone, PartialEq)]
pub enum PyPackageFormatSelection {
    OnlyTarBz2,
    OnlyConda,
    PreferConda,
    Both,
}

impl Default for PyPackageFormatSelection {
    fn default() -> Self {
        PackageFormatSelection::default().into()
    }
}

impl From<PackageFormatSelection> for PyPackageFormatSelection {
    fn from(value: PackageFormatSelection) -> Self {
        match value {
            PackageFormatSelection::OnlyTarBz2 => PyPackageFormatSelection::OnlyTarBz2,
            PackageFormatSelection::OnlyConda => PyPackageFormatSelection::OnlyConda,
            PackageFormatSelection::PreferConda => PyPackageFormatSelection::PreferConda,
            PackageFormatSelection::Both => PyPackageFormatSelection::Both,
        }
    }
}

impl From<PyPackageFormatSelection> for PackageFormatSelection {
    fn from(value: PyPackageFormatSelection) -> Self {
        match value {
            PyPackageFormatSelection::OnlyTarBz2 => PackageFormatSelection::OnlyTarBz2,
            PyPackageFormatSelection::OnlyConda => PackageFormatSelection::OnlyConda,
            PyPackageFormatSelection::PreferConda => PackageFormatSelection::PreferConda,
            PyPackageFormatSelection::Both => PackageFormatSelection::Both,
        }
    }
}

#[pymethods]
impl PyPackageFormatSelection {
    fn __repr__(&self) -> &'static str {
        PackageFormatSelection::from(*self).into()
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
        package_format_selection: PyPackageFormatSelection,
    ) -> PyResult<Vec<PyRecord>> {
        Ok(self
            .inner
            .load_records(&package_name.inner, package_format_selection.into())?
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
        package_format_selection: PyPackageFormatSelection,
    ) -> PyResult<Vec<Vec<PyRecord>>> {
        py.allow_threads(move || {
            let repo_data = repo_data.iter().map(Into::into);
            let package_names = package_names.into_iter().map(Into::into);
            Ok(SparseRepoData::load_records_recursive(
                repo_data,
                package_names,
                None,
                package_format_selection.into(),
            )?
            .into_iter()
            .map(|v| v.into_iter().map(Into::into).collect::<Vec<_>>())
            .collect::<Vec<_>>())
        })
    }
}
