use crate::error::PyRattlerError;
use crate::platform::PyPlatform;
use pyo3::{exceptions::PyValueError, pyclass, pymethods, Bound, FromPyObject, PyAny, PyResult};
use rattler_shell::{
    activation::{ActivationResult, ActivationVariables, Activator, PathModificationBehavior},
    shell::{Bash, CmdExe, Fish, PowerShell, ShellEnum, Xonsh, Zsh},
};
use std::path::{Path, PathBuf};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyActivationVariables {
    inner: ActivationVariables,
}

impl From<ActivationVariables> for PyActivationVariables {
    fn from(value: ActivationVariables) -> Self {
        PyActivationVariables { inner: value }
    }
}

#[repr(transparent)]
pub struct Wrap<T>(pub T);

impl<'py> FromPyObject<'py> for Wrap<PathModificationBehavior> {
    fn extract_bound(ob: &Bound<'py, PyAny>) -> PyResult<Self> {
        let parsed = match <&'py str>::extract_bound(ob)? {
            "prepend" => PathModificationBehavior::Prepend,
            "append" => PathModificationBehavior::Append,
            "replace" => PathModificationBehavior::Replace,
            v => {
                return Err(PyValueError::new_err(format!(
                    "keep must be one of {{'prepend', 'append', 'replace'}}, got {v}",
                )))
            }
        };
        Ok(Wrap(parsed))
    }
}

#[pymethods]
impl PyActivationVariables {
    #[new]
    #[pyo3(signature = (conda_prefix, path, path_modification_behavior))]
    pub fn __init__(
        conda_prefix: Option<PathBuf>,
        path: Option<Vec<PathBuf>>,
        path_modification_behavior: Wrap<PathModificationBehavior>,
    ) -> Self {
        let activation_vars = ActivationVariables {
            conda_prefix,
            path,
            path_modification_behavior: path_modification_behavior.0,
        };
        activation_vars.into()
    }

    #[getter]
    pub fn conda_prefix(&self) -> Option<&Path> {
        self.inner.conda_prefix.as_deref()
    }

    #[getter]
    pub fn path(&self) -> Option<Vec<&Path>> {
        self.inner
            .path
            .as_ref()
            .map(|p| p.iter().map(std::path::PathBuf::as_path).collect())
    }
}

#[pyclass]
pub struct PyActivationResult {
    pub inner: ActivationResult<ShellEnum>,
}

impl From<ActivationResult<ShellEnum>> for PyActivationResult {
    fn from(value: ActivationResult<ShellEnum>) -> Self {
        PyActivationResult { inner: value }
    }
}

#[pymethods]
impl PyActivationResult {
    #[getter]
    pub fn path(&self) -> Vec<PathBuf> {
        self.inner.path.clone()
    }

    #[getter]
    pub fn script(&self) -> PyResult<String> {
        Ok(self
            .inner
            .script
            .contents()
            .map_err(PyRattlerError::ActivationScriptFormatError)?)
    }
}

#[pyclass(eq, eq_int)]
#[derive(Clone, Eq, PartialEq)]
pub enum PyShellEnum {
    Bash,
    Zsh,
    Xonsh,
    CmdExe,
    PowerShell,
    Fish,
}

impl PyShellEnum {
    pub fn to_shell_enum(&self) -> ShellEnum {
        match self {
            PyShellEnum::Bash => Bash.into(),
            PyShellEnum::Zsh => Zsh.into(),
            PyShellEnum::Xonsh => Xonsh.into(),
            PyShellEnum::CmdExe => CmdExe.into(),
            PyShellEnum::PowerShell => PowerShell::default().into(),
            PyShellEnum::Fish => Fish.into(),
        }
    }
}

#[pyclass]
pub struct PyActivator;

#[pymethods]
impl PyActivator {
    #[staticmethod]
    pub fn activate(
        prefix: PathBuf,
        activation_vars: PyActivationVariables,
        platform: PyPlatform,
        shell: PyShellEnum,
    ) -> Result<PyActivationResult, PyRattlerError> {
        let activation_vars = activation_vars.inner;
        let shell = shell.to_shell_enum();
        let platform = platform.inner;

        let activation_result =
            Activator::from_path(prefix.as_path(), shell, platform)?.activation(activation_vars)?;

        Ok(activation_result.into())
    }
}
