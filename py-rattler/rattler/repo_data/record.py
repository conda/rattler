from __future__ import annotations

from rattler.rattler import PyRepoDataRecord
from rattler.repo_data import PackageRecord


class RepoDataRecord:
    _record: PyRepoDataRecord

    @property
    def package_record(self) -> PackageRecord:
        """
        The data stored in the repodata.json.
        """
        return PackageRecord._from_py_package_record(self._record.package_record)

    @property
    def url(self) -> str:
        """
        The canonical URL from where to get this package.
        """
        return self._record.url

    @property
    def channel(self) -> str:
        """
        String representation of the channel where the
        package comes from. This could be a URL but it
        could also be a channel name.
        """
        return self._record.channel

    @property
    def file_name(self) -> str:
        """
        The filename of the package.
        """
        return self._record.file_name

    @classmethod
    def _from_py_record(cls, py_record: PyRepoDataRecord) -> RepoDataRecord:
        """
        Construct Rattler RepoDataRecord from FFI PyRepoDataRecord object.
        """
        record = cls.__new__(cls)
        record._record = py_record
        return record

    def __repr__(self) -> str:
        """
        Returns a representation of the RepoDataRecord.
        """
        return f"{type(self).__name__}()"
