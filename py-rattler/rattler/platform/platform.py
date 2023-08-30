from __future__ import annotations
from typing import Literal

from rattler.rattler import PyPlatform, PyArch


ArchLiteral = Literal[
    "x86",
    "x86_64",
    "aarch64",
    "armv6l",
    "armv7l",
    "ppc64le",
    "ppc64",
    "s390x",
    "riscv32",
    "riscv64",
]


class Arch:
    def __init__(self, value: ArchLiteral):
        self._inner = PyArch(value)

    @classmethod
    def _from_py_arch(cls, py_arch: PyArch) -> Arch:
        """Construct Rattler version from FFI PyArch object."""
        arch = cls.__new__(cls)
        arch._inner = py_arch
        return arch

    def __str__(self):
        """
        Returns a string representation of the architecture.

        >>> str(Arch("x86_64"))
        'x86_64'
        """
        return self._inner.as_str()

    def __repr__(self) -> str:
        """
        Returns a representation of the architecture.

        >>> Arch("aarch64")
        Arch(aarch64)
        """
        return f"Arch({self._inner.as_str()})"


PlatformLiteral = Literal[
    "noarch",
    "linux-32",
    "linux-64",
    "linux-aarch64",
    "linux-armv6l",
    "linux-armv7l",
    "linux-ppc64le",
    "linux-ppc64",
    "linux-s390x",
    "linux-riscv32",
    "linux-riscv64",
    "osx-64",
    "osx-arm64",
    "win-32",
    "win-64",
    "win-arm64",
    "emscripten-32",
]


class Platform:
    def __init__(self, value: PlatformLiteral):
        self._inner = PyPlatform(value)

    @classmethod
    def _from_py_platform(cls, py_platform: PyPlatform) -> Platform:
        """Construct Rattler version from FFI PyArch object."""
        platform = cls.__new__(cls)
        platform._inner = py_platform
        return platform

    def __str__(self):
        """
        Returns a string representation of the platform.

        >>> str(Platform("linux-64"))
        'linux-64'
        """
        return self._inner.name

    def __repr__(self) -> str:
        """
        Returnrs a representation of the platform.

        >>> Platform("linux-64")
        Platform(linux-64)
        """
        return f"Platform({self._inner.name})"

    def current() -> Platform:
        """
        Returns the current platform.

        # >>> Platform.current()
        # Platform(linux-64)
        """
        return Platform._from_py_platform(PyPlatform.current())

    @property
    def is_linux(self):
        """
        Return True if the platform is linux.

        >>> Platform("linux-64").is_linux
        True
        >>> Platform("osx-64").is_linux
        False
        """
        return self._inner.is_linux

    @property
    def is_osx(self):
        """
        Return True if the platform is osx.

        >>> Platform("osx-64").is_osx
        True
        >>> Platform("linux-64").is_osx
        False
        """
        return self._inner.is_osx

    @property
    def is_windows(self):
        """
        Return True if the platform is win.

        >>> Platform("win-64").is_windows
        True
        >>> Platform("linux-64").is_windows
        False
        """
        return self._inner.is_windows

    @property
    def is_unix(self):
        """
        Return True if the platform is unix.

        >>> Platform("linux-64").is_unix
        True
        >>> Platform("win-64").is_unix
        False
        """
        return self._inner.is_unix

    @property
    def arch(self):
        """
        Return the architecture of the platform.

        >>> Platform("linux-64").arch
        Arch(x86_64)
        >>> Platform("linux-aarch64").arch
        Arch(aarch64)
        """
        return Arch._from_py_arch(self._inner.arch())
