from __future__ import annotations
from typing import List, Optional, Set
from rattler.lock.hash import PackageHashes

from rattler.rattler import PyPypiPackageData, PyPypiPackageEnvironmentData


class PypiPackageData:
    """
    A pinned Pypi package.
    """

    _data: PyPypiPackageData

    def satisfies(self, spec: str) -> bool:
        """
        Returns true if this package satisfies the given `spec`.

        Examples
        --------
        ```python
        >>> from rattler import LockFile, Platform
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> pypi_packages = env.pypi_packages()
        >>> data = pypi_packages[Platform("osx-arm64")][0][0]
        >>> data.satisfies("charset-normalizer")
        True
        >>>
        ```
        """
        return self._data.satisfies(spec)

    @property
    def name(self) -> str:
        """
        The name of the package.

        Examples
        --------
        ```python
        >>> from rattler import LockFile, Platform
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> pypi_packages = env.pypi_packages()
        >>> data = pypi_packages[Platform("osx-arm64")][0][0]
        >>> data.name
        'charset-normalizer'
        >>>
        ```
        """
        return self._data.name

    @property
    def version(self) -> str:
        """
        The version of the package.

        Examples
        --------
        ```python
        >>> from rattler import LockFile, Platform
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> pypi_packages = env.pypi_packages()
        >>> data = pypi_packages[Platform("osx-arm64")][0][0]
        >>> data.version
        '3.3.2'
        >>>
        ```
        """
        return self._data.version

    @property
    def url_or_path(self) -> str:
        """
        The URL that points to where the artifact can be downloaded from.

        Examples
        --------
        ```python
        >>> from rattler import LockFile, Platform
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> pypi_packages = env.pypi_packages()
        >>> data = pypi_packages[Platform("osx-arm64")][0][0]
        >>> data.url_or_path
        'https://files.pythonhosted.org/...'
        >>>
        ```
        """
        return self._data.url_or_path

    @property
    def hash(self) -> Optional[PackageHashes]:
        """
        Hashes of the file pointed to by `url`.

        Examples
        --------
        ```python
        >>> from rattler import LockFile, Platform
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> pypi_packages = env.pypi_packages()
        >>> data = pypi_packages[Platform("osx-arm64")][0][0]
        >>> data.hash
        PackageHashes()
        >>>
        ```
        """
        return PackageHashes._from_py_package_hashes(self._data.hash)

    @property
    def requires_dist(self) -> List[str]:
        """
        A list of dependencies on other packages.

        Examples
        --------
        ```python
        >>> from rattler import LockFile, Platform
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> pypi_packages = env.pypi_packages()
        >>> data = pypi_packages[Platform("osx-arm64")][0][0]
        >>> data.requires_dist
        []
        >>>
        ```
        """
        return self._data.requires_dist

    @property
    def requires_python(self) -> Optional[str]:
        """
        The python version that this package requires.

        Examples
        --------
        ```python
        >>> from rattler import LockFile, Platform
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> pypi_packages = env.pypi_packages()
        >>> data = pypi_packages[Platform("osx-arm64")][0][0]
        >>> data.requires_python
        '>=3.7.0'
        >>>
        ```
        """
        return self._data.requires_python

    @property
    def is_editable(self) -> bool:
        """
        Whether the package should be installed in editable mode or not.

        Examples
        --------
        ```python
        >>> from rattler import LockFile, Platform
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> pypi_packages = env.pypi_packages()
        >>> data = pypi_packages[Platform("osx-arm64")][0][0]
        >>> data.is_editable
        False
        >>>
        ```
        """
        return self._data.is_editable

    @classmethod
    def _from_py_pypi_package_data(cls, pkg_data: PyPypiPackageData) -> PypiPackageData:
        """
        Construct Rattler PypiPackageData from FFI PyPypiPackageData object.
        """
        data = cls.__new__(cls)
        data._data = pkg_data
        return data

    def __repr__(self) -> str:
        """
        Returns a representation of the PypiPackageData.
        """
        return "PypiPackageData()"


class PypiPackageEnvironmentData:
    _data: PyPypiPackageEnvironmentData

    @property
    def extras(self) -> Set[str]:
        """
        The extras enabled for the package.
        Note that the order doesn't matter.

        Examples
        --------
        ```python
        >>> from rattler import LockFile, Platform
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> pypi_packages = env.pypi_packages()
        >>> env_data = pypi_packages[Platform("osx-arm64")][0][1]
        >>> env_data.extras
        set()
        >>>
        ```
        """
        return self._data.extras

    @classmethod
    def _from_py_pypi_env_data(cls, env_data: PyPypiPackageEnvironmentData) -> PypiPackageEnvironmentData:
        """
        Construct Rattler PypiPackageEnvironmentData from FFI PyPypiPackageEnvironmentData object.
        """
        data = cls.__new__(cls)
        data._data = env_data
        return data

    def __repr__(self) -> str:
        """
        Returns a representation of the PypiPackageEnvironmentData.
        """
        return "PypiPackageEnvironmentData()"
