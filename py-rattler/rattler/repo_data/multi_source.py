from __future__ import annotations

from collections.abc import Sequence
from typing import TYPE_CHECKING

from rattler.channel.channel import Channel
from rattler.rattler import PyMultiSource
from rattler.repo_data.gateway import _convert_sources

if TYPE_CHECKING:
    from rattler.repo_data.source import RepoDataSource
    from rattler.repo_data.sparse import SparseRepoData


class MultiSource:
    def __init__(
        self,
        name: str,
        sources: Sequence[Channel | str | SparseRepoData | RepoDataSource],
    ) -> None:
        """
        Create a named group of sources, like conda's `defaults` channel or its
        `custom_multichannels`. The sources can be channels (by name, URL, or
        Channel object), `SparseRepoData` or custom `RepoDataSource`
        implementations, but not another `MultiSource`.

        The sources of a group share a single channel priority tier when
        solving: with strict channel priority a package in an earlier source does
        not hide a newer build in a later one, while sources outside the
        group are still excluded. A match spec like `defaults::python`
        selects packages from any of the sources. The order of the sources only
        breaks ties between otherwise identical packages.

        ```python
        >>> MultiSource("defaults", ["pkgs/main", "pkgs/r"])
        MultiSource(name="defaults")
        >>> MultiSource("defaults", [])
        Traceback (most recent call last):
        ...
        ValueError: multichannel 'defaults' does not contain any sources
        >>>
        ```
        """
        self._sources = list(sources)
        self._multi_source = PyMultiSource(name, _convert_sources(self._sources))

    @property
    def name(self) -> str:
        """
        Return the name of this group.

        Examples
        --------
        ```python
        >>> MultiSource("defaults", ["pkgs/main"]).name
        'defaults'
        >>>
        ```
        """
        return self._multi_source.name

    @property
    def sources(self) -> list[Channel | str | SparseRepoData | RepoDataSource]:
        """Return the sources of this group, in the order they were given."""
        return list(self._sources)

    def __repr__(self) -> str:
        return f'MultiSource(name="{self.name}")'
