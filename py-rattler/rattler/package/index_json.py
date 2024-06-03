from __future__ import annotations
import os
import datetime
from pathlib import Path
from typing import List, Optional

from rattler.package.package_name import PackageName
from rattler.rattler import PyIndexJson


class IndexJson:
    _inner: PyIndexJson

    @staticmethod
    def from_package_archive(path: os.PathLike[str]) -> IndexJson:
        """
        Parses the package file from archive.
        Note: If you want to extract multiple `info/*` files then this will be slightly
              slower than manually iterating over the archive entries with
              custom logic as this skips over the rest of the archive

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json
        IndexJson()
        >>>
        ```
        """
        return IndexJson._from_py_index_json(PyIndexJson.from_package_archive(path))

    @staticmethod
    def from_package_directory(path: os.PathLike[str]) -> IndexJson:
        """
        Parses the object by looking up the appropriate file from the root of the
        specified Conda archive directory, using a format appropriate for the file
        type.

        For example, if the file is in JSON format, this function reads the
        appropriate file from the archive, parse the JSON string and return the
        resulting object. If the file is not in a parsable format or if the file
        could not be read, this function returns an error.
        """
        return IndexJson._from_py_index_json(PyIndexJson.from_package_directory(Path(path)))

    @staticmethod
    def from_str(string: str) -> IndexJson:
        """
        Parses the object from a string, using a format appropriate for the file
        type.

        For example, if the file is in JSON format, this function parses the JSON
        string and returns the resulting object. If the file is not in a parsable
        format, this function returns an error.
        """
        return IndexJson._from_py_index_json(PyIndexJson.from_str(string))

    @staticmethod
    def package_path() -> str:
        """
        Returns the path to the file within the Conda archive.

        The path is relative to the root of the archive and includes any necessary
        directories.
        """
        return PyIndexJson.package_path()

    @property
    def arch(self) -> Optional[str]:
        """
        Optionally, the architecture the package is build for.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.arch
        'ppc64le'
        >>>
        ```
        """
        if arch := self._inner.arch:
            return arch

        return None

    @property
    def build(self) -> str:
        """
        The build string of the package.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.build
        'h2c4edbf_0_cpython'
        >>>
        ```
        """
        return self._inner.build

    @property
    def build_number(self) -> int:
        """
        The build number of the package.
        This is also included in the build string.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.build_number
        0
        >>>
        ```
        """
        return self._inner.build_number

    @property
    def constrains(self) -> List[str]:
        """
        The package constraints of the package.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.constrains
        ['python_abi 3.10.* *_cp310']
        >>>
        ```
        """
        return self._inner.constrains

    @property
    def depends(self) -> List[str]:
        """
        The dependencies of the package.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.depends
        ['bzip2 >=1.0.8,<2.0a0', ..., 'xz >=5.2.6,<5.3.0a0']
        >>>
        ```
        """
        return self._inner.depends

    @property
    def features(self) -> Optional[str]:
        """
        Features are a deprecated way to specify different feature sets for the conda solver. This is not
        supported anymore and should not be used. Instead, `mutex` packages should be used to specify
        mutually exclusive features.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.features
        >>>
        ```
        """
        if features := self._inner.features:
            return features

        return None

    @property
    def license(self) -> Optional[str]:
        """
        Optionally, the license.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.license
        'Python-2.0'
        >>>
        ```
        """
        if license := self._inner.license:
            return license

        return None

    @property
    def license_family(self) -> Optional[str]:
        """
        Optionally, the license.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.license_family
        >>>
        ```
        """
        if license_family := self._inner.license_family:
            return license_family

        return None

    @property
    def name(self) -> PackageName:
        """
        The lowercase name of the package.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.name
        PackageName("python")
        >>>
        ```
        """
        return PackageName._from_py_package_name(self._inner.name)

    @property
    def platform(self) -> Optional[str]:
        """
        Optionally, the OS the package is build for.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.platform
        'linux'
        >>>
        ```
        """
        if platform := self._inner.platform:
            return platform

        return None

    @property
    def subdir(self) -> Optional[str]:
        """
        The subdirectory that contains this package.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.subdir
        'linux-ppc64le'
        >>>
        ```
        """
        if subdir := self._inner.subdir:
            return subdir

        return None

    @property
    def timestamp(self) -> Optional[datetime.datetime]:
        """
        The timestamp when this package was created

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.timestamp
        datetime.datetime(2022, 8, 22, 20, 39, 2, 467000, tzinfo=datetime.timezone.utc)
        >>>
        ```
        """
        if timestamp := self._inner.timestamp:
            return datetime.datetime.fromtimestamp(timestamp / 1000.0, tz=datetime.timezone.utc)

        return None

    @property
    def track_features(self) -> List[str]:
        """
        Track features are nowadays only used to downweight packages (ie. give them less priority). To
        that effect, the number of track features is counted (number of commas) and the package is downweighted
        by the number of track_features.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_package_archive(
        ...     "../test-data/with-symlinks/python-3.10.6-h2c4edbf_0_cpython.tar.bz2"
        ... )
        >>> idx_json.track_features
        []
        >>>
        ```
        """
        return self._inner.track_features

    @classmethod
    def _from_py_index_json(cls, py_index_json: PyIndexJson) -> IndexJson:
        index_json = cls.__new__(cls)
        index_json._inner = py_index_json

        return index_json

    def __repr__(self) -> str:
        """
        Returns a representation of the IndexJson.
        """
        return "IndexJson()"
