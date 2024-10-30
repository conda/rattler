use crate::networking::middleware::PyMiddleware;
use pyo3::{pyclass, pymethods};
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage, MirrorMiddleware};
use reqwest_middleware::ClientWithMiddleware;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyAuthenticatedClient {
    pub(crate) inner: ClientWithMiddleware,
}

#[pymethods]
impl PyAuthenticatedClient {
    #[new]
    pub fn new(middlewares: Option<Vec<PyMiddleware>>) -> Self {
        match middlewares {
            Some(middlewares) => return Self::new_with_middlewares(middlewares),
            None => Self::default(),
        }
    }
}

impl PyAuthenticatedClient {
    pub fn new_with_middlewares(middlewares: Vec<PyMiddleware>) -> Self {
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

impl From<ClientWithMiddleware> for PyAuthenticatedClient {
    fn from(value: ClientWithMiddleware) -> Self {
        Self { inner: value }
    }
}

impl From<PyAuthenticatedClient> for ClientWithMiddleware {
    fn from(value: PyAuthenticatedClient) -> Self {
        value.inner
    }
}

impl Default for PyAuthenticatedClient {
    fn default() -> Self {
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(AuthenticationMiddleware::new(
                AuthenticationStorage::default(),
            ))
            .build();

        Self { inner: client }
    }
}
