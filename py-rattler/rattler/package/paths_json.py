from __future__ import annotations
import os
from typing import List, Optional
from rattler.rattler import (
    PyPathsJson,
    PyPathsEntry,
    PyPathType,
    PyPrefixPlaceholder,
    PyFileMode,
)


class PathsJson:
    """
    A representation of the `paths.json` file found in package archives.
    The `paths.json` file contains information about every file included with the package.
    """
    _inner: PyPathsJson

    @staticmethod
    def from_deprecated_package_directory(path: os.PathLike[str]) -> PathsJson:
        """
        Constructs a new instance by reading older (deprecated) files from a package directory.
        
        In older package archives the `paths.json` file does not exist. These packages contain the
        information normally present in the `paths.json` file spread over different files in the
        archive.
        
        This function reads the different files and tries to reconstruct a `paths.json` from it.
        """
        return PathsJson._from_py_paths_json(PyPathsJson.from_deprecated_package_directory(path))

    @staticmethod
    def from_package_directory_with_deprecated_fallback(
        path: os.PathLike[str],
    ) -> PathsJson:
        """
        Reads the file from a package archive directory. If the `paths.json` file could not be found
        use the `from_deprecated_package_directory` method as a fallback.
        """
        return PathsJson._from_py_paths_json(PyPathsJson.from_package_directory_with_deprecated_fallback(path))

    @property
    def paths(self) -> List[PathsEntry]:
        """
        All entries included in the package.
        """
        return [PathsEntry._from_py_paths_entry(path) for path in self._inner.paths]

    @property
    def paths_version(self) -> int:
        """
        The version of the file.
        """
        return self._inner.paths_version

    @classmethod
    def _from_py_paths_json(cls, py_paths_json: PyPathsJson) -> PathsJson:
        paths_json = cls.__new__(cls)
        paths_json._inner = py_paths_json

        return paths_json

    def __repr__(self) -> str:
        """
        Returns a representation of the PathsJson.
        """
        return "PathsJson()"


class PathsEntry:
    """
    A single entry in the `paths.json` file.
    """
    _inner: PyPathsEntry

    @property
    def relative_path(self) -> str:
        """
        The relative path from the root of the package.
        """
        return self._inner.relative_path

    @property
    def no_link(self) -> bool:
        """
        Whether or not this file should be linked or not when installing the package.
        """
        return self._inner.no_link

    @property
    def path_type(self) -> PathType:
        """
        Determines how to include the file when installing the package.
        """
        return PathType._from_py_path_type(self._inner.path_type)

    @property
    def prefix_placeholder(self) -> Optional[PrefixPlaceholder]:
        """
        Optionally the placeholder prefix used in the file. If this value is `None` 
        the prefix is not present in the file.
        """
        if placeholder := self._inner.prefix_placeholder:
            return PrefixPlaceholder._from_py_prefix_placeholder(placeholder)

        return None

    @property
    def sha256(self) -> Optional[bytes]:
        """
        A hex representation of the SHA256 hash of the contents of the file.
        This entry is only present in version 1 of the paths.json file.
        """
        return self._inner.sha256

    @property
    def size_in_bytes(self) -> Optional[int]:
        """
        The size of the file in bytes.
        This entry is only present in version 1 of the paths.json file.
        """
        if size := self._inner.size_in_bytes:
            return size

        return None

    @classmethod
    def _from_py_paths_entry(cls, py_paths_entry: PyPathsEntry) -> PathsEntry:
        paths_entry = cls.__new__(cls)
        paths_entry._inner = py_paths_entry

        return paths_entry

    def __repr__(self) -> str:
        """
        Returns a representation of the PathsEntry.
        """
        return "PathsEntry()"


class PathType:
    """
    The path type of the path entry
    """
    _inner: PyPathType

    @property
    def hardlink(self) -> bool:
        """
        The path should be hard linked (the default).
        """
        return self._inner.hardlink

    @property
    def softlink(self) -> bool:
        """
        The path should be soft linked.
        """
        return self._inner.softlink

    @property
    def directory(self) -> bool:
        """
        This should explicitly create an empty directory.
        """
        return self._inner.directory

    @classmethod
    def _from_py_path_type(cls, py_paths_type: PyPathType) -> PathType:
        path_type = cls.__new__(cls)
        path_type._inner = py_paths_type

        return path_type

    def __repr__(self) -> str:
        """
        Returns a representation of the PathType.
        """
        return "PathType()"


class PrefixPlaceholder:
    """
    Description off a placeholder text found in a file that must be replaced 
    when installing the file into the prefix.
    """
    _inner: PyPrefixPlaceholder

    @property
    def file_mode(self) -> FileMode:
        """
        The type of the file, either binary or text.
        """
        return self._inner.file_mode

    @property
    def placeholder(self) -> str:
        """
        The placeholder prefix used in the file. This is the path of the 
        prefix when the package was build.
        """
        return self._inner.placeholder

    @classmethod
    def _from_py_prefix_placeholder(cls, py_prefix_placeholder: PyPrefixPlaceholder) -> PrefixPlaceholder:
        prefix_placeholder = cls.__new__(cls)
        prefix_placeholder._inner = py_prefix_placeholder

        return prefix_placeholder

    def __repr__(self) -> str:
        """
        Returns a representation of the PrefixPlaceholder.
        """
        return "PrefixPlaceholder()"


class FileMode:
    """
    The file mode of the entry.
    """
    _inner: PyFileMode

    @property
    def binary(self) -> bool:
        """
        The file is a binary file (needs binary prefix replacement).
        """
        return self._inner.binary

    @property
    def text(self) -> bool:
        """
        The file is a text file (needs text prefix replacement).
        """
        return self._inner.text

    @classmethod
    def _from_py_file_mode(cls, py_file_mode: PyFileMode) -> FileMode:
        file_mode = cls.__new__(cls)
        file_mode._inner = py_file_mode

        return file_mode

    def __repr__(self) -> str:
        """
        Returns a representation of the FileMode.
        """
        return "FileMode()"