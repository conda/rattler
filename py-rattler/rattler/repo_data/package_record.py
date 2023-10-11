from __future__ import annotations

from rattler.rattler import PyPackageRecord


class PackageRecord:
    """
    A single record in the Conda repodata. A single
    record refers to a single binary distribution
    of a package on a Conda channel.
    """

    def __init__(self) -> None:
        self._package_record = PyPackageRecord()

    @classmethod
    def _from_py_package_record(
        cls, py_package_record: PyPackageRecord
    ) -> PackageRecord:
        """
        Construct Rattler PackageRecord from FFI PyPackageRecord object.
        """
        package_record = cls.__new__(cls)
        package_record._package_record = py_package_record
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
