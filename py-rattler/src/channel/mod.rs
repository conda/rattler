use pyo3::{pyclass, pymethods};
use rattler_conda_types::{Channel, ChannelConfig};
use rattler_solve::ChannelPriority;
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
    pub fn __init__(channel_alias: &str, root_dir: &str) -> pyo3::PyResult<Self> {
        Ok(Self {
            inner: ChannelConfig {
                channel_alias: Url::parse(channel_alias).map_err(PyRattlerError::from)?,
                root_dir: root_dir.into(),
            },
        })
    }

    /// Returns the channel alias that is configured
    #[getter]
    fn channel_alias(&self) -> String {
        self.inner.channel_alias.to_string()
    }

    /// Returns the root directory for local channels
    #[getter]
    fn root_dir(&self) -> String {
        self.inner.root_dir.to_string_lossy().into()
    }
}

#[pyclass]
#[repr(transparent)]
#[derive(Clone, Hash, Eq, PartialEq)]
pub struct PyChannel {
    pub(crate) inner: Channel,
}

impl From<Channel> for PyChannel {
    fn from(value: Channel) -> Self {
        Self { inner: value }
    }
}

impl From<PyChannel> for Channel {
    fn from(val: PyChannel) -> Self {
        val.inner
    }
}

#[pymethods]
impl PyChannel {
    #[new]
    pub fn __init__(version: &str, config: &PyChannelConfig) -> pyo3::PyResult<Self> {
        Ok(Channel::from_str(version, &config.inner)
            .map(Into::into)
            .map_err(PyRattlerError::from)?)
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
        self.inner.platform_url((*platform).into()).into()
    }
}

#[pyclass(eq, eq_int)]
#[derive(Clone, PartialEq, Eq)]
pub enum PyChannelPriority {
    /// The channel that the package is first found in will be used as the only channel
    /// for that package.
    Strict,

    /// Packages can be retrieved from any channel as package version takes precedence.
    Disabled,
}

impl From<ChannelPriority> for PyChannelPriority {
    fn from(channel_priority: ChannelPriority) -> Self {
        match channel_priority {
            ChannelPriority::Strict => PyChannelPriority::Strict,
            ChannelPriority::Disabled => PyChannelPriority::Disabled,
        }
    }
}

impl From<PyChannelPriority> for ChannelPriority {
    fn from(py_channel_priority: PyChannelPriority) -> Self {
        match py_channel_priority {
            PyChannelPriority::Strict => ChannelPriority::Strict,
            PyChannelPriority::Disabled => ChannelPriority::Disabled,
        }
    }
}
