from __future__ import annotations
from os import PathLike
from typing import Dict, List, Optional, Union

from rattler.rattler import (
    PyConfig,
    PyConcurrencyConfig,
    PyProxyConfig,
    PyRepodataChannelConfig,
    PyRepodataConfig,
    PyS3Options,
)


class RepodataChannelConfig:
    """Configuration for repodata fetching for a specific channel."""

    _inner: PyRepodataChannelConfig

    @classmethod
    def _from_py_repodata_channel_config(
        cls, py_repodata_channel_config: PyRepodataChannelConfig
    ) -> RepodataChannelConfig:
        """Construct from FFI PyRepodataChannelConfig object."""
        config = cls.__new__(cls)
        config._inner = py_repodata_channel_config
        return config

    @property
    def disable_bzip2(self) -> Optional[bool]:
        """
        Whether bzip2 compression is disabled for repodata.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.repodata_config.default_config.disable_bzip2
        >>>
        ```
        """
        return self._inner.disable_bzip2

    @property
    def disable_zstd(self) -> Optional[bool]:
        """
        Whether zstd compression is disabled for repodata.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.repodata_config.default_config.disable_zstd
        >>>
        ```
        """
        return self._inner.disable_zstd

    @property
    def disable_sharded(self) -> Optional[bool]:
        """
        Whether sharded repodata is disabled.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.repodata_config.default_config.disable_sharded
        >>>
        ```
        """
        return self._inner.disable_sharded

    def __repr__(self) -> str:
        """
        Returns a representation of the RepodataChannelConfig.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.repodata_config.default_config
        RepodataChannelConfig(disable_bzip2=None, disable_zstd=None, disable_sharded=None)
        >>>
        ```
        """
        return self._inner.__repr__()


class RepodataConfig:
    """Configuration for repodata fetching.

    Contains a default configuration that applies to all channels,
    and optional per-channel overrides.
    """

    _inner: PyRepodataConfig

    @classmethod
    def _from_py_repodata_config(cls, py_repodata_config: PyRepodataConfig) -> RepodataConfig:
        """Construct from FFI PyRepodataConfig object."""
        config = cls.__new__(cls)
        config._inner = py_repodata_config
        return config

    @property
    def default_config(self) -> RepodataChannelConfig:
        """
        The default repodata channel configuration.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.repodata_config.default_config
        RepodataChannelConfig(disable_bzip2=None, disable_zstd=None, disable_sharded=None)
        >>>
        ```
        """
        return RepodataChannelConfig._from_py_repodata_channel_config(self._inner.default_config)

    @property
    def per_channel(self) -> Dict[str, RepodataChannelConfig]:
        """
        Per-channel repodata configuration (mapping from URL to config).

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.repodata_config.per_channel
        {}
        >>>
        ```
        """
        return {
            url: RepodataChannelConfig._from_py_repodata_channel_config(cfg)
            for url, cfg in self._inner.per_channel.items()
        }

    def __repr__(self) -> str:
        """
        Returns a representation of the RepodataConfig.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.repodata_config
        RepodataConfig(per_channel_count=0)
        >>>
        ```
        """
        return self._inner.__repr__()


class ConcurrencyConfig:
    """Configuration for concurrency limits."""

    _inner: PyConcurrencyConfig

    @classmethod
    def _from_py_concurrency_config(cls, py_concurrency_config: PyConcurrencyConfig) -> ConcurrencyConfig:
        """Construct from FFI PyConcurrencyConfig object."""
        config = cls.__new__(cls)
        config._inner = py_concurrency_config
        return config

    @property
    def solves(self) -> int:
        """
        The maximum number of concurrent solves.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.concurrency.solves > 0
        True
        >>>
        ```
        """
        return self._inner.solves

    @property
    def downloads(self) -> int:
        """
        The maximum number of concurrent downloads.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.concurrency.downloads
        50
        >>>
        ```
        """
        return self._inner.downloads

    def __repr__(self) -> str:
        """
        Returns a representation of the ConcurrencyConfig.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.concurrency  # doctest: +ELLIPSIS
        ConcurrencyConfig(solves=..., downloads=50)
        >>>
        ```
        """
        return self._inner.__repr__()


class ProxyConfig:
    """Proxy configuration for HTTP/HTTPS requests."""

    _inner: PyProxyConfig

    @classmethod
    def _from_py_proxy_config(cls, py_proxy_config: PyProxyConfig) -> ProxyConfig:
        """Construct from FFI PyProxyConfig object."""
        config = cls.__new__(cls)
        config._inner = py_proxy_config
        return config

    @property
    def https(self) -> Optional[str]:
        """
        The HTTPS proxy URL, if set.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.proxy_config.https
        >>>
        ```
        """
        return self._inner.https

    @property
    def http(self) -> Optional[str]:
        """
        The HTTP proxy URL, if set.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.proxy_config.http
        >>>
        ```
        """
        return self._inner.http

    @property
    def non_proxy_hosts(self) -> List[str]:
        """
        List of hosts that should bypass the proxy.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.proxy_config.non_proxy_hosts
        []
        >>>
        ```
        """
        return self._inner.non_proxy_hosts

    def __repr__(self) -> str:
        """
        Returns a representation of the ProxyConfig.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.proxy_config
        ProxyConfig(https=None, http=None, non_proxy_hosts_count=0)
        >>>
        ```
        """
        return self._inner.__repr__()


class S3Options:
    """S3 bucket configuration options."""

    _inner: PyS3Options

    @classmethod
    def _from_py_s3_options(cls, py_s3_options: PyS3Options) -> S3Options:
        """Construct from FFI PyS3Options object."""
        opts = cls.__new__(cls)
        opts._inner = py_s3_options
        return opts

    @property
    def endpoint_url(self) -> str:
        """
        The S3 endpoint URL.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> config = Config()
        >>> config.s3_options  # no S3 options by default
        {}
        >>>
        ```
        """
        return self._inner.endpoint_url

    @property
    def region(self) -> str:
        """The S3 region."""
        return self._inner.region

    @property
    def force_path_style(self) -> bool:
        """Whether to force path-style URLs instead of subdomain-style."""
        return self._inner.force_path_style

    def __repr__(self) -> str:
        """
        Returns a representation of the S3Options.
        """
        return self._inner.__repr__()


class Config:
    """Rattler configuration.

    This class represents the base configuration used by rattler and
    derived tools like pixi. It can be loaded from one or more TOML
    configuration files which are merged in order (later files override
    earlier ones).
    """

    _inner: PyConfig

    def __init__(self) -> None:
        """
        Create a new default configuration.

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config
        Config(default_channels=None, tls_no_verify=False, mirrors_count=0)
        >>>
        ```
        """
        self._inner = PyConfig()

    @staticmethod
    def load_from_files(*paths: Union[str, PathLike[str]]) -> Config:
        """
        Load configuration from one or more TOML files.

        Configurations are merged in order; later files override earlier ones.

        Arguments:
            *paths: Paths to TOML configuration files.

        Returns:
            The merged configuration.

        Examples
        --------
        ```python
        >>> config = Config.load_from_files("/path/to/config.toml")  # doctest: +SKIP
        >>> config.default_channels  # doctest: +SKIP
        ['conda-forge']
        >>>
        ```
        """
        import os

        resolved = [os.fspath(p) for p in paths]
        cfg = Config.__new__(Config)
        cfg._inner = PyConfig.load_from_files(*resolved)
        return cfg

    @property
    def default_channels(self) -> Optional[List[str]]:
        """
        Default channels as a list of strings, or ``None`` if not set.

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config.default_channels
        >>>
        ```
        """
        return self._inner.default_channels

    @property
    def authentication_override_file(self) -> Optional[str]:
        """
        Path to the authentication override file, or ``None`` if not set.

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config.authentication_override_file
        >>>
        ```
        """
        val = self._inner.authentication_override_file
        return str(val) if val is not None else None

    @property
    def tls_no_verify(self) -> Optional[bool]:
        """
        Whether TLS verification is disabled.

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config.tls_no_verify
        False
        >>>
        ```
        """
        return self._inner.tls_no_verify

    @property
    def mirrors(self) -> Dict[str, List[str]]:
        """
        Mirror configuration (URL to list of mirror URLs).

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config.mirrors
        {}
        >>>
        ```
        """
        return self._inner.mirrors

    @property
    def concurrency(self) -> ConcurrencyConfig:
        """
        The concurrency configuration.

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config.concurrency.downloads
        50
        >>>
        ```
        """
        return ConcurrencyConfig._from_py_concurrency_config(self._inner.concurrency)

    @property
    def proxy_config(self) -> ProxyConfig:
        """
        The proxy configuration.

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config.proxy_config
        ProxyConfig(https=None, http=None, non_proxy_hosts_count=0)
        >>>
        ```
        """
        return ProxyConfig._from_py_proxy_config(self._inner.proxy_config)

    @property
    def repodata_config(self) -> RepodataConfig:
        """
        The repodata configuration.

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config.repodata_config.default_config
        RepodataChannelConfig(disable_bzip2=None, disable_zstd=None, disable_sharded=None)
        >>>
        ```
        """
        return RepodataConfig._from_py_repodata_config(self._inner.repodata_config)

    @property
    def s3_options(self) -> Dict[str, S3Options]:
        """
        S3 options (bucket name to S3Options).

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config.s3_options
        {}
        >>>
        ```
        """
        return {name: S3Options._from_py_s3_options(opts) for name, opts in self._inner.s3_options.items()}

    @property
    def loaded_from(self) -> List[str]:
        """
        List of file paths this configuration was loaded from.

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config.loaded_from
        []
        >>>
        ```
        """
        return [str(p) for p in self._inner.loaded_from]

    def __repr__(self) -> str:
        """
        Returns a representation of the Config.

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config
        Config(default_channels=None, tls_no_verify=False, mirrors_count=0)
        >>>
        ```
        """
        return self._inner.__repr__()
