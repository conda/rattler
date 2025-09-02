use pyo3::{pyfunction, Bound, PyAny, PyResult, Python};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_conda_types::Platform;
use rattler_config::config::concurrency::default_max_concurrent_solves;
use rattler_index::{index_fs, index_s3, IndexFsConfig, IndexS3Config};
use url::Url;

use crate::{error::PyRattlerError, platform::PyPlatform};
use pyo3::exceptions::PyValueError;
use pythonize::depythonize;
use rattler_networking::AuthenticationStorage;
use rattler_s3::{ResolvedS3Credentials, S3Credentials};
use std::path::PathBuf;

#[pyfunction]
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
#[pyo3(signature = (channel_directory, target_platform=None, repodata_patch=None, write_zst=true, write_shards=true, force=false, max_parallel=None))]
pub fn py_index_fs(
    py: Python<'_>,
    channel_directory: PathBuf,
    target_platform: Option<PyPlatform>,
    repodata_patch: Option<String>,
    write_zst: bool,
    write_shards: bool,
    force: bool,
    max_parallel: Option<usize>,
) -> PyResult<Bound<'_, PyAny>> {
    future_into_py(py, async move {
        let target_platform = target_platform.map(Platform::from);
        index_fs(IndexFsConfig {
            channel: channel_directory,
            target_platform,
            repodata_patch,
            write_zst,
            write_shards,
            force,
            max_parallel: max_parallel.unwrap_or_else(default_max_concurrent_solves),
            multi_progress: None,
        })
        .await
        .map_err(|e| PyRattlerError::from(e).into())
    })
}

#[pyfunction]
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
#[pyo3(signature = (channel_url, credentials=None, force_path_style=None, target_platform=None, repodata_patch=None, write_zst=true, write_shards=true, force=false, max_parallel=None))]
pub fn py_index_s3<'py>(
    py: Python<'py>,
    channel_url: String,
    credentials: Option<Bound<'py, PyAny>>,
    force_path_style: Option<bool>,
    target_platform: Option<PyPlatform>,
    repodata_patch: Option<String>,
    write_zst: bool,
    write_shards: bool,
    force: bool,
    max_parallel: Option<usize>,
) -> PyResult<Bound<'py, PyAny>> {
    let channel_url = Url::parse(&channel_url).map_err(PyRattlerError::from)?;
    let credentials = match credentials {
        Some(dict) => {
            let credentials: S3Credentials = depythonize(&dict)?;
            let auth_storage =
                AuthenticationStorage::from_env_and_defaults().map_err(PyRattlerError::from)?;
            Some((credentials, auth_storage))
        }
        None => None,
    };
    let target_platform = target_platform.map(Platform::from);
    future_into_py(py, async move {
        // Resolve the credentials
        let credentials =
            match credentials {
                Some((credentials, auth_storage)) => credentials
                    .resolve(&channel_url, &auth_storage)
                    .ok_or_else(|| PyValueError::new_err("could not resolve s3 credentials"))?,
                None => ResolvedS3Credentials::from_sdk().await.map_err(PyRattlerError::from)?,
            };

        index_s3(IndexS3Config {
            channel: channel_url,
            credentials,
            force_path_style,
            target_platform,
            repodata_patch,
            write_zst,
            write_shards,
            force,
            max_parallel: max_parallel.unwrap_or_else(default_max_concurrent_solves),
            multi_progress: None,
        })
        .await
        .map_err(|e| PyRattlerError::from(e).into())
    })
}
