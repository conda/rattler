from __future__ import annotations

import os
from pathlib import Path
from typing import TYPE_CHECKING, Any

from rattler.rattler import (
    PyEntryPoint,
    PyLinkJson,
    PyNoArchLinks,
    PyPythonEntryPoints,
)

if TYPE_CHECKING:
    from rattler.networking.client import Client


class EntryPoint:
    """
    A single Python entry point described in `link.json`.
    """

    _inner: PyEntryPoint

    def __init__(self, command: str, module: str, function: str) -> None:
        """
        Create a new EntryPoint instance.

        Parameters
        ----------
        command : str
            The name of the executable command to create.
        module : str
            The Python module containing the function.
        function : str
            The function or callable to run.
        """
        self._inner = PyEntryPoint(command, module, function)

    @classmethod
    def _from_py_entry_point(cls, py_entry_point: PyEntryPoint) -> EntryPoint:
        entry_point = cls.__new__(cls)
        entry_point._inner = py_entry_point
        return entry_point

    @staticmethod
    def from_str(string: str) -> EntryPoint:
        """
        Parses an entry point from a string formatted as `command = module:function`.

        Examples
        --------
        ```python
        >>> ep = EntryPoint.from_str("jupyter-lab = jupyterlab.labapp:main")
        >>> ep.command
        'jupyter-lab'
        >>> ep.module
        'jupyterlab.labapp'
        >>> ep.function
        'main'
        >>>
        ```
        """
        return EntryPoint._from_py_entry_point(PyEntryPoint.from_str(string))

    @property
    def command(self) -> str:
        """
        The name of the executable command.
        """
        return self._inner.command

    @command.setter
    def command(self, value: str) -> None:
        self._inner.command = value

    @property
    def module(self) -> str:
        """
        The module where the entry point function is located.
        """
        return self._inner.module

    @module.setter
    def module(self, value: str) -> None:
        self._inner.module = value

    @property
    def function(self) -> str:
        """
        The function to invoke for this entry point.
        """
        return self._inner.function

    @function.setter
    def function(self, value: str) -> None:
        self._inner.function = value

    def __str__(self) -> str:
        return self._inner.__str__()

    def __repr__(self) -> str:
        return self._inner.__repr__()

    def __eq__(self, other: Any) -> bool:
        if not isinstance(other, EntryPoint):
            return False
        return self._inner == other._inner


class PythonEntryPoints:
    """
    Describes Python noarch specific entry points in `link.json`.
    """

    _inner: PyPythonEntryPoints

    def __init__(self, entry_points: list[EntryPoint] | None = None) -> None:
        """
        Create a new PythonEntryPoints instance.
        """
        raw_eps = [ep._inner for ep in entry_points] if entry_points is not None else None
        self._inner = PyPythonEntryPoints(raw_eps)

    @classmethod
    def _from_py_python_entry_points(cls, py_python_entry_points: PyPythonEntryPoints) -> PythonEntryPoints:
        pep = cls.__new__(cls)
        pep._inner = py_python_entry_points
        return pep

    @property
    def entry_points(self) -> list[EntryPoint]:
        """
        A list of commands that should execute certain Python functions.
        """
        return [EntryPoint._from_py_entry_point(ep) for ep in self._inner.entry_points]

    @entry_points.setter
    def entry_points(self, value: list[EntryPoint]) -> None:
        self._inner.entry_points = [ep._inner for ep in value]

    def __repr__(self) -> str:
        return self._inner.__repr__()


class NoArchLinks:
    """
    Links for specific types of noarch packages in `link.json`.
    """

    _inner: PyNoArchLinks

    @classmethod
    def _from_py_noarch_links(cls, py_noarch_links: PyNoArchLinks) -> NoArchLinks:
        links = cls.__new__(cls)
        links._inner = py_noarch_links
        return links

    @staticmethod
    def generic() -> NoArchLinks:
        """
        Create a generic NoArchLinks variant (without Python entry points).
        """
        return NoArchLinks._from_py_noarch_links(PyNoArchLinks.generic())

    @staticmethod
    def python(entry_points: list[EntryPoint] | None = None) -> NoArchLinks:
        """
        Create a Python NoArchLinks variant with optional entry points.
        """
        raw_eps = [ep._inner for ep in entry_points] if entry_points is not None else None
        return NoArchLinks._from_py_noarch_links(PyNoArchLinks.python(raw_eps))

    @property
    def is_python(self) -> bool:
        """
        True if the package is a Python noarch package.
        """
        return self._inner.is_python

    @property
    def is_generic(self) -> bool:
        """
        True if the package is a generic noarch package.
        """
        return self._inner.is_generic

    @property
    def python_entry_points(self) -> PythonEntryPoints | None:
        """
        The PythonEntryPoints if this is a Python noarch package, or None.
        """
        if (pep := self._inner.python_entry_points) is not None:
            return PythonEntryPoints._from_py_python_entry_points(pep)
        return None

    @property
    def entry_points(self) -> list[EntryPoint] | None:
        """
        List of entry points if this is a Python noarch package, or None.
        """
        if (eps := self._inner.entry_points) is not None:
            return [EntryPoint._from_py_entry_point(ep) for ep in eps]
        return None

    def __repr__(self) -> str:
        return self._inner.__repr__()


class LinkJson:
    """
    A representation of the `link.json` file found in noarch package archives.

    The `link.json` file contains information about entrypoints that need to be
    installed for the package.
    """

    _inner: PyLinkJson

    def __init__(self, noarch: NoArchLinks, package_metadata_version: int = 1) -> None:
        """
        Create a new LinkJson instance.

        Parameters
        ----------
        noarch : NoArchLinks
            The noarch linking metadata (Python or generic).
        package_metadata_version : int, optional
            The package metadata version (defaults to 1).
        """
        self._inner = PyLinkJson(noarch._inner, package_metadata_version)

    @classmethod
    def _from_py_link_json(cls, py_link_json: PyLinkJson) -> LinkJson:
        link_json = cls.__new__(cls)
        link_json._inner = py_link_json
        return link_json

    @staticmethod
    def from_package_archive(path: os.PathLike[str]) -> LinkJson:
        """
        Parses the package file from archive.
        """
        return LinkJson._from_py_link_json(PyLinkJson.from_package_archive(Path(path)))

    @staticmethod
    def from_path(path: os.PathLike[str]) -> LinkJson:
        """
        Parses the object from a file specified by a `path`.
        """
        return LinkJson._from_py_link_json(PyLinkJson.from_path(Path(path)))

    @staticmethod
    def from_package_directory(path: os.PathLike[str]) -> LinkJson:
        """
        Parses the object by looking up `info/link.json` from the root of the
        specified Conda archive directory.
        """
        return LinkJson._from_py_link_json(PyLinkJson.from_package_directory(Path(path)))

    @staticmethod
    def from_str(string: str) -> LinkJson:
        """
        Parses the object from a JSON string.
        """
        return LinkJson._from_py_link_json(PyLinkJson.from_str(string))

    @classmethod
    async def from_remote_url(cls, client: Client, url: str) -> LinkJson | None:
        """
        Fetches `info/link.json` from a remote package archive URL.
        """
        py_link_json = await PyLinkJson.from_remote_url(client._client, url)
        if py_link_json is None:
            return None
        return cls._from_py_link_json(py_link_json)

    @staticmethod
    def package_path() -> Path:
        """
        Returns the path to the file within the Conda archive ('info/link.json').
        """
        return PyLinkJson.package_path()

    def to_json(self) -> str:
        """
        Convert the LinkJson to a JSON string.
        """
        return self._inner.to_json()

    @property
    def package_metadata_version(self) -> int:
        """
        The version of the package metadata file.
        """
        return self._inner.package_metadata_version

    @package_metadata_version.setter
    def package_metadata_version(self, value: int) -> None:
        self._inner.package_metadata_version = value

    @property
    def noarch(self) -> NoArchLinks:
        """
        Links for specific noarch packages.
        """
        return NoArchLinks._from_py_noarch_links(self._inner.noarch)

    @noarch.setter
    def noarch(self, value: NoArchLinks) -> None:
        self._inner.noarch = value._inner

    def __repr__(self) -> str:
        return self._inner.__repr__()
