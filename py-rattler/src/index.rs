use pyo3::{pyfunction, PyResult, Python};
use rattler_conda_types::Platform;
use rattler_index::index;

use std::path::PathBuf;

use crate::{error::PyRattlerError, platform::PyPlatform};

#[pyfunction]
#[pyo3(signature = (channel_directory, target_platform=None))]
pub fn py_index(
    py: Python<'_>,
    channel_directory: PathBuf,
    target_platform: Option<PyPlatform>,
) -> PyResult<()> {
    py.allow_threads(move || {
        let path = channel_directory.as_path();
        match index(path, target_platform.map(Platform::from).as_ref()) {
            Ok(_v) => Ok(()),
            Err(e) => Err(PyRattlerError::from(e).into()),
        }
    })
}
