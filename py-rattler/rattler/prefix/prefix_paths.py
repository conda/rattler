from __future__ import annotations
import os
from typing import List

from rattler.rattler import PyPrefixPaths


class PrefixPaths:
    _paths: PyPrefixPaths

    @classmethod
    def _from_py_prefix_paths(cls, py_prefix_paths: PyPrefixPaths) -> PrefixPaths:
        """Construct Rattler PrefixRecord from FFI PyPrefixRecord object."""
        paths = cls.__new__(cls)
        paths._paths = py_prefix_paths
        return paths

    @property
    def paths_version(self) -> int:
        """
        The version of the file.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> paths = r.paths_data
        >>> paths.paths_version
        1
        >>>
        ```
        """
        return self._paths.paths_version

    @property
    def paths(self) -> List[os.PathLike[str]]:
        """
        All entries included in the package.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> paths = r.paths_data
        >>> paths.paths # doctest:+ELLIPSIS
        [...]
        >>>
        ```
        """
        return self._paths.paths

    def __repr__(self) -> str:
        """
        Returns a representation of the PrefixPaths.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> r.paths_data
        PrefixPaths()
        >>>
        ```
        """
        return "PrefixPaths()"
