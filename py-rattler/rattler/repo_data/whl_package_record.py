from __future__ import annotations

from rattler.rattler import PyWhlPackageRecord
from rattler.repo_data.package_record import PackageRecord


class WhlPackageRecord:
    """
    A wheel package record pairing a PackageRecord with its URL or path.

    Used to build repodata from PyPI/wheel metadata without conda archives.
    """

    def __init__(self, package_record: PackageRecord, url: str) -> None:
        self._inner = PyWhlPackageRecord(package_record._record, url)

    @property
    def package_record(self) -> PackageRecord:
        return PackageRecord._from_py_record(self._inner.package_record)

    @property
    def url(self) -> str:
        return self._inner.url

    def __repr__(self) -> str:
        """Returns a representation of the WhlPackageRecord."""
        return f'WhlPackageRecord(url="{self.url}")'
