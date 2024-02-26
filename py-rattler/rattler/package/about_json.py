from __future__ import annotations

import os
from pathlib import Path
from typing import List, Optional

from rattler.rattler import PyAboutJson


class AboutJson:
    """
    The `about.json` file contains metadata about the package.
    """

    _inner: PyAboutJson

    @staticmethod
    def from_path(path: os.PathLike[str]) -> AboutJson:
        """
        Parses the object from a file specified by a `path`, using a format
        appropriate for the file type.

        For example, if the file is in JSON format, this function reads the data
        from the file at the specified path, parse the JSON string and return the
        resulting object. If the file is not in a parsable format or if the file
        could not read, this function returns an error.
        """
        return AboutJson._from_py_about_json(PyAboutJson.from_path(Path(path)))

    @staticmethod
    def from_package_directory(path: os.PathLike[str]) -> AboutJson:
        """
        Parses the object by looking up the appropriate file from the root of the
        specified Conda archive directory, using a format appropriate for the file
        type.

        For example, if the file is in JSON format, this function reads the
        appropriate file from the archive, parse the JSON string and return the
        resulting object. If the file is not in a parsable format or if the file
        could not be read, this function returns an error.
        """
        return AboutJson._from_py_about_json(
            PyAboutJson.from_package_directory(Path(path))
        )

    @staticmethod
    def from_str(string: str) -> AboutJson:
        """
        Parses the object from a string, using a format appropriate for the file
        type.

        For example, if the file is in JSON format, this function parses the JSON
        string and returns the resulting object. If the file is not in a parsable
        format, this function returns an error.
        """
        return AboutJson._from_py_about_json(PyAboutJson.from_str(string))

    @staticmethod
    def package_path() -> str:
        """
        Returns the path to the file within the Conda archive.

        The path is relative to the root of the archive and includes any necessary
        directories.
        """
        return PyAboutJson.package_path()

    @property
    def channels(self) -> List[str]:
        """
        A list of channels that where used during the build.
        """
        return self._inner.channels

    @property
    def description(self) -> Optional[str]:
        """
        Description of the package.
        """
        return self._inner.description

    @property
    def dev_url(self) -> List[str]:
        """
        A list of URLs to the development page of the package.
        """
        return self._inner.dev_url

    @property
    def doc_url(self) -> List[str]:
        """
        A list of URLs to the documentation of the package.
        """
        return self._inner.doc_url

    @property
    def home(self) -> List[str]:
        """
        A list URL to the homepage of the package.
        """
        return self._inner.home

    @property
    def license(self) -> Optional[str]:
        """
        The license of the package.
        """
        return self._inner.license

    @property
    def license_family(self) -> Optional[str]:
        """
        The license family of the package.
        """
        return self._inner.license_family

    @property
    def source_url(self) -> Optional[str]:
        """
        The URL to the latest source code of the package.
        """
        return self._inner.source_url

    @property
    def summary(self) -> Optional[str]:
        """
        A Short summary description.
        """
        return self._inner.summary

    @classmethod
    def _from_py_about_json(cls, py_about_json: PyAboutJson) -> AboutJson:
        about_json = cls.__new__(cls)
        about_json._inner = py_about_json

        return about_json

    def __repr__(self) -> str:
        return "AboutJson()"
