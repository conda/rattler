from __future__ import annotations
from collections.abc import Iterator
from typing import Any, Dict, Literal, Tuple, Optional

from rattler.rattler import PySubdir
from rattler.platform.arch import Arch

SubdirLiteral = Literal[
    "noarch",
    "linux-32",
    "linux-64",
    "linux-aarch64",
    "linux-armv6l",
    "linux-armv7l",
    "linux-loongarch64",
    "linux-ppc64le",
    "linux-ppc64",
    "linux-ppc",
    "linux-s390x",
    "linux-riscv32",
    "linux-riscv64",
    "freebsd-32",
    "freebsd-64",
    "freebsd-arm64",
    "osx-64",
    "osx-arm64",
    "ios-arm64",
    "iossimulator-arm64",
    "iossimulator-64",
    "android-aarch64",
    "android-armv7a",
    "android-64",
    "android-32",
    "win-32",
    "win-64",
    "win-arm64",
    "emscripten-wasm32",
    "emscripten-wasm64",
    "wasi-wasm32",
    "zos-z",
]


class SubdirSingleton(type):
    _instances: Dict[str, Subdir]

    def __init__(cls, *args: Tuple[Any], **kwargs: Dict[Any, Any]) -> None:
        cls._instances = {}

    def __call__(cls, subdir: str, *args: Tuple[Any], **kwargs: Dict[Any, Any]) -> Subdir:
        try:
            return cls._instances[subdir]
        except KeyError:
            pass

        instance = super().__call__(subdir, *args, **kwargs)
        cls._instances[subdir] = instance
        return instance


class Subdir(metaclass=SubdirSingleton):
    def __init__(self, value: SubdirLiteral | str):
        self._inner = PySubdir(value)

    @classmethod
    def _from_py_subdir(cls, py_subdir: PySubdir) -> Subdir:
        """Construct a Subdir from an FFI PySubdir object."""
        try:
            subdir = cls._instances[py_subdir.name]
        except KeyError:
            subdir = cls.__new__(cls)
            subdir._inner = py_subdir
            cls._instances[str(subdir)] = subdir
        return subdir

    def __str__(self) -> str:
        """
        Returns a string representation of the subdir.

        Examples
        --------
        ```python
        >>> str(Subdir("linux-64"))
        'linux-64'
        >>>
        ```
        """
        return self._inner.name

    def __repr__(self) -> str:
        """
        Returns a representation of the subdir.

        Examples
        --------
        ```python
        >>> Subdir("linux-64")
        Subdir(linux-64)
        >>>
        ```
        """
        return f"Subdir({self._inner.name})"

    @classmethod
    def current(cls) -> Subdir:
        """
        Returns the subdir of the current host.

        Raises `RuntimeError` when the host is not a known conda platform.
        """
        return cls._from_py_subdir(PySubdir.current())

    @classmethod
    def all(cls) -> Iterator[Subdir]:
        """
        Returns all known subdirs.

        Examples
        --------
        ```python
        >>> next(Subdir.all())
        Subdir(noarch)
        >>> len(list(Subdir.all()))
        32
        >>>
        """
        return (cls._from_py_subdir(p) for p in PySubdir.all())

    @property
    def is_linux(self) -> bool:
        """
        Return True if the subdir is linux.

        Examples
        --------
        ```python
        >>> Subdir("linux-64").is_linux
        True
        >>> Subdir("osx-64").is_linux
        False
        >>>
        ```
        """
        return self._inner.is_linux

    @property
    def is_osx(self) -> bool:
        """
        Return True if the subdir is osx.

        Examples
        --------
        ```python
        >>> Subdir("osx-64").is_osx
        True
        >>> Subdir("linux-64").is_osx
        False
        >>>
        ```
        """
        return self._inner.is_osx

    @property
    def is_windows(self) -> bool:
        """
        Return True if the subdir is win.

        Examples
        --------
        ```python
        >>> Subdir("win-64").is_windows
        True
        >>> Subdir("linux-64").is_windows
        False
        >>>
        ```
        """
        return self._inner.is_windows

    @property
    def is_unix(self) -> bool:
        """
        Return True if the subdir is unix.

        Examples
        --------
        ```python
        >>> Subdir("linux-64").is_unix
        True
        >>> Subdir("win-64").is_unix
        False
        >>>
        ```
        """
        return self._inner.is_unix

    @property
    def arch(self) -> Optional[Arch]:
        """
        Return the architecture of the subdir.

        Examples
        --------
        ```python
        >>> Subdir("linux-64").arch
        Arch(x86_64)
        >>> Subdir("linux-aarch64").arch
        Arch(aarch64)
        >>>
        ```
        """
        arch = self._inner.arch()
        return Arch._from_py_arch(arch) if arch is not None else None

    @property
    def only_platform(self) -> Optional[str]:
        """
        Return the platform (os) part of the subdir.

        Examples
        --------
        ```python
        >>> Subdir("linux-64").only_platform
        'linux'
        >>>
        ```
        """
        return self._inner.only_platform


#: Alias for [Subdir][rattler.platform.subdir.Subdir], kept for backwards compatibility.
Platform = Subdir

#: Alias for [SubdirLiteral][rattler.platform.subdir.SubdirLiteral], kept for backwards compatibility.
PlatformLiteral = SubdirLiteral
