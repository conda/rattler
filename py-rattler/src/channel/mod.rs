use pyo3::{pyclass, pymethods};
use rattler_conda_types::{Channel, ChannelConfig};
use url::Url;

use crate::{error::PyRattlerError, platform::PyPlatform};

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyChannelConfig {
    pub(crate) inner: ChannelConfig,
}

#[pymethods]
impl PyChannelConfig {
    #[new]
    pub fn __init__(channel_alias: &str) -> pyo3::PyResult<Self> {
        Ok(Self {
            inner: ChannelConfig {
                channel_alias: Url::parse(channel_alias).map_err(PyRattlerError::from)?,
            },
        })
    }

    /// Returns the channel alias that is configured
    #[getter]
    fn channel_alias(&self) -> String {
        self.inner.channel_alias.to_string()
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone)]
pub struct PyChannel {
    pub(crate) inner: rattler_conda_types::Channel,
}

impl From<rattler_conda_types::Channel> for PyChannel {
    fn from(value: Channel) -> Self {
        Self { inner: value }
    }
}

impl From<PyChannel> for rattler_conda_types::Channel {
    fn from(val: PyChannel) -> Self {
        val.inner
    }
}

impl From<PyChannel> for rattler_lock::Channel {
    fn from(val: PyChannel) -> Self {
        rattler_lock::Channel::from(val.inner.canonical_name())
    }
}

#[pymethods]
impl PyChannel {
    #[new]
    pub fn __init__(version: &str, config: &PyChannelConfig) -> pyo3::PyResult<Self> {
        Ok(
            rattler_conda_types::Channel::from_str(version, &config.inner)
                .map(Into::into)
                .map_err(PyRattlerError::from)?,
        )
    }

    /// Returns the name of the channel.
    #[getter]
    fn name(&self) -> Option<String> {
        self.inner.name.clone()
    }

    /// Returns the base url of the channel.
    #[getter]
    fn base_url(&self) -> String {
        self.inner.base_url.to_string()
    }

    /// Returns the Urls for the given platform.
    pub fn platform_url(&self, platform: &PyPlatform) -> String {
        self.inner.platform_url(platform.clone().into()).into()
    }
}
