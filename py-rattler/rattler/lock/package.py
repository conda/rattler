from __future__ import annotations
from typing import Optional, Tuple
from rattler.lock.pypi import PypiPackageData, PypiPackageEnvironmentData

from rattler.rattler import PyLockedPackage
from rattler.repo_data.record import RepoDataRecord


class LockedPackage:
    _package: PyLockedPackage

    def as_conda(self) -> Optional[RepoDataRecord]:
        """
        Returns this instance as a [`RepoDataRecord`] if this instance represents a conda package.

        Examples
        --------
        ```python
        >>> from rattler import Platform, LockFile
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> lock_packages = env.packages(Platform("osx-arm64"))
        >>> conda_pkg = [pkg for pkg in lock_packages if pkg.is_conda]
        >>> conda_pkg[0].as_conda()
        RepoDataRecord(...)
        >>>
        ```
        """
        if pkg := self._package.as_conda():
            return RepoDataRecord._from_py_record(pkg)
        return None

    def as_pypi(self) -> Optional[Tuple[PypiPackageData, PypiPackageEnvironmentData]]:
        """
        Returns this instance as a tuple of [`PypiPackageData`] & [`PypiPackageEnvironmentData`]
        if this instance represents a pypi package.

        Examples
        --------
        ```python
        >>> from rattler import Platform, LockFile
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> lock_packages = env.packages(Platform("osx-arm64"))
        >>> pypi_pkg = [pkg for pkg in lock_packages if pkg.is_pypi]
        >>> pypi_pkg[0].as_pypi()
        (PypiPackageData(), PypiPackageEnvironmentData())
        >>>
        ```
        """
        if pkg := self._package.as_pypi():
            (pkg_data, env_data) = pkg
            return (
                PypiPackageData._from_py_pypi_package_data(pkg_data),
                PypiPackageEnvironmentData._from_py_pypi_env_data(env_data),
            )
        return None

    @property
    def is_conda(self) -> bool:
        """
        Returns true if this package represents a conda package.

        Examples
        --------
        ```python
        >>> from rattler import Platform, LockFile
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> lock_packages = env.packages(Platform("osx-arm64"))
        >>> conda_pkgs = [pkg for pkg in lock_packages if pkg.url_or_path.startswith("https://conda.anaconda.org/")]
        >>> conda_pkgs[0].is_conda
        True
        >>>
        ```
        """
        return self._package.is_conda

    @property
    def is_pypi(self) -> bool:
        """
        Returns true if this package represents a conda package.

        Examples
        --------
        ```python
        >>> from rattler import Platform, LockFile
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> lock_packages = env.packages(Platform("osx-arm64"))
        >>> pypi_pkgs = [pkg for pkg in lock_packages if pkg.url_or_path.startswith("https://files.pythonhosted.org/")]
        >>> pypi_pkgs[0].is_pypi
        True
        >>>
        ```
        """
        return self._package.is_pypi

    @property
    def name(self) -> str:
        """
        Returns the name of the package.

        Examples
        --------
        ```python
        >>> from rattler import Platform, LockFile
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> lock_package = env.packages(Platform("osx-arm64"))[0]
        >>> lock_package.name
        'tzdata'
        >>>
        ```
        """
        return self._package.name

    @property
    def url_or_path(self) -> str:
        """
        Returns the URL of the package

        Examples
        --------
        ```python
        >>> from rattler import Platform, LockFile
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> lock_package = env.packages(Platform("osx-arm64"))[0]
        >>> lock_package.url_or_path
        'https://conda.anaconda.org/...'
        >>>
        ```
        """
        return self._package.url_or_path

    @property
    def version(self) -> str:
        """
        Returns the version string of the package.

        Examples
        --------
        ```python
        >>> from rattler import Platform, LockFile
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> lock_package = env.packages(Platform("osx-arm64"))[0]
        >>> lock_package.version
        '2024a'
        >>>
        ```
        """
        return self._package.version

    @classmethod
    def _from_py_lock_package(cls, lock_pkg: PyLockedPackage) -> LockedPackage:
        """
        Construct Rattler LockedPackage from FFI PyLockedPackage object.
        """
        pkg = cls.__new__(cls)
        pkg._package = lock_pkg
        return pkg

    def __repr__(self) -> str:
        """
        Returns a representation of the LockedPackage.
        """
        return "LockedPackage()"
