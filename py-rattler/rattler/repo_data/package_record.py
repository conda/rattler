from __future__ import annotations

from rattler.rattler import PyPackageRecord


class PackageRecord:
    def __init__(self):
        self._package_record = PyPackageRecord()

    def __str__(self) -> str:
        return self._package_record.as_str()

    def __repr__(self) -> str:
        return self.__str__()
