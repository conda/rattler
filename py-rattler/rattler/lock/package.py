from __future__ import annotations
from typing import Self

from rattler.rattler import PyLockPackage


class LockPackage:
    _package: PyLockPackage

    @classmethod
    def _from_py_lock_package(cls, lock_pkg: PyLockPackage) -> Self:
        """
        Construct Rattler LockPackage from FFI PyLockPackage object.
        """
        pkg = cls.__new__(cls)
        pkg._package = lock_pkg
        return pkg
