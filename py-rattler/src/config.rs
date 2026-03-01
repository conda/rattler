use std::collections::HashMap;
use std::path::PathBuf;

use pyo3::exceptions::PyValueError;
use pyo3::{pyclass, pymethods, PyResult};
use rattler_config::config::concurrency::ConcurrencyConfig;
use rattler_config::config::proxy::ProxyConfig;
use rattler_config::config::repodata_config::{RepodataChannelConfig, RepodataConfig};
use rattler_config::config::s3::S3Options;
use rattler_config::config::{ConfigBase, LoadError};

type RattlerConfig = ConfigBase<()>;

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyRepodataChannelConfig {
    inner: RepodataChannelConfig,
}

impl From<RepodataChannelConfig> for PyRepodataChannelConfig {
    fn from(value: RepodataChannelConfig) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyRepodataChannelConfig {
    #[getter]
    pub fn disable_bzip2(&self) -> Option<bool> {
        self.inner.disable_bzip2
    }

    #[getter]
    pub fn disable_zstd(&self) -> Option<bool> {
        self.inner.disable_zstd
    }

    #[getter]
    pub fn disable_sharded(&self) -> Option<bool> {
        self.inner.disable_sharded
    }

    pub fn __repr__(&self) -> String {
        format!(
            "RepodataChannelConfig(disable_bzip2={}, disable_zstd={}, disable_sharded={})",
            fmt_opt_bool(self.inner.disable_bzip2),
            fmt_opt_bool(self.inner.disable_zstd),
            fmt_opt_bool(self.inner.disable_sharded),
        )
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyRepodataConfig {
    inner: RepodataConfig,
}

impl From<RepodataConfig> for PyRepodataConfig {
    fn from(value: RepodataConfig) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyRepodataConfig {
    #[getter]
    pub fn default_config(&self) -> PyRepodataChannelConfig {
        self.inner.default.clone().into()
    }

    #[getter]
    pub fn per_channel(&self) -> HashMap<String, PyRepodataChannelConfig> {
        self.inner
            .per_channel
            .iter()
            .map(|(url, config)| (url.to_string(), config.clone().into()))
            .collect()
    }

    pub fn __repr__(&self) -> String {
        format!(
            "RepodataConfig(per_channel_count={})",
            self.inner.per_channel.len()
        )
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyConcurrencyConfig {
    inner: ConcurrencyConfig,
}

impl From<ConcurrencyConfig> for PyConcurrencyConfig {
    fn from(value: ConcurrencyConfig) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyConcurrencyConfig {
    #[getter]
    pub fn solves(&self) -> usize {
        self.inner.solves
    }

    #[getter]
    pub fn downloads(&self) -> usize {
        self.inner.downloads
    }

    pub fn __repr__(&self) -> String {
        format!(
            "ConcurrencyConfig(solves={}, downloads={})",
            self.inner.solves, self.inner.downloads
        )
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyProxyConfig {
    inner: ProxyConfig,
}

impl From<ProxyConfig> for PyProxyConfig {
    fn from(value: ProxyConfig) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyProxyConfig {
    #[getter]
    pub fn https(&self) -> Option<String> {
        self.inner.https.as_ref().map(|u| u.to_string())
    }

    #[getter]
    pub fn http(&self) -> Option<String> {
        self.inner.http.as_ref().map(|u| u.to_string())
    }

    #[getter]
    pub fn non_proxy_hosts(&self) -> Vec<String> {
        self.inner.non_proxy_hosts.clone()
    }

    pub fn __repr__(&self) -> String {
        format!(
            "ProxyConfig(https={}, http={}, non_proxy_hosts_count={})",
            self.inner
                .https
                .as_ref()
                .map_or("None".to_string(), |u| format!("\"{}\"", u)),
            self.inner
                .http
                .as_ref()
                .map_or("None".to_string(), |u| format!("\"{}\"", u)),
            self.inner.non_proxy_hosts.len(),
        )
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyS3Options {
    inner: S3Options,
}

impl From<S3Options> for PyS3Options {
    fn from(value: S3Options) -> Self {
        Self { inner: value }
    }
}

#[pymethods]
impl PyS3Options {
    #[getter]
    pub fn endpoint_url(&self) -> String {
        self.inner.endpoint_url.to_string()
    }

    #[getter]
    pub fn region(&self) -> &str {
        &self.inner.region
    }

    #[getter]
    pub fn force_path_style(&self) -> bool {
        self.inner.force_path_style
    }

    pub fn __repr__(&self) -> String {
        format!(
            "S3Options(endpoint_url=\"{}\", region=\"{}\", force_path_style={})",
            self.inner.endpoint_url, self.inner.region, self.inner.force_path_style,
        )
    }
}

#[pyclass]
#[derive(Clone)]
pub struct PyConfig {
    inner: RattlerConfig,
}

impl From<RattlerConfig> for PyConfig {
    fn from(value: RattlerConfig) -> Self {
        Self { inner: value }
    }
}

fn load_error_to_py(e: LoadError) -> pyo3::PyErr {
    PyValueError::new_err(format!("{}", e))
}

#[pymethods]
impl PyConfig {
    #[new]
    pub fn new() -> Self {
        RattlerConfig::default().into()
    }

    #[staticmethod]
    #[pyo3(signature = (*paths))]
    pub fn load_from_files(paths: Vec<PathBuf>) -> PyResult<Self> {
        let refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
        let config = RattlerConfig::load_from_files(refs).map_err(load_error_to_py)?;
        Ok(config.into())
    }

    #[getter]
    pub fn default_channels(&self) -> Option<Vec<String>> {
        self.inner
            .default_channels
            .as_ref()
            .map(|channels| channels.iter().map(|c| c.to_string()).collect())
    }

    #[getter]
    pub fn authentication_override_file(&self) -> Option<PathBuf> {
        self.inner.authentication_override_file.clone()
    }

    #[getter]
    pub fn tls_no_verify(&self) -> Option<bool> {
        self.inner.tls_no_verify
    }

    #[getter]
    pub fn mirrors(&self) -> HashMap<String, Vec<String>> {
        self.inner
            .mirrors
            .iter()
            .map(|(url, mirrors)| {
                (
                    url.to_string(),
                    mirrors.iter().map(|m| m.to_string()).collect(),
                )
            })
            .collect()
    }

    #[getter]
    pub fn concurrency(&self) -> PyConcurrencyConfig {
        self.inner.concurrency.clone().into()
    }

    #[getter]
    pub fn proxy_config(&self) -> PyProxyConfig {
        self.inner.proxy_config.clone().into()
    }

    #[getter]
    pub fn repodata_config(&self) -> PyRepodataConfig {
        self.inner.repodata_config.clone().into()
    }

    #[getter]
    pub fn s3_options(&self) -> HashMap<String, PyS3Options> {
        self.inner
            .s3_options
            .0
            .iter()
            .map(|(name, opts)| (name.clone(), opts.clone().into()))
            .collect()
    }

    #[getter]
    pub fn loaded_from(&self) -> Vec<PathBuf> {
        self.inner.loaded_from.clone()
    }

    pub fn __repr__(&self) -> String {
        let channels = self
            .inner
            .default_channels
            .as_ref()
            .map_or("None".to_string(), |c| {
                format!(
                    "[{}]",
                    c.iter()
                        .map(|ch| format!("\"{}\"", ch))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
        format!(
            "Config(default_channels={}, tls_no_verify={}, mirrors_count={})",
            channels,
            fmt_opt_bool(self.inner.tls_no_verify),
            self.inner.mirrors.len(),
        )
    }
}

fn fmt_opt_bool(v: Option<bool>) -> String {
    match v {
        Some(true) => "True".to_string(),
        Some(false) => "False".to_string(),
        None => "None".to_string(),
    }
}
