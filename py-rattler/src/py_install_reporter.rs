use pyo3::types::PyAnyMethods;
use pyo3::{Py, PyAny, Python};
use rattler::install::{Reporter, Transaction};
use rattler_conda_types::{PrefixRecord, RepoDataRecord};
use std::sync::{Arc, Mutex};

/// Shared error state between the reporter and the caller so that a delegate
/// exception can be propagated after `install()` returns.
pub type SharedError = Arc<Mutex<Option<pyo3::PyErr>>>;

/// A reporter that bridges Rust install events to a Python delegate object.
///
/// The delegate may implement any subset of the following methods:
///   - `on_unlink_start(package_name: str) -> None`
///   - `on_unlink_complete(package_name: str) -> None`
///   - `on_link_start(package_name: str) -> None`
///   - `on_link_complete(package_name: str) -> None`
///
/// If a method is missing on the delegate it is silently skipped.
/// If a method raises an exception the error is captured and the install will
/// fail with that exception once the current batch of concurrent operations
/// completes.
pub struct PyInstallReporter {
    delegate: Py<PyAny>,
    index_names: Mutex<Vec<String>>,
    error: SharedError,
}

impl PyInstallReporter {
    pub fn new(delegate: Py<PyAny>, error: SharedError) -> Self {
        Self {
            delegate,
            index_names: Mutex::new(Vec::new()),
            error,
        }
    }

    fn store_name(&self, name: String) -> usize {
        let mut names = self.index_names.lock().unwrap();
        let idx = names.len();
        names.push(name);
        idx
    }

    fn get_name(&self, index: usize) -> String {
        let names = self.index_names.lock().unwrap();
        names[index].clone()
    }

    fn call_delegate(&self, method: &str, arg: &str) {
        if self.error.lock().unwrap().is_some() {
            return;
        }
        Python::with_gil(|py| {
            let delegate = self.delegate.bind(py);
            if let Ok(true) = delegate.hasattr(method) {
                if let Err(err) = delegate.call_method1(method, (arg,)) {
                    let mut guard = self.error.lock().unwrap();
                    if guard.is_none() {
                        *guard = Some(err);
                    }
                }
            }
        });
    }
}

impl Reporter for PyInstallReporter {
    fn on_transaction_start(&self, _transaction: &Transaction<PrefixRecord, RepoDataRecord>) {}
    fn on_transaction_operation_start(&self, _operation: usize) {}
    fn on_populate_cache_start(&self, _operation: usize, _record: &RepoDataRecord) -> usize {
        0
    }
    fn on_validate_start(&self, _cache_entry: usize) -> usize {
        0
    }
    fn on_validate_complete(&self, _validate_idx: usize) {}
    fn on_download_start(&self, _cache_entry: usize) -> usize {
        0
    }
    fn on_download_progress(&self, _download_idx: usize, _progress: u64, _total: Option<u64>) {}
    fn on_download_completed(&self, _download_idx: usize) {}
    fn on_populate_cache_complete(&self, _cache_entry: usize) {}

    fn on_unlink_start(&self, _operation: usize, record: &PrefixRecord) -> usize {
        let name = record
            .repodata_record
            .package_record
            .name
            .as_normalized()
            .to_string();
        let idx = self.store_name(name.clone());
        self.call_delegate("on_unlink_start", &name);
        idx
    }

    fn on_unlink_complete(&self, index: usize) {
        let name = self.get_name(index);
        self.call_delegate("on_unlink_complete", &name);
    }

    fn on_link_start(&self, _operation: usize, record: &RepoDataRecord) -> usize {
        let name = record.package_record.name.as_normalized().to_string();
        let idx = self.store_name(name.clone());
        self.call_delegate("on_link_start", &name);
        idx
    }

    fn on_link_complete(&self, index: usize) {
        let name = self.get_name(index);
        self.call_delegate("on_link_complete", &name);
    }

    fn on_transaction_operation_complete(&self, _operation: usize) {}
    fn on_transaction_complete(&self) {}
    fn on_post_link_start(&self, _package_name: &str, _script_path: &str) -> usize {
        0
    }
    fn on_post_link_complete(&self, _index: usize, _success: bool) {}
    fn on_pre_unlink_start(&self, _package_name: &str, _script_path: &str) -> usize {
        0
    }
    fn on_pre_unlink_complete(&self, _index: usize, _success: bool) {}
}
