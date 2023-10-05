use std::{future::ready, io::ErrorKind, path::PathBuf};

use futures::{stream, FutureExt, StreamExt, TryFutureExt, TryStreamExt};
use pyo3::{pyfunction, PyAny, PyResult, Python};
use pyo3_asyncio::tokio::future_into_py;
use rattler::{
    install::{link_package, InstallDriver, InstallOptions, Transaction, TransactionOperation},
    package_cache::PackageCache,
};
use rattler_conda_types::{PackageRecord, PrefixRecord, RepoDataRecord};
use rattler_networking::{retry_policies::default_retry_policy, AuthenticatedClient};

use crate::{
    error::PyRattlerError, networking::authenticated_client::PyAuthenticatedClient,
    platform::PyPlatform, prefix_record::PyPrefixRecord,
    repo_data::repo_data_record::PyRepoDataRecord,
};

// TODO: Accept functions to report progress
#[pyfunction]
pub fn py_link<'a>(
    py: Python<'a>,
    dependencies: Vec<&'a PyAny>,
    target_prefix: PathBuf,
    cache_dir: PathBuf,
    installed_packages: Vec<&'a PyAny>,
    platform: &PyPlatform,
    client: PyAuthenticatedClient,
) -> PyResult<&'a PyAny> {
    let dependencies = dependencies
        .into_iter()
        .map(|rdr| Ok(PyRepoDataRecord::try_from(rdr)?.into()))
        .collect::<PyResult<Vec<RepoDataRecord>>>()?;

    let installed_packages = installed_packages
        .iter()
        .map(|&rdr| Ok(PyPrefixRecord::try_from(rdr)?.into()))
        .collect::<PyResult<Vec<PrefixRecord>>>()?;

    let txn = py.allow_threads(move || {
        let reqired_packages = PackageRecord::sort_topologically(dependencies);

        Transaction::from_current_and_desired(installed_packages, reqired_packages, platform.inner)
            .map_err(PyRattlerError::from)
    })?;

    future_into_py(py, async move {
        Ok(execute_transaction(txn, target_prefix, cache_dir, client.inner).await?)
    })
}

async fn execute_transaction(
    transaction: Transaction<PrefixRecord, RepoDataRecord>,
    target_prefix: PathBuf,
    cache_dir: PathBuf,
    client: AuthenticatedClient,
) -> Result<(), PyRattlerError> {
    let package_cache = PackageCache::new(cache_dir.join("pkgs"));

    let install_driver = InstallDriver::default();

    let install_options = InstallOptions {
        python_info: transaction.python_info.clone(),
        platform: Some(transaction.platform),
        ..Default::default()
    };

    stream::iter(transaction.operations)
        .map(Ok)
        .try_for_each_concurrent(50, |op| {
            let target_prefix = target_prefix.clone();
            let client = client.clone();
            let package_cache = &package_cache;
            let install_driver = &install_driver;
            let install_options = &install_options;
            async move {
                execute_operation(
                    op,
                    target_prefix,
                    package_cache,
                    client,
                    install_driver,
                    install_options,
                )
                .await
            }
        })
        .await?;

    Ok(())
}

pub async fn execute_operation(
    op: TransactionOperation<PrefixRecord, RepoDataRecord>,
    target_prefix: PathBuf,
    package_cache: &PackageCache,
    client: AuthenticatedClient,
    install_driver: &InstallDriver,
    install_options: &InstallOptions,
) -> Result<(), PyRattlerError> {
    let install_record = op.record_to_install();
    let remove_record = op.record_to_remove();

    let remove_future = if let Some(remove_record) = remove_record {
        remove_package_from_environment(target_prefix.clone(), remove_record).left_future()
    } else {
        ready(Ok(())).right_future()
    };

    let cached_package_dir_fut = if let Some(install_record) = install_record {
        async {
            package_cache
                .get_or_fetch_from_url_with_retry(
                    &install_record.package_record,
                    install_record.url.clone(),
                    client.clone(),
                    default_retry_policy(),
                )
                .map_ok(|cache_dir| Some((install_record.clone(), cache_dir)))
                .map_err(|e| PyRattlerError::LinkError(e.to_string()))
                .await
        }
        .left_future()
    } else {
        ready(Ok(None)).right_future()
    };

    let (_, install_package) = tokio::try_join!(remove_future, cached_package_dir_fut)?;

    if let Some((record, package_dir)) = install_package {
        install_package_to_environment(
            target_prefix,
            package_dir,
            record.clone(),
            install_driver,
            install_options,
        )
        .await?;
    }

    Ok(())
}

// TODO: expose as python seperate function
pub async fn install_package_to_environment(
    target_prefix: PathBuf,
    package_dir: PathBuf,
    repodata_record: RepoDataRecord,
    install_driver: &InstallDriver,
    install_options: &InstallOptions,
) -> Result<(), PyRattlerError> {
    let paths = link_package(
        &package_dir,
        target_prefix.as_path(),
        install_driver,
        install_options.clone(),
    )
    .await
    .map_err(|e| PyRattlerError::LinkError(e.to_string()))?;

    let prefix_record = PrefixRecord {
        repodata_record,
        package_tarball_full_path: None,
        extracted_package_dir: Some(package_dir),
        files: paths
            .iter()
            .map(|entry| entry.relative_path.clone())
            .collect(),
        paths_data: paths.into(),
        requested_spec: None,
        link: None,
    };

    let target_prefix = target_prefix.to_path_buf();
    match tokio::task::spawn_blocking(move || {
        let conda_meta_path = target_prefix.join("conda-meta");
        std::fs::create_dir_all(&conda_meta_path)?;

        let pkg_meta_path = conda_meta_path.join(format!(
            "{}-{}-{}.json",
            prefix_record
                .repodata_record
                .package_record
                .name
                .as_normalized(),
            prefix_record.repodata_record.package_record.version,
            prefix_record.repodata_record.package_record.build
        ));
        prefix_record.write_to_path(pkg_meta_path, true)
    })
    .await
    {
        Ok(result) => Ok(result?),
        Err(err) => {
            if let Ok(panic) = err.try_into_panic() {
                std::panic::resume_unwind(panic);
            }
            Ok(())
        }
    }
}

// TODO: expose as python seperate function
async fn remove_package_from_environment(
    target_prefix: PathBuf,
    package: &PrefixRecord,
) -> Result<(), PyRattlerError> {
    for paths in package.paths_data.paths.iter() {
        match tokio::fs::remove_file(target_prefix.join(&paths.relative_path)).await {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(_) => {
                return Err(PyRattlerError::LinkError(format!(
                    "failed to delete {}",
                    paths.relative_path.display()
                )))
            }
        }
    }

    let conda_meta_path = target_prefix.join("conda-meta").join(format!(
        "{}-{}-{}.json",
        package.repodata_record.package_record.name.as_normalized(),
        package.repodata_record.package_record.version,
        package.repodata_record.package_record.build
    ));

    tokio::fs::remove_file(&conda_meta_path).await.map_err(|_| {
        PyRattlerError::LinkError(format!("failed to delete {}", conda_meta_path.display()))
    })
}
