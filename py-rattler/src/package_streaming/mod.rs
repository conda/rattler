use futures::StreamExt;
use pyo3::{prelude::*, types::PyBytes};
use pyo3_async_runtimes::tokio::future_into_py;
use pyo3_file::PyFileLikeObject;
use rattler_package_streaming::ExtractResult;
use rattler_package_streaming::seek::PackageFileEntry;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use url::Url;

use crate::{networking::client::PyClientWithMiddleware, utils::sha256_from_pybytes};

fn convert_result(py: Python<'_>, result: ExtractResult) -> (PyObject, PyObject) {
    let sha256_bytes = PyBytes::new(py, &result.sha256);
    let md5_bytes = PyBytes::new(py, &result.md5);

    (sha256_bytes.into(), md5_bytes.into())
}

fn parse_url(url: &str) -> PyResult<Url> {
    Url::parse(url)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Invalid URL: {e}")))
}

fn io_error<E: std::fmt::Display>(error: E) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyIOError, _>(error.to_string())
}

#[pyclass]
#[derive(Clone)]
pub struct PyPackageFileEntry {
    pub(crate) inner: PackageFileEntry,
}

impl From<PackageFileEntry> for PyPackageFileEntry {
    fn from(value: PackageFileEntry) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyPackageFileEntry {
    #[getter]
    pub fn path(&self) -> PathBuf {
        self.inner.path.clone()
    }

    #[getter]
    pub fn size(&self) -> u64 {
        self.inner.size
    }

    fn __repr__(&self) -> String {
        format!(
            "PackageFileEntry(path='{}', size={})",
            self.inner.path.display(),
            self.inner.size
        )
    }
}

#[pyfunction]
pub fn extract_tar_bz2(
    py: Python<'_>,
    reader: PyObject,
    destination: String,
) -> PyResult<(PyObject, PyObject)> {
    // Convert Python file-like object to Read implementation
    let reader = PyFileLikeObject::new(reader)?;
    let destination = Path::new(&destination);

    // Call the Rust function
    match rattler_package_streaming::read::extract_tar_bz2(reader, destination) {
        Ok(result) => Ok(convert_result(py, result)),
        Err(e) => Err(PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string())),
    }
}

#[pyfunction]
pub fn extract(
    py: Python<'_>,
    source: PathBuf,
    destination: PathBuf,
) -> PyResult<(PyObject, PyObject)> {
    match rattler_package_streaming::fs::extract(&source, &destination) {
        Ok(result) => Ok(convert_result(py, result)),
        Err(e) => Err(PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string())),
    }
}

#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (client, url, destination, expected_sha256=None))]
pub fn download_and_extract<'a>(
    py: Python<'a>,
    client: PyClientWithMiddleware,
    url: String,
    destination: PathBuf,
    expected_sha256: Option<Bound<'_, PyBytes>>,
) -> PyResult<Bound<'a, PyAny>> {
    // Parse URL
    let url = Url::parse(&url).map_err(|e| {
        PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Invalid URL: {e}"))
    })?;

    // Convert SHA256 if provided
    let sha256 = expected_sha256
        .map(|b| sha256_from_pybytes(b))
        .transpose()?;

    // Create the async future
    let future = async move {
        rattler_package_streaming::reqwest::tokio::extract(
            client.into(),
            url,
            &destination,
            sha256,
            None,
        )
        .await
        .map(|result| Python::with_gil(|py| convert_result(py, result)))
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string()))
    };

    // Convert the future to a Python awaitable
    future_into_py(py, future)
}

#[pyfunction]
pub fn fetch_raw_package_file_from_url<'a>(
    py: Python<'a>,
    client: PyClientWithMiddleware,
    url: String,
    path: String,
) -> PyResult<Bound<'a, PyAny>> {
    let url = parse_url(&url)?;
    let path = PathBuf::from(path);
    let future = async move {
        let bytes = rattler_package_streaming::reqwest::fetch::fetch_file_from_remote_url(
            client.into(),
            url,
            &path,
        )
        .await
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string()))?
        .ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyFileNotFoundError, _>(format!(
                "file '{}' not found in package",
                path.display()
            ))
        })?;

        Python::with_gil(|py| Ok(PyBytes::new(py, &bytes).into_any().unbind()))
    };

    future_into_py(py, future)
}

#[pyfunction]
pub fn list_info_files(path: PathBuf) -> PyResult<Vec<PyPackageFileEntry>> {
    rattler_package_streaming::seek::list_info_files(&path)
        .map(|entries| entries.into_iter().map(Into::into).collect())
        .map_err(io_error)
}

#[pyfunction]
pub fn list_info_files_from_url<'a>(
    py: Python<'a>,
    client: PyClientWithMiddleware,
    url: String,
) -> PyResult<Bound<'a, PyAny>> {
    let url = parse_url(&url)?;
    let future = async move {
        let entries = rattler_package_streaming::reqwest::fetch::list_info_files_from_remote_url(
            client.into(),
            url,
        )
        .await
        .map_err(io_error)?;

        Python::with_gil(|py| {
            let items = entries
                .into_iter()
                .map(|entry| Py::new(py, PyPackageFileEntry::from(entry)))
                .collect::<PyResult<Vec<_>>>()?;
            Ok(pyo3::types::PyList::new(py, items)?.into_any().unbind())
        })
    };

    future_into_py(py, future)
}


#[pyfunction]
pub fn download_to_path<'a>(
    py: Python<'a>,
    client: PyClientWithMiddleware,
    url: String,
    destination: PathBuf,
) -> PyResult<Bound<'a, PyAny>> {
    let url = parse_url(&url)?;
    let future = async move {
        let client: reqwest_middleware::ClientWithMiddleware = client.into();

        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(io_error)?;
        }

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?
            .error_for_status()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;

        let mut file = tokio::fs::File::create(&destination)
            .await
            .map_err(io_error)?;
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(io_error)?;
            file.write_all(&chunk).await.map_err(io_error)?;
        }

        file.flush().await.map_err(io_error)?;
        Ok(())
    };

    future_into_py(py, future)
}

#[pyfunction]
pub fn download_bytes<'a>(
    py: Python<'a>,
    client: PyClientWithMiddleware,
    url: String,
) -> PyResult<Bound<'a, PyAny>> {
    let url = parse_url(&url)?;
    let future = async move {
        let client: reqwest_middleware::ClientWithMiddleware = client.into();
        let bytes = client
            .get(url)
            .send()
            .await
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?
            .error_for_status()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?
            .bytes()
            .await
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;

        Python::with_gil(|py| Ok(PyBytes::new(py, &bytes).into_any().unbind()))
    };

    future_into_py(py, future)
}

#[pyfunction]
pub fn download_to_writer<'a>(
    py: Python<'a>,
    client: PyClientWithMiddleware,
    url: String,
    writer: Py<PyAny>,
) -> PyResult<Bound<'a, PyAny>> {
    let url = parse_url(&url)?;
    let future = async move {
        let client: reqwest_middleware::ClientWithMiddleware = client.into();
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?
            .error_for_status()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;

        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(io_error)?;
            Python::with_gil(|py| {
                writer
                    .bind(py)
                    .call_method1("write", (PyBytes::new(py, &chunk),))
                    .map(|_| ())
            })?;
        }

        Ok(())
    };

    future_into_py(py, future)
}
