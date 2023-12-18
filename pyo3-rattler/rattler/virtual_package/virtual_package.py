from __future__ import annotations

from rattler.rattler import PyVirtualPackage
from typing import List

from rattler.virtual_package.generic import GenericVirtualPackage


class VirtualPackage:
    _virtual_package: PyVirtualPackage

    @classmethod
    def _from_py_virtual_package(
        cls, py_virtual_package: PyVirtualPackage
    ) -> VirtualPackage:
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

    def into_generic(self) -> GenericVirtualPackage:
        """
        Returns a GenericVirtualPackage from VirtualPackage.
        """
        # subclass from Generic instead.
        return GenericVirtualPackage._from_py_generic_virtual_package(
            self._virtual_package.as_generic()
        )

    def __str__(self) -> str:
        """
        Returns string representation of the VirtualPackage.
        """
        return self._virtual_package.as_str()

    def __repr__(self) -> str:
        """
        Returns a representation of the VirtualPackage.
        """
        return f"VirtualPackage({self._virtual_package.as_str()})"
