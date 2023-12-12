use pyo3::{pyfunction, PyResult};
use rattler_conda_types::Platform;
use rattler_index::index;

use std::path::PathBuf;

use crate::{error::PyRattlerError, platform::PyPlatform};

#[pyfunction]
pub fn py_index(channel_directory: PathBuf, target_platform: Option<PyPlatform>) -> PyResult<bool> {
    let path = channel_directory.as_path();
    match index(path, target_platform.map(Platform::from).as_ref()) {
        Ok(_v) => Ok(true),
        Err(e) => Err(PyRattlerError::from(e).into()),
    }
}
