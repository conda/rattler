from __future__ import annotations

import os
from typing import TYPE_CHECKING, Any

from rattler.rattler import (
    PyArchiveIdentifier,
    PyCondaArchiveIdentifier,
    PyCondaArchiveType,
)

if TYPE_CHECKING:
    from pathlib import Path


class ArchiveIdentifier:
    """
    An archive identifier consisting of a package `name`, `version`, and `build_string`.

    This uniquely identifies a package build.

    Examples
    --------
    ```python
    >>> ident = ArchiveIdentifier("python", "3.11.0", "h12345_0")
    >>> ident.name
    'python'
    >>> ident.version
    '3.11.0'
    >>> ident.build_string
    'h12345_0'
    >>> str(ident)
    'python-3.11.0-h12345_0'
    >>> ident == ArchiveIdentifier.from_str("python-3.11.0-h12345_0")
    True
    >>>
    ```
    """

    _inner: PyArchiveIdentifier

    def __init__(
        self,
        name_or_str: str,
        version: str | None = None,
        build_string: str | None = None,
    ) -> None:
        """
        Create a new ArchiveIdentifier.

        Parameters
        ----------
        name_or_str : str
            Either the package name or a full archive identifier string (e.g. "python-3.11.0-0").
        version : str, optional
            The package version (required if name_or_str is just the package name).
        build_string : str, optional
            The package build string (required if name_or_str is just the package name).
        """
        if version is not None and build_string is not None:
            self._inner = PyArchiveIdentifier(name_or_str, version, build_string)
        elif version is None and build_string is None:
            self._inner = PyArchiveIdentifier.from_str(name_or_str)
        else:
            raise ValueError(
                "ArchiveIdentifier requires either a single identifier string or name, version, and build_string"
            )

    @classmethod
    def _from_py_archive_identifier(cls, py_archive_identifier: PyArchiveIdentifier) -> ArchiveIdentifier:
        archive_identifier = cls.__new__(cls)
        archive_identifier._inner = py_archive_identifier
        return archive_identifier

    @classmethod
    def from_str(cls, string: str) -> ArchiveIdentifier:
        """
        Parses an ArchiveIdentifier from a string formatted as `<name>-<version>-<build_string>`.

        Examples
        --------
        ```python
        >>> ident = ArchiveIdentifier.from_str("numpy-1.24.0-py310_0")
        >>> ident.name
        'numpy'
        >>> ident.version
        '1.24.0'
        >>> ident.build_string
        'py310_0'
        >>>
        ```
        """
        return cls._from_py_archive_identifier(PyArchiveIdentifier.from_str(string))

    @property
    def name(self) -> str:
        """The package name."""
        return self._inner.name

    @name.setter
    def name(self, value: str) -> None:
        self._inner.name = value

    @property
    def version(self) -> str:
        """The package version."""
        return self._inner.version

    @version.setter
    def version(self, value: str) -> None:
        self._inner.version = value

    @property
    def build_string(self) -> str:
        """The package build string."""
        return self._inner.build_string

    @build_string.setter
    def build_string(self, value: str) -> None:
        self._inner.build_string = value

    def __str__(self) -> str:
        return str(self._inner)

    def __repr__(self) -> str:
        return f"ArchiveIdentifier(name={self.name!r}, version={self.version!r}, build_string={self.build_string!r})"

    def __hash__(self) -> int:
        return self._inner.__hash__()

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, ArchiveIdentifier):
            return self._inner == other._inner
        return False


class CondaArchiveType:
    """
    Describes the type of a conda package archive (`.conda` or `.tar.bz2`).
    """

    _inner: PyCondaArchiveType

    def __init__(self, value: str | PyCondaArchiveType) -> None:
        if isinstance(value, PyCondaArchiveType):
            self._inner = value
        elif isinstance(value, str):
            val_lower = value.lower().strip()
            if val_lower in ("conda", ".conda"):
                self._inner = PyCondaArchiveType.conda()
            elif val_lower in ("tar.bz2", ".tar.bz2", "tar_bz2"):
                self._inner = PyCondaArchiveType.tar_bz2()
            else:
                raise ValueError(f"Unknown conda archive type: {value!r}")
        else:
            raise TypeError(f"expected str or PyCondaArchiveType, got {type(value).__name__}")

    @classmethod
    def _from_py_conda_archive_type(cls, py_type: PyCondaArchiveType) -> CondaArchiveType:
        archive_type = cls.__new__(cls)
        archive_type._inner = py_type
        return archive_type

    @classmethod
    def conda(cls) -> CondaArchiveType:
        """Returns the `.conda` archive type."""
        return cls._from_py_conda_archive_type(PyCondaArchiveType.conda())

    @classmethod
    def tar_bz2(cls) -> CondaArchiveType:
        """Returns the `.tar.bz2` archive type."""
        return cls._from_py_conda_archive_type(PyCondaArchiveType.tar_bz2())

    @property
    def extension(self) -> str:
        """The file extension for this archive type (e.g. '.conda' or '.tar.bz2')."""
        return self._inner.extension

    @property
    def is_conda(self) -> bool:
        """True if this archive type is `.conda`."""
        return self._inner.is_conda

    @property
    def is_tar_bz2(self) -> bool:
        """True if this archive type is `.tar.bz2`."""
        return self._inner.is_tar_bz2

    @classmethod
    def try_from_magic_bytes(cls, bytes_: bytes | bytearray) -> CondaArchiveType | None:
        """
        Tries to determine the type of a Conda archive from its magic bytes.
        """
        res = PyCondaArchiveType.try_from_magic_bytes(bytes(bytes_))
        return cls._from_py_conda_archive_type(res) if res is not None else None

    @classmethod
    def try_from_path(cls, path: str | Path | os.PathLike[str]) -> CondaArchiveType | None:
        """
        Tries to determine the type of conda archive from its path or filename.
        """
        res = PyCondaArchiveType.try_from_path(os.fspath(path))
        return cls._from_py_conda_archive_type(res) if res is not None else None

    @classmethod
    def split_str(cls, path: str) -> tuple[str, CondaArchiveType] | None:
        """
        Split the given string into its stem and archive type, removing the extension.
        Returns None if the path does not have a recognized conda archive extension.
        """
        res = PyCondaArchiveType.split_str(path)
        if res is None:
            return None
        stem, archive_type = res
        return stem, cls._from_py_conda_archive_type(archive_type)

    def __str__(self) -> str:
        return str(self._inner)

    def __repr__(self) -> str:
        if self.is_conda:
            return "CondaArchiveType.Conda"
        return "CondaArchiveType.TarBz2"

    def __hash__(self) -> int:
        return self._inner.__hash__()

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, CondaArchiveType):
            return self._inner == other._inner
        return False


class CondaArchiveIdentifier:
    """
    A conda package archive identifier containing `name`, `version`, `build_string`
    and `archive_type` (`.conda` or `.tar.bz2`).

    Examples
    --------
    ```python
    >>> ident = CondaArchiveIdentifier.try_from_filename("python-3.11.0-0.conda")
    >>> ident.name
    'python'
    >>> ident.version
    '3.11.0'
    >>> ident.build_string
    '0'
    >>> ident.archive_type.extension
    '.conda'
    >>> ident.to_file_name()
    'python-3.11.0-0.conda'
    >>>
    ```
    """

    _inner: PyCondaArchiveIdentifier

    def __init__(
        self,
        identifier: ArchiveIdentifier,
        archive_type: CondaArchiveType,
    ) -> None:
        """
        Construct a CondaArchiveIdentifier from an ArchiveIdentifier and CondaArchiveType.
        """
        self._inner = PyCondaArchiveIdentifier(identifier._inner, archive_type._inner)

    @classmethod
    def _from_py_conda_archive_identifier(cls, py_ident: PyCondaArchiveIdentifier) -> CondaArchiveIdentifier:
        ident = cls.__new__(cls)
        ident._inner = py_ident
        return ident

    @classmethod
    def from_str(cls, string: str) -> CondaArchiveIdentifier:
        """
        Parses a CondaArchiveIdentifier from a filename string.
        Raises ValueError if the string is not a valid conda archive filename.
        """
        return cls._from_py_conda_archive_identifier(PyCondaArchiveIdentifier.from_str(string))

    @classmethod
    def try_from_filename(cls, filename: str) -> CondaArchiveIdentifier | None:
        """
        Tries to convert the specified filename into a CondaArchiveIdentifier.
        Returns None if parsing fails or extension is not a valid conda archive.
        """
        res = PyCondaArchiveIdentifier.try_from_filename(filename)
        return cls._from_py_conda_archive_identifier(res) if res is not None else None

    @classmethod
    def try_from_path(cls, path: str | Path | os.PathLike[str]) -> CondaArchiveIdentifier | None:
        """
        Tries to convert the specified file path into a CondaArchiveIdentifier.
        Returns None if parsing fails or extension is not a valid conda archive.
        """
        res = PyCondaArchiveIdentifier.try_from_path(os.fspath(path))
        return cls._from_py_conda_archive_identifier(res) if res is not None else None

    @classmethod
    def try_from_url(cls, url: str) -> CondaArchiveIdentifier | None:
        """
        Tries to convert a URL into a CondaArchiveIdentifier.
        Returns None if parsing fails or extension is not a valid conda archive.
        """
        res = PyCondaArchiveIdentifier.try_from_url(url)
        return cls._from_py_conda_archive_identifier(res) if res is not None else None

    @property
    def identifier(self) -> ArchiveIdentifier:
        """The underlying ArchiveIdentifier (`name`, `version`, `build_string`)."""
        return ArchiveIdentifier._from_py_archive_identifier(self._inner.identifier)

    @identifier.setter
    def identifier(self, value: ArchiveIdentifier) -> None:
        self._inner.identifier = value._inner

    @property
    def archive_type(self) -> CondaArchiveType:
        """The CondaArchiveType of the package."""
        return CondaArchiveType._from_py_conda_archive_type(self._inner.archive_type)

    @archive_type.setter
    def archive_type(self, value: CondaArchiveType) -> None:
        self._inner.archive_type = value._inner

    @property
    def name(self) -> str:
        """The package name."""
        return self._inner.name

    @property
    def version(self) -> str:
        """The package version."""
        return self._inner.version

    @property
    def build_string(self) -> str:
        """The package build string."""
        return self._inner.build_string

    def to_file_name(self) -> str:
        """Converts the archive identifier into a filename for a Conda package."""
        return self._inner.to_file_name()

    def __str__(self) -> str:
        return str(self._inner)

    def __repr__(self) -> str:
        return f"CondaArchiveIdentifier(identifier={self.identifier!r}, archive_type={self.archive_type!r})"

    def __hash__(self) -> int:
        return self._inner.__hash__()

    def __eq__(self, other: Any) -> bool:
        if isinstance(other, CondaArchiveIdentifier):
            return self._inner == other._inner
        return False
