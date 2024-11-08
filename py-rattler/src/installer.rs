use std::path::PathBuf;

use pyo3::{pyfunction, Bound, PyAny, PyResult, Python};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler::{
    install::{IndicatifReporter, Installer},
    package_cache::PackageCache,
};
use rattler_conda_types::{PrefixRecord, RepoDataRecord};

use crate::{
    error::PyRattlerError, networking::client::PyClientWithMiddleware, platform::PyPlatform,
    record::PyRecord,
};

// TODO: Accept functions to report progress
#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (records, target_prefix, execute_link_scripts=false, show_progress=false, platform=None, client=None, cache_dir=None, installed_packages=None))]
pub fn py_install<'a>(
    py: Python<'a>,
    records: Vec<Bound<'a, PyAny>>,
    target_prefix: PathBuf,
    execute_link_scripts: bool,
    show_progress: bool,
    platform: Option<PyPlatform>,
    client: Option<PyClientWithMiddleware>,
    cache_dir: Option<PathBuf>,
    installed_packages: Option<Vec<Bound<'a, PyAny>>>,
) -> PyResult<Bound<'a, PyAny>> {
    let dependencies = records
        .into_iter()
        .map(|rdr| PyRecord::try_from(rdr)?.try_into())
        .collect::<PyResult<Vec<RepoDataRecord>>>()?;

    let installed_packages = installed_packages
        .map(|pkgs| {
            pkgs.into_iter()
                .map(|rdr| PyRecord::try_from(rdr)?.try_into())
                .collect::<PyResult<Vec<PrefixRecord>>>()
        })
        .transpose()?;

    let platform = platform.map(|p| p.inner);
    let client = client.map(|c| c.inner);

    future_into_py(py, async move {
        let mut installer = Installer::new().with_execute_link_scripts(execute_link_scripts);

        if show_progress {
            installer.set_reporter(IndicatifReporter::builder().finish());
        }

        if let Some(target_platform) = platform {
            installer.set_target_platform(target_platform);
        }

        if let Some(client) = client {
            installer.set_download_client(client);
        }

        if let Some(cache_dir) = cache_dir {
            installer.set_package_cache(PackageCache::new(cache_dir));
        }

        if let Some(installed_packages) = installed_packages {
            installer.set_installed_packages(installed_packages);
        }

        // TODO: Return the installation result to python
        let _installation_result = installer
            .install(target_prefix, dependencies)
            .await
            .map_err(PyRattlerError::from)?;

        Ok(())
    })
}
