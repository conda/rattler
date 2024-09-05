from __future__ import annotations

import os
from typing import Optional, List, Literal
from dataclasses import dataclass

from rattler.rattler import PyGateway, PySourceConfig, PyMatchSpec

from rattler.channel import Channel
from rattler.match_spec import MatchSpec
from rattler.repo_data.record import RepoDataRecord
from rattler.platform import Platform, PlatformLiteral
from rattler.package.package_name import PackageName

CacheAction = Literal["cache-or-fetch", "use-cache-only", "force-cache-only", "no-cache"]


@dataclass
class SourceConfig:
    """
    Describes properties about a channel.

    This can be used to configure the Gateway to handle channels in a certain
    way.
    """

    jlap_enabled: bool = True
    """Whether the JLAP compression is enabled or not."""

    zstd_enabled: bool = True
    """Whether the ZSTD compression is enabled or not."""

    bz2_enabled: bool = True
    """Whether the BZ2 compression is enabled or not."""

    cache_action: CacheAction = "cache-or-fetch"
    """How to interact with the cache.
    
    * `'cache-or-fetch'` (default): Use the cache if its up to date or fetch from the URL if there is no valid cached value.
    * `'use-cache-only'`: Only use the cache, but error out if the cache is not up to date
    * `'force-cache-only'`: Only use the cache, ignore whether or not it is up to date.
    * `'no-cache'`: Do not use the cache even if there is an up to date entry
    """

    def _into_py(self) -> PySourceConfig:
        """
        Converts this object into a type that can be used by the Rust code.

        Examples
        --------
        ```python
        >>> SourceConfig()._into_py() # doctest: +ELLIPSIS
        <builtins.PySourceConfig object at 0x...>
        >>>
        ```
        """
        return PySourceConfig(
            jlap_enabled=self.jlap_enabled,
            zstd_enabled=self.zstd_enabled,
            bz2_enabled=self.bz2_enabled,
            cache_action=self.cache_action,
        )


class Gateway:
    """
    The gateway manages all the quircks and complex bits of efficiently acquiring
    repodata. It implements all the necessary logic to fetch the repodata from a
    remote server, cache it locally and convert it into python objects.

    The gateway can also easily be used concurrently, as it is designed to be
    thread-safe. When two threads are querying the same channel at the same time,
    their requests are coalesced into a single request. This is done to reduce the
    number of requests made to the remote server and reduce the overall memory usage.

    The gateway caches the repodata internally, so if the same channel is queried
    multiple times the records will only be fetched once. However, the conversion
    of the records to a python object is done every time the query method is called.
    Therefor, instead of requesting records directly, its more efficient to pass the
    gateway itself to methods that accepts it.
    """

    def __init__(
        self,
        cache_dir: Optional[os.PathLike[str]] = None,
        default_config: Optional[SourceConfig] = None,
        per_channel_config: Optional[dict[Channel | str, SourceConfig]] = None,
        max_concurrent_requests: int = 100,
    ) -> None:
        """
        Arguments:
            cache_dir: The directory where the repodata should be cached. If not specified the
                       default cache directory is used.
            default_config: The default configuration for channels.
            per_channel_config: Per channel configuration.
            max_concurrent_requests: The maximum number of concurrent requests that can be made.

        Examples
        --------
        ```python
        >>> Gateway()
        Gateway()
        >>>
        ```
        """
        default_config = default_config or SourceConfig()

        self._gateway = PyGateway(
            cache_dir=cache_dir,
            default_config=default_config._into_py(),
            per_channel_config={
                channel._channel if isinstance(channel, Channel) else Channel(channel)._channel: config._into_py()
                for channel, config in (per_channel_config or {}).items()
            },
            max_concurrent_requests=max_concurrent_requests,
        )

    async def query(
        self,
        channels: List[Channel | str],
        platforms: List[Platform | PlatformLiteral],
        specs: List[MatchSpec | PackageName | str],
        recursive: bool = True,
    ) -> List[List[RepoDataRecord]]:
        """Queries the gateway for repodata.

        If `recursive` is `True` the gateway will recursively fetch the dependencies of the
        encountered records. If `recursive` is `False` only the records with the package names
        specified in `specs` are returned.

        The `specs` can either be a `MatchSpec`, `PackageName` or a string. If a string or a
        `PackageName` is provided it will be converted into a MatchSpec that matches any record
        with the given name. If a `MatchSpec` is provided all records that match the name
        specified in the spec will be returned, but only the dependencies of the records
        that match the entire spec are recursively fetched.

        The gateway caches the records internally, so if the same channel is queried multiple
        times the records will only be fetched once. However, the conversion of the records to
        a python object is done every time the query method is called.

        Arguments:
            channels: The channels to query.
            platforms: The platforms to query.
            specs: The specs to query.
            recursive: Whether recursively fetch dependencies or not.

        Returns:
            A list of lists of `RepoDataRecord`s. The outer list contains the results for each
            channel in the same order they are provided in the `channels` argument.

        Examples
        --------
        ```python
        >>> import asyncio
        >>> gateway = Gateway()
        >>> records = asyncio.run(gateway.query(["conda-forge"], ["linux-aarch64"], ["python"]))
        >>> assert len(records) == 1
        >>>
        ```
        """
        py_records = await self._gateway.query(
            channels=[
                channel._channel if isinstance(channel, Channel) else Channel(channel)._channel for channel in channels
            ],
            platforms=[
                platform._inner if isinstance(platform, Platform) else Platform(platform)._inner
                for platform in platforms
            ],
            specs=[spec._match_spec if isinstance(spec, MatchSpec) else PyMatchSpec(str(spec), True) for spec in specs],
            recursive=recursive,
        )

        # Convert the records into python objects
        return [[RepoDataRecord._from_py_record(record) for record in records] for records in py_records]

    def clear_repodata_cache(
        self, channel: Channel | str, subdirs: Optional[List[Platform | PlatformLiteral]] = None
    ) -> None:
        """
        Clears any in-memory cache for the given channel.

        Any subsequent query will re-fetch any required data from the source.

        This method does not clear any on-disk cache.

        Arguments:
            channel: The channel to clear the cache for.
            subdirs: A selection of subdirectories to clear, if `None` is specified
                     all subdirectories of the channel are cleared.

        Examples
        --------
        >>> gateway = Gateway()
        >>> gateway.clear_repodata_cache("conda-forge", ["linux-64"])
        >>> gateway.clear_repodata_cache("robostack")
        >>>
        """
        self._gateway.clear_repodata_cache(
            channel._channel if isinstance(channel, Channel) else Channel(channel)._channel,
            {subdir._inner if isinstance(subdir, Platform) else Platform(subdir)._inner for subdir in subdirs}
            if subdirs is not None
            else None,
        )

    def __repr__(self) -> str:
        """
        Returns a representation of the Gateway.

        Examples
        --------
        ```python
        >>> Gateway()
        Gateway()
        >>>
        ```
        """
        return f"{type(self).__name__}()"
