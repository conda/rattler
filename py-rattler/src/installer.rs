use std::path::PathBuf;

use pyo3::{exceptions::PyTypeError, pyfunction, Bound, PyAny, PyResult, Python};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler::{
    install::{IndicatifReporter, Installer},
    package_cache::PackageCache,
};
use rattler_conda_types::{PackageName, PrefixRecord, RepoDataRecord};
use std::collections::HashSet;

use crate::match_spec::PyMatchSpec;
use crate::{
    error::PyRattlerError, networking::client::PyClientWithMiddleware, platform::PyPlatform,
    record::PyRecord,
};

// Import the py_install_reporter
use crate::py_install_reporter::{PyInstallReporter, SharedError};

#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (records, target_prefix, execute_link_scripts=false, show_progress=false, platform=None, client=None, cache_dir=None, installed_packages=None, reinstall_packages=None, ignored_packages=None, requested_specs=None, progress_delegate=None))]
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
    reinstall_packages: Option<HashSet<String>>,
    ignored_packages: Option<HashSet<String>>,
    requested_specs: Option<Vec<PyMatchSpec>>,
    progress_delegate: Option<pyo3::Py<PyAny>>,
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

    let reinstall_packages = reinstall_packages
        .map(|pkgs| {
            pkgs.into_iter()
                .map(PackageName::try_from)
                .collect::<Result<HashSet<_>, _>>()
        })
        .transpose()
        .map_err(|_err| PyTypeError::new_err("cannot convert to conda PackageName"))?;

    let ignored_packages = ignored_packages
        .map(|pkgs| {
            pkgs.into_iter()
                .map(PackageName::try_from)
                .collect::<Result<HashSet<_>, _>>()
        })
        .transpose()
        .map_err(|_err| PyTypeError::new_err("cannot convert to conda PackageName"))?;

    let platform = platform.map(|p| p.inner);
    let client = client.map(|c| c.inner);

    future_into_py(py, async move {
        let mut installer = Installer::new().with_execute_link_scripts(execute_link_scripts);

        // If a Python progress delegate is provided, wrap it in a PyInstallReporter
        // and attach it to the installer. We also create a shared error slot
        // (Arc<Mutex<Option<PyErr>>>) that allows the reporter to capture any
        // Python-side exception raised during progress callbacks. After the
        // installation finishes, we check this slot and propagate the error
        // back to Python. If no delegate we default back to the IndicatifReporter if progress is enabled.
        let delegate_error: Option<SharedError> = if let Some(delegate) = progress_delegate {
            let error: SharedError = std::sync::Arc::new(std::sync::Mutex::new(None));
            let reporter = PyInstallReporter::new(delegate, error.clone());
            installer.set_reporter(reporter);
            Some(error)
        } else {
            if show_progress {
                installer.set_reporter(IndicatifReporter::builder().finish());
            }
            None
        };

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

        if let Some(reinstall_packages) = reinstall_packages {
            installer.set_reinstall_packages(reinstall_packages);
        }

        if let Some(ignored_packages) = ignored_packages {
            installer.set_ignored_packages(ignored_packages);
        }

        if let Some(requested_specs) = requested_specs {
            installer
                .set_requested_specs(requested_specs.into_iter().map(|spec| spec.inner).collect());
        }

        // TODO: Return the installation result to python
        let _installation_result = installer
            .install(target_prefix, dependencies)
            .await
            .map_err(PyRattlerError::from)?;

        // We inspect the slot and propagate the error back to python if any occurred
        if let Some(error_slot) = delegate_error {
            if let Some(err) = error_slot.lock().unwrap().take() {
                return Err(err);
            }
        }

        Ok(())
    })
}
