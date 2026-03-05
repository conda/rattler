use pyo3::{prelude::*, types::PyBytes};
use pyo3_async_runtimes::tokio::future_into_py;
use pyo3_file::PyFileLikeObject;
use rattler_package_streaming::ExtractResult;
use std::path::{Path, PathBuf};
use url::Url;

use crate::{networking::client::PyClientWithMiddleware, utils::sha256_from_pybytes};

fn convert_result(py: Python<'_>, result: ExtractResult) -> (PyObject, PyObject) {
    let sha256_bytes = PyBytes::new(py, &result.sha256);
    let md5_bytes = PyBytes::new(py, &result.md5);

    (sha256_bytes.into(), md5_bytes.into())
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
