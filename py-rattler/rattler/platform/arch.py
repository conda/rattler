from __future__ import annotations

from rattler._enum import StrEnum
from rattler.rattler import PyArch


class ArchName(StrEnum):
    X86 = "x86"
    X86_64 = "x86_64"
    AARCH64 = "aarch64"
    ARMV6L = "armv6l"
    ARMV7L = "armv7l"
    ARMV7A = "armv7a"
    LOONGARCH64 = "loongarch64"
    PPC64LE = "ppc64le"
    PPC64 = "ppc64"
    S390X = "s390x"
    RISCV32 = "riscv32"
    RISCV64 = "riscv64"


# Compatibility alias for the former Literal type.
ArchLiteral = ArchName


class Arch:
    def __init__(self, value: ArchName) -> None:
        self._inner = PyArch(value)

    @classmethod
    def _from_py_arch(cls, py_arch: PyArch) -> Arch:
        """Construct Rattler version from FFI PyArch object."""
        arch = cls.__new__(cls)
        arch._inner = py_arch
        return arch

    def __str__(self) -> str:
        """
        Returns a string representation of the architecture.

        Examples
        --------
        ```python
        >>> str(Arch(ArchName.X86_64))
        'x86_64'
        >>>
        ```
        """
        return self._inner.as_str()

    def __repr__(self) -> str:
        """
        Returns a representation of the architecture.

        Examples
        --------
        ```python
        >>> Arch(ArchName.AARCH64)
        Arch(aarch64)
        >>>
        ```
        """
        return f"Arch({self._inner.as_str()})"
