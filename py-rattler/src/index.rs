use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::Context;
use opendal::{services::FsConfig, Configurator, Operator};
use pyo3::{exceptions::PyTypeError, pyfunction, Bound, PyAny, PyResult, Python};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_conda_types::{
    package::{ArchiveIdentifier, CondaArchiveType, DistArchiveType},
    ChannelInfo, ExperimentalV3Packages, Platform, RepoData, RepoDataRecord, WhlPackageRecord,
};
use rattler_config::config::concurrency::default_max_concurrent_solves;
use rattler_index::{
    ensure_channel_initialized, index_fs, index_s3, write_repodata, IndexFsConfig, IndexS3Config,
    PreconditionChecks, RepodataMetadataCollection,
};
use url::Url;

use crate::{error::PyRattlerError, platform::PyPlatform, record::PyRecord};
use pyo3::exceptions::PyValueError;
use pythonize::depythonize;
use rattler_networking::AuthenticationStorage;
use rattler_s3::{ResolvedS3Credentials, S3Credentials};

fn archive_identifier_from_whl_record(record: &WhlPackageRecord) -> ArchiveIdentifier {
    ArchiveIdentifier {
        name: record.package_record.name.as_source().to_string(),
        version: record.package_record.version.to_string(),
        build_string: record.package_record.build.clone(),
    }
}

enum RepodataInputRecord {
    Conda(RepoDataRecord),
    Whl(WhlPackageRecord),
}

fn repodata_record_from_py_record(record: PyRecord) -> PyResult<RepodataInputRecord> {
    match record.inner {
        crate::record::RecordInner::Prefix(record) => Ok(RepodataInputRecord::Conda(
            Arc::unwrap_or_clone(record).repodata_record,
        )),
        crate::record::RecordInner::RepoData(record) => {
            Ok(RepodataInputRecord::Conda(Arc::unwrap_or_clone(record)))
        }
        crate::record::RecordInner::Whl(record) => {
            Ok(RepodataInputRecord::Whl(Arc::unwrap_or_clone(record)))
        }
        crate::record::RecordInner::Package(_) => Err(PyTypeError::new_err(
            "write_repodata expects RepoDataRecord, PrefixRecord, or WhlPackageRecord values",
        )),
    }
}

fn repo_data_from_subdir(subdir: Platform) -> RepoData {
    RepoData {
        info: Some(ChannelInfo {
            subdir: Some(subdir.to_string()),
            base_url: None,
            channel_relations: None,
        }),
        packages: std::iter::empty().collect(),
        conda_packages: std::iter::empty().collect(),
        experimental_v3: ExperimentalV3Packages::default(),
        removed: std::iter::empty().collect(),
        version: Some(3),
    }
}

fn insert_repodata_input(
    repodata: &mut RepoData,
    record: RepodataInputRecord,
) -> anyhow::Result<()> {
    match record {
        RepodataInputRecord::Conda(record) => {
            let RepoDataRecord {
                package_record,
                identifier,
                url,
                ..
            } = record;

            let archive_type = identifier.archive_type;
            match archive_type {
                DistArchiveType::Conda(CondaArchiveType::TarBz2) => {
                    repodata.packages.insert(identifier, package_record);
                }
                DistArchiveType::Conda(CondaArchiveType::Conda) => {
                    repodata.conda_packages.insert(identifier, package_record);
                }
                DistArchiveType::Wheel(_) => {
                    let url = url.as_str().parse().context("invalid wheel url")?;
                    repodata.experimental_v3.whl.insert(
                        identifier.identifier,
                        WhlPackageRecord {
                            package_record,
                            url,
                        },
                    );
                }
            }
        }
        RepodataInputRecord::Whl(record) => {
            repodata
                .experimental_v3
                .whl
                .insert(archive_identifier_from_whl_record(&record), record);
        }
    }

    Ok(())
}

async fn write_fs_repodata(
    channel_directory: PathBuf,
    records: Vec<RepodataInputRecord>,
    write_zst: bool,
    write_shards: bool,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(&channel_directory)?;

    let mut config = FsConfig::default();
    config.root = Some(
        channel_directory
            .canonicalize()?
            .to_string_lossy()
            .to_string(),
    );
    let op = Operator::new(config.into_builder())?.finish();

    ensure_channel_initialized(&op).await?;

    let mut repodata_by_subdir = HashMap::<Platform, RepoData>::new();

    for record in records {
        let subdir = match &record {
            RepodataInputRecord::Conda(record) => record.package_record.subdir.as_str(),
            RepodataInputRecord::Whl(record) => record.package_record.subdir.as_str(),
        }
        .parse::<Platform>()
        .with_context(|| match &record {
            RepodataInputRecord::Conda(record) => {
                format!("invalid platform '{}'", record.package_record.subdir)
            }
            RepodataInputRecord::Whl(record) => {
                format!("invalid platform '{}'", record.package_record.subdir)
            }
        })?;

        let repodata = repodata_by_subdir
            .entry(subdir)
            .or_insert_with(|| repo_data_from_subdir(subdir));

        insert_repodata_input(repodata, record)?;
    }

    for (subdir, repodata) in repodata_by_subdir {
        let subdir_path = format!("{subdir}/");
        if !op.exists(&subdir_path).await? {
            op.create_dir(&subdir_path).await?;
        }

        let metadata = RepodataMetadataCollection::new(
            &op,
            subdir,
            false,
            write_zst,
            write_shards,
            PreconditionChecks::Disabled,
        )
        .await?;

        write_repodata(repodata, None, subdir, op.clone(), &metadata).await?;
    }

    Ok(())
}

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
#[pyo3(signature = (channel_directory, records, write_zst=true, write_shards=true))]
pub fn py_write_repodata(
    py: Python<'_>,
    channel_directory: PathBuf,
    records: Vec<PyRecord>,
    write_zst: bool,
    write_shards: bool,
) -> PyResult<Bound<'_, PyAny>> {
    let records = records
        .into_iter()
        .map(repodata_record_from_py_record)
        .collect::<PyResult<Vec<_>>>()?;

    future_into_py(py, async move {
        write_fs_repodata(channel_directory, records, write_zst, write_shards)
            .await
            .map_err(|e| PyRattlerError::from(e).into())
    })
}

#[pyfunction]
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
#[pyo3(signature = (channel_url, credentials=None, target_platform=None, repodata_patch=None, write_zst=true, write_shards=true, force=false, max_parallel=None, precondition_checks=true))]
pub fn py_index_s3<'py>(
    py: Python<'py>,
    channel_url: String,
    credentials: Option<Bound<'py, PyAny>>,
    target_platform: Option<PyPlatform>,
    repodata_patch: Option<String>,
    write_zst: bool,
    write_shards: bool,
    force: bool,
    max_parallel: Option<usize>,
    precondition_checks: bool,
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
