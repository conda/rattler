use crate::{error::PyRattlerError, networking::middleware::PyMiddleware};
use pyo3::{pyclass, pymethods, PyResult};
use rattler_networking::{
    AuthenticationMiddleware, GCSMiddleware, MirrorMiddleware, OciMiddleware,
};
use reqwest_middleware::ClientWithMiddleware;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyClientWithMiddleware {
    pub(crate) inner: ClientWithMiddleware,
}

#[pymethods]
impl PyClientWithMiddleware {
    #[new]
    #[pyo3(signature = (middlewares=None))]
    pub fn new(middlewares: Option<Vec<PyMiddleware>>) -> PyResult<Self> {
        let middlewares = middlewares.unwrap_or_default();
        let mut client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new());
        for middleware in middlewares {
            match middleware {
                PyMiddleware::Mirror(middleware) => {
                    client = client.with(MirrorMiddleware::from(middleware));
                }
                PyMiddleware::Authentication(_) => {
                    client =
                        client.with(AuthenticationMiddleware::new().map_err(PyRattlerError::from)?);
                }
                PyMiddleware::Oci(middleware) => {
                    client = client.with(OciMiddleware::from(middleware));
                }
                PyMiddleware::Gcs(middleware) => {
                    client = client.with(GCSMiddleware::from(middleware));
                }
            }
        }
        let client = client.build();

        Ok(Self { inner: client })
    }
}

impl From<PyClientWithMiddleware> for ClientWithMiddleware {
    fn from(value: PyClientWithMiddleware) -> Self {
        value.inner
    }
}
