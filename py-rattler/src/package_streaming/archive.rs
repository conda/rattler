use std::path::PathBuf;
use std::sync::Arc;

use pyo3::exceptions::{PyRuntimeError, PyStopAsyncIteration, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PySlice};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_conda_types::package::{AboutJson, IndexJson, PathsJson, RunExportsJson};
use rattler_package_streaming::archive::{
    ArchiveAccess, ArchiveEntryKind, ByteRange, FileRanges, PackageArchive, RemoteArchiveOptions,
    Section, SectionEntry, SectionStream, SparsePolicy,
};
use url::Url;

use super::io_error;
use crate::about_json::PyAboutJson;
use crate::index_json::PyIndexJson;
use crate::networking::client::PyClientWithMiddleware;
use crate::paths_json::PyPathsJson;
use crate::run_exports_json::PyRunExportsJson;

fn parse_sparse_policy(policy: &str) -> PyResult<SparsePolicy> {
    match policy {
        "prefer" => Ok(SparsePolicy::Prefer),
        "require" => Ok(SparsePolicy::Require),
        "disable" => Ok(SparsePolicy::Disable),
        _ => Err(PyValueError::new_err(format!(
            "invalid sparse policy {policy:?}: expected 'prefer', 'require' or 'disable'"
        ))),
    }
}

fn parse_section(section: &str) -> PyResult<Section> {
    match section {
        "info" => Ok(Section::Info),
        "pkg" => Ok(Section::Content),
        _ => Err(PyValueError::new_err(format!(
            "invalid section {section:?}: expected 'info' or 'pkg'"
        ))),
    }
}

/// Converts Python `slice` objects into byte ranges.
///
/// Slice semantics apply: negative bounds count from the end of the file and
/// `None` bounds mean the start or end. A step other than `None` or `1` is
/// rejected with a `ValueError`; anything that is not a `slice` with a
/// `TypeError`.
fn parse_ranges(ranges: &[Bound<'_, PyAny>]) -> PyResult<Vec<ByteRange>> {
    ranges
        .iter()
        .map(|range| {
            let Ok(slice) = range.cast::<PySlice>() else {
                return Err(PyTypeError::new_err(format!(
                    "ranges must be slice objects, got {}",
                    range
                        .get_type()
                        .name()
                        .map_or_else(|_err| "<unknown>".to_owned(), |name| name.to_string())
                )));
            };
            let step: Option<i64> = slice.getattr("step")?.extract()?;
            if !matches!(step, None | Some(1)) {
                return Err(PyValueError::new_err(
                    "byte ranges must be contiguous: slice step must be None or 1",
                ));
            }
            let start: Option<i64> = slice.getattr("start")?.extract()?;
            let stop: Option<i64> = slice.getattr("stop")?.extract()?;
            let start = start.unwrap_or(0);
            Ok(match (start, stop) {
                (start, Some(end)) if start >= 0 && end >= 0 => ByteRange::Bounded {
                    start: start as u64,
                    end: end as u64,
                },
                (start, None) if start >= 0 => ByteRange::From(start as u64),
                (start, None) => ByteRange::Suffix(start.unsigned_abs()),
                (start, end) => ByteRange::Slice { start, end },
            })
        })
        .collect()
}

/// The result of a ranged read of one file: its size and one `bytes` object
/// per requested range, in request order.
#[pyclass]
pub struct PyFileRanges {
    #[pyo3(get)]
    size: u64,
    chunks: Vec<Py<PyBytes>>,
}

impl PyFileRanges {
    fn new(py: Python<'_>, ranges: FileRanges) -> Self {
        Self {
            size: ranges.size,
            chunks: ranges
                .chunks
                .iter()
                .map(|chunk| PyBytes::new(py, chunk).unbind())
                .collect(),
        }
    }
}

#[pymethods]
impl PyFileRanges {
    #[getter]
    fn chunks(&self, py: Python<'_>) -> Vec<Py<PyBytes>> {
        self.chunks
            .iter()
            .map(|chunk| chunk.clone_ref(py))
            .collect()
    }
}

/// A conda package archive (local or remote) that is opened once and can then
/// be read many times.
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PyPackageArchive {
    inner: PackageArchive,
}

#[pymethods]
impl PyPackageArchive {
    /// Opens a remote package archive. For `.conda` archives on servers with
    /// range support this costs a single HTTP range request.
    #[staticmethod]
    pub fn from_url<'a>(
        py: Python<'a>,
        client: PyClientWithMiddleware,
        url: String,
        sparse: String,
        max_spool_size: Option<u64>,
    ) -> PyResult<Bound<'a, PyAny>> {
        let url =
            Url::parse(&url).map_err(|e| PyValueError::new_err(format!("Invalid URL: {e}")))?;
        let mut options =
            RemoteArchiveOptions::new().with_sparse_policy(parse_sparse_policy(&sparse)?);
        if let Some(max_spool_size) = max_spool_size {
            options = options.with_max_spool_size(max_spool_size);
        }
        future_into_py(py, async move {
            let inner = PackageArchive::from_url_with_options(client.into(), url, options)
                .await
                .map_err(io_error)?;
            Ok(Self { inner })
        })
    }

    /// Opens a package archive from a local file.
    #[staticmethod]
    pub fn from_path(py: Python<'_>, path: PathBuf) -> PyResult<Bound<'_, PyAny>> {
        future_into_py(py, async move {
            let inner = PackageArchive::from_path(&path).await.map_err(io_error)?;
            Ok(Self { inner })
        })
    }

    /// Returns how the archive is accessed: "sparse", "local" or "spooled".
    pub fn access(&self) -> &'static str {
        match self.inner.access() {
            ArchiveAccess::Sparse => "sparse",
            ArchiveAccess::Local => "local",
            ArchiveAccess::Spooled => "spooled",
            _ => "unknown",
        }
    }

    /// Reads a single file from the package; `None` if it does not exist.
    pub fn read_file<'a>(&self, py: Python<'a>, path: String) -> PyResult<Bound<'a, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let content = inner.read_file(&path).await.map_err(io_error)?;
            Python::attach(|py| {
                Ok(match content {
                    Some(bytes) => PyBytes::new(py, &bytes).into_any().unbind(),
                    None => py.None(),
                })
            })
        })
    }

    /// Reads only the given byte ranges (Python slices) of a single file in
    /// one forward pass; `None` if the file does not exist.
    pub fn read_file_ranges<'a>(
        &self,
        py: Python<'a>,
        path: String,
        ranges: Vec<Bound<'a, PyAny>>,
    ) -> PyResult<Bound<'a, PyAny>> {
        let inner = self.inner.clone();
        let ranges = parse_ranges(&ranges)?;
        future_into_py(py, async move {
            let result = inner
                .read_file_ranges(&path, &ranges)
                .await
                .map_err(io_error)?;
            Python::attach(|py| Ok(result.map(|ranges| PyFileRanges::new(py, ranges))))
        })
    }

    /// Reads multiple files, grouped per section with one early-aborted
    /// streaming pass per section. Returns a dict mapping every requested
    /// path to its contents (or `None` when absent).
    pub fn read_files<'a>(&self, py: Python<'a>, paths: Vec<String>) -> PyResult<Bound<'a, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let result = inner.read_files(paths).await.map_err(io_error)?;
            Python::attach(|py| {
                let dict = PyDict::new(py);
                for (path, content) in result {
                    let key = path.to_string_lossy().into_owned();
                    match content {
                        Some(bytes) => dict.set_item(key, PyBytes::new(py, &bytes))?,
                        None => dict.set_item(key, py.None())?,
                    }
                }
                Ok(dict.unbind())
            })
        })
    }

    /// Reads and parses `info/index.json`.
    pub fn index_json<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let value: IndexJson = inner.read_package_file().await.map_err(io_error)?;
            Ok(PyIndexJson::from(value))
        })
    }

    /// Reads and parses `info/about.json`.
    pub fn about_json<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let value: AboutJson = inner.read_package_file().await.map_err(io_error)?;
            Ok(PyAboutJson::from(value))
        })
    }

    /// Reads and parses `info/paths.json`.
    pub fn paths_json<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let value: PathsJson = inner.read_package_file().await.map_err(io_error)?;
            Ok(PyPathsJson::from(value))
        })
    }

    /// Reads and parses `info/run_exports.json`; `None` when absent.
    pub fn run_exports_json<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let value: Option<RunExportsJson> =
                inner.try_read_package_file().await.map_err(io_error)?;
            Ok(value.map(PyRunExportsJson::from))
        })
    }

    /// Lists the paths of all files in one section ("info" or "pkg").
    pub fn list_files<'a>(&self, py: Python<'a>, section: String) -> PyResult<Bound<'a, PyAny>> {
        let inner = self.inner.clone();
        let section = parse_section(&section)?;
        future_into_py(py, async move {
            let paths = inner.list_files(section).await.map_err(io_error)?;
            Ok(paths
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<String>>())
        })
    }

    /// Opens a stream over the tar entries of one section ("info" or "pkg").
    pub fn stream<'a>(&self, py: Python<'a>, section: String) -> PyResult<Bound<'a, PyAny>> {
        let inner = self.inner.clone();
        let section = parse_section(&section)?;
        future_into_py(py, async move {
            let stream = inner.stream(section).await.map_err(io_error)?;
            Ok(PySectionStream {
                state: Arc::new(tokio::sync::Mutex::new(StreamState {
                    stream,
                    current: None,
                    generation: 0,
                })),
            })
        })
    }
}

struct StreamState {
    stream: SectionStream,
    /// The most recently yielded entry. Kept here (not inside the Python
    /// entry object) because the tar stream only allows reading the current
    /// entry before advancing.
    current: Option<SectionEntry>,
    generation: u64,
}

/// An async iterator over the tar entries of one package section.
#[pyclass]
pub struct PySectionStream {
    state: Arc<tokio::sync::Mutex<StreamState>>,
}

#[pymethods]
impl PySectionStream {
    fn __aiter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __anext__<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let state = self.state.clone();
        future_into_py(py, async move {
            let mut guard = state.lock().await;
            // Advancing discards the previous entry; its unread body is
            // skipped by the tar reader.
            guard.current = None;
            match guard.stream.next_entry().await.map_err(io_error)? {
                None => Err(PyStopAsyncIteration::new_err(())),
                Some(entry) => {
                    let name = entry.path().to_string_lossy().into_owned();
                    let size = entry.size().map_err(io_error)?;
                    let kind = entry.kind();
                    let is_file = kind == ArchiveEntryKind::File;
                    let is_symlink = kind == ArchiveEntryKind::Symlink;
                    let is_hardlink = kind == ArchiveEntryKind::Hardlink;
                    let is_link = kind.is_link();
                    let link_target = entry
                        .link_target()
                        .map_err(io_error)?
                        .map(|target| target.to_string_lossy().into_owned());
                    guard.generation += 1;
                    let generation = guard.generation;
                    guard.current = Some(entry);
                    drop(guard);
                    Ok(PyArchiveEntry {
                        name,
                        size,
                        is_file,
                        is_link,
                        is_symlink,
                        is_hardlink,
                        link_target,
                        generation,
                        state: state.clone(),
                    })
                }
            }
        })
    }
}

/// One tar entry yielded by a section stream. Call `read()` to get the entry
/// contents before advancing the stream; not calling it skips the entry.
#[pyclass]
pub struct PyArchiveEntry {
    #[pyo3(get)]
    name: String,
    #[pyo3(get)]
    size: u64,
    #[pyo3(get)]
    is_file: bool,
    #[pyo3(get)]
    is_link: bool,
    #[pyo3(get)]
    is_symlink: bool,
    #[pyo3(get)]
    is_hardlink: bool,
    #[pyo3(get)]
    link_target: Option<String>,
    generation: u64,
    state: Arc<tokio::sync::Mutex<StreamState>>,
}

#[pymethods]
impl PyArchiveEntry {
    /// Reads the contents of this entry. Reading a link is an error; links
    /// are not followed.
    pub fn read<'a>(&self, py: Python<'a>) -> PyResult<Bound<'a, PyAny>> {
        let state = self.state.clone();
        let generation = self.generation;
        future_into_py(py, async move {
            let mut guard = state.lock().await;
            if guard.generation != generation {
                return Err(PyRuntimeError::new_err(
                    "entry is no longer readable because the stream has advanced past it",
                ));
            }
            let entry = guard
                .current
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("entry has already been read"))?;
            let buf = entry.read().await.map_err(io_error)?;
            guard.current = None;
            Python::attach(|py| Ok(PyBytes::new(py, &buf).unbind()))
        })
    }

    /// Reads only the given byte ranges (Python slices) of this entry in one
    /// forward pass. Like `read()`, this consumes the entry: it can be called
    /// once, and not after `read()`.
    pub fn read_ranges<'a>(
        &self,
        py: Python<'a>,
        ranges: Vec<Bound<'a, PyAny>>,
    ) -> PyResult<Bound<'a, PyAny>> {
        let state = self.state.clone();
        let generation = self.generation;
        let ranges = parse_ranges(&ranges)?;
        future_into_py(py, async move {
            let mut guard = state.lock().await;
            if guard.generation != generation {
                return Err(PyRuntimeError::new_err(
                    "entry is no longer readable because the stream has advanced past it",
                ));
            }
            let entry = guard
                .current
                .as_mut()
                .ok_or_else(|| PyRuntimeError::new_err("entry has already been read"))?;
            let result = entry.read_ranges(&ranges).await.map_err(io_error)?;
            // The body has been consumed up to the largest requested end; a
            // second read would continue from there.
            guard.current = None;
            Python::attach(|py| Ok(PyFileRanges::new(py, result)))
        })
    }
}
