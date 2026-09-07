from __future__ import annotations
import os
import datetime
from pathlib import Path
from typing import TYPE_CHECKING, Dict, List, Optional, Union

from rattler.package.no_arch_type import NoArchLiteral, NoArchType
from rattler.package.package_name import PackageName
from rattler.rattler import PyIndexJson
from rattler.version.version import Version
from rattler.version.with_source import VersionWithSource

if TYPE_CHECKING:
    from rattler.networking.client import Client


class IndexJson:
    _inner: PyIndexJson

    @staticmethod
    def from_path(path: os.PathLike[str]) -> IndexJson:
        """
        Parses an `index.json` file pointed to by the path.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json
        IndexJson()
        >>>
        ```
        """
        return IndexJson._from_py_index_json(PyIndexJson.from_path(path))

    @staticmethod
    def from_package_archive(path: os.PathLike[str]) -> IndexJson:
        """
        Parses the package file from archive.
        Note: If you want to extract multiple `info/*` files then this will be slightly
              slower than manually iterating over the archive entries with
              custom logic as this skips over the rest of the archive
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

    @classmethod
    async def from_remote_url(cls, client: Client, url: str) -> Optional[IndexJson]:
        """
        Fetches `info/index.json` from a remote package archive URL.
        """
        py_index_json = await PyIndexJson.from_remote_url(client._client, url)
        if py_index_json is None:
            return None
        return cls._from_py_index_json(py_index_json)

    @staticmethod
    def package_path() -> Path:
        """
        Returns the path to the file within the Conda archive.

        The path is relative to the root of the archive and includes any necessary
        directories.
        """
        return PyIndexJson.package_path()

    @property
    def version(self) -> Version:
        """
        The version of the package.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.version
        VersionWithSource(version="22.11.1", source="22.11.1")
        >>> idx_json.version = VersionWithSource("1.02.3")
        >>> idx_json.version
        VersionWithSource(version="1.2.3", source="1.02.3")
        >>>
        ```
        """
        version, source = self._inner.version
        return VersionWithSource._from_py_version(version, source)

    @version.setter
    def version(self, value: VersionWithSource) -> None:
        self._inner.version = (value._version, value._source)

    @property
    def arch(self) -> Optional[str]:
        """
        Optionally, the architecture the package is build for.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.arch
        'x86_64'
        >>>
        ```
        """
        if arch := self._inner.arch:
            return arch

        return None

    @arch.setter
    def arch(self, value: Optional[str]) -> None:
        self._inner.arch = value

    @property
    def build(self) -> str:
        """
        The build string of the package.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.build
        'py38haa244fe_1'
        >>>
        ```
        """
        return self._inner.build

    @build.setter
    def build(self, value: str) -> None:
        self._inner.build = value

    @property
    def build_number(self) -> int:
        """
        The build number of the package.
        This is also included in the build string.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.build_number
        1
        >>>
        ```
        """
        return self._inner.build_number

    @build_number.setter
    def build_number(self, value: int) -> None:
        self._inner.build_number = value

    @property
    def constrains(self) -> List[str]:
        """
        The package constraints of the package.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.constrains # doctest: +ELLIPSIS
        ['conda-content-trust >=0.1.1', ..., 'conda-env >=2.6', 'conda-build >=3']
        >>>
        ```
        """
        return self._inner.constrains

    @constrains.setter
    def constrains(self, value: List[str]) -> None:
        self._inner.constrains = value

    @property
    def depends(self) -> List[str]:
        """
        The dependencies of the package.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.depends # doctest: +ELLIPSIS
        ['conda-package-handling >=1.3.0', ..., 'tqdm >=4']
        >>>
        ```
        """
        return self._inner.depends

    @depends.setter
    def depends(self, value: List[str]) -> None:
        self._inner.set_depends(value)

    @property
    def extra_depends(self) -> Dict[str, List[str]]:
        """
        Extra dependency groups that can be selected using `foobar[extras=["scientific"]]`.
        The implementation is specified in this CEP: <https://github.com/conda/ceps/pull/111>

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.extra_depends
        {}
        >>> idx_json.extra_depends = {"security": ["cryptography >=3.0"]}
        >>> idx_json.extra_depends
        {'security': ['cryptography >=3.0']}
        >>>
        ```
        """
        return self._inner.extra_depends

    @extra_depends.setter
    def extra_depends(self, value: Dict[str, List[str]]) -> None:
        self._inner.extra_depends = value

    @property
    def features(self) -> Optional[str]:
        """
        Features are a deprecated way to specify different feature sets for the conda solver. This is not
        supported anymore and should not be used. Instead, `mutex` packages should be used to specify
        mutually exclusive features.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.features
        >>>
        ```
        """
        if features := self._inner.features:
            return features

        return None

    @features.setter
    def features(self, value: Optional[str]) -> None:
        self._inner.set_features(value)

    @property
    def flags(self) -> List[str]:
        """
        Plain string flags used to select package variants.

        Rattler preserves unrecognized flags so applications can round-trip
        flags introduced by newer repodata revisions.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.flags
        []
        >>> idx_json.flags = ["optional", "future-flag"]
        >>> idx_json.flags
        ['optional', 'future-flag']
        >>>
        ```
        """
        return self._inner.flags

    @flags.setter
    def flags(self, value: List[str]) -> None:
        self._inner.flags = value

    @property
    def license(self) -> Optional[str]:
        """
        Optionally, the license.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.license
        'BSD-3-Clause'
        >>>
        ```
        """
        if license := self._inner.license:
            return license

        return None

    @license.setter
    def license(self, value: Optional[str]) -> None:
        self._inner.set_license(value)

    @property
    def license_family(self) -> Optional[str]:
        """
        Optionally, the license.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.license_family
        >>>
        ```
        """
        if license_family := self._inner.license_family:
            return license_family

        return None

    @license_family.setter
    def license_family(self, value: Optional[str]) -> None:
        self._inner.set_license_family(value)

    @property
    def name(self) -> PackageName:
        """
        The lowercase name of the package.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.name
        PackageName("conda")
        >>> idx_json.name = PackageName("rattler")
        >>> idx_json.name
        PackageName("rattler")
        >>>
        ```
        """
        return PackageName._from_py_package_name(self._inner.name)

    @name.setter
    def name(self, value: PackageName) -> None:
        self._inner.name = value._name

    @property
    def noarch(self) -> NoArchType:
        """
        If this package is independent of architecture this field specifies in what way.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.noarch
        NoArchType(None)
        >>> idx_json.noarch = NoArchType("python")
        >>> idx_json.noarch
        NoArchType("python")
        >>> idx_json.noarch = "generic"
        >>> idx_json.noarch
        NoArchType("generic")
        >>> idx_json.noarch = None
        >>> idx_json.noarch.none
        True
        >>>
        ```
        """
        return NoArchType._from_py_no_arch_type(self._inner.noarch)

    @noarch.setter
    def noarch(self, value: NoArchType | NoArchLiteral) -> None:
        if not isinstance(value, NoArchType):
            value = NoArchType(value)
        self._inner.noarch = value._noarch

    @property
    def platform(self) -> Optional[str]:
        """
        Optionally, the OS the package is build for.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.platform
        'win'
        >>>
        ```
        """
        if platform := self._inner.platform:
            return platform

        return None

    @platform.setter
    def platform(self, value: Optional[str]) -> None:
        self._inner.set_platform(value)

    @property
    def purls(self) -> Optional[List[str]]:
        """
        A list of Package URLs identifying this package.
        See this CEP: <https://github.com/conda/ceps/pull/63>

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.purls is None
        True
        >>> idx_json.purls = ["pkg:pypi/conda@22.11.1"]
        >>> idx_json.purls
        ['pkg:pypi/conda@22.11.1']
        >>> idx_json.purls = None
        >>> idx_json.purls is None
        True
        >>>
        ```
        """
        return self._inner.purls

    @purls.setter
    def purls(self, value: Optional[List[str]]) -> None:
        self._inner.purls = value

    @property
    def python_site_packages_path(self) -> Optional[str]:
        """
        Optionally a path within the environment of the site-packages directory. This field is only
        present for python interpreter packages.
        This field was introduced with <https://github.com/conda/ceps/blob/main/cep-17.md>.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.python_site_packages_path is None
        True
        >>> idx_json.python_site_packages_path = "lib/python3.11/site-packages"
        >>> idx_json.python_site_packages_path
        'lib/python3.11/site-packages'
        >>>
        ```
        """
        return self._inner.python_site_packages_path

    @python_site_packages_path.setter
    def python_site_packages_path(self, value: Optional[str]) -> None:
        self._inner.python_site_packages_path = value

    @property
    def repodata_revision(self) -> Optional[str]:
        """
        The repodata revision required by this package, formatted as `vN` (e.g. `"v3"`).

        Indexers use this field to decide whether the record can be written to the legacy
        `packages` / `packages.conda` maps or must be written to a newer top-level `vN` map.
        The setter accepts either a `vN` string or the integer revision number.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.repodata_revision is None
        True
        >>> idx_json.repodata_revision = "v3"
        >>> idx_json.repodata_revision
        'v3'
        >>> idx_json.repodata_revision = 3
        >>> idx_json.repodata_revision
        'v3'
        >>> idx_json.repodata_revision = None
        >>> idx_json.repodata_revision is None
        True
        >>>
        ```
        """
        return self._inner.repodata_revision

    @repodata_revision.setter
    def repodata_revision(self, value: Optional[Union[str, int]]) -> None:
        self._inner.repodata_revision = None if value is None else str(value)

    @property
    def subdir(self) -> Optional[str]:
        """
        The subdirectory that contains this package.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.subdir
        'win-64'
        >>>
        ```
        """
        if subdir := self._inner.subdir:
            return subdir

        return None

    @subdir.setter
    def subdir(self, value: Optional[str]) -> None:
        self._inner.set_subdir(value)

    @property
    def timestamp(self) -> Optional[datetime.datetime]:
        """
        The timestamp when this package was created

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.timestamp
        datetime.datetime(2022, 12, 7, 23, 45, 42, 50000, tzinfo=datetime.timezone.utc)
        >>> idx_json.timestamp = datetime.datetime(2021, 1, 1, 1, 1, 1, 50000, tzinfo=datetime.timezone.utc)
        >>> idx_json.timestamp
        datetime.datetime(2021, 1, 1, 1, 1, 1, 50000, tzinfo=datetime.timezone.utc)
        >>>
        ```
        """
        if timestamp := self._inner.timestamp:
            return datetime.datetime.fromtimestamp(timestamp / 1000.0, tz=datetime.timezone.utc)

        return None

    @timestamp.setter
    def timestamp(self, value: Optional[datetime.datetime]) -> None:
        if value is None:
            self._inner.timestamp = None
        else:
            # convert to integer milliseconds
            self._inner.timestamp = int(value.timestamp() * 1000.0)

    @property
    def track_features(self) -> List[str]:
        """
        Track features are nowadays only used to downweight packages (ie. give them less priority). To
        that effect, the number of track features is counted (number of commas) and the package is downweighted
        by the number of track_features.

        Examples
        --------
        ```python
        >>> idx_json = IndexJson.from_path(
        ...     "../test-data/conda-22.11.1-py38haa244fe_1-index.json"
        ... )
        >>> idx_json.track_features
        []
        >>>
        ```
        """
        return self._inner.track_features

    @track_features.setter
    def track_features(self, value: List[str]) -> None:
        self._inner.set_track_features(value)

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
