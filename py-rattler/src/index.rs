use pyo3::{Bound, PyAny, PyResult, Python, pyfunction};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_conda_types::Platform;
use rattler_config::config::concurrency::default_max_concurrent_solves;
use rattler_index::{
    ChannelMetadata, IndexFsConfig, IndexS3Config, PackageRevisionAssignment,
    RepodataRevisionSelection, index_fs_with_channel_metadata, index_s3_with_channel_metadata,
};
use url::Url;

use crate::{
    config::PyConfig, error::PyRattlerError,
    networking::client::authentication_storage_from_config, platform::PyPlatform,
};
use pyo3::exceptions::PyValueError;
use pythonize::depythonize;
use rattler_networking::AuthenticationStorage;
use rattler_s3::{ResolvedS3Credentials, S3AddressingStyle, S3Credentials};
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
#[pyo3(signature = (channel_directory, target_platform=None, repodata_patch=None, write_zst=None, write_shards=None, repodata_revisions=None, package_revision_assignment=None, force=false, max_parallel=None, config=None))]
pub fn py_index_fs<'py>(
    py: Python<'py>,
    channel_directory: PathBuf,
    target_platform: Option<PyPlatform>,
    repodata_patch: Option<String>,
    write_zst: Option<bool>,
    write_shards: Option<bool>,
    repodata_revisions: Option<Bound<'py, PyAny>>,
    package_revision_assignment: Option<String>,
    force: bool,
    max_parallel: Option<usize>,
    config: Option<PyConfig>,
) -> PyResult<Bound<'py, PyAny>> {
    let target = channel_directory
        .canonicalize()
        .unwrap_or_else(|_| channel_directory.clone())
        .to_string_lossy()
        .into_owned();
    let resolved = config
        .as_ref()
        .map(|config| config.inner.index_config.resolve(&target))
        .unwrap_or_default();
    let package_revision_assignment = match package_revision_assignment {
        Some(value) => parse_package_revision_assignment(&value)?,
        None => resolved.package_revision_assignment.unwrap_or_default(),
    };
    let repodata_revisions = match repodata_revisions {
        Some(value) => depythonize::<Vec<RepodataRevisionSelection>>(&value)?,
        None => resolved.repodata_revisions.clone().unwrap_or_default(),
    };
    let write_zst = write_zst.or(resolved.write_zst).unwrap_or(true);
    let write_shards = write_shards.or(resolved.write_shards).unwrap_or(true);
    let max_parallel = max_parallel
        .or_else(|| {
            config
                .as_ref()
                .map(|config| config.inner.concurrency.downloads)
        })
        .unwrap_or_else(default_max_concurrent_solves);
    let channel_metadata = ChannelMetadata::from_index_config(&resolved);
    future_into_py(py, async move {
        let target_platform = target_platform.map(Platform::from);
        index_fs_with_channel_metadata(
            IndexFsConfig {
                channel: channel_directory,
                target_platform,
                repodata_patch,
                write_zst,
                write_shards,
                repodata_revisions,
                package_revision_assignment,
                force,
                max_parallel,
                multi_progress: None,
            },
            channel_metadata,
        )
        .await
        .map_err(|e| PyRattlerError::from(e).into())
    })
}

#[pyfunction]
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
#[pyo3(signature = (channel_url, credentials=None, target_platform=None, repodata_patch=None, write_zst=None, write_shards=None, repodata_revisions=None, package_revision_assignment=None, force=false, max_parallel=None, precondition_checks=true, config=None))]
pub fn py_index_s3<'py>(
    py: Python<'py>,
    channel_url: String,
    credentials: Option<Bound<'py, PyAny>>,
    target_platform: Option<PyPlatform>,
    repodata_patch: Option<String>,
    write_zst: Option<bool>,
    write_shards: Option<bool>,
    repodata_revisions: Option<Bound<'py, PyAny>>,
    package_revision_assignment: Option<String>,
    force: bool,
    max_parallel: Option<usize>,
    precondition_checks: bool,
    config: Option<PyConfig>,
) -> PyResult<Bound<'py, PyAny>> {
    let channel_url = Url::parse(&channel_url).map_err(PyRattlerError::from)?;
    let resolved = config
        .as_ref()
        .map(|config| config.inner.index_config.resolve(channel_url.as_str()))
        .unwrap_or_default();
    let package_revision_assignment = match package_revision_assignment {
        Some(value) => parse_package_revision_assignment(&value)?,
        None => resolved.package_revision_assignment.unwrap_or_default(),
    };
    let repodata_revisions = match repodata_revisions {
        Some(value) => depythonize::<Vec<RepodataRevisionSelection>>(&value)?,
        None => resolved.repodata_revisions.clone().unwrap_or_default(),
    };
    let write_zst = write_zst.or(resolved.write_zst).unwrap_or(true);
    let write_shards = write_shards.or(resolved.write_shards).unwrap_or(true);
    let max_parallel = max_parallel
        .or_else(|| {
            config
                .as_ref()
                .map(|config| config.inner.concurrency.downloads)
        })
        .unwrap_or_else(default_max_concurrent_solves);
    let channel_metadata = ChannelMetadata::from_index_config(&resolved);

    let configured_s3 = channel_url
        .host_str()
        .and_then(|bucket| config.as_ref()?.inner.s3_options.0.get(bucket));
    let credentials = match credentials {
        Some(dict) => {
            let credentials: S3Credentials = depythonize(&dict)?;
            let auth_storage = match &config {
                Some(config) => authentication_storage_from_config(config),
                None => AuthenticationStorage::from_env_and_defaults(),
            }
            .map_err(PyRattlerError::from)?;
            Some((credentials, auth_storage))
        }
        None => configured_s3
            .map(|options| {
                let credentials = S3Credentials {
                    endpoint_url: options.endpoint_url.clone(),
                    region: options.region.clone(),
                    access_key_id: None,
                    secret_access_key: None,
                    session_token: None,
                    addressing_style: if options.force_path_style {
                        S3AddressingStyle::Path
                    } else {
                        S3AddressingStyle::VirtualHost
                    },
                };
                let auth_storage = config
                    .as_ref()
                    .map_or_else(
                        AuthenticationStorage::from_env_and_defaults,
                        authentication_storage_from_config,
                    )
                    .map_err(PyRattlerError::from)?;
                Ok::<_, PyRattlerError>((credentials, auth_storage))
            })
            .transpose()?,
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

        index_s3_with_channel_metadata(
            IndexS3Config {
                channel: channel_url,
                credentials,
                target_platform,
                repodata_patch,
                write_zst,
                write_shards,
                repodata_revisions,
                package_revision_assignment,
                force,
                max_parallel,
                multi_progress: None,
                precondition_checks: if precondition_checks {
                    rattler_index::PreconditionChecks::Enabled
                } else {
                    rattler_index::PreconditionChecks::Disabled
                },
            },
            channel_metadata,
        )
        .await
        .map_err(|e| PyRattlerError::from(e).into())
    })
}
