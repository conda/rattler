use std::path::PathBuf;
use std::sync::Arc;

use pyo3::{exceptions::PyTypeError, pyfunction, Bound, PyAny, PyObject, PyResult, Python};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler::{
    install::{IndicatifReporter, Installer, Reporter, Transaction},
    package_cache::PackageCache,
};
use rattler_conda_types::{PackageName, PrefixRecord, RepoDataRecord};
use std::collections::HashSet;

use crate::match_spec::PyMatchSpec;
use crate::{
    error::PyRattlerError, networking::client::PyClientWithMiddleware, platform::PyPlatform,
    record::PyRecord,
};

/// A [`Reporter`] implementation that delegates progress events to a Python object. The Python object should implement the following methods:
struct PyReporter {
    py_obj: Arc<PyObject>,
}

impl Reporter for PyReporter {
    fn on_transaction_start(&self, transaction: &Transaction<PrefixRecord, RepoDataRecord>) {
        let total = transaction.operations.len();
        Python::with_gil(|py| {
            let _ = self
                .py_obj
                .call_method1(py, "on_transaction_start", (total,));
        });
    }

    fn on_transaction_operation_start(&self, operation: usize) {
        Python::with_gil(|py| {
            let _ = self
                .py_obj
                .call_method1(py, "on_transaction_operation_start", (operation,));
        });
    }

    fn on_populate_cache_start(&self, operation: usize, record: &RepoDataRecord) -> usize {
        let name = record.package_record.name.as_normalized().to_string();
        Python::with_gil(|py| {
            match self
                .py_obj
                .call_method1(py, "on_populate_cache_start", (operation, name))
            {
                Ok(val) => val.extract::<usize>(py).unwrap_or(0),
                Err(_) => 0,
            }
        })
    }

    fn on_validate_start(&self, cache_entry: usize) -> usize {
        Python::with_gil(|py| {
            match self
                .py_obj
                .call_method1(py, "on_validate_start", (cache_entry,))
            {
                Ok(val) => val.extract::<usize>(py).unwrap_or(0),
                Err(_) => 0,
            }
        })
    }

    fn on_validate_complete(&self, validate_idx: usize) {
        Python::with_gil(|py| {
            let _ = self
                .py_obj
                .call_method1(py, "on_validate_complete", (validate_idx,));
        });
    }

    fn on_download_start(&self, cache_entry: usize) -> usize {
        Python::with_gil(|py| {
            match self
                .py_obj
                .call_method1(py, "on_download_start", (cache_entry,))
            {
                Ok(val) => val.extract::<usize>(py).unwrap_or(0),
                Err(_) => 0,
            }
        })
    }

    fn on_download_progress(&self, download_idx: usize, progress: u64, total: Option<u64>) {
        Python::with_gil(|py| {
            let _ = self.py_obj.call_method1(
                py,
                "on_download_progress",
                (download_idx, progress, total),
            );
        });
    }

    fn on_download_completed(&self, download_idx: usize) {
        Python::with_gil(|py| {
            let _ = self
                .py_obj
                .call_method1(py, "on_download_completed", (download_idx,));
        });
    }

    fn on_populate_cache_complete(&self, cache_entry: usize) {
        Python::with_gil(|py| {
            let _ = self
                .py_obj
                .call_method1(py, "on_populate_cache_complete", (cache_entry,));
        });
    }

    fn on_unlink_start(&self, operation: usize, record: &PrefixRecord) -> usize {
        let name = record
            .repodata_record
            .package_record
            .name
            .as_normalized()
            .to_string();
        Python::with_gil(|py| {
            match self
                .py_obj
                .call_method1(py, "on_unlink_start", (operation, name))
            {
                Ok(val) => val.extract::<usize>(py).unwrap_or(0),
                Err(_) => 0,
            }
        })
    }

    fn on_unlink_complete(&self, index: usize) {
        Python::with_gil(|py| {
            let _ = self.py_obj.call_method1(py, "on_unlink_complete", (index,));
        });
    }

    fn on_link_start(&self, operation: usize, record: &RepoDataRecord) -> usize {
        let name = record.package_record.name.as_normalized().to_string();
        Python::with_gil(|py| {
            match self
                .py_obj
                .call_method1(py, "on_link_start", (operation, name))
            {
                Ok(val) => val.extract::<usize>(py).unwrap_or(0),
                Err(_) => 0,
            }
        })
    }

    fn on_link_complete(&self, index: usize) {
        Python::with_gil(|py| {
            let _ = self.py_obj.call_method1(py, "on_link_complete", (index,));
        });
    }

    fn on_transaction_operation_complete(&self, operation: usize) {
        Python::with_gil(|py| {
            let _ = self
                .py_obj
                .call_method1(py, "on_transaction_operation_complete", (operation,));
        });
    }

    fn on_transaction_complete(&self) {
        Python::with_gil(|py| {
            let _ = self.py_obj.call_method0(py, "on_transaction_complete");
        });
    }

    fn on_post_link_start(&self, package_name: &str, script_path: &str) -> usize {
        let pkg = package_name.to_string();
        let path = script_path.to_string();
        Python::with_gil(|py| {
            match self
                .py_obj
                .call_method1(py, "on_post_link_start", (pkg, path))
            {
                Ok(val) => val.extract::<usize>(py).unwrap_or(0),
                Err(_) => 0,
            }
        })
    }

    fn on_post_link_complete(&self, index: usize, success: bool) {
        Python::with_gil(|py| {
            let _ = self
                .py_obj
                .call_method1(py, "on_post_link_complete", (index, success));
        });
    }

    fn on_pre_unlink_start(&self, package_name: &str, script_path: &str) -> usize {
        let pkg = package_name.to_string();
        let path = script_path.to_string();
        Python::with_gil(|py| {
            match self
                .py_obj
                .call_method1(py, "on_pre_unlink_start", (pkg, path))
            {
                Ok(val) => val.extract::<usize>(py).unwrap_or(0),
                Err(_) => 0,
            }
        })
    }

    fn on_pre_unlink_complete(&self, index: usize, success: bool) {
        Python::with_gil(|py| {
            let _ = self
                .py_obj
                .call_method1(py, "on_pre_unlink_complete", (index, success));
        });
    }
}

#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (records, target_prefix, execute_link_scripts=false, show_progress=false, platform=None, client=None, cache_dir=None, installed_packages=None, reinstall_packages=None, ignored_packages=None, requested_specs=None, reporter=None))]
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
    reporter: Option<PyObject>,
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

        if let Some(py_reporter) = reporter {
            installer.set_reporter(PyReporter {
                py_obj: Arc::new(py_reporter),
            });
        } else if show_progress {
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

        Ok(())
    })
}
