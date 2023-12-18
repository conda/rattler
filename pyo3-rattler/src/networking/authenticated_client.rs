use pyo3::{pyclass, pymethods};
use rattler_networking::AuthenticatedClient;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyAuthenticatedClient {
    pub(crate) inner: AuthenticatedClient,
}

#[pymethods]
impl PyAuthenticatedClient {
    #[new]
    pub fn new() -> Self {
        Self::default()
    }
}

impl From<AuthenticatedClient> for PyAuthenticatedClient {
    fn from(value: AuthenticatedClient) -> Self {
        Self { inner: value }
    }
}

impl From<PyAuthenticatedClient> for AuthenticatedClient {
    fn from(value: PyAuthenticatedClient) -> Self {
        value.inner
    }
}

impl Default for PyAuthenticatedClient {
    fn default() -> Self {
        AuthenticatedClient::default().into()
    }
}
