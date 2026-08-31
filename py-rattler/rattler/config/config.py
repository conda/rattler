from __future__ import annotations

import os
from pathlib import Path
from typing import Any, Dict, List, Literal, Optional, Tuple, Union

from rattler.channel import ChannelConfig
from rattler.rattler import PyConfig

TlsRootCerts = Literal["webpki", "system"]
"""Which root certificates to use for HTTPS connections.

* `'webpki'`: use the bundled Mozilla root certificates.
* `'system'`: use the system's native certificate store.
"""

RunPostLinkScripts = Literal["insecure", "false"]
"""Whether to run a package's post-link scripts.

* `'insecure'`: run them. Named "insecure" because they are arbitrary code.
* `'false'`: do not run them.
"""


class Config:
    """
    The configuration shared by all rattler-based tools (pixi, rattler-build,
    rattler-index, ...), as read from `config.toml` files.

    This exposes the keys every rattler-based tool understands. Tool-specific
    extension keys are a compile-time Rust generic and cannot be supplied from
    Python, so files containing them report those keys as unused.

    Examples
    --------
    ```python
    >>> config = Config.from_toml('''
    ...     default-channels = ["conda-forge"]
    ...     tls-no-verify = true
    ...
    ...     [concurrency]
    ...     downloads = 10
    ... ''')
    >>> config.default_channels
    ['conda-forge']
    >>> config.tls_no_verify
    True
    >>> config.concurrency_downloads
    10
    >>>
    ```
    """

    _inner: PyConfig

    def __init__(self) -> None:
        """
        Create a configuration with every key at its default.

        Examples
        --------
        ```python
        >>> Config()
        Config(loaded_from=[])
        >>>
        ```
        """
        self._inner = PyConfig()

    @classmethod
    def _from_py_config(cls, py_config: PyConfig) -> Config:
        """Construct from a raw `PyConfig`, bypassing `__init__`."""
        config = cls.__new__(cls)
        config._inner = py_config
        return config

    @staticmethod
    def from_toml(toml: str, shared: bool = False) -> Config:
        """
        Parse a configuration from a TOML string, discarding the keys that
        were not recognized. Use `from_toml_with_unused_keys` to inspect
        them.

        When `shared` is `True` the string is parsed as a *shared*
        configuration file: keys that a specific tool would understand but
        that are not shared by all rattler-based tools are not accepted.

        Examples
        --------
        ```python
        >>> Config.from_toml("tls-no-verify = true").tls_no_verify
        True
        >>>
        ```
        """
        config, _unused = Config.from_toml_with_unused_keys(toml, shared)
        return config

    @staticmethod
    def from_toml_with_unused_keys(toml: str, shared: bool = False) -> Tuple[Config, List[str]]:
        """
        Parse a configuration from a TOML string, returning it together with
        the sorted keys that were not recognized. Unrecognized keys are
        typos, keys of other tools, or — in a shared file — tool-specific
        keys.

        Examples
        --------
        ```python
        >>> config, unused = Config.from_toml_with_unused_keys("tls-no-verifi = true")
        >>> unused
        ['tls-no-verifi']
        >>>
        ```
        """
        if shared:
            py_config, unused = PyConfig.from_toml_shared(toml)
        else:
            py_config, unused = PyConfig.from_toml(toml)
        return Config._from_py_config(py_config), unused

    @staticmethod
    def load_from_files(paths: List[Union[str, os.PathLike[str]]]) -> Config:
        """
        Load a configuration by merging the given files, in order: later
        files take precedence over earlier ones. Every file is parsed as a
        tool configuration file and must exist. The merged configuration is
        validated.

        Raises
        ------
        ConfigError
            If a file is missing, cannot be parsed, or the merged
            configuration is invalid.
        """
        return Config._from_py_config(PyConfig.load_from_files([Path(path) for path in paths]))

    @staticmethod
    def load_from_locations(locations: List[Tuple[Union[str, os.PathLike[str]], bool]]) -> Config:
        """
        Load a configuration by merging the given `(path, is_shared)`
        locations, in order: later locations take precedence. Locations
        marked shared accept only the keys shared by all rattler-based
        tools.

        Raises
        ------
        ConfigError
            If a file is missing, cannot be parsed, or the merged
            configuration is invalid.
        """
        return Config._from_py_config(
            PyConfig.load_from_locations([(Path(path), is_shared) for path, is_shared in locations])
        )

    @staticmethod
    def load_from_default_locations(tool: str) -> Config:
        """
        Load the configuration from the default locations of `tool` (e.g.
        `'rattler-build'`), skipping files that do not exist. See
        `Config.config_search_paths` for the search order.

        Examples
        --------
        ```python
        >>> isinstance(Config.load_from_default_locations("rattler-build"), Config)
        True
        >>>
        ```
        """
        return Config._from_py_config(PyConfig.load_from_default_locations(tool))

    @staticmethod
    def config_search_paths(tool: str) -> List[Tuple[Path, bool]]:
        """
        The configuration file locations of `tool` as `(path, is_shared)`
        pairs, from lowest to highest precedence. The paths are candidates
        and are not checked for existence.

        Examples
        --------
        ```python
        >>> paths = Config.config_search_paths("rattler-build")
        >>> all(isinstance(path, Path) and isinstance(is_shared, bool) for path, is_shared in paths)
        True
        >>>
        ```
        """
        return [(Path(path), is_shared) for path, is_shared in PyConfig.config_search_paths(tool)]

    @property
    def loaded_from(self) -> List[Path]:
        """
        The files this configuration was loaded from, in load order (lowest
        precedence first).

        Examples
        --------
        ```python
        >>> Config().loaded_from
        []
        >>>
        ```
        """
        return [Path(path) for path in self._inner.loaded_from]

    @property
    def default_channels(self) -> Optional[List[str]]:
        """
        The channels to use when none are given explicitly.

        Examples
        --------
        ```python
        >>> Config.from_toml('default-channels = ["conda-forge"]').default_channels
        ['conda-forge']
        >>> Config().default_channels is None
        True
        >>>
        ```
        """
        return self._inner.default_channels

    @property
    def authentication_override_file(self) -> Optional[Path]:
        """
        The file to read authentication credentials from, instead of the
        default storage.
        """
        path = self._inner.authentication_override_file
        return Path(path) if path is not None else None

    @property
    def tls_no_verify(self) -> Optional[bool]:
        """
        Whether to skip verification of TLS server certificates.

        Examples
        --------
        ```python
        >>> Config.from_toml("tls-no-verify = true").tls_no_verify
        True
        >>>
        ```
        """
        return self._inner.tls_no_verify

    @property
    def tls_root_certs(self) -> Optional[TlsRootCerts]:
        """
        Which TLS root certificates to use. Whether this has any effect
        depends on the TLS backend the consumer is built with.

        Examples
        --------
        ```python
        >>> Config.from_toml('tls-root-certs = "webpki"').tls_root_certs
        'webpki'
        >>>
        ```
        """
        return self._inner.tls_root_certs

    @property
    def mirrors(self) -> Dict[str, List[str]]:
        """
        The configured mirrors, mapping an upstream channel URL to the
        mirrors to use for it.

        Examples
        --------
        ```python
        >>> config = Config.from_toml('''
        ...     [mirrors]
        ...     "https://conda.anaconda.org/conda-forge" = ["https://repo.prefix.dev/conda-forge"]
        ... ''')
        >>> config.mirrors
        {'https://conda.anaconda.org/conda-forge': ['https://repo.prefix.dev/conda-forge']}
        >>>
        ```
        """
        return dict(self._inner.mirrors)

    @property
    def build_package_format(self) -> Optional[str]:
        """
        The package format and compression level to build, as
        `'<format>:<level>'` — e.g. `'conda:max'` or `'tarbz2:5'`.

        Examples
        --------
        ```python
        >>> config = Config.from_toml('''
        ...     [build]
        ...     package-format = "conda:max"
        ... ''')
        >>> config.build_package_format
        'conda:max'
        >>>
        ```
        """
        return self._inner.build_package_format

    @property
    def channel_config(self) -> ChannelConfig:
        """
        The channel configuration used to resolve channel names into URLs.

        This key is not read from the configuration file; it always holds
        the default, rooted at the current working directory.
        """
        channel_config = ChannelConfig.__new__(ChannelConfig)
        channel_config._channel_configuration = self._inner.channel_config
        return channel_config

    @property
    def concurrency_solves(self) -> int:
        """
        The maximum number of solves to run concurrently. Defaults to the
        number of available CPUs.

        Examples
        --------
        ```python
        >>> config = Config.from_toml('''
        ...     [concurrency]
        ...     solves = 4
        ... ''')
        >>> config.concurrency_solves
        4
        >>>
        ```
        """
        return self._inner.concurrency_solves

    @property
    def concurrency_downloads(self) -> int:
        """
        The maximum number of concurrent HTTP requests to make. Defaults to
        50.

        Examples
        --------
        ```python
        >>> Config().concurrency_downloads
        50
        >>>
        ```
        """
        return self._inner.concurrency_downloads

    @property
    def proxy_https(self) -> Optional[str]:
        """The HTTPS proxy to use."""
        return self._inner.proxy_https

    @property
    def proxy_http(self) -> Optional[str]:
        """The HTTP proxy to use."""
        return self._inner.proxy_http

    @property
    def proxy_non_proxy_hosts(self) -> List[str]:
        """The hosts to reach without going through the proxy."""
        return self._inner.proxy_non_proxy_hosts

    @property
    def run_post_link_scripts(self) -> Optional[RunPostLinkScripts]:
        """
        Whether to run a package's post-link scripts.

        Examples
        --------
        ```python
        >>> Config.from_toml('run-post-link-scripts = "insecure"').run_post_link_scripts
        'insecure'
        >>>
        ```
        """
        return self._inner.run_post_link_scripts

    @property
    def allow_symbolic_links(self) -> Optional[bool]:
        """Whether symbolic links may be used when installing packages."""
        return self._inner.allow_symbolic_links

    @property
    def allow_hard_links(self) -> Optional[bool]:
        """Whether hard links may be used when installing packages."""
        return self._inner.allow_hard_links

    @property
    def allow_ref_links(self) -> Optional[bool]:
        """Whether ref links (copy-on-write) may be used when installing packages."""
        return self._inner.allow_ref_links

    @property
    def repodata_config(self) -> Dict[str, Any]:
        """
        The repodata fetching configuration, as a nested dictionary. The
        channel-independent options are at the top level; per-channel
        overrides are keyed by channel URL.

        Examples
        --------
        ```python
        >>> config = Config.from_toml('''
        ...     [repodata-config]
        ...     disable-zstd = true
        ... ''')
        >>> config.repodata_config["disable-zstd"]
        True
        >>>
        ```
        """
        return dict(self._inner.repodata_config)

    @property
    def s3_options(self) -> Dict[str, Any]:
        """
        The S3 configuration, mapping a bucket name to its options
        (`endpoint-url`, `region`, `force-path-style`).

        Examples
        --------
        ```python
        >>> config = Config.from_toml('''
        ...     [s3-options.my-bucket]
        ...     endpoint-url = "https://my-s3.example.com"
        ...     region = "eu-central-1"
        ...     force-path-style = false
        ... ''')
        >>> config.s3_options["my-bucket"]["region"]
        'eu-central-1'
        >>>
        ```
        """
        return dict(self._inner.s3_options)

    @property
    def index_config(self) -> Dict[str, Any]:
        """
        The `rattler-index` configuration, as a nested dictionary. The
        default options are at the top level; per-channel overrides are
        keyed by channel URL, URL prefix, or absolute path.

        Use `resolve_index_config` to get the effective options for a
        specific channel.
        """
        return dict(self._inner.index_config)

    def resolve_index_config(self, channel: str) -> Dict[str, Any]:
        """
        The effective `rattler-index` options for `channel`, resolved by
        layering the matching per-channel entries onto the defaults.
        The longest matching prefix wins.

        Examples
        --------
        ```python
        >>> config = Config.from_toml('''
        ...     [index-config]
        ...     write-zst = true
        ...
        ...     [index-config."s3://my-bucket/staging"]
        ...     write-shards = false
        ... ''')
        >>> config.resolve_index_config("s3://my-bucket/staging")["write-shards"]
        False
        >>> config.resolve_index_config("s3://my-bucket/staging")["write-zst"]
        True
        >>>
        ```
        """
        return dict(self._inner.resolve_index_config(channel))

    def merge(self, other: Config) -> Config:
        """
        Merge `other` into a copy of this configuration and return it.
        `other` takes precedence.

        Examples
        --------
        ```python
        >>> low = Config.from_toml("tls-no-verify = false")
        >>> high = Config.from_toml("tls-no-verify = true")
        >>> low.merge(high).tls_no_verify
        True
        >>>
        ```
        """
        return Config._from_py_config(self._inner.merge(other._inner))

    def validate(self) -> None:
        """
        Validate this configuration.

        Raises
        ------
        ValueError
            If the configuration is invalid.

        Examples
        --------
        ```python
        >>> Config().validate()
        >>>
        ```
        """
        self._inner.validate()

    def keys(self) -> List[str]:
        """
        The dotted TOML key paths this configuration understands. These are
        the keys accepted by `set` and `unset`.

        Examples
        --------
        ```python
        >>> "concurrency.solves" in Config().keys()
        True
        >>>
        ```
        """
        return self._inner.keys()

    def set(self, key: str, value: str) -> None:
        """
        Set the value at the dotted TOML key path `key`, modifying this
        configuration in place. `value` is interpreted as JSON when
        possible (`true`, `5`, `["conda-forge"]`) and as a plain string
        otherwise. Segments containing dots can be quoted:
        `mirrors."https://conda.anaconda.org"`.

        Raises
        ------
        ValueError
            If `key` is not a known configuration key, or `value` does not
            fit it.

        Examples
        --------
        ```python
        >>> config = Config()
        >>> config.set("concurrency.solves", "4")
        >>> config.concurrency_solves
        4
        >>> config.set("default-channels", '["conda-forge"]')
        >>> config.default_channels
        ['conda-forge']
        >>>
        ```
        """
        self._inner.set(key, value)

    def unset(self, key: str) -> None:
        """
        Reset the dotted TOML key path `key` to its default, modifying this
        configuration in place. Unsetting a key that is not set is not an
        error, but the key itself must be known.

        Raises
        ------
        ValueError
            If `key` is not a known configuration key.

        Examples
        --------
        ```python
        >>> config = Config.from_toml("tls-no-verify = true")
        >>> config.unset("tls-no-verify")
        >>> config.tls_no_verify is None
        True
        >>>
        ```
        """
        self._inner.set(key, None)

    def to_toml(self) -> str:
        """
        Serialize this configuration to a TOML string. Keys at their
        default are omitted.

        Examples
        --------
        ```python
        >>> Config.from_toml("tls-no-verify = true").to_toml()
        'tls-no-verify = true\\n'
        >>>
        ```
        """
        return self._inner.to_toml()

    def save(self, path: Union[str, os.PathLike[str]]) -> None:
        """
        Write this configuration to `path` as TOML, creating parent
        directories as needed.

        Raises
        ------
        ValueError
            If the configuration cannot be serialized or written.
        """
        self._inner.save(Path(path))

    def __repr__(self) -> str:
        """
        Examples
        --------
        ```python
        >>> Config.from_toml("tls-no-verify = true")
        Config(loaded_from=[])
        >>>
        ```
        """
        return f"Config(loaded_from={[str(path) for path in self.loaded_from]!r})"
