from __future__ import annotations
from typing import List, Optional
from rattler.package.package_name import PackageName

from rattler.rattler import PyRecord
from rattler.version.version import Version


class PackageRecord:
    """
    A single record in the Conda repodata. A single
    record refers to a single binary distribution
    of a package on a Conda channel.
    """

    _record: PyRecord

    @classmethod
    def _from_py_record(cls, py_record: PyRecord) -> PackageRecord:
        """
        Construct Rattler PackageRecord from FFI PyPackageRecord object.
        """
        record = cls.__new__(cls)
        record._record = py_record
        return record

    @property
    def arch(self) -> Optional[str]:
        return self._record.arch

    @property
    def build(self) -> str:
        return self._record.build

    @property
    def build_number(self) -> int:
        return self._record.build_number

    @property
    def constrains(self) -> List[str]:
        return self._record.constrains

    @property
    def depends(self) -> List[str]:
        return self._record.depends

    @property
    def features(self) -> Optional[str]:
        return self._record.features

    @property
    def legacy_bz2_md5(self) -> Optional[str]:
        return self._record.legacy_bz2_md5

    @property
    def legacy_bz2_size(self) -> Optional[int]:
        return self._record.legacy_bz2_size

    @property
    def license(self) -> Optional[str]:
        return self._record.license

    @property
    def license_family(self) -> Optional[str]:
        return self._record.license_family

    @property
    def md5(self) -> Optional[str]:
        return self._record.md5

    @property
    def name(self) -> PackageName:
        return PackageName._from_py_package_name(self._record.name)

    @property
    def platform(self) -> Optional[str]:
        return self._record.platform

    @property
    def sha256(self) -> Optional[str]:
        return self._record.sha256

    @property
    def size(self) -> Optional[int]:
        return self._record.size

    @property
    def subdir(self) -> str:
        return self._record.subdir

    @property
    def timestamp(self) -> Optional[int]:
        return self._record.timestamp

    @property
    def track_features(self) -> List[str]:
        return self._record.track_features

    @property
    def version(self) -> Version:
        return Version._from_py_version(self._record.version)

    def __str__(self) -> str:
        """
        Returns the string representation of the PackageRecord.

        Examples
        --------
        ```python
        >>> from rattler import RepoData, Channel
        >>> repo_data = RepoData(
        ...     "../test-data/test-server/repo/noarch/repodata.json"
        ... )
        >>> record_list = repo_data.into_repo_data(Channel("test"))
        >>> record = record_list[0]
        >>> package_record = record.package_record
        >>> str(package_record)
        'test-package=0.1=0'
        >>>
        ```
        """
        return self._record.as_package_record.as_str()

    def __repr__(self) -> str:
        """
        Returns a representation of the PackageRecord.

        Examples
        --------
        ```python
        >>> from rattler import RepoData, Channel
        >>> repo_data = RepoData(
        ...     "../test-data/test-server/repo/noarch/repodata.json"
        ... )
        >>> record_list = repo_data.into_repo_data(Channel("test"))
        >>> record = record_list[0]
        >>> package_record = record.package_record
        >>> package_record
        PackageRecord("test-package=0.1=0")
        >>>
        ```
        """
        return f'PackageRecord("{self.__str__()}")'
