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

        Examples
        --------
        ```python
        >>> about = AboutJson.from_path("../test-data/dummy-about.json")
        >>> about
        AboutJson()
        >>>
        ```
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
        return AboutJson._from_py_about_json(PyAboutJson.from_package_directory(Path(path)))

    @staticmethod
    def from_str(string: str) -> AboutJson:
        """
        Parses the object from a string, using a format appropriate for the file
        type.

        For example, if the file is in JSON format, this function parses the JSON
        string and returns the resulting object. If the file is not in a parsable
        format, this function returns an error.

        Examples
        --------
        ```python
        >>> import json
        >>> with open("../test-data/dummy-about.json", 'r') as file:
        ...     json_str = json.dumps(json.load(file))
        >>> about = AboutJson.from_str(json_str)
        >>> about
        AboutJson()
        >>>
        ```
        """
        return AboutJson._from_py_about_json(PyAboutJson.from_str(string))

    @staticmethod
    def package_path() -> str:
        """
        Returns the path to the file within the Conda archive.

        The path is relative to the root of the archive and includes any necessary
        directories.

        Examples
        --------
        ```python
        >>> AboutJson.package_path()
        'info/about.json'
        >>>
        ```
        """
        return PyAboutJson.package_path()

    @property
    def channels(self) -> List[str]:
        """
        A list of channels that where used during the build.

        Examples
        --------
        ```python
        >>> about = AboutJson.from_path("../test-data/dummy-about.json")
        >>> about.channels
        ['https://conda.anaconda.org/conda-forge']
        >>>
        ```
        """
        return self._inner.channels

    @property
    def description(self) -> Optional[str]:
        """
        Description of the package.

        Examples
        --------
        ```python
        >>> about = AboutJson.from_path("../test-data/dummy-about.json")
        >>> about.description
        'A dummy description.'
        >>>
        ```
        """
        if description := self._inner.description:
            return description

        return None

    @property
    def dev_url(self) -> List[str]:
        """
        A list of URLs to the development page of the package.

        Examples
        --------
        ```python
        >>> about = AboutJson.from_path("../test-data/dummy-about.json")
        >>> about.dev_url
        ['https://github.com/conda/rattler']
        >>>
        ```
        """
        return self._inner.dev_url

    @property
    def doc_url(self) -> List[str]:
        """
        A list of URLs to the documentation of the package.

        Examples
        --------
        ```python
        >>> about = AboutJson.from_path("../test-data/dummy-about.json")
        >>> about.doc_url
        ['https://conda.github.io/rattler/py-rattler/']
        >>>
        ```
        """
        return self._inner.doc_url

    @property
    def home(self) -> List[str]:
        """
        A list URL to the homepage of the package.

        Examples
        --------
        ```python
        >>> about = AboutJson.from_path("../test-data/dummy-about.json")
        >>> about.home
        ['http://github.com/conda/rattler']
        >>>
        ```
        """
        return self._inner.home

    @property
    def license(self) -> Optional[str]:
        """
        The license of the package.

        Examples
        --------
        ```python
        >>> about = AboutJson.from_path("../test-data/dummy-about.json")
        >>> about.license
        'BSD-3-Clause'
        >>>
        ```
        """
        if license := self._inner.license:
            return license

        return None

    @property
    def license_family(self) -> Optional[str]:
        """
        The license family of the package.

        Examples
        --------
        ```python
        >>> about = AboutJson.from_path("../test-data/dummy-about.json")
        >>> about.license_family
        >>> type(about.license_family)
        <class 'NoneType'>
        >>>
        ```
        """
        if license_family := self._inner.license_family:
            return license_family

        return None

    @property
    def source_url(self) -> Optional[str]:
        """
        The URL to the latest source code of the package.

        Examples
        --------
        ```python
        >>> about = AboutJson.from_path("../test-data/dummy-about.json")
        >>> about.source_url
        'https://github.com/conda/rattler'
        >>>
        ```
        """
        if source_url := self._inner.source_url:
            return source_url

        return None

    @property
    def summary(self) -> Optional[str]:
        """
        A Short summary description.

        Examples
        --------
        ```python
        >>> about = AboutJson.from_path("../test-data/dummy-about.json")
        >>> about.summary
        'A dummy summary.'
        >>>
        ```
        """
        if summary := self._inner.summary:
            return summary

        return None

    @classmethod
    def _from_py_about_json(cls, py_about_json: PyAboutJson) -> AboutJson:
        about_json = cls.__new__(cls)
        about_json._inner = py_about_json

        return about_json

    def __repr__(self) -> str:
        """
        Returns a representation of the AboutJson.
        """
        return "AboutJson()"
