from __future__ import annotations

from rattler.rattler import PyVirtualPackage
from typing import List, Self


class VirtualPackage:
    _virtual_package: PyVirtualPackage

    @classmethod
    def _from_py_virtual_package(cls, py_virtual_package: PyVirtualPackage) -> Self:
        """Construct Rattler VirtualPackage from FFI PyVirtualPackage object."""
        virtual_package = cls.__new__(cls)
        virtual_package._virtual_package = py_virtual_package
        return virtual_package

    @staticmethod
    def current() -> List[VirtualPackage]:
        """
        Returns virtual packages detected for the current system or an error
        if the versions could not be properly detected.
        """
        return [
            VirtualPackage._from_py_virtual_package(vp)
            for vp in PyVirtualPackage.current()
        ]
