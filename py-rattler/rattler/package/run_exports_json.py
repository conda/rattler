from __future__ import annotations
import os
from pathlib import Path
from typing import List

from rattler.rattler import PyRunExportsJson


class RunExportsJson:
    """
    A representation of the `run_exports.json` file found in package archives.
    The `run_exports.json` file contains information about the run exports of a package
    """

    _inner: PyRunExportsJson

    @staticmethod
    def from_package_archive(path: os.PathLike[str]) -> RunExportsJson:
        """
        Parses the package file from archive.
        Note: If you want to extract multiple `info/*` files then this will be slightly
              slower than manually iterating over the archive entries with
              custom logic as this skips over the rest of the archive
        """
        return RunExportsJson._from_py_run_exports_json(PyRunExportsJson.from_package_archive(path))

    @staticmethod
    def from_path(path: os.PathLike[str]) -> RunExportsJson:
        """
        Parses the object from a file specified by a `path`, using a format
        appropriate for the file type.

        For example, if the file is in JSON format, this function reads the data
        from the file at the specified path, parse the JSON string and return the
        resulting object. If the file is not in a parsable format or if the file
        could not read, this function returns an error.

        Examples
        --------
        ```python
        >>> run_exports = RunExportsJson.from_path(
        ...     "../test-data/python-3.10.6-h2c4edbf_0_cpython-run_exports.json"
        ... )
        >>> run_exports
        RunExportsJson()
        >>>
        ```
        """
        return RunExportsJson._from_py_run_exports_json(PyRunExportsJson.from_path(Path(path)))

    @staticmethod
    def from_package_directory(path: os.PathLike[str]) -> RunExportsJson:
        """
        Parses the object by looking up the appropriate file from the root of the
        specified Conda archive directory, using a format appropriate for the file
        type.

        For example, if the file is in JSON format, this function reads the
        appropriate file from the archive, parse the JSON string and return the
        resulting object. If the file is not in a parsable format or if the file
        could not be read, this function returns an error.
        """
        return RunExportsJson._from_py_run_exports_json(PyRunExportsJson.from_package_directory(Path(path)))

    @staticmethod
    def from_str(string: str) -> RunExportsJson:
        """
        Parses the object from a string, using a format appropriate for the file
        type.

        For example, if the file is in JSON format, this function parses the JSON
        string and returns the resulting object. If the file is not in a parsable
        format, this function returns an error.
        """
        return RunExportsJson._from_py_run_exports_json(PyRunExportsJson.from_str(string))

    @staticmethod
    def package_path() -> str:
        """
        Returns the path to the file within the Conda archive.

        The path is relative to the root of the archive and includes any necessary
        directories.
        """
        return PyRunExportsJson.package_path()

    @property
    def weak(self) -> List[str]:
        """
        Weak run exports apply a dependency from host to run.

        Examples
        --------
        ```python
        >>> run_exports = RunExportsJson.from_path(
        ...     "../test-data/python-3.10.6-h2c4edbf_0_cpython-run_exports.json"
        ... )
        >>> run_exports.weak
        ['python_abi 3.10.* *_cp310']
        >>>
        ```
        """
        return self._inner.weak

    @property
    def strong(self) -> List[str]:
        """
        Strong run exports apply a dependency from build to host and run.

        Examples
        --------
        ```python
        >>> run_exports = RunExportsJson.from_path(
        ...     "../test-data/python-3.10.6-h2c4edbf_0_cpython-run_exports.json"
        ... )
        >>> run_exports.strong
        []
        >>>
        ```
        """
        return self._inner.strong

    @property
    def noarch(self) -> List[str]:
        """
        NoArch run exports apply a run export only to noarch packages (other run exports are ignored).
        For example, python uses this to apply a dependency on python to all noarch packages, but not to
        the python_abi package.

        Examples
        --------
        ```python
        >>> run_exports = RunExportsJson.from_path(
        ...     "../test-data/python-3.10.6-h2c4edbf_0_cpython-run_exports.json"
        ... )
        >>> run_exports.noarch
        ['python']
        >>>
        ```
        """
        return self._inner.noarch

    @property
    def weak_constrains(self) -> List[str]:
        """
        Weak constrains apply a constrain dependency from host to build, or run to host.

        Examples
        --------
        ```python
        >>> run_exports = RunExportsJson.from_path(
        ...     "../test-data/python-3.10.6-h2c4edbf_0_cpython-run_exports.json"
        ... )
        >>> run_exports.weak_constrains
        []
        >>>
        ```
        """
        return self._inner.weak_constrains

    @property
    def strong_constrains(self) -> List[str]:
        """
        Strong constrains apply a constrain dependency from build to host and run.

        Examples
        --------
        ```python
        >>> run_exports = RunExportsJson.from_path(
        ...     "../test-data/python-3.10.6-h2c4edbf_0_cpython-run_exports.json"
        ... )
        >>> run_exports.strong_constrains
        []
        >>>
        ```
        """
        return self._inner.strong_constrains

    @classmethod
    def _from_py_run_exports_json(cls, py_run_exports_json: PyRunExportsJson) -> RunExportsJson:
        run_exports_json = cls.__new__(cls)
        run_exports_json._inner = py_run_exports_json

        return run_exports_json

    def __repr__(self) -> str:
        """
        Returns a representation of the RunExportsJson.
        """
        return "RunExportsJson()"
