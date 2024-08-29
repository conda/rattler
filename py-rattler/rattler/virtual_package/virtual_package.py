from __future__ import annotations
from typing import List
import warnings

from rattler.rattler import PyVirtualPackage, PyOverride, PyVirtualPackageOverrides

from rattler.virtual_package.generic import GenericVirtualPackage

class Override:
    """
    Represents an override for a virtual package. 
    An override can be build using
    - `Override.default_env_var()` for overriding the detection with the default environment variable,
    - `Override.env_var(str)` for overriding the detection with a custom environment variable,
    - `Override.string(str)` for passing the version directly, or
    - `Override.none()` for disabling the override process all together.
    """

    _override: PyOverride

    @classmethod
    def _from_py_override(cls, py_override: PyOverride) -> Override:
        """Construct Rattler Override from FFI PyOverride object."""
        override = cls.__new__(cls)
        override._override = py_override
        return override

    @classmethod
    def default_env_var(cls) -> Override:
        """
        Returns a new instance to indicate that the default environment variable should overwrite the detected information from the host if specified.
        """
        return cls._from_py_override(PyOverride.default_env_var())

    @classmethod
    def env_var(cls, env_var: str) -> Override:
        """
        Returns the environment variable override for the given environment variable.
        """
        return cls._from_py_override(PyOverride.env_var(env_var))

    @classmethod
    def string(cls, override: str) -> Override:
        """
        Returns the override for the given string.
        """
        return cls._from_py_override(PyOverride.string(override))

    @classmethod
    def none(cls) -> Override:
        """
        Returns the override for None.
        """
        return cls._from_py_override(PyOverride.none())

    def __str__(self) -> str:
        """
        Returns string representation of the Override.
        """
        return self._override.as_str()

    def __repr__(self) -> str:
        """
        Returns a representation of the Override.
        """
        return f"Override({self._override.as_str()})"

    def __eq__(self, other: object) -> bool:
        """
        Returns True if the Overrides are equal, False otherwise.
        """
        if not isinstance(other, Override):
            return NotImplemented
        return self._override == other._override


class VirtualPackageOverrides:
    _overrides: PyVirtualPackageOverrides

    @classmethod
    def _from_py_virtual_package_overrides(
        cls, py_virtual_package_overrides: PyVirtualPackageOverrides
    ) -> VirtualPackageOverrides:
        """Construct Rattler VirtualPackageOverrides from FFI PyVirtualPackageOverrides object."""
        virtual_package_overrides = cls.__new__(cls)
        virtual_package_overrides._overrides = py_virtual_package_overrides
        return virtual_package_overrides

    
    def __init__(self) -> VirtualPackageOverrides:
        """
        Returns the default virtual package overrides.
        """
        self._overrides = PyVirtualPackageOverrides.default()

    @classmethod
    def none(cls) -> VirtualPackageOverrides:
        """
        Returns the virtual package overrides for None.
        """
        return cls._from_py_virtual_package_overrides(PyVirtualPackageOverrides.none())

    @property
    def osx(self) -> Override:
        """
        Returns the OSX override.
        """
        return Override._from_py_override(self._overrides.osx)

    @osx.setter
    def osx(self, override: Override) -> VirtualPackageOverrides:
        """
        Sets the OSX override.
        """
        self._overrides.osx = override._override
        return self._overrides.osx

    @property
    def libc(self) -> Override:
        """
        Returns the libc override.
        """
        return Override._from_py_override(self._overrides.libc)

    @libc.setter
    def libc(self, override: Override) -> VirtualPackageOverrides:
        """
        Sets the libc override.
        """
        self._overrides.libc = override._override
        return self._overrides.libc

    @property
    def cuda(self) -> Override:
        """
        Returns the CUDA override.
        """
        return Override._from_py_override(self._overrides.cuda)

    @cuda.setter
    def cuda(self, override: Override) -> VirtualPackageOverrides:
        """
        Sets the CUDA override.
        """
        self._overrides.cuda = override._override
        return self._overrides.cuda

    def __str__(self) -> str:
        """
        Returns string representation of the VirtualPackageOverrides.
        """
        return self._overrides.as_str()

    def __repr__(self) -> str:
        """
        Returns a representation of the VirtualPackageOverrides.
        """
        return f"VirtualPackageOverrides({self._overrides.as_str()})"


class VirtualPackage:
    _virtual_package: PyVirtualPackage

    @classmethod
    def _from_py_virtual_package(cls, py_virtual_package: PyVirtualPackage) -> VirtualPackage:
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
        warnings.warn("Use `detect` instead")
        return [VirtualPackage._from_py_virtual_package(vp) for vp in PyVirtualPackage.current()]

    @staticmethod
    def detect() -> List[VirtualPackage]:
        """
        Returns virtual packages detected for the current system or an error
        if the versions could not be properly detected.
        """
        return [VirtualPackage._from_py_virtual_package(vp) for vp in PyVirtualPackage.current()]

    @staticmethod
    def detect_with_overrides(overrides: VirtualPackageOverrides) -> List[VirtualPackage]:
        """
        Returns virtual packages detected for the current system with the given overrides.
        """
        return [
            VirtualPackage._from_py_virtual_package(vp)
            for vp in PyVirtualPackage.detect_with_overrides(overrides._overrides)
        ]

    def into_generic(self) -> GenericVirtualPackage:
        """
        Returns a GenericVirtualPackage from VirtualPackage.
        """
        # subclass from Generic instead.
        return GenericVirtualPackage._from_py_generic_virtual_package(self._virtual_package.as_generic())

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
