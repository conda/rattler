from __future__ import annotations

import os
from pathlib import Path
from typing import TYPE_CHECKING

from rattler.match_spec import MatchSpec
from rattler.rattler import (
    PyCacheIndex,
    PyPackageCache,
    PyPackageCacheLayer,
    PyValidationMode,
)

if TYPE_CHECKING:
    from rattler.repo_data import PackageRecord, RepoDataRecord

    BasePathLike = os.PathLike[str]
else:
    BasePathLike = os.PathLike


class ValidationMode:
    """The mode in which the package cache validation should be performed."""

    _inner: PyValidationMode

    def __init__(self) -> None:
        raise NotImplementedError("Use ValidationMode.skip(), ValidationMode.fast(), or ValidationMode.full()")

    @classmethod
    def _from_py_validation_mode(cls, py_val_mode: PyValidationMode) -> ValidationMode:
        mode = cls.__new__(cls)
        mode._inner = py_val_mode
        return mode

    @classmethod
    def skip(cls) -> ValidationMode:
        """Only check if the package directory exists and contains a valid index.json."""
        return cls._from_py_validation_mode(PyValidationMode.skip())

    @classmethod
    def fast(cls) -> ValidationMode:
        """Only check if the files exist. Do not check if the hashes match."""
        return cls._from_py_validation_mode(PyValidationMode.fast())

    @classmethod
    def full(cls) -> ValidationMode:
        """Check if the files exist and the content matches the hashes."""
        return cls._from_py_validation_mode(PyValidationMode.full())

    @property
    def is_skip(self) -> bool:
        """Returns True if this is skip validation mode."""
        return self._inner.is_skip

    @property
    def is_fast(self) -> bool:
        """Returns True if this is fast validation mode."""
        return self._inner.is_fast

    @property
    def is_full(self) -> bool:
        """Returns True if this is full validation mode."""
        return self._inner.is_full

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, ValidationMode):
            return False
        return self._inner == other._inner

    def __hash__(self) -> int:
        return hash(self._inner)

    def __repr__(self) -> str:
        return self._inner.__repr__()


class PackageCacheLayer:
    """A layer in a `PackageCache` representing a specific cache directory."""

    _inner: PyPackageCacheLayer

    def __init__(self, path: str | BasePathLike) -> None:
        """Creates an unfiltered package-cache layer pointing to the given path."""
        self._inner = PyPackageCacheLayer(Path(path))

    @classmethod
    def _from_py_layer(cls, py_layer: PyPackageCacheLayer) -> PackageCacheLayer:
        layer = cls.__new__(cls)
        layer._inner = py_layer
        return layer

    @property
    def path(self) -> Path:
        """Returns the root directory of this layer."""
        return Path(self._inner.path)

    def is_readonly(self) -> bool:
        """Returns whether the layer is read-only on the filesystem."""
        return self._inner.is_readonly()

    def with_validation_mode(self, validation_mode: ValidationMode) -> PackageCacheLayer:
        """Sets the validation mode used by this layer."""
        return PackageCacheLayer._from_py_layer(self._inner.with_validation_mode(validation_mode._inner))

    def with_filter(self, spec: MatchSpec) -> PackageCacheLayer:
        """Adds an inclusion MatchSpec to this layer."""
        return PackageCacheLayer._from_py_layer(self._inner.with_filter(spec._match_spec))

    def excluding(self, spec: MatchSpec) -> PackageCacheLayer:
        """Excludes packages matching spec from this layer."""
        return PackageCacheLayer._from_py_layer(self._inner.excluding(spec._match_spec))

    def __repr__(self) -> str:
        return self._inner.__repr__()


class CacheIndex:
    """A snapshot of the packages present in a `PackageCache`."""

    _inner: PyCacheIndex

    def __init__(self) -> None:
        raise NotImplementedError("Use PackageCache.index() to create a CacheIndex instance.")

    @classmethod
    def _from_py_index(cls, py_index: PyCacheIndex) -> CacheIndex:
        idx = cls.__new__(cls)
        idx._inner = py_index
        return idx

    def contains_record(self, record: RepoDataRecord) -> bool:
        """Returns whether the package described by `RepoDataRecord` is present in the cache."""
        return self._inner.contains_record(record._record)

    def contains_url(self, record: PackageRecord | RepoDataRecord, url: str) -> bool:
        """Returns whether a package fetched from url is present in the cache."""
        return self._inner.contains_url(record._record, url)

    def contains_path(self, record: PackageRecord | RepoDataRecord, path: str | BasePathLike) -> bool:
        """Returns whether a package fetched from path is present in the cache."""
        return self._inner.contains_path(record._record, Path(path))

    def is_empty(self) -> bool:
        """Returns True if the cache index contains no packages."""
        return self._inner.is_empty()

    def __len__(self) -> int:
        return len(self._inner)

    def __repr__(self) -> str:
        return self._inner.__repr__()


class PackageCache:
    """Manages a cache of extracted Conda packages on disk."""

    _inner: PyPackageCache

    def __init__(self, path: str | BasePathLike) -> None:
        """Creates a new PackageCache targeting the given directory."""
        self._inner = PyPackageCache(Path(path))

    @classmethod
    def _from_py_package_cache(cls, py_cache: PyPackageCache) -> PackageCache:
        cache = cls.__new__(cls)
        cache._inner = py_cache
        return cache

    @classmethod
    def new_layered(
        cls,
        paths: list[str | BasePathLike],
        cache_origin: bool = False,
        validation_mode: ValidationMode | None = None,
    ) -> PackageCache:
        """Constructs a new `PackageCache` located at the specified paths."""
        rust_paths = [Path(p) for p in paths]
        val_mode = validation_mode._inner if validation_mode is not None else None
        return cls._from_py_package_cache(PyPackageCache.new_layered(rust_paths, cache_origin, val_mode))

    @classmethod
    def from_layers(cls, layers: list[PackageCacheLayer], cache_origin: bool = False) -> PackageCache:
        """Creates a PackageCache from multiple configured layers."""
        py_layers = [layer._inner for layer in layers]
        return cls._from_py_package_cache(PyPackageCache.from_layers(py_layers, cache_origin))

    def with_cached_origin(self) -> PackageCache:
        """Adds the origin (url or path) to the cache key."""
        return PackageCache._from_py_package_cache(self._inner.with_cached_origin())

    def with_prepended_layer(self, layer: PackageCacheLayer) -> PackageCache:
        """Prepends a configured layer to this cache."""
        return PackageCache._from_py_package_cache(self._inner.with_prepended_layer(layer._inner))

    def excluding_from_all_layers(self, spec: MatchSpec) -> PackageCache:
        """Excludes packages matching spec from every existing layer."""
        return PackageCache._from_py_package_cache(self._inner.excluding_from_all_layers(spec._match_spec))

    def first_writable(self) -> Path | None:
        """Returns the first writable directory in the package cache, if any."""
        path = self._inner.first_writable()
        return Path(path) if path is not None else None

    def layers(self) -> list[PackageCacheLayer]:
        """Returns the layers in this package cache in lookup order."""
        return [PackageCacheLayer._from_py_layer(layer) for layer in self._inner.layers()]

    async def index(self) -> CacheIndex:
        """Takes an async snapshot of the packages present in the cache."""
        py_index = await self._inner.index()
        return CacheIndex._from_py_index(py_index)

    def __repr__(self) -> str:
        return self._inner.__repr__()
