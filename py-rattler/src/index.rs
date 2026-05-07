use pyo3::{pyfunction, Bound, PyAny, PyResult, Python};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_conda_types::Platform;
use rattler_config::config::concurrency::default_max_concurrent_solves;
use rattler_index::{
    index_fs, index_s3, IndexFsConfig, IndexS3Config, PackageRevisionAssignment,
    RepodataRevisionInfo,
};
use url::Url;

use crate::{error::PyRattlerError, platform::PyPlatform};
use pyo3::exceptions::PyValueError;
use pythonize::depythonize;
use rattler_networking::AuthenticationStorage;
use rattler_s3::{ResolvedS3Credentials, S3Credentials};
use std::path::PathBuf;

fn parse_package_revision_assignment(value: &str) -> PyResult<PackageRevisionAssignment> {
    match value {
        "from-index-json" => Ok(PackageRevisionAssignment::FromIndexJson),
        "latest" => Ok(PackageRevisionAssignment::Latest),
        _ => Err(PyValueError::new_err(format!(
            "invalid package_revision_assignment '{value}', expected 'from-index-json' or 'latest'"
        ))),
    }
}

#[pyfunction]
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
#[pyo3(signature = (channel_directory, target_platform=None, repodata_patch=None, write_zst=true, write_shards=true, repodata_revisions=None, package_revision_assignment=None, force=false, max_parallel=None))]
pub fn py_index_fs<'py>(
    py: Python<'py>,
    channel_directory: PathBuf,
    target_platform: Option<PyPlatform>,
    repodata_patch: Option<String>,
    write_zst: bool,
    write_shards: bool,
    repodata_revisions: Option<Bound<'py, PyAny>>,
    package_revision_assignment: Option<String>,
    force: bool,
    max_parallel: Option<usize>,
) -> PyResult<Bound<'py, PyAny>> {
    let package_revision_assignment = parse_package_revision_assignment(
        package_revision_assignment
            .as_deref()
            .unwrap_or("from-index-json"),
    )?;
    let repodata_revisions = match repodata_revisions {
        Some(value) => depythonize::<Vec<RepodataRevisionInfo>>(&value)?,
        None => Vec::new(),
    };
    future_into_py(py, async move {
        let target_platform = target_platform.map(Platform::from);
        index_fs(IndexFsConfig {
            channel: channel_directory,
            target_platform,
            repodata_patch,
            write_zst,
            write_shards,
            repodata_revisions,
            package_revision_assignment,
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
#[pyo3(signature = (channel_url, credentials=None, target_platform=None, repodata_patch=None, write_zst=true, write_shards=true, repodata_revisions=None, package_revision_assignment=None, force=false, max_parallel=None, precondition_checks=true))]
pub fn py_index_s3<'py>(
    py: Python<'py>,
    channel_url: String,
    credentials: Option<Bound<'py, PyAny>>,
    target_platform: Option<PyPlatform>,
    repodata_patch: Option<String>,
    write_zst: bool,
    write_shards: bool,
    repodata_revisions: Option<Bound<'py, PyAny>>,
    package_revision_assignment: Option<String>,
    force: bool,
    max_parallel: Option<usize>,
    precondition_checks: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let package_revision_assignment = parse_package_revision_assignment(
        package_revision_assignment
            .as_deref()
            .unwrap_or("from-index-json"),
    )?;
    let repodata_revisions = match repodata_revisions {
        Some(value) => depythonize::<Vec<RepodataRevisionInfo>>(&value)?,
        None => Vec::new(),
    };
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
                None => ResolvedS3Credentials::from_sdk()
                    .await
                    .map_err(PyRattlerError::from)?,
            };

        index_s3(IndexS3Config {
            channel: channel_url,
            credentials,
            target_platform,
            repodata_patch,
            write_zst,
            write_shards,
            repodata_revisions,
            package_revision_assignment,
            force,
            max_parallel: max_parallel.unwrap_or_else(default_max_concurrent_solves),
            multi_progress: None,
            precondition_checks: if precondition_checks {
                rattler_index::PreconditionChecks::Enabled
            } else {
                rattler_index::PreconditionChecks::Disabled
            },
        })
        .await
        .map_err(|e| PyRattlerError::from(e).into())
    })
}
