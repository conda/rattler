use pyo3::{
    pyclass, pymethods,
    types::{PyAnyMethods, PyDict, PyDictMethods, PyTypeMethods},
    FromPyObject, Py, PyAny, PyResult, Python,
};
use rattler_networking::{
    mirror_middleware::Mirror, s3_middleware::S3Config, GCSMiddleware, MirrorMiddleware,
    OciMiddleware,
};
use reqwest::{Request, Response};
use reqwest_middleware::{Middleware, Next};
use std::collections::HashMap;
use std::sync::Arc;
use url::Url;

use crate::error::PyRattlerError;

#[derive(FromPyObject)]
pub enum PyMiddleware {
    Mirror(PyMirrorMiddleware),
    Authentication(PyAuthenticationMiddleware),
    Oci(PyOciMiddleware),
    Gcs(PyGCSMiddleware),
    S3(PyS3Middleware),
    AddHeaders(PyAddHeadersMiddleware),
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyMirrorMiddleware {
    pub(crate) inner: HashMap<Url, Vec<Mirror>>,
}

#[pymethods]
impl PyMirrorMiddleware {
    #[new]
    pub fn __init__(inner: HashMap<String, Vec<String>>) -> PyResult<Self> {
        let mut map = HashMap::new();
        for (k, v) in inner {
            let key = Url::parse(&k).map_err(PyRattlerError::from)?;
            let value = v
                .into_iter()
                .map(|url| {
                    Url::parse(&url)
                        .map(|url| Mirror {
                            url,
                            no_zstd: false,
                            no_bz2: false,
                            max_failures: None,
                        })
                        .map_err(PyRattlerError::from)
                })
                .collect::<Result<Vec<Mirror>, PyRattlerError>>()?;
            map.insert(key, value);
        }

        Ok(Self { inner: map })
    }
}

impl From<PyMirrorMiddleware> for MirrorMiddleware {
    fn from(value: PyMirrorMiddleware) -> Self {
        MirrorMiddleware::from_map(value.inner)
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyAuthenticationMiddleware {}

#[pymethods]
impl PyAuthenticationMiddleware {
    #[new]
    pub fn __init__() -> Self {
        Self {}
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyOciMiddleware {}

#[pymethods]
impl PyOciMiddleware {
    #[new]
    pub fn __init__() -> Self {
        Self {}
    }
}

impl From<PyOciMiddleware> for OciMiddleware {
    fn from(_value: PyOciMiddleware) -> Self {
        OciMiddleware::default()
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyGCSMiddleware {}

#[pymethods]
impl PyGCSMiddleware {
    #[new]
    pub fn __init__() -> Self {
        Self {}
    }
}

impl From<PyGCSMiddleware> for GCSMiddleware {
    fn from(_value: PyGCSMiddleware) -> Self {
        GCSMiddleware
    }
}

#[derive(Clone)]
#[pyclass]
pub struct PyS3Config {
    // non-trivial enums are not supported by pyo3 as pyclasses
    pub(crate) custom: Option<PyS3ConfigCustom>,
}

#[derive(Clone)]
pub(crate) struct PyS3ConfigCustom {
    pub(crate) endpoint_url: Url,
    pub(crate) region: String,
    pub(crate) force_path_style: bool,
}

#[pymethods]
impl PyS3Config {
    #[new]
    #[pyo3(signature = (endpoint_url=None, region=None, force_path_style=None))]
    pub fn __init__(
        endpoint_url: Option<String>,
        region: Option<String>,
        force_path_style: Option<bool>,
    ) -> PyResult<Self> {
        match (endpoint_url, region, force_path_style) {
            (Some(endpoint_url), Some(region), Some(force_path_style)) => Ok(Self {
                custom: Some(PyS3ConfigCustom {
                    endpoint_url: Url::parse(&endpoint_url).map_err(PyRattlerError::from)?,
                    region,
                    force_path_style,
                }),
            }),
            (None, None, None) => Ok(Self { custom: None }),
            _ => unreachable!("Case handled in python"),
        }
    }
}

impl From<PyS3Config> for S3Config {
    fn from(_value: PyS3Config) -> Self {
        match _value.custom {
            None => S3Config::FromAWS,
            Some(custom) => S3Config::Custom {
                endpoint_url: custom.endpoint_url,
                region: custom.region,
                force_path_style: custom.force_path_style,
            },
        }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyS3Middleware {
    pub(crate) s3_config: HashMap<String, PyS3Config>,
}

#[pymethods]
impl PyS3Middleware {
    #[new]
    pub fn __init__(s3_config: HashMap<String, PyS3Config>) -> PyResult<Self> {
        Ok(Self { s3_config })
    }
}

/// A middleware that adds headers to requests based on a Python callback.
///
/// The callback receives (host, path) and should return a dict of headers to add,
/// or None to add no headers.
#[pyclass]
pub struct PyAddHeadersMiddleware {
    pub(crate) callback: Py<PyAny>,
}

impl Clone for PyAddHeadersMiddleware {
    fn clone(&self) -> Self {
        Python::with_gil(|py| Self {
            callback: self.callback.clone_ref(py),
        })
    }
}

#[pymethods]
impl PyAddHeadersMiddleware {
    #[new]
    pub fn __init__(callback: Py<PyAny>) -> Self {
        Self { callback }
    }
}

/// The actual middleware implementation that wraps the Python callback.
/// Uses Arc to allow cloning without requiring Py<PyAny> to be Clone.
#[derive(Clone)]
pub struct AddHeadersMiddleware {
    callback: Arc<Py<PyAny>>,
}

impl AddHeadersMiddleware {
    /// Create a new `AddHeadersMiddleware` from a Python callback.
    pub fn new(callback: Py<PyAny>) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }
}

impl From<PyAddHeadersMiddleware> for AddHeadersMiddleware {
    fn from(value: PyAddHeadersMiddleware) -> Self {
        AddHeadersMiddleware::new(value.callback)
    }
}

#[async_trait::async_trait]
impl Middleware for AddHeadersMiddleware {
    async fn handle(
        &self,
        mut req: Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        // Extract host and path from the URL
        let url = req.url();
        let host = url.host_str().unwrap_or("").to_string();
        let path = url.path().to_string();

        // Call the Python callback with host and path
        let callback = self.callback.clone();
        let headers_to_add: Option<HashMap<String, String>> = Python::with_gil(
            |py| -> reqwest_middleware::Result<Option<HashMap<String, String>>> {
                let result = callback
                    .call1(py, (host.as_str(), path.as_str()))
                    .map_err(|e| {
                        reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                            "Python callback failed: {e}"
                        ))
                    })?;

                // Check if the result is None
                if result.is_none(py) {
                    return Ok(None);
                }

                // Try to extract as a dictionary
                let dict = result.downcast_bound::<PyDict>(py).map_err(|_e| {
                    let type_name = result
                        .bind(py)
                        .get_type()
                        .name()
                        .map_or_else(|_| "unknown".to_string(), |n| n.to_string());
                    reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                        "Python callback must return a dict or None, got: {type_name}",
                    ))
                })?;

                // Convert the dict to a HashMap<String, String>
                let mut headers = HashMap::new();
                for (key, value) in dict.iter() {
                    let key_str: String = key.extract().map_err(|e| {
                        reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                            "Header key must be a string: {e}"
                        ))
                    })?;
                    let value_str: String = value.extract().map_err(|e| {
                        reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                            "Header value must be a string: {e}"
                        ))
                    })?;
                    headers.insert(key_str, value_str);
                }

                Ok(Some(headers))
            },
        )?;

        // Add the headers to the request
        if let Some(headers) = headers_to_add {
            for (key, value) in headers {
                let header_name =
                    reqwest::header::HeaderName::from_bytes(key.as_bytes()).map_err(|e| {
                        reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                            "Invalid header name '{key}': {e}"
                        ))
                    })?;
                let header_value = reqwest::header::HeaderValue::from_str(&value).map_err(|e| {
                    reqwest_middleware::Error::Middleware(anyhow::anyhow!(
                        "Invalid header value for '{key}': {e}"
                    ))
                })?;
                req.headers_mut().insert(header_name, header_value);
            }
        }

        next.run(req, extensions).await
    }
}
