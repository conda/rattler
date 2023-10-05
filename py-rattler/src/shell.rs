use crate::error::PyRattlerError;
use crate::platform::PyPlatform;
use pyo3::{exceptions::PyValueError, pyclass, pymethods, FromPyObject, PyAny, PyResult};
use rattler_shell::{
    activation::{ActivationResult, ActivationVariables, Activator, PathModificationBehavior},
    shell::{Bash, CmdExe, Fish, PowerShell, Xonsh, Zsh},
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

impl FromPyObject<'_> for Wrap<PathModificationBehavior> {
    fn extract(ob: &PyAny) -> PyResult<Self> {
        let parsed = match ob.extract::<&str>()? {
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
    #[pyo3(signature = (conda_prefix, path, path_modification_behaviour))]
    pub fn __init__(
        conda_prefix: Option<PathBuf>,
        path: Option<Vec<PathBuf>>,
        path_modification_behaviour: Wrap<PathModificationBehavior>,
    ) -> Self {
        let activation_vars = ActivationVariables {
            conda_prefix,
            path,
            path_modification_behaviour: path_modification_behaviour.0,
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
            .map(|p| p.iter().map(|p| p.as_path()).collect())
    }
}

#[pyclass]
pub struct PyActivationResult {
    pub inner: ActivationResult,
}

impl From<ActivationResult> for PyActivationResult {
    fn from(value: ActivationResult) -> Self {
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
    pub fn script(&self) -> String {
        self.inner.script.clone()
    }
}

#[pyclass]
#[derive(Clone)]
pub enum PyShellEnum {
    Bash,
    Zsh,
    Xonsh,
    CmdExe,
    PowerShell,
    Fish,
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
        let activation_result = match shell {
            PyShellEnum::Bash => {
                Activator::<Bash>::from_path(prefix.as_path(), Bash, platform.into())?
                    .activation(activation_vars)?
            }
            PyShellEnum::Zsh => {
                Activator::<Zsh>::from_path(prefix.as_path(), Zsh, platform.into())?
                    .activation(activation_vars)?
            }
            PyShellEnum::Xonsh => {
                Activator::<Xonsh>::from_path(prefix.as_path(), Xonsh, platform.into())?
                    .activation(activation_vars)?
            }
            PyShellEnum::CmdExe => {
                Activator::<CmdExe>::from_path(prefix.as_path(), CmdExe, platform.into())?
                    .activation(activation_vars)?
            }
            PyShellEnum::PowerShell => Activator::<PowerShell>::from_path(
                prefix.as_path(),
                PowerShell::default(),
                platform.into(),
            )?
            .activation(activation_vars)?,
            PyShellEnum::Fish => {
                Activator::<Fish>::from_path(prefix.as_path(), Fish, platform.into())?
                    .activation(activation_vars)?
            }
        };

        Ok(activation_result.into())
    }
}
