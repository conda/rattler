use crate::networking::middleware::PyMiddleware;
use pyo3::{pyclass, pymethods};
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
    pub fn new(middlewares: Option<Vec<PyMiddleware>>) -> Self {
        let middlewares = middlewares.unwrap_or_default();
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new());
        let client = middlewares
            .into_iter()
            .fold(client, |client, middleware| match middleware {
                PyMiddleware::Mirror(middleware) => client.with(MirrorMiddleware::from(middleware)),
                PyMiddleware::Authentication(middleware) => {
                    client.with(AuthenticationMiddleware::from(middleware))
                }
                PyMiddleware::Oci(middleware) => client.with(OciMiddleware::from(middleware)),
                PyMiddleware::Gcs(middleware) => client.with(GCSMiddleware::from(middleware)),
            });
        let client = client.build();

        Self { inner: client }
    }
}

impl From<PyClientWithMiddleware> for ClientWithMiddleware {
    fn from(value: PyClientWithMiddleware) -> Self {
        value.inner
    }
}
