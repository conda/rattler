use pyo3::{pyclass, pymethods};
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage};
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
    pub fn new() -> Self {
        Self::default()
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
