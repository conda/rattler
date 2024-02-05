from __future__ import annotations
from rattler.rattler import PyPackageHashes


class PackageHashes:
    _hashes: PyPackageHashes

    @classmethod
    def _from_py_package_hashes(cls, pkg_hashes: PyPackageHashes) -> PackageHashes:
        """
        Construct Rattler PackageHashes from FFI PyPackageHashes object.
        """
        hashes = cls.__new__(cls)
        hashes._hashes = pkg_hashes
        return hashes
