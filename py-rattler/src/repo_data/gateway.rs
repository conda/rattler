use crate::{PyChannel, Wrap};
use pyo3::exceptions::PyValueError;
use pyo3::{pyclass, pymethods, FromPyObject, PyAny, PyResult};
use rattler_repodata_gateway::fetch::CacheAction;
use rattler_repodata_gateway::{ChannelConfig, Gateway, SourceConfig};
use std::collections::HashMap;
use std::path::PathBuf;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyGateway {
    pub(crate) inner: Gateway,
}

impl From<PyGateway> for Gateway {
    fn from(value: PyGateway) -> Self {
        value.inner
    }
}

impl From<Gateway> for PyGateway {
    fn from(value: Gateway) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyGateway {
    #[new]
    pub fn new(
        max_concurrent_requests: usize,
        default_config: PySourceConfig,
        per_channel_config: HashMap<PyChannel, PySourceConfig>,
        cache_dir: Option<PathBuf>,
    ) -> PyResult<Self> {
        let mut channel_config = ChannelConfig::default();
        channel_config.default = default_config.into();
        channel_config.per_channel = per_channel_config
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();

        let mut gateway = Gateway::builder()
            .with_max_concurrent_requests(max_concurrent_requests)
            .with_channel_config(channel_config);

        if let Some(cache_dir) = cache_dir {
            gateway.set_cache_dir(cache_dir);
        }

        Ok(Self {
            inner: gateway.finish(),
        })
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PySourceConfig {
    pub(crate) inner: SourceConfig,
}

impl From<PySourceConfig> for SourceConfig {
    fn from(value: PySourceConfig) -> Self {
        value.inner
    }
}

impl From<SourceConfig> for PySourceConfig {
    fn from(value: SourceConfig) -> Self {
        Self { inner: value }
    }
}

impl FromPyObject<'_> for Wrap<CacheAction> {
    fn extract(ob: &'_ PyAny) -> PyResult<Self> {
        let parsed = match &*ob.extract::<String>()? {
            "cache-or-fetch" => CacheAction::CacheOrFetch,
            "use-cache-only" => CacheAction::UseCacheOnly,
            "force-cache-only" => CacheAction::ForceCacheOnly,
            "no-cache" => CacheAction::NoCache,
            v => {
                return Err(PyValueError::new_err(format!(
                    "cache action must be one of {{'cache-or-fetch', 'use-cache-only', 'force-cache-only', 'no-cache'}}, got {v}",
                )))
            },
        };
        Ok(Wrap(parsed))
    }
}

#[pymethods]
impl PySourceConfig {
    #[new]
    pub fn new(
        jlap_enabled: bool,
        zstd_enabled: bool,
        bz2_enabled: bool,
        cache_action: Wrap<CacheAction>,
    ) -> Self {
        Self {
            inner: SourceConfig {
                jlap_enabled,
                zstd_enabled,
                bz2_enabled,
                cache_action: cache_action.0,
            },
        }
    }
}
