from __future__ import annotations
from typing import Self

from rattler.rattler import PyPackageName


class PackageName:
    def __init__(self, source: str) -> None:
        if not isinstance(source, str):
            raise TypeError(
                "PackageName constructor received unsupported type "
                f" {type(source).__name__!r} for the `source` parameter"
            )
        self._name = PyPackageName(source)

    @staticmethod
    def unchecked(normalized: str) -> PackageName:
        """
        Constructs a new `PackageName` from a string without checking if the string is actually a
        valid or normalized conda package name. This should only be used if you are sure that the
        input string is valid.

        Examples
        --------
        >>> p = PackageName.unchecked("test_xyz")
        """
        return PackageName._from_py_package_name(
            PyPackageName.new_unchecked(normalized)
        )

    @classmethod
    def _from_py_package_name(cls, py_package_name: PyPackageName) -> Self:
        """Construct Rattler PackageName from FFI PyPackageName object."""
        package_name = cls.__new__(cls)
        package_name._name = py_package_name
        return package_name

    @property
    def source(self) -> str:
        """
        Returns the source representation of the package name.
        This is the string from which this instance was created.

        Examples
        --------
        >>> p = PackageName("test_xyz")
        >>> p.source
        "test_xyz"
        """
        return self._name.source

    @property
    def normalized(self) -> str:
        """
        Returns the normalized version of the package name.
        The normalized string is guaranteed to be a valid conda package name.

        Examples
        --------
        >>> p = PackageName("test_xyz")
        >>> p.normalized
        "test_xyz"
        """
        return self._name.normalized

    def __repr__(self) -> str:
        return "PackageName()"
