use pyo3::{pyfunction, Bound, PyAny, PyResult, Python};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_conda_types::Platform;
use rattler_index::{index_fs, index_s3};
use url::Url;

use std::path::PathBuf;

use crate::{error::PyRattlerError, platform::PyPlatform};

#[pyfunction]
#[pyo3(signature = (channel_directory, target_platform=None, repodata_patch=None, force=false, max_parallel=32))]
pub fn py_index_fs(
    py: Python<'_>,
    channel_directory: PathBuf,
    target_platform: Option<PyPlatform>,
    repodata_patch: Option<String>,
    force: bool,
    max_parallel: usize,
) -> PyResult<Bound<'_, PyAny>> {
    future_into_py(py, async move {
        let target_platform = target_platform.map(Platform::from);
        index_fs(
            channel_directory,
            target_platform,
            repodata_patch,
            force,
            max_parallel,
            None,
        )
        .await
        .map_err(|e| PyRattlerError::from(e).into())
    })
}

#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (channel_url, region, endpoint_url, force_path_style, access_key_id=None,secret_access_key=None, session_token=None, target_platform=None, repodata_patch=None, force=false, max_parallel=32))]
pub fn py_index_s3(
    py: Python<'_>,
    channel_url: String,
    region: String,
    endpoint_url: String,
    force_path_style: bool,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    session_token: Option<String>,
    target_platform: Option<PyPlatform>,
    repodata_patch: Option<String>,
    force: bool,
    max_parallel: usize,
) -> PyResult<Bound<'_, PyAny>> {
    let channel_url = Url::parse(&channel_url).map_err(PyRattlerError::from)?;
    let endpoint_url = Url::parse(&endpoint_url).map_err(PyRattlerError::from)?;
    let target_platform = target_platform.map(Platform::from);
    future_into_py(py, async move {
        index_s3(
            channel_url,
            region,
            endpoint_url,
            force_path_style,
            access_key_id,
            secret_access_key,
            session_token,
            target_platform,
            repodata_patch,
            force,
            max_parallel,
            None,
        )
        .await
        .map_err(|e| PyRattlerError::from(e).into())
    })
}
