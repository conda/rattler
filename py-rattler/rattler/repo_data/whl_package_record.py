from __future__ import annotations

from rattler.rattler import PyWhlPackageRecord
from rattler.repo_data.package_record import PackageRecord


class WhlPackageRecord(PackageRecord):
    """
    A wheel package record pairing a PackageRecord with its URL or path.
    WhlPackageRecord extends PackageRecord and adds a url property.

    Used to build repodata from PyPI/wheel metadata without conda archives.
    """

    def __init__(self, package_record: PackageRecord, url: str) -> None:
        inner = PyWhlPackageRecord(package_record._record, url)
        self._record = inner.package_record
        self._whl_inner = inner

    @property
    def package_record(self) -> PackageRecord:
        return PackageRecord._from_py_record(self._record)

    @property
    def url(self) -> str:
        return self._whl_inner.url

    def __repr__(self) -> str:
        """Returns a representation of the WhlPackageRecord."""
        return f'WhlPackageRecord(url="{self.url}")'
