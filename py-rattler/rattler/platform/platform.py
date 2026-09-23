from __future__ import annotations

from collections.abc import Iterator
from typing import Any, Dict, Optional, Tuple

from rattler._enum import StrEnum
from rattler.platform.arch import Arch
from rattler.rattler import PyPlatform


class PlatformName(StrEnum):
    """The set of platform names that can be used to construct a
    [`Platform`][rattler.platform.platform.Platform]."""

    NOARCH = "noarch"
    LINUX_32 = "linux-32"
    LINUX_64 = "linux-64"
    LINUX_AARCH64 = "linux-aarch64"
    LINUX_ARMV6L = "linux-armv6l"
    LINUX_ARMV7L = "linux-armv7l"
    LINUX_LOONGARCH64 = "linux-loongarch64"
    LINUX_PPC64LE = "linux-ppc64le"
    LINUX_PPC64 = "linux-ppc64"
    LINUX_PPC = "linux-ppc"
    LINUX_S390X = "linux-s390x"
    LINUX_RISCV32 = "linux-riscv32"
    LINUX_RISCV64 = "linux-riscv64"
    FREEBSD_32 = "freebsd-32"
    FREEBSD_64 = "freebsd-64"
    FREEBSD_ARM64 = "freebsd-arm64"
    OSX_64 = "osx-64"
    OSX_ARM64 = "osx-arm64"
    IOS_ARM64 = "ios-arm64"
    IOSSIMULATOR_ARM64 = "iossimulator-arm64"
    IOSSIMULATOR_64 = "iossimulator-64"
    ANDROID_AARCH64 = "android-aarch64"
    ANDROID_ARMV7A = "android-armv7a"
    ANDROID_64 = "android-64"
    ANDROID_32 = "android-32"
    WIN_32 = "win-32"
    WIN_64 = "win-64"
    WIN_ARM64 = "win-arm64"
    EMSCRIPTEN_WASM32 = "emscripten-wasm32"
    EMSCRIPTEN_WASM64 = "emscripten-wasm64"
    WASI_WASM32 = "wasi-wasm32"
    ZOS_Z = "zos-z"


# Compatibility alias for the former Literal type.
PlatformLiteral = PlatformName


class PlatformSingleton(type):
    _instances: Dict[str, Platform]

    def __init__(cls, *args: Tuple[Any], **kwargs: Dict[Any, Any]) -> None:
        cls._instances = {}

    def __call__(cls, platform: PlatformName, *args: Tuple[Any], **kwargs: Dict[Any, Any]) -> Platform:
        try:
            return cls._instances[platform]
        except KeyError:
            pass

        instance = super().__call__(platform, *args, **kwargs)
        cls._instances[platform] = instance
        return instance


class Platform(metaclass=PlatformSingleton):
    def __init__(self, value: PlatformName):
        self._inner = PyPlatform(value)

    @classmethod
    def _from_py_platform(cls, py_platform: PyPlatform) -> Platform:
        """Construct Rattler version from FFI PyArch object."""
        try:
            platform = cls._instances[py_platform.name]
        except KeyError:
            platform = cls.__new__(cls)
            platform._inner = py_platform
            cls._instances[str(platform)] = platform
        return platform

    def __str__(self) -> str:
        """
        Returns a string representation of the platform.

        Examples
        --------
        ```python
        >>> str(Platform(PlatformName.LINUX_64))
        'linux-64'
        >>>
        ```
        """
        return self._inner.name

    def __repr__(self) -> str:
        """
        Returns a representation of the platform.

        Examples
        --------
        ```python
        >>> Platform(PlatformName.LINUX_64)
        Platform(linux-64)
        >>>
        ```
        """
        return f"Platform({self._inner.name})"

    @classmethod
    def current(cls) -> Platform:
        """
        Returns the platform of the current host.

        Raises `RuntimeError` when the host is not a known conda platform.
        """
        return cls._from_py_platform(PyPlatform.current())

    @classmethod
    def all(cls) -> Iterator[Platform]:
        """
        Returns all supported platforms.

        Examples
        --------
        ```python
        >>> next(Platform.all())
        Platform(noarch)
        >>> len(list(Platform.all()))
        32
        >>>
        """
        return (cls._from_py_platform(p) for p in PyPlatform.all())

    @property
    def is_linux(self) -> bool:
        """
        Return True if the platform is linux.

        Examples
        --------
        ```python
        >>> Platform(PlatformName.LINUX_64).is_linux
        True
        >>> Platform(PlatformName.OSX_64).is_linux
        False
        >>>
        ```
        """
        return self._inner.is_linux

    @property
    def is_osx(self) -> bool:
        """
        Return True if the platform is osx.

        Examples
        --------
        ```python
        >>> Platform(PlatformName.OSX_64).is_osx
        True
        >>> Platform(PlatformName.LINUX_64).is_osx
        False
        >>>
        ```
        """
        return self._inner.is_osx

    @property
    def is_windows(self) -> bool:
        """
        Return True if the platform is win.

        Examples
        --------
        ```python
        >>> Platform(PlatformName.WIN_64).is_windows
        True
        >>> Platform(PlatformName.LINUX_64).is_windows
        False
        >>>
        ```
        """
        return self._inner.is_windows

    @property
    def is_unix(self) -> bool:
        """
        Return True if the platform is unix.

        Examples
        --------
        ```python
        >>> Platform(PlatformName.LINUX_64).is_unix
        True
        >>> Platform(PlatformName.WIN_64).is_unix
        False
        >>>
        ```
        """
        return self._inner.is_unix

    @property
    def arch(self) -> Optional[Arch]:
        """
        Return the architecture of the platform.

        Examples
        --------
        ```python
        >>> Platform(PlatformName.LINUX_64).arch
        Arch(x86_64)
        >>> Platform(PlatformName.LINUX_AARCH64).arch
        Arch(aarch64)
        >>>
        ```
        """
        arch = self._inner.arch()
        return Arch._from_py_arch(arch) if arch is not None else None

    @property
    def only_platform(self) -> Optional[str]:
        """
        Return the platform without the architecture.

        Examples
        --------
        ```python
        >>> Platform(PlatformName.LINUX_64).only_platform
        'linux'
        >>>
        ```
        """
        return self._inner.only_platform
