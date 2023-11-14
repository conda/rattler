from __future__ import annotations
import os
from typing import List, Optional

from rattler.rattler import PyPackageRecord

from rattler.version import Version
from rattler.package import PackageName


class PackageRecordProps:
    def __init__(self, package_record: PyPackageRecord) -> None:
        self._package_record = package_record

    @property
    def arch(self) -> Optional[str]:
        return self._package_record.arch

    @property
    def build(self) -> str:
        return self._package_record.build

    @property
    def build_number(self) -> int:
        return self._package_record.build_number

    @property
    def constrains(self) -> List[str]:
        return self._package_record.constrains

    @property
    def depends(self) -> List[str]:
        return self._package_record.depends

    @property
    def features(self) -> Optional[str]:
        return self._package_record.features

    @property
    def legacy_bz2_md5(self) -> Optional[str]:
        return self._package_record.legacy_bz2_md5

    @property
    def legacy_bz2_size(self) -> Optional[int]:
        return self._package_record.legacy_bz2_size

    @property
    def license(self) -> Optional[str]:
        return self._package_record.license

    @property
    def license_family(self) -> Optional[str]:
        return self._package_record.license_family

    @property
    def md5(self) -> Optional[str]:
        return self._package_record.md5

    @property
    def name(self) -> PackageName:
        return PackageName._from_py_package_name(self._package_record.name)

    @property
    def platform(self) -> Optional[str]:
        return self._package_record.platform

    @property
    def sha256(self) -> Optional[str]:
        return self._package_record.sha256

    @property
    def size(self) -> Optional[int]:
        return self._package_record.size

    @property
    def subdir(self) -> str:
        return self._package_record.subdir

    @property
    def timestamp(self) -> Optional[int]:
        return self._package_record.timestamp

    @property
    def track_features(self) -> List[str]:
        return self._package_record.track_features

    @property
    def timestamp(self) -> Version:
        return Version._from_py_version(self._package_record.version)


class PackageRecord(PackageRecordProps):
    """
    A single record in the Conda repodata. A single
    record refers to a single binary distribution
    of a package on a Conda channel.
    """

    def __init__(self, name: PackageName, version: Version, build: str) -> None:
        self._package_record = PyPackageRecord(name._name, version._version, build)
        super().__init__(self._package_record)

    @staticmethod
    def from_index_json(path: os.PathLike[str]) -> PackageRecord:
        return PackageRecord._from_py_package_record(
            PyPackageRecord.from_index_json(path)
        )

    @staticmethod
    def sort_topologically(records: List[PackageRecord]) -> List[PackageRecord]:
        # TODO: accept `PyObject` in FFI lib
        return [
            PackageRecord._from_py_package_record(p)
            for p in PyPackageRecord.sort_topologically(records)
        ]

    @classmethod
    def _from_py_package_record(
        cls, py_package_record: PyPackageRecord
    ) -> PackageRecord:
        """
        Construct Rattler PackageRecord from FFI PyPackageRecord object.
        """
        package_record = cls.__new__(cls)
        package_record._package_record = py_package_record
        PackageRecordProps.__init__(
            package_record,
            package_record._package_record,
        )
        return package_record

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
        return self._package_record.as_str()

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
