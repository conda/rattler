from __future__ import annotations

from enum import Enum
from pathlib import Path
from typing import List

from rattler.rattler import PyPackageCache, PyValidationMode


class ValidationMode(Enum):
    """Controls how cached packages are validated."""

    Skip = PyValidationMode.Skip
    Fast = PyValidationMode.Fast
    Full = PyValidationMode.Full


class PackageCache:
    """Manages a cache of extracted Conda packages on disk.

    Supports single-directory and layered (multi-directory) configurations.
    In a layered setup, layers are queried in the order they were provided.
    The first writable layer is used for storing newly fetched packages.

    Examples
    --------
    >>> from pathlib import Path
    >>> import tempfile, os
    >>> d = tempfile.mkdtemp()
    >>> cache = PackageCache(Path(d))
    >>> os.path.isdir(d)
    True
    """

    def __init__(
        self,
        path: Path,
        *,
        cache_origin: bool = False,
        validation_mode: ValidationMode = ValidationMode.Skip,
    ) -> None:
        self._inner = PyPackageCache(path, cache_origin, validation_mode.value)

    @classmethod
    def _from_py_package_cache(cls, py_package_cache: PyPackageCache) -> PackageCache:
        """Construct PackageCache from FFI PyPackageCache object."""
        cache = cls.__new__(cls)
        cache._inner = py_package_cache
        return cache

    @classmethod
    def new_layered(
        cls,
        paths: List[Path],
        *,
        cache_origin: bool = False,
        validation_mode: ValidationMode = ValidationMode.Skip,
    ) -> PackageCache:
        """Construct a layered PackageCache from multiple directories.

        Parameters
        ----------
        paths : list of Path
            Cache directories in priority order.
        cache_origin : bool, optional
            Include the origin in the cache key. Defaults to ``False``.
        validation_mode : ValidationMode, optional
            Validation level. Defaults to ``ValidationMode.Skip``.

        Examples
        --------
        >>> import tempfile
        >>> d1, d2 = tempfile.mkdtemp(), tempfile.mkdtemp()
        >>> cache = PackageCache.new_layered([Path(d1), Path(d2)])
        >>> len(cache.paths()) == 2
        True
        """
        cache = cls.__new__(cls)
        cache._inner = PyPackageCache.new_layered(
            paths, cache_origin, validation_mode.value
        )
        return cache

    def paths(self) -> List[Path]:
        """Returns all cache layer paths in their original insertion order."""
        return [Path(p) for p in self._inner.paths()]

    def writable_paths(self) -> List[Path]:
        """Returns only writable cache layer paths."""
        return [Path(p) for p in self._inner.writable_paths()]

    def readonly_paths(self) -> List[Path]:
        """Returns only read-only cache layer paths."""
        return [Path(p) for p in self._inner.readonly_paths()]

    def __repr__(self) -> str:
        return f"PackageCache(paths={self.paths()!r})"
