use pyo3::{pyclass, pymethods, FromPyObject};
use rattler_networking::{
    mirror_middleware::Mirror, AuthenticationMiddleware, AuthenticationStorage, MirrorMiddleware,
};
use std::collections::HashMap;
use url::Url;

#[derive(FromPyObject)]
pub enum PyMiddleware {
    MirrorMiddleware(PyMirrorMiddleware),
    AuthenticationMiddleware(PyAuthenticationMiddleware),
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
    pub fn __init__(inner: HashMap<String, Vec<String>>) -> Self {
        let map = inner
            .into_iter()
            .map(|(k, v)| {
                (
                    Url::parse(&k).unwrap(),
                    v.into_iter()
                        .map(|url| Mirror {
                            url: Url::parse(&url).unwrap(),
                            no_zstd: false,
                            no_bz2: false,
                            no_jlap: false,
                            max_failures: None,
                        })
                        .collect(),
                )
            })
            .collect();

        Self { inner: map }
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
