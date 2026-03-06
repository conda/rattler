use pyo3::{prelude::*, types::PyBytes};
use pyo3_async_runtimes::tokio::future_into_py;
use pyo3_file::PyFileLikeObject;
use rattler_conda_types::package::{AboutJson, IndexJson, PathsJson, RunExportsJson};
use rattler_package_streaming::ExtractResult;
use std::path::{Path, PathBuf};
use url::Url;

use crate::{
    about_json::PyAboutJson, index_json::PyIndexJson, networking::client::PyClientWithMiddleware,
    paths_json::PyPathsJson, run_exports_json::PyRunExportsJson, utils::sha256_from_pybytes,
};

fn convert_result(py: Python<'_>, result: ExtractResult) -> (PyObject, PyObject) {
    let sha256_bytes = PyBytes::new(py, &result.sha256);
    let md5_bytes = PyBytes::new(py, &result.md5);

    (sha256_bytes.into(), md5_bytes.into())
}

#[pyclass(eq, eq_int)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum PyPackageFile {
    Index,
    About,
    Paths,
    RunExports,
}

fn parse_url(url: &str) -> PyResult<Url> {
    Url::parse(url)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Invalid URL: {e}")))
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
pub fn fetch_package_file_from_url<'a>(
    py: Python<'a>,
    client: PyClientWithMiddleware,
    url: String,
    package_file: PyPackageFile,
) -> PyResult<Bound<'a, PyAny>> {
    let url = parse_url(&url)?;
    let future = async move {
        match package_file {
            PyPackageFile::Index => {
                let index_json =
                    rattler_package_streaming::reqwest::fetch::fetch_package_file_from_url::<
                        IndexJson,
                    >(client.into(), url)
                    .await
                    .map(PyIndexJson::from)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string()))?;
                Python::with_gil(|py| Ok(Py::new(py, index_json)?.into_any()))
            }
            PyPackageFile::About => {
                let about_json =
                    rattler_package_streaming::reqwest::fetch::fetch_package_file_from_url::<
                        AboutJson,
                    >(client.into(), url)
                    .await
                    .map(PyAboutJson::from)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string()))?;
                Python::with_gil(|py| Ok(Py::new(py, about_json)?.into_any()))
            }
            PyPackageFile::Paths => {
                let paths_json =
                    rattler_package_streaming::reqwest::fetch::fetch_package_file_from_url::<
                        PathsJson,
                    >(client.into(), url)
                    .await
                    .map(PyPathsJson::from)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string()))?;
                Python::with_gil(|py| Ok(Py::new(py, paths_json)?.into_any()))
            }
            PyPackageFile::RunExports => {
                let run_exports_json =
                    rattler_package_streaming::reqwest::fetch::fetch_package_file_from_url::<
                        RunExportsJson,
                    >(client.into(), url)
                    .await
                    .map(PyRunExportsJson::from)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyIOError, _>(e.to_string()))?;
                Python::with_gil(|py| Ok(Py::new(py, run_exports_json)?.into_any()))
            }
        }
    };

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
        let bytes =
            rattler_package_streaming::reqwest::sparse::fetch_file_from_remote_conda(
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
