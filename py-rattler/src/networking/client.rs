use crate::{
    config::PyConfig,
    error::PyRattlerError,
    networking::middleware::{AddHeadersMiddleware, PyMiddleware},
};
use pyo3::{PyResult, exceptions::PyValueError, pyclass, pymethods};
use rattler_networking::{
    AuthenticationMiddleware, AuthenticationStorage, GCSMiddleware, LazyClient, MirrorMiddleware,
    OciMiddleware, S3Middleware,
    authentication_storage::{AuthenticationStorageError, backends::file::FileStorage},
    proxy::proxies_from_config,
    s3_middleware::compute_s3_config_from_config,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest_middleware::ClientWithMiddleware;
use reqwest_retry::RetryTransientMiddleware;
use reqwest_retry::policies::ExponentialBackoff;
use std::{collections::HashMap, sync::Arc};

static RATTLER_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

fn reqwest_client_builder(
    headers: Option<HashMap<String, String>>,
    user_agent: Option<String>,
    timeout: Option<u64>,
) -> PyResult<reqwest::ClientBuilder> {
    let mut client_builder = reqwest::Client::builder();

    if let Some(timeout) = timeout {
        client_builder = client_builder.timeout(std::time::Duration::from_secs(timeout));
    }

    let has_headers = headers.is_some();
    if let Some(headers) = headers {
        let mut header_map = HeaderMap::new();
        for (key, value) in headers {
            let header_name =
                HeaderName::from_bytes(key.as_bytes()).map_err(PyRattlerError::from)?;
            let header_value = HeaderValue::from_str(&value).map_err(PyRattlerError::from)?;
            header_map.insert(header_name, header_value);
        }
        client_builder = client_builder.default_headers(header_map);
    }

    if let Some(user_agent) = user_agent {
        client_builder = client_builder.user_agent(user_agent);
    } else if !has_headers {
        client_builder = client_builder.user_agent(RATTLER_USER_AGENT);
    }

    Ok(client_builder)
}

pub(crate) fn authentication_storage_from_config(
    config: &PyConfig,
) -> Result<AuthenticationStorage, AuthenticationStorageError> {
    if let Some(path) = &config.inner.authentication_override_file {
        let mut storage = AuthenticationStorage::empty();
        storage.add_backend(Arc::new(FileStorage::from_path(path.clone())?));
        Ok(storage)
    } else {
        AuthenticationStorage::from_env_and_defaults()
    }
}

#[pyclass(from_py_object)]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyClientWithMiddleware {
    pub(crate) inner: ClientWithMiddleware,
}

#[pymethods]
impl PyClientWithMiddleware {
    #[new]
    #[pyo3(signature = (middlewares=None, headers=None, user_agent=None, timeout=None))]
    pub fn new(
        middlewares: Option<Vec<PyMiddleware>>,
        headers: Option<HashMap<String, String>>,
        user_agent: Option<String>,
        timeout: Option<u64>,
    ) -> PyResult<Self> {
        let middlewares = middlewares.unwrap_or_default();
        let reqwest_client = reqwest_client_builder(headers, user_agent, timeout)?
            .build()
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        let mut client = reqwest_middleware::ClientBuilder::new(reqwest_client.clone());

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
                PyMiddleware::Retry(middleware) => {
                    let policy = ExponentialBackoff::builder()
                        .build_with_max_retries(middleware.max_retries);
                    client = client.with(RetryTransientMiddleware::new_with_policy(policy));
                }
                PyMiddleware::Oci(_middleware) => {
                    client = client.with(
                        OciMiddleware::new(reqwest_client.clone()).with_authentication_storage(
                            AuthenticationStorage::from_env_and_defaults()
                                .map_err(PyRattlerError::from)?,
                        ),
                    );
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
                PyMiddleware::AddHeaders(middleware) => {
                    client = client.with(AddHeadersMiddleware::from(middleware));
                }
            }
        }
        let client = client.build();

        Ok(Self { inner: client })
    }

    /// Build the standard client and apply all networking-related settings
    /// from a shared rattler configuration.
    #[staticmethod]
    #[pyo3(signature = (config, max_retries=3, headers=None, user_agent=None, timeout=None))]
    pub fn from_config(
        config: &PyConfig,
        max_retries: u32,
        headers: Option<HashMap<String, String>>,
        user_agent: Option<String>,
        timeout: Option<u64>,
    ) -> PyResult<Self> {
        let mut client_builder = reqwest_client_builder(headers, user_agent, timeout)?;

        #[cfg(any(feature = "native-tls", feature = "rustls"))]
        if config.inner.tls_no_verify.unwrap_or(false) {
            client_builder = client_builder.tls_danger_accept_invalid_certs(true);
        }
        for proxy in proxies_from_config(&config.inner.proxy_config)
            .map_err(|err| PyValueError::new_err(err.to_string()))?
        {
            client_builder = client_builder.proxy(proxy);
        }

        let reqwest_client = client_builder
            .build()
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        let auth_storage =
            authentication_storage_from_config(config).map_err(PyRattlerError::from)?;
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(max_retries);
        let mut client = reqwest_middleware::ClientBuilder::new(reqwest_client.clone())
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .with(AuthenticationMiddleware::from_auth_storage(
                auth_storage.clone(),
            ));

        if !config.inner.mirrors.is_empty() {
            client = client.with(MirrorMiddleware::from_config(&config.inner));
        }

        client = client
            .with(
                OciMiddleware::new(reqwest_client)
                    .with_authentication_storage(auth_storage.clone()),
            )
            .with(GCSMiddleware::default())
            .with(S3Middleware::new(
                compute_s3_config_from_config(&config.inner),
                auth_storage,
            ));

        Ok(Self {
            inner: client.build(),
        })
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
