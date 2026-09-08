use std::path::PathBuf;

use pyo3::{exceptions::PyValueError, prelude::*, pyclass, pymethods};
use pythonize::pythonize;
use rattler_config::{
    Config, ConfigBase,
    config::{run_post_link_scripts::RunPostLinkScripts, tls::TlsRootCerts},
    locations::{ConfigLayer, ConfigLocation, config_search_paths},
};

use crate::{channel::PyChannelConfig, error::PyRattlerError};

/// The configuration shared by all rattler-based tools.
///
/// This binds `ConfigBase<NoExtension>`: only the keys understood by every
/// rattler-based tool. Tool-specific extension keys (the generic `T` of
/// `ConfigBase<T>`) are resolved at compile time in Rust and cannot be
/// supplied from Python, so a file containing them reports those keys as
/// unused.
#[pyclass(name = "PyConfig", from_py_object)]
#[repr(transparent)]
#[derive(Clone, Default)]
pub struct PyConfig {
    pub(crate) inner: ConfigBase,
}

impl From<ConfigBase> for PyConfig {
    fn from(value: ConfigBase) -> Self {
        Self { inner: value }
    }
}

impl From<PyConfig> for ConfigBase {
    fn from(value: PyConfig) -> Self {
        value.inner
    }
}

/// Serialize a `serde` value into a Python object.
fn to_py<'py, T: serde::Serialize>(py: Python<'py>, value: &T) -> PyResult<Bound<'py, PyAny>> {
    pythonize(py, value).map_err(|err| PyValueError::new_err(err.to_string()))
}

#[pymethods]
impl PyConfig {
    /// Create a configuration with every key at its default.
    #[new]
    fn __init__() -> Self {
        Self::default()
    }

    /// Parse a configuration from a TOML string, as a tool configuration
    /// file. Returns the configuration and the keys that were not
    /// recognized.
    #[staticmethod]
    fn from_toml(toml: &str) -> PyResult<(Self, Vec<String>)> {
        let (config, unused) = ConfigBase::from_toml_str(toml)
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        Ok((config.into(), unused.into_iter().collect()))
    }

    /// Parse a configuration from a TOML string, as a *shared* configuration
    /// file: only the keys shared by all rattler-based tools are accepted.
    /// Returns the configuration and the keys that were not recognized.
    #[staticmethod]
    fn from_toml_shared(toml: &str) -> PyResult<(Self, Vec<String>)> {
        let (config, unused) = ConfigBase::from_toml_str_shared(toml)
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        Ok((config.into(), unused.into_iter().collect()))
    }

    /// Load a configuration by merging the given files, in order: later
    /// files take precedence. Every file is parsed as a tool configuration
    /// file and must exist.
    #[staticmethod]
    fn load_from_files(paths: Vec<PathBuf>) -> PyResult<Self> {
        Ok(ConfigBase::load_from_files(paths)
            .map_err(PyRattlerError::from)?
            .into())
    }

    /// Load a configuration by merging the given `(path, is_shared)`
    /// locations, in order: later locations take precedence. Shared
    /// locations accept only the common keys.
    #[staticmethod]
    fn load_from_locations(locations: Vec<(PathBuf, bool)>) -> PyResult<Self> {
        let locations = locations.into_iter().map(|(path, shared)| ConfigLocation {
            path,
            layer: if shared {
                ConfigLayer::Shared
            } else {
                ConfigLayer::Tool
            },
        });
        Ok(ConfigBase::load_from_locations(locations)
            .map_err(PyRattlerError::from)?
            .into())
    }

    /// Load the configuration from the default locations of `tool`, skipping
    /// files that do not exist.
    #[staticmethod]
    fn load_from_default_locations(tool: &str) -> PyResult<Self> {
        Ok(ConfigBase::load_from_default_locations(tool)
            .map_err(PyRattlerError::from)?
            .into())
    }

    /// The configuration file locations of `tool`, from lowest to highest
    /// precedence, as `(path, is_shared)` pairs. The paths are candidates
    /// and are not checked for existence.
    #[staticmethod]
    fn config_search_paths(tool: &str) -> Vec<(PathBuf, bool)> {
        config_search_paths(tool)
            .into_iter()
            .map(|location| (location.path, location.layer == ConfigLayer::Shared))
            .collect()
    }

    /// The files this configuration was loaded from, in load order.
    #[getter]
    fn loaded_from(&self) -> Vec<PathBuf> {
        self.inner.loaded_from.clone()
    }

    #[getter]
    fn default_channels(&self) -> Option<Vec<String>> {
        self.inner
            .default_channels
            .as_ref()
            .map(|channels| channels.iter().map(ToString::to_string).collect())
    }

    #[getter]
    fn authentication_override_file(&self) -> Option<PathBuf> {
        self.inner.authentication_override_file.clone()
    }

    #[getter]
    fn tls_no_verify(&self) -> Option<bool> {
        self.inner.tls_no_verify
    }

    /// Which TLS root certificates to use: `'webpki'` or `'system'`.
    #[getter]
    fn tls_root_certs(&self) -> Option<&'static str> {
        self.inner.tls_root_certs.map(|certs| match certs {
            TlsRootCerts::Webpki => "webpki",
            TlsRootCerts::System => "system",
        })
    }

    /// The configured mirrors, as a mapping of upstream URL to mirror URLs.
    #[getter]
    fn mirrors(&self) -> Vec<(String, Vec<String>)> {
        self.inner
            .mirrors
            .iter()
            .map(|(url, mirrors)| {
                (
                    url.to_string(),
                    mirrors.iter().map(ToString::to_string).collect(),
                )
            })
            .collect()
    }

    /// The package format and compression level, e.g. `'conda:max'`.
    #[getter]
    fn build_package_format(&self) -> PyResult<Option<String>> {
        let Some(format) = &self.inner.build.package_format else {
            return Ok(None);
        };
        // `PackageFormatAndCompression` serializes to its string form.
        serde_json::to_value(format)
            .ok()
            .and_then(|value| value.as_str().map(ToString::to_string))
            .map(Some)
            .ok_or_else(|| PyValueError::new_err("could not render the package format"))
    }

    #[getter]
    fn channel_config(&self) -> PyChannelConfig {
        PyChannelConfig {
            inner: self.inner.channel_config.clone(),
        }
    }

    #[getter]
    fn concurrency_solves(&self) -> usize {
        self.inner.concurrency.solves
    }

    #[getter]
    fn concurrency_downloads(&self) -> usize {
        self.inner.concurrency.downloads
    }

    #[getter]
    fn proxy_https(&self) -> Option<String> {
        self.inner
            .proxy_config
            .https
            .as_ref()
            .map(ToString::to_string)
    }

    #[getter]
    fn proxy_http(&self) -> Option<String> {
        self.inner
            .proxy_config
            .http
            .as_ref()
            .map(ToString::to_string)
    }

    #[getter]
    fn proxy_non_proxy_hosts(&self) -> Vec<String> {
        self.inner.proxy_config.non_proxy_hosts.clone()
    }

    /// Whether to run post-link scripts: `'insecure'` or `'false'`.
    #[getter]
    fn run_post_link_scripts(&self) -> Option<&'static str> {
        self.inner
            .run_post_link_scripts
            .as_ref()
            .map(|value| match value {
                RunPostLinkScripts::Insecure => "insecure",
                RunPostLinkScripts::False => "false",
            })
    }

    #[getter]
    fn allow_symbolic_links(&self) -> Option<bool> {
        self.inner.allow_symbolic_links
    }

    #[getter]
    fn allow_hard_links(&self) -> Option<bool> {
        self.inner.allow_hard_links
    }

    #[getter]
    fn allow_ref_links(&self) -> Option<bool> {
        self.inner.allow_ref_links
    }

    /// The repodata configuration, as a nested dictionary. The `default`
    /// key holds the channel-independent options; every other key is a
    /// channel URL.
    #[getter]
    fn repodata_config<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.inner.repodata_config)
    }

    /// The S3 configuration, as a mapping of bucket name to options.
    #[getter]
    fn s3_options<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.inner.s3_options)
    }

    /// The `rattler-index` configuration, as a nested dictionary.
    #[getter]
    fn index_config<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.inner.index_config)
    }

    /// The effective `rattler-index` options for a channel, resolved from
    /// the defaults and the per-channel entries.
    fn resolve_index_config<'py>(
        &self,
        py: Python<'py>,
        channel: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.inner.index_config.resolve(channel))
    }

    /// Merge `other` into a copy of this configuration. `other` takes
    /// precedence.
    fn merge(&self, other: &Self) -> PyResult<Self> {
        self.inner
            .clone()
            .merge_config(&other.inner)
            .map(Into::into)
            .map_err(|err| PyValueError::new_err(err.to_string()))
    }

    /// Validate this configuration, raising `ValueError` when it is
    /// invalid.
    fn validate(&self) -> PyResult<()> {
        self.inner
            .validate()
            .map_err(|err| PyValueError::new_err(err.to_string()))
    }

    /// The dotted TOML key paths this configuration understands.
    fn keys(&self) -> Vec<String> {
        self.inner.keys()
    }

    /// Set the value at the dotted TOML key path `key`. The value is
    /// interpreted as JSON when possible and as a plain string otherwise.
    fn set(&mut self, key: &str, value: Option<String>) -> PyResult<()> {
        self.inner
            .set(key, value)
            .map_err(|err| PyValueError::new_err(err.to_string()))
    }

    /// Serialize this configuration to a TOML string.
    fn to_toml(&self) -> PyResult<String> {
        self.inner
            .to_toml()
            .map_err(|err| PyValueError::new_err(err.to_string()))
    }

    /// Write this configuration to `path`, creating parent directories as
    /// needed.
    fn save(&self, path: PathBuf) -> PyResult<()> {
        self.inner
            .save(&path)
            .map_err(|err| PyValueError::new_err(err.to_string()))
    }
}
