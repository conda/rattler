from __future__ import annotations
from rattler.package.package_name import PackageName

from rattler.rattler import PyRepoDataRecord
from rattler.repo_data import PackageRecord
from rattler.repo_data.package_record import PackageRecordProps
from rattler.version import Version


class RepoDataRecordProps(PackageRecordProps):
    def __init__(self, record: PyRepoDataRecord) -> None:
        self._repodata_record = record
        PackageRecordProps.__init__(self, self._repodata_record.package_record)

    @property
    def package_record(self) -> PackageRecord:
        """
        The data stored in the repodata.json.

        Examples
        --------
        ```python
        >>> from rattler import RepoData, Channel
        >>> repo_data = RepoData(
        ...     "../test-data/test-server/repo/noarch/repodata.json"
        ... )
        >>> record_list = repo_data.into_repo_data(Channel("test"))
        >>> record = record_list[0]
        >>> record.package_record
        PackageRecord("test-package=0.1=0")
        >>>
        ```
        """
        return PackageRecord._from_py_package_record(
            self._repodata_record.package_record
        )

    @property
    def url(self) -> str:
        """
        The canonical URL from where to get this package.

        Examples
        --------
        ```python
        >>> from rattler import RepoData, Channel
        >>> repo_data = RepoData(
        ...     "../test-data/test-server/repo/noarch/repodata.json"
        ... )
        >>> record_list = repo_data.into_repo_data(Channel("test"))
        >>> record = record_list[0]
        >>> record.url
        'https://conda.anaconda.org/test/noarch/test-package-0.1-0.tar.bz2'
        >>>
        ```
        """
        return self._repodata_record.url

    @property
    def channel(self) -> str:
        """
        String representation of the channel where the
        package comes from. This could be a URL but it
        could also be a channel name.

        Examples
        --------
        ```python
        >>> from rattler import RepoData, Channel
        >>> repo_data = RepoData(
        ...     "../test-data/test-server/repo/noarch/repodata.json"
        ... )
        >>> record_list = repo_data.into_repo_data(Channel("test"))
        >>> record = record_list[0]
        >>> record.channel
        'https://conda.anaconda.org/test/'
        >>>
        ```
        """
        return self._repodata_record.channel

    @property
    def file_name(self) -> str:
        """
        The filename of the package.

        Examples
        --------
        ```python
        >>> from rattler import RepoData, Channel
        >>> repo_data = RepoData(
        ...     "../test-data/test-server/repo/noarch/repodata.json"
        ... )
        >>> record_list = repo_data.into_repo_data(Channel("test"))
        >>> record = record_list[0]
        >>> record.file_name
        'test-package-0.1-0.tar.bz2'
        >>>
        ```
        """
        return self._repodata_record.file_name


class RepoDataRecord(RepoDataRecordProps):
    _record: PyRepoDataRecord

    def __init__(
        self,
        package_record: PackageRecord,
        file_name: str,
        url: str,
        channel: str,
    ) -> None:
        self._record = PyRepoDataRecord(
            package_record._package_record, file_name, url, channel
        )
        PackageRecordProps.__init__(self, self._record.package_record)

    @classmethod
    def _from_py_record(cls, py_record: PyRepoDataRecord) -> RepoDataRecord:
        """
        Construct Rattler RepoDataRecord from FFI PyRepoDataRecord object.
        """
        record = cls.__new__(cls)
        record._record = py_record
        RepoDataRecordProps.__init__(record, record._record.package_record)
        return record

    def __repr__(self) -> str:
        """
        Returns a representation of the RepoDataRecord.

        Examples
        --------
        ```python
        >>> from rattler import RepoData, Channel
        >>> repo_data = RepoData(
        ...     "../test-data/test-server/repo/noarch/repodata.json"
        ... )
        >>> repo_data.into_repo_data(Channel("test"))[0]
        RepoDataRecord(url="https://conda.anaconda.org/test/noarch/test-package-0.1-0.tar.bz2")
        >>>
        ```
        """
        return f'{type(self).__name__}(url="{self.url}")'
