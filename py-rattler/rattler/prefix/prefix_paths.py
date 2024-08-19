from __future__ import annotations
import os
from typing import List, TYPE_CHECKING

from rattler.package.paths_json import FileMode
from rattler.rattler import PyPrefixPaths, PyPrefixPathsEntry, PyPrefixPathType

# `os.PathLike` started to be generic from Python 3.9
if TYPE_CHECKING:
    BasePathLike = os.PathLike[str]
else:
    BasePathLike = os.PathLike


class PrefixPathType:
    _inner: PyPrefixPathType

    @classmethod
    def from_py_path_type(cls, py_path_type: PyPrefixPathType) -> PrefixPathType:
        """Construct Rattler PathType from FFI PyPathType object."""
        path_type = cls.__new__(cls)
        path_type._inner = py_path_type
        return path_type

    @property
    def hardlink(self) -> bool:
        """
        Whether the path should be hardlinked (the default) (once installed)
        """
        return self._inner.hardlink

    @property
    def softlink(self) -> bool:
        """
        Whether the path should be softlinked (once installed)
        """
        return self._inner.softlink

    @property
    def directory(self) -> bool:
        """
        This is a directory
        """
        return self._inner.directory

    @property
    def pyc_file(self) -> bool:
        """
        This is a file compiled from Python code when a noarch package was installed
        """
        return self._inner.pyc_file

    @property
    def windows_python_entrypoint_script(self) -> bool:
        """
        A Windows entry point python script (a <entrypoint>-script.py Python script file)
        """
        return self._inner.windows_python_entrypoint_script

    @property
    def windows_python_entrypoint_exe(self) -> bool:
        """
        A Windows entry point python script (a <entrypoint>.exe executable)
        """
        return self._inner.windows_python_entrypoint_exe

    @property
    def unix_python_entrypoint(self) -> bool:
        """
        A Unix entry point python script (a <entrypoint> Python script file)
        """
        return self._inner.unix_python_entrypoint


class PrefixPathsEntry(BasePathLike):
    _inner: PyPrefixPathsEntry

    def __fspath__(self) -> str:
        return str(self._inner.path)

    @classmethod
    def _from_py_paths_entry(cls, py_paths_entry: PyPrefixPathsEntry) -> PrefixPathsEntry:
        """Construct Rattler PathsEntry from FFI PyPathsEntry object."""
        entry = cls.__new__(cls)
        entry._inner = py_paths_entry
        return entry

    @property
    def relative_path(self) -> os.PathLike[str]:
        """
        The relative path of the entry.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/tk-8.6.12-h8ffe710_0.json"
        ... )
        >>> paths = r.paths_data
        >>> relative_path = paths.paths[0].relative_path
        >>>
        ...
        ```
        """
        return self._inner.relative_path

    @property
    def no_link(self) -> bool:
        """
        Whether this file should be linked

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> paths = r.paths_data
        >>> no_link = paths.paths[0].no_link
        >>>
        ```
        """

    @property
    def path_type(self) -> PrefixPathType:
        """
        The type of the path.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> paths = r.paths_data
        >>> path_type = paths.paths[0].path_type
        >>>
        ```
        """
        return PrefixPathType.from_py_path_type(self._inner.path_type)

    @property
    def prefix_placeholder(self) -> str | None:
        """
        The prefix placeholder for the path.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> paths = r.paths_data
        >>> prefix_placeholder = paths.paths[0].prefix_placeholder
        >>>
        ```
        """
        return self._inner.prefix_placeholder

    @property
    def file_mode(self) -> FileMode:
        """
        The file mode of the path.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> paths = r.paths_data
        >>> file_mode = paths.paths[0].file_mode
        >>>
        ```
        """
        return FileMode._from_py_file_mode(self._inner.file_mode)

    @property
    def sha256(self) -> bytes:
        """
        The sha256 of the path.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> paths = r.paths_data
        >>> sha256 = paths.paths[0].sha256
        >>>
        ```
        """
        return self._inner.sha256

    @property
    def sha256_in_prefix(self) -> bytes:
        """
        The sha256 of the path in the prefix.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> paths = r.paths_data
        >>> sha256_in_prefix = paths.paths[0].sha256_in_prefix
        >>>
        ```
        """
        return self._inner.sha256_in_prefix

    @property
    def size_in_bytes(self) -> int:
        """
        The size of the path in bytes.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> paths = r.paths_data
        >>> size_in_bytes = paths.paths[0].size_in_bytes
        >>>
        ```
        """
        return self._inner.size_in_bytes


class PrefixPaths:
    _paths: PyPrefixPaths

    @classmethod
    def _from_py_prefix_paths(cls, py_prefix_paths: PyPrefixPaths) -> PrefixPaths:
        """Construct Rattler PrefixRecord from FFI PyPrefixRecord object."""
        paths = cls.__new__(cls)
        paths._paths = py_prefix_paths
        return paths

    @property
    def paths_version(self) -> int:
        """
        The version of the file.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> paths = r.paths_data
        >>> paths.paths_version
        1
        >>>
        ```
        """
        return self._paths.paths_version

    @property
    def paths(self) -> List[PrefixPathsEntry]:
        """
        All entries included in the package.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> paths = r.paths_data
        >>> paths.paths # doctest:+ELLIPSIS
        [...]
        >>>
        ```
        """
        return [PrefixPathsEntry._from_py_paths_entry(path) for path in self._paths.paths]

    def __repr__(self) -> str:
        """
        Returns a representation of the PrefixPaths.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_record import PrefixRecord
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> r.paths_data # doctest:+ELLIPSIS
        PrefixPaths(paths=[...])
        >>>
        ```
        """
        return f"PrefixPaths(paths={self.paths})"
