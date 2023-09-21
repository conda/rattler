from __future__ import annotations
from typing import Self

from rattler.rattler import PyPrefixPaths


class PrefixPaths:
    _paths: PyPrefixPaths

    @classmethod
    def _from_py_prefix_paths(cls, py_prefix_paths: PyPrefixPaths) -> Self:
        """Construct Rattler PrefixRecord from FFI PyPrefixRecord object."""
        paths = cls.__new__(cls)
        paths._paths = py_prefix_paths
        return paths
