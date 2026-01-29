use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::pybacked::PyBackedStr;
use pyo3::types::PyAnyMethods;
use pyo3::{pyclass, pymethods, Bound, FromPyObject, PyAny, PyResult, Python};
use pyo3_async_runtimes::tokio::future_into_py;
use rattler_repodata_gateway::fetch::{CacheAction, FetchRepoDataOptions, Variant};
use rattler_repodata_gateway::{
    CacheClearMode, ChannelConfig, Gateway, Source, SourceConfig, SubdirSelection,
};
use url::Url;

use crate::error::PyRattlerError;
use crate::match_spec::PyMatchSpec;
use crate::networking::client::PyClientWithMiddleware;
use crate::package_name::PyPackageName;
use crate::platform::PyPlatform;
use crate::record::PyRecord;
use crate::repo_data::source::PyRepoDataSource;
use crate::{PyChannel, Wrap};

#[pyclass]
#[derive(Clone)]
pub struct PyGateway {
    pub(crate) inner: Gateway,
    show_progress: bool,
}

impl From<PyGateway> for Gateway {
    fn from(value: PyGateway) -> Self {
        value.inner
    }
}

impl From<Gateway> for PyGateway {
    fn from(value: Gateway) -> Self {
        Self {
            inner: value,
            show_progress: false,
        }
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

/// Convert a Python object to a Rust Source.
///
/// Accepts either:
/// - A PyChannel object (wrapped Channel)
/// - Any object implementing the RepoDataSource protocol
///   (has `fetch_package_records` and `package_names` methods)
pub fn py_object_to_source(obj: Bound<'_, PyAny>) -> PyResult<Source> {
    // First try to extract as PyChannel
    if let Ok(channel) = obj.extract::<PyChannel>() {
        return Ok(Source::from(channel.inner));
    }

    // Check if it implements the RepoDataSource protocol
    if obj.hasattr("fetch_package_records")? && obj.hasattr("package_names")? {
        let source = PyRepoDataSource::new(obj.unbind());
        return Ok(Source::from(
            Arc::new(source) as Arc<dyn rattler_repodata_gateway::RepoDataSource>
        ));
    }

    Err(PyTypeError::new_err(
        "Expected Channel or object implementing RepoDataSource protocol \
         (with fetch_package_records and package_names methods)",
    ))
}

#[pymethods]
impl PyGateway {
    #[new]
    #[pyo3(signature = (max_concurrent_requests, default_config, per_channel_config, cache_dir=None, client=None, show_progress=false)
    )]
    pub fn new(
        max_concurrent_requests: usize,
        default_config: PySourceConfig,
        per_channel_config: HashMap<String, PySourceConfig>,
        cache_dir: Option<PathBuf>,
        client: Option<PyClientWithMiddleware>,
        show_progress: bool,
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
            gateway.set_client(client);
        } else {
            // Set a default client if no client is provided to
            // make sure a default user-agent is set.
            gateway.set_client(PyClientWithMiddleware::new(None, None)?);
        }

        Ok(Self {
            inner: gateway.finish(),
            show_progress,
        })
    }

    #[pyo3(signature = (channel, subdirs, clear_disk=false))]
    pub fn clear_repodata_cache(
        &self,
        channel: &PyChannel,
        subdirs: Wrap<SubdirSelection>,
        clear_disk: bool,
    ) -> PyResult<()> {
        let mode = if clear_disk {
            CacheClearMode::InMemoryAndDisk
        } else {
            CacheClearMode::InMemoryOnly
        };
        self.inner
            .clear_repodata_cache(&channel.inner, subdirs.0, mode)
            .map_err(PyRattlerError::from)?;
        Ok(())
    }

    pub fn query<'a>(
        &self,
        py: Python<'a>,
        sources: Vec<Bound<'a, PyAny>>,
        platforms: Vec<PyPlatform>,
        specs: Vec<PyMatchSpec>,
        recursive: bool,
    ) -> PyResult<Bound<'a, PyAny>> {
        // Convert Python sources to Rust Source enum
        let rust_sources: Vec<Source> = sources
            .into_iter()
            .map(py_object_to_source)
            .collect::<PyResult<_>>()?;

        let gateway = self.inner.clone();
        let show_progress = self.show_progress;
        future_into_py(py, async move {
            let mut query = gateway
                .query(rust_sources, platforms.into_iter().map(|p| p.inner), specs)
                .recursive(recursive);

            if show_progress {
                query = query
                    .with_reporter(rattler_repodata_gateway::IndicatifReporter::builder().finish());
            }

            let repodatas = query.execute().await.map_err(PyRattlerError::from)?;

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
        sources: Vec<Bound<'a, PyAny>>,
        platforms: Vec<PyPlatform>,
    ) -> PyResult<Bound<'a, PyAny>> {
        // Convert Python sources to Rust Source enum
        let rust_sources: Vec<Source> = sources
            .into_iter()
            .map(py_object_to_source)
            .collect::<PyResult<_>>()?;

        // Separate channels and custom sources
        let mut channels: Vec<rattler_conda_types::Channel> = Vec::new();
        let mut custom_sources: Vec<Arc<dyn rattler_repodata_gateway::RepoDataSource>> = Vec::new();

        for source in rust_sources {
            match source {
                Source::Channel(channel) => channels.push(channel),
                Source::Custom(custom) => custom_sources.push(custom),
            }
        }

        let platforms_vec: Vec<rattler_conda_types::Platform> =
            platforms.into_iter().map(|p| p.inner).collect();

        let gateway = self.inner.clone();
        let show_progress = self.show_progress;
        future_into_py(py, async move {
            // Collect names from channels via the gateway
            let mut all_names: std::collections::HashSet<rattler_conda_types::PackageName> =
                std::collections::HashSet::new();

            if !channels.is_empty() {
                let mut query = gateway.names(channels, platforms_vec.iter().copied());

                if show_progress {
                    query = query.with_reporter(
                        rattler_repodata_gateway::IndicatifReporter::builder().finish(),
                    );
                }

                let channel_names = query.execute().await.map_err(PyRattlerError::from)?;
                all_names.extend(channel_names);
            }

            // Collect names from custom sources directly
            for custom_source in custom_sources {
                for platform in &platforms_vec {
                    let names = custom_source.package_names(*platform);
                    for name_str in names {
                        if let Ok(name) = name_str.parse() {
                            all_names.insert(name);
                        }
                    }
                }
            }

            // Convert to list of PyPackageName
            Ok(all_names
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
