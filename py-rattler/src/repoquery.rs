use pyo3::prelude::PyAnyMethods;
use pyo3::{Bound, PyAny, PyResult, exceptions::PyTypeError, pyclass, pymethods};
use rattler_repodata_gateway::repoquery::{
    DependencyKind, Dependent, RunExportKind, WhoNeedsTarget,
};

use crate::{
    generic_virtual_package::PyGenericVirtualPackage, package_name::PyPackageName, record::PyRecord,
};

/// A record that references the package queried through
/// `PyGateway::who_needs`.
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
    /// `depends`, `constrains`, `extra_depends`, or `run_export`.
    #[pyo3(get)]
    pub kind: String,

    /// The run export field for `run_export` kinds: `weak`, `strong`,
    /// `noarch`, `weak_constrains`, or `strong_constrains`.
    #[pyo3(get)]
    pub run_export_kind: Option<String>,

    /// The name of the optional feature for `extra_depends` kinds; the
    /// reference only applies when that extra is enabled.
    #[pyo3(get)]
    pub extra: Option<String>,
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

impl From<Dependent> for PyDependent {
    fn from(dependent: Dependent) -> Self {
        let kind = SplitKind::from(dependent.kind);
        Self {
            record: PyRecord::from(dependent.record),
            dependency: dependent.dependency,
            kind: kind.kind.to_string(),
            run_export_kind: kind.run_export_kind.map(String::from),
            extra: kind.extra,
        }
    }
}

/// A [`DependencyKind`] flattened into the plain string fields exposed on
/// [`PyDependent`].
struct SplitKind {
    kind: &'static str,
    run_export_kind: Option<&'static str>,
    extra: Option<String>,
}

impl From<DependencyKind> for SplitKind {
    fn from(kind: DependencyKind) -> Self {
        let (kind, run_export_kind, extra) = match kind {
            DependencyKind::Depends => ("depends", None, None),
            DependencyKind::Constrains => ("constrains", None, None),
            DependencyKind::ExtraDepends(extra) => ("extra_depends", None, Some(extra)),
            DependencyKind::RunExport(run_export) => (
                "run_export",
                Some(match run_export {
                    RunExportKind::Weak => "weak",
                    RunExportKind::Strong => "strong",
                    RunExportKind::Noarch => "noarch",
                    RunExportKind::WeakConstrains => "weak_constrains",
                    RunExportKind::StrongConstrains => "strong_constrains",
                }),
                None,
            ),
        };
        Self {
            kind,
            run_export_kind,
            extra,
        }
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
