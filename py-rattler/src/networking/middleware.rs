use pyo3::{pyclass, pymethods, FromPyObject, PyResult};
use rattler_networking::{
    mirror_middleware::Mirror, AuthenticationMiddleware, AuthenticationStorage, GCSMiddleware,
    MirrorMiddleware, OciMiddleware,
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
