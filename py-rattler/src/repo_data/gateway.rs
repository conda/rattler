use crate::error::PyRattlerError;
use crate::match_spec::PyMatchSpec;
use crate::networking::client::PyClientWithMiddleware;
use crate::package_name::PyPackageName;
use crate::platform::PyPlatform;
use crate::record::PyRecord;
use crate::{PyChannel, Wrap};
use pyo3::exceptions::PyValueError;
use pyo3::pybacked::PyBackedStr;
use pyo3::types::PyAnyMethods;
use pyo3::{pyclass, pymethods, Bound, FromPyObject, PyAny, PyResult, Python};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_repodata_gateway::fetch::{CacheAction, FetchRepoDataOptions, Variant};
use rattler_repodata_gateway::{ChannelConfig, Gateway, SourceConfig, SubdirSelection};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use url::Url;

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

impl<'source> FromPyObject<'source> for Wrap<SubdirSelection> {
    fn extract_bound(ob: &Bound<'source, PyAny>) -> PyResult<Self> {
        let parsed = match <Option<HashSet<PyPlatform>>>::extract_bound(ob)? {
            Some(platforms) => SubdirSelection::Some(
                platforms
                    .into_iter()
                    .map(|p| p.inner.as_str().to_owned())
                    .collect(),
            ),
            None => SubdirSelection::All,
        };
        Ok(Wrap(parsed))
    }
}

#[pymethods]
impl PyGateway {
    #[new]
    #[pyo3(signature = (max_concurrent_requests, default_config, per_channel_config, cache_dir=None, client=None)
    )]
    pub fn new(
        max_concurrent_requests: usize,
        default_config: PySourceConfig,
        per_channel_config: HashMap<String, PySourceConfig>,
        cache_dir: Option<PathBuf>,
        client: Option<PyClientWithMiddleware>,
    ) -> PyResult<Self> {
        let channel_config = ChannelConfig {
            default: default_config.into(),
            per_channel: per_channel_config
                .into_iter()
                .map(|(k, v)| {
                    let url = Url::parse(&k).map_err(PyRattlerError::from)?;
                    Ok((url, v.into()))
                })
                .collect::<Result<_, PyRattlerError>>()?,
        };

        let mut gateway = Gateway::builder()
            .with_max_concurrent_requests(max_concurrent_requests)
            .with_channel_config(channel_config);

        if let Some(cache_dir) = cache_dir {
            gateway.set_cache_dir(cache_dir);
        }

        if let Some(client) = client {
            gateway.set_client(client.into());
        }

        Ok(Self {
            inner: gateway.finish(),
        })
    }

    pub fn clear_repodata_cache(&self, channel: &PyChannel, subdirs: Wrap<SubdirSelection>) {
        self.inner.clear_repodata_cache(&channel.inner, subdirs.0);
    }

    pub fn query<'a>(
        &self,
        py: Python<'a>,
        channels: Vec<PyChannel>,
        platforms: Vec<PyPlatform>,
        specs: Vec<PyMatchSpec>,
        recursive: bool,
    ) -> PyResult<Bound<'a, PyAny>> {
        let gateway = self.inner.clone();
        future_into_py(py, async move {
            let repodatas = gateway
                .query(channels, platforms.into_iter().map(|p| p.inner), specs)
                .recursive(recursive)
                .execute()
                .await
                .map_err(PyRattlerError::from)?;

            // Convert the records into a list of lists
            Ok(repodatas
                .into_iter()
                .map(|r| {
                    r.into_iter()
                        .cloned()
                        .map(PyRecord::from)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>())
        })
    }

    pub fn names<'a>(
        &self,
        py: Python<'a>,
        channels: Vec<PyChannel>,
        platforms: Vec<PyPlatform>,
    ) -> PyResult<Bound<'a, PyAny>> {
        let gateway = self.inner.clone();
        future_into_py(py, async move {
            let names = gateway
                .names(channels, platforms.into_iter().map(|p| p.inner))
                .execute()
                .await
                .map_err(PyRattlerError::from)?;

            // Convert the records into a list of lists
            Ok(names
                .into_iter()
                .map(PyPackageName::from)
                .collect::<Vec<_>>())
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

impl<'py> FromPyObject<'py> for Wrap<CacheAction> {
    fn extract_bound(ob: &Bound<'py, PyAny>) -> PyResult<Self> {
        let as_py_str: PyBackedStr = ob.extract()?;
        let parsed = match as_py_str.as_ref() {
            "cache-or-fetch" => CacheAction::CacheOrFetch,
            "use-cache-only" => CacheAction::UseCacheOnly,
            "force-cache-only" => CacheAction::ForceCacheOnly,
            "no-cache" => CacheAction::NoCache,
            v => {
                return Err(PyValueError::new_err(format!(
                    "cache action must be one of {{'cache-or-fetch', 'use-cache-only', 'force-cache-only', 'no-cache'}}, got {v}",
                )))
            }
        };
        Ok(Wrap(parsed))
    }
}

#[pymethods]
impl PySourceConfig {
    #[new]
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn new(
        jlap_enabled: bool,
        zstd_enabled: bool,
        bz2_enabled: bool,
        sharded_enabled: bool,
        cache_action: Wrap<CacheAction>,
    ) -> Self {
        Self {
            inner: SourceConfig {
                jlap_enabled,
                zstd_enabled,
                bz2_enabled,
                sharded_enabled,
                cache_action: cache_action.0,
            },
        }
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyFetchRepoDataOptions {
    pub(crate) inner: FetchRepoDataOptions,
}

impl From<PyFetchRepoDataOptions> for FetchRepoDataOptions {
    fn from(value: PyFetchRepoDataOptions) -> Self {
        value.inner
    }
}

impl From<FetchRepoDataOptions> for PyFetchRepoDataOptions {
    fn from(value: FetchRepoDataOptions) -> Self {
        Self { inner: value }
    }
}

impl<'py> FromPyObject<'py> for Wrap<Variant> {
    fn extract_bound(ob: &Bound<'py, PyAny>) -> PyResult<Self> {
        let as_py_str: PyBackedStr = ob.extract()?;
        let parsed = match as_py_str.as_ref() {
            "after-patches" => Variant::AfterPatches,
            "from-packages" => Variant::FromPackages,
            "current" => Variant::Current,
            v => {
                return Err(PyValueError::new_err(format!(
                "variant must be one of {{'after-patches', 'from-packages', 'current'}}, got {v}",
            )))
            }
        };
        Ok(Wrap(parsed))
    }
}

#[pymethods]
impl PyFetchRepoDataOptions {
    #[new]
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn new(
        cache_action: Wrap<CacheAction>,
        variant: Wrap<Variant>,
        jlap_enabled: bool,
        zstd_enabled: bool,
        bz2_enabled: bool,
    ) -> Self {
        Self {
            inner: FetchRepoDataOptions {
                cache_action: cache_action.0,
                variant: variant.0,
                jlap_enabled,
                zstd_enabled,
                bz2_enabled,
                retry_policy: None,
            },
        }
    }
}
