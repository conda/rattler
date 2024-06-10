from __future__ import annotations
import os
from pathlib import Path
from typing import List
from rattler.channel.channel import Channel
from rattler.package.package_name import PackageName

from rattler.rattler import PySparseRepoData
from rattler.repo_data.record import RepoDataRecord


class SparseRepoData:
    """
    A class to enable loading records from a `repodata.json` file on demand.
    Since most of the time you don't need all the records from the `repodata.json`
    this can help provide some significant speedups.
    """

    def __init__(
        self,
        channel: Channel,
        subdir: str,
        path: os.PathLike[str] | str,
    ) -> None:
        if not isinstance(channel, Channel):
            raise TypeError(
                "SparseRepoData constructor received unsupported type "
                f" {type(channel).__name__!r} for the `channel` parameter"
            )
        if not isinstance(subdir, str):
            raise TypeError(
                "SparseRepoData constructor received unsupported type "
                f" {type(subdir).__name__!r} for the `subdir` parameter"
            )
        if not isinstance(path, (str, Path)):
            raise TypeError(
                "SparseRepoData constructor received unsupported type "
                f" {type(path).__name__!r} for the `path` parameter"
            )
        self._sparse = PySparseRepoData(channel._channel, subdir, str(path))

    def package_names(self) -> List[str]:
        """
        Returns a list over all package names in this repodata file.
        This works by iterating over all elements in the `packages` and
        `conda_packages` fields of the repodata and returning the unique
        package names.

        Examples
        --------
        ```python
        >>> from rattler import Channel, ChannelConfig
        >>> channel = Channel("dummy", ChannelConfig())
        >>> subdir = "test-data/dummy/noarch"
        >>> path = "../test-data/channels/dummy/linux-64/repodata.json"
        >>> sparse_data = SparseRepoData(channel, subdir, path)
        >>> package_names = sparse_data.package_names()
        >>> package_names
        [...]
        >>> isinstance(package_names[0], str)
        True
        >>>
        ```
        """
        return self._sparse.package_names()

    def load_records(self, package_name: PackageName) -> List[RepoDataRecord]:
        """
        Returns all the records for the specified package name.

        Examples
        --------
        ```python
        >>> from rattler import Channel, ChannelConfig, RepoDataRecord, PackageName
        >>> channel = Channel("dummy", ChannelConfig())
        >>> subdir = "test-data/dummy/noarch"
        >>> path = "../test-data/channels/dummy/linux-64/repodata.json"
        >>> sparse_data = SparseRepoData(channel, subdir, path)
        >>> package_name = PackageName(sparse_data.package_names()[0])
        >>> records = sparse_data.load_records(package_name)
        >>> records
        [...]
        >>> isinstance(records[0], RepoDataRecord)
        True
        >>>
        ```
        """
        # maybe change package_name to Union[str, PackageName]
        return [RepoDataRecord._from_py_record(record) for record in self._sparse.load_records(package_name._name)]

    @property
    def subdir(self) -> str:
        """
        Returns the subdirectory from which this repodata was loaded.

        Examples
        --------
        ```python
        >>> from rattler import Channel, ChannelConfig
        >>> channel = Channel("dummy", ChannelConfig())
        >>> subdir = "test-data/dummy/noarch"
        >>> path = "../test-data/channels/dummy/linux-64/repodata.json"
        >>> sparse_data = SparseRepoData(channel, subdir, path)
        >>> sparse_data.subdir
        'test-data/dummy/noarch'
        >>>
        ```
        """
        return self._sparse.subdir

    @staticmethod
    def load_records_recursive(
        repo_data: List[SparseRepoData],
        package_names: List[PackageName],
    ) -> List[List[RepoDataRecord]]:
        """
        Given a set of [`SparseRepoData`]s load all the records
        for the packages with the specified names and all the packages
        these records depend on. This will parse the records for the
        specified packages as well as all the packages these records
        depend on.

        Examples
        --------
        ```python
        >>> from rattler import Channel, ChannelConfig, PackageName
        >>> channel = Channel("dummy")
        >>> subdir = "test-data/dummy/linux-64"
        >>> path = "../test-data/channels/dummy/linux-64/repodata.json"
        >>> sparse_data = SparseRepoData(channel, subdir, path)
        >>> package_name = PackageName("python")
        >>> SparseRepoData.load_records_recursive([sparse_data], [package_name])
        [...]
        >>>
        ```
        """
        return [
            [RepoDataRecord._from_py_record(record) for record in list_of_records]
            for list_of_records in PySparseRepoData.load_records_recursive(
                [r._sparse for r in repo_data],
                [p._name for p in package_names],
            )
        ]

    @classmethod
    def _from_py_sparse_repo_data(cls, py_sparse_repo_data: PySparseRepoData) -> SparseRepoData:
        """
        Construct Rattler SparseRepoData from FFI PySparseRepoData object.
        """
        sparse_repo_data = cls.__new__(cls)
        sparse_repo_data._sparse = py_sparse_repo_data
        return sparse_repo_data

    def __repr__(self) -> str:
        """
        Returns a representation of the SparseRepoData.

        Examples
        --------
        ```python
        >>> from rattler import Channel, ChannelConfig
        >>> channel = Channel("dummy", ChannelConfig())
        >>> subdir = "test-data/dummy/noarch"
        >>> path = "../test-data/channels/dummy/linux-64/repodata.json"
        >>> sparse_data = SparseRepoData(channel, subdir, path)
        >>> sparse_data
        SparseRepoData(subdir="test-data/dummy/noarch")
        >>>
        ```
        """
        return f'SparseRepoData(subdir="{self.subdir}")'
