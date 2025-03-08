use pyo3::{pyclass, pymethods, FromPyObject, PyResult};
use rattler_networking::{
    mirror_middleware::Mirror, s3_middleware::S3Config, GCSMiddleware, MirrorMiddleware,
    OciMiddleware,
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
