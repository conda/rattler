from __future__ import annotations

import warnings
from collections.abc import Callable
from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal

from rattler.networking.client import Client
from rattler.rattler import PyFetchRepoDataOptions, py_fetch_repo_data
from rattler.repo_data.sparse import SparseRepoData

if TYPE_CHECKING:
    import os

    from rattler.channel import Channel
    from rattler.platform import Subdir


CacheAction = Literal["cache-or-fetch", "use-cache-only", "force-cache-only", "no-cache"]
"""How the repodata cache should be used. `cache-or-fetch` validates the cache and
re-fetches when it is stale, `use-cache-only` never hits the network and errors if
nothing is cached, `force-cache-only` uses whatever is cached even if it is stale,
and `no-cache` always re-fetches."""

Variant = Literal["after-patches", "from-packages", "current"]
"""Which repodata variant to fetch from the channel."""


@dataclass
class FetchRepoDataOptions:
    cache_action: CacheAction = "cache-or-fetch"
    """How to interact with the cache.

    * `'cache-or-fetch'` (default): Use the cache if its up to date or fetch from the URL if there is no valid cached value.
    * `'use-cache-only'`: Only use the cache, but error out if the cache is not up to date
    * `'force-cache-only'`: Only use the cache, ignore whether or not it is up to date.
    * `'no-cache'`: Do not use the cache even if there is an up to date entry
    """

    variant: Variant = "after-patches"
    """Which type of repodata to download

    * `'after-patches'` (default): Fetch the `repodata.json` file. This `repodata.json` has repodata patches applied.
    * `'from-packages'` Fetch the `repodata_from_packages.json` file
    * `'current'`: Fetch `current_repodata.json` file. This file contains only the latest version of each package.
    """

    zstd_enabled: bool = True
    """Whether the ZSTD compression is enabled or not."""

    bz2_enabled: bool = True
    """Whether the BZ2 compression is enabled or not."""

    jlap_enabled: bool | None = None
    """Deprecated: JLAP support has been removed. This field is ignored."""

    def __post_init__(self) -> None:
        if self.jlap_enabled is not None:
            warnings.warn(
                "The 'jlap_enabled' option is deprecated and has no effect. JLAP support has been removed.",
                DeprecationWarning,
                stacklevel=2,
            )

    def _into_py(self) -> PyFetchRepoDataOptions:
        """
        Converts this object into a type that can be used by the Rust code.

        Examples
        --------
        ```python
        >>> FetchRepoDataOptions()._into_py() # doctest: +ELLIPSIS
        <builtins.PyFetchRepoDataOptions object at 0x...>
        >>>
        ```
        """
        return PyFetchRepoDataOptions(
            cache_action=self.cache_action,
            variant=self.variant,
            zstd_enabled=self.zstd_enabled,
            bz2_enabled=self.bz2_enabled,
        )


async def fetch_repo_data(
    *,
    channels: list[Channel],
    platforms: list[Subdir],
    cache_path: str | os.PathLike[str],
    callback: Callable[[int, int], None] | None,
    client: Client | None = None,
    fetch_options: FetchRepoDataOptions | None = None,
) -> list[SparseRepoData]:
    """
    Returns a list of RepoData for given channels and platform.

    Arguments:
        channels: A list of `Channel`s to fetch repo data.
        platforms: A list of `Subdir`s for which the repo data
                   should be fetched.
        cache_path: A `os.PathLike[str]` where the repo data should
                    be downloaded.
        callback: A `Callable[[int, int], None]` to report the download
                  progress of repo data.
        client: A `Client` to use for fetching the repo data.

    Returns:
        A list of `SparseRepoData` for requested channels and platforms.
    """
    fetch_options = fetch_options or FetchRepoDataOptions()
    repo_data_list = await py_fetch_repo_data(
        [channel._channel for channel in channels],
        [platform._inner for platform in platforms],
        cache_path,
        callback,
        client._client if client else None,
        fetch_options._into_py(),
    )

    return [SparseRepoData._from_py_sparse_repo_data(repo_data) for repo_data in repo_data_list]
