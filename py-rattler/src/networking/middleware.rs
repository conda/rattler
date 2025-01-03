use pyo3::{pyclass, pymethods, FromPyObject, PyResult};
use rattler_networking::{
    mirror_middleware::Mirror, s3_middleware::S3Config, AuthenticationMiddleware, AuthenticationStorage, GCSMiddleware, MirrorMiddleware, OciMiddleware, S3Middleware
};
use std::collections::HashMap;
use url::Url;

use crate::error::PyRattlerError;

#[derive(FromPyObject)]
pub enum PyMiddleware {
    Mirror(PyMirrorMiddleware),
    Authentication(PyAuthenticationMiddleware),
    Oci(PyOciMiddleware),
    Gcs(PyGCSMiddleware),
    S3(PyS3Middleware),
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
                            no_jlap: false,
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

impl From<PyAuthenticationMiddleware> for AuthenticationMiddleware {
    fn from(_value: PyAuthenticationMiddleware) -> Self {
        AuthenticationMiddleware::new(AuthenticationStorage::default())
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
        OciMiddleware
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

#[pyclass]
#[derive(Clone)]
pub struct PyS3Config {
    pub(crate) endpoint_url: Url,
    pub(crate) region: String,
    pub(crate) force_path_style: bool,
}

#[pymethods]
impl PyS3Config {
    #[new]
    pub fn __init__(
        endpoint_url: String,
        region: String,
        force_path_style: bool,
    ) -> PyResult<Self> {
        Ok(Self {
            endpoint_url: Url::parse(&endpoint_url).map_err(PyRattlerError::from)?,
            region,
            force_path_style,
        })
    }
}

impl From<PyS3Config> for S3Config {
    fn from(_value: PyS3Config) -> Self {
        S3Config {
            auth_storage: AuthenticationStorage::default(),
            endpoint_url: _value.endpoint_url,
            region: _value.region,
            force_path_style: _value.force_path_style,
        }
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyS3Middleware {
    pub(crate) s3_config: Option<PyS3Config>,
}

#[pymethods]
impl PyS3Middleware {
    #[new]
    #[pyo3(signature = (s3_config=None))]
    pub fn __init__(
        s3_config: Option<PyS3Config>,
    ) -> PyResult<Self> {
        Ok(Self { s3_config })
    }
}

impl From<PyS3Middleware> for S3Middleware {
    fn from(_value: PyS3Middleware) -> Self {
        match _value.s3_config {
            Some(config) => S3Middleware::new(Some(config.into())),
            None => S3Middleware::new(None),
        }
    }
}
