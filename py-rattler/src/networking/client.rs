use crate::{error::PyRattlerError, networking::middleware::PyMiddleware};
use pyo3::{pyclass, pymethods, PyResult};
use rattler_networking::{
    s3_middleware::S3Config, AuthenticationMiddleware, AuthenticationStorage, GCSMiddleware,
    MirrorMiddleware, OciMiddleware, S3Middleware,
};
use reqwest_middleware::ClientWithMiddleware;

use super::middleware::PyS3Config;

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
                    let PyS3Config { custom } = middleware.s3_config;
                    let s3_config = if let Some(config) = custom {
                        S3Config::Custom {
                            endpoint_url: config.endpoint_url,
                            region: config.region,
                            force_path_style: config.force_path_style,
                        }
                    } else {
                        S3Config::FromAWS
                    };
                    client = client.with(S3Middleware::new(
                        s3_config,
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
