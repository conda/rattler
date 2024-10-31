use crate::networking::middleware::PyMiddleware;
use pyo3::{pyclass, pymethods};
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage, MirrorMiddleware};
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
    pub fn new(middlewares: Option<Vec<PyMiddleware>>) -> Self {
        let middlewares = match middlewares {
            Some(middlewares) => middlewares,
            None => vec![],
        };
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new());
        let client = middlewares
            .into_iter()
            .fold(client, |client, middleware| match middleware {
                PyMiddleware::MirrorMiddleware(middleware) => {
                    client.with(MirrorMiddleware::from(middleware))
                }
                PyMiddleware::AuthenticationMiddleware(middleware) => {
                    client.with(AuthenticationMiddleware::from(middleware))
                }
            });
        let client = client.build();

        Self { inner: client }
    }
}

impl From<ClientWithMiddleware> for PyClientWithMiddleware {
    fn from(value: ClientWithMiddleware) -> Self {
        Self { inner: value }
    }
}

impl From<PyClientWithMiddleware> for ClientWithMiddleware {
    fn from(value: PyClientWithMiddleware) -> Self {
        value.inner
    }
}
