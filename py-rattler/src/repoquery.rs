use std::collections::HashMap;

use pyo3::prelude::PyAnyMethods;
use pyo3::{Bound, PyAny, PyResult, exceptions::PyTypeError, pyclass, pyfunction, pymethods};
use rattler_conda_types::RepoDataRecord;
use rattler_repodata_gateway::repoquery::{
    DependencyKind, OwnedDependent, RunExportKind, WhoNeedsTarget, who_needs,
};

use crate::{
    generic_virtual_package::PyGenericVirtualPackage, package_name::PyPackageName, record::PyRecord,
};

/// A record that references the package queried through `py_who_needs`.
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PyDependent {
    /// The record that references the queried package.
    #[pyo3(get)]
    pub record: PyRecord,

    /// The dependency string through which the record references the
    /// queried package.
    #[pyo3(get)]
    pub dependency: String,

    /// The field of the record the dependency comes from:
    /// `depends`, `constrains`, or `run_export`.
    #[pyo3(get)]
    pub kind: String,

    /// The run export field for `run_export` kinds: `weak`, `strong`,
    /// `noarch`, `weak_constrains`, or `strong_constrains`.
    #[pyo3(get)]
    pub run_export_kind: Option<String>,
}

#[pymethods]
impl PyDependent {
    fn __repr__(&self) -> String {
        format!(
            "Dependent(record={}, dependency={:?}, kind={:?})",
            self.record.as_package_record().name.as_normalized(),
            self.dependency,
            self.kind,
        )
    }
}

impl From<OwnedDependent> for PyDependent {
    fn from(dependent: OwnedDependent) -> Self {
        let (kind, run_export_kind) = split_kind(dependent.kind);
        Self {
            record: PyRecord::from(dependent.record),
            dependency: dependent.dependency,
            kind: kind.to_string(),
            run_export_kind: run_export_kind.map(String::from),
        }
    }
}

fn split_kind(kind: DependencyKind) -> (&'static str, Option<&'static str>) {
    match kind {
        DependencyKind::Depends => ("depends", None),
        DependencyKind::Constrains => ("constrains", None),
        DependencyKind::RunExport(run_export) => (
            "run_export",
            Some(match run_export {
                RunExportKind::Weak => "weak",
                RunExportKind::Strong => "strong",
                RunExportKind::Noarch => "noarch",
                RunExportKind::WeakConstrains => "weak_constrains",
                RunExportKind::StrongConstrains => "strong_constrains",
            }),
        ),
    }
}

/// Extracts a [`WhoNeedsTarget`] from a Python object that is either a
/// `PyPackageName`, a `PyGenericVirtualPackage`, or a `PyRecord`.
pub fn extract_who_needs_target(target: &Bound<'_, PyAny>) -> PyResult<WhoNeedsTarget> {
    if let Ok(name) = target.extract::<PyPackageName>() {
        Ok(name.inner.into())
    } else if let Ok(virtual_package) = target.extract::<PyGenericVirtualPackage>() {
        Ok(virtual_package.inner.into())
    } else if let Ok(record) = target.extract::<PyRecord>() {
        Ok(record.as_package_record().clone().into())
    } else {
        Err(PyTypeError::new_err(
            "expected a PackageName, PackageRecord, or GenericVirtualPackage as the target",
        ))
    }
}

#[pyfunction]
pub fn py_who_needs(
    records: Vec<PyRecord>,
    target: &Bound<'_, PyAny>,
) -> PyResult<Vec<PyDependent>> {
    let target = extract_who_needs_target(target)?;

    let repodata_records = records
        .iter()
        .map(PyRecord::try_as_repodata_record)
        .collect::<PyResult<Vec<&RepoDataRecord>>>()?;

    // `who_needs` hands back references into `repodata_records`; map them
    // back to the input `PyRecord`s by pointer identity so the results
    // share the records passed in instead of deep copies.
    let record_by_ptr: HashMap<*const RepoDataRecord, &PyRecord> = repodata_records
        .iter()
        .map(|record| std::ptr::from_ref(*record))
        .zip(records.iter())
        .collect();

    Ok(who_needs(repodata_records.iter().copied(), target)
        .into_iter()
        .map(|dependent| {
            let (kind, run_export_kind) = split_kind(dependent.kind);
            PyDependent {
                record: (*record_by_ptr[&std::ptr::from_ref(dependent.record)]).clone(),
                dependency: dependent.dependency.to_string(),
                kind: kind.to_string(),
                run_export_kind: run_export_kind.map(String::from),
            }
        })
        .collect())
}
