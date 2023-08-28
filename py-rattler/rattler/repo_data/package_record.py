from __future__ import annotations

from rattler.rattler import PyPackageRecord


class PackageRecord:
    """
    A single record in the Conda repodata. A single
    record refers to a single binary distribution
    of a package on a Conda channel.
    """

    def __init__(self):
        self._package_record = PyPackageRecord()

    def __str__(self) -> str:
        return self._package_record.as_str()

    def __repr__(self) -> str:
        return self.__str__()
