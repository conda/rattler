use crate::{error::PyRattlerError, networking::middleware::PyMiddleware};
use pyo3::{pyclass, pymethods, PyResult};
use rattler_networking::{
    AuthenticationMiddleware, AuthenticationStorage, GCSMiddleware, LazyClient, MirrorMiddleware,
    OciMiddleware, S3Middleware,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest_middleware::ClientWithMiddleware;
use std::collections::HashMap;

static RATTLER_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyClientWithMiddleware {
    pub(crate) inner: ClientWithMiddleware,
}

#[pymethods]
impl PyClientWithMiddleware {
    #[new]
    #[pyo3(signature = (middlewares=None, headers=None))]
    pub fn new(
        middlewares: Option<Vec<PyMiddleware>>,
        headers: Option<HashMap<String, String>>,
    ) -> PyResult<Self> {
        let middlewares = middlewares.unwrap_or_default();

        let mut client_builder = reqwest::Client::builder();

        if let Some(headers) = headers {
            let mut header_map = HeaderMap::new();
            for (key, value) in headers {
                let header_name =
                    HeaderName::from_bytes(key.as_bytes()).map_err(PyRattlerError::from)?;
                let header_value = HeaderValue::from_str(&value).map_err(PyRattlerError::from)?;
                header_map.insert(header_name, header_value);
            }
            client_builder = client_builder.default_headers(header_map);
        } else {
            client_builder = client_builder.user_agent(RATTLER_USER_AGENT);
        }

        let mut client = reqwest_middleware::ClientBuilder::new(client_builder.build().unwrap());

        for middleware in middlewares {
            match middleware {
                PyMiddleware::Mirror(middleware) => {
                    client = client.with(MirrorMiddleware::from(middleware));
                }
                PyMiddleware::Authentication(_) => {
                    client = client.with(
                        AuthenticationMiddleware::from_env_and_defaults()
                            .map_err(PyRattlerError::from)?,
                    );
                }
                PyMiddleware::Oci(middleware) => {
                    client = client.with(OciMiddleware::from(middleware));
                }
                PyMiddleware::Gcs(middleware) => {
                    client = client.with(GCSMiddleware::from(middleware));
                }
                PyMiddleware::S3(middleware) => {
                    client = client.with(S3Middleware::new(
                        middleware
                            .s3_config
                            .iter()
                            .map(|(k, v)| (k.clone(), v.clone().into()))
                            .collect(),
                        AuthenticationStorage::from_env_and_defaults()
                            .map_err(PyRattlerError::from)?,
                    ));
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

impl From<PyClientWithMiddleware> for LazyClient {
    fn from(value: PyClientWithMiddleware) -> Self {
        LazyClient::from(value.inner)
    }
}
