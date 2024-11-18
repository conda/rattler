from __future__ import annotations
from abc import ABC
from typing import Optional, Set, List

from rattler import PackageRecord, Version, RepoDataRecord

from rattler.rattler import PyLockedPackage
from rattler.lock.hash import PackageHashes
from rattler.match_spec import MatchSpec


class LockedPackage(ABC):
    """
    Base class for any package in a lock file.
    """

    _package: PyLockedPackage

    @property
    def name(self) -> str:
        """
        Returns the name of the package as recorded in the lock-file.

        Note that this might not perse be the normalized name according to the specific ecosystem for the package.

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
    def location(self) -> str:
        """
        Returns the location of the package as recorded in the lock-file.

        Examples
        --------
        ```python
        >>> from rattler import Platform, LockFile
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> lock_package = env.packages(Platform("osx-arm64"))[0]
        >>> lock_package.location
        'https://conda.anaconda.org/...'
        >>>
        ```
        """
        return self._package.location

    @property
    def hashes(self) -> Optional[PackageHashes]:
        """
        Hashes of the file pointed to by `url`.

        Examples
        --------
        ```python
        >>> from rattler import LockFile, Platform
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> pypi_packages = env.pypi_packages()
        >>> data = pypi_packages[Platform("osx-arm64")][0]
        >>> data.hashes
        PackageHashes()
        >>>
        ```
        """
        return PackageHashes._from_py_package_hashes(self._package.hashes)

    def __repr__(self) -> str:
        """
        Returns a representation of the LockedPackage.
        """
        return f"{type(self).__name__}(" f"name={self.name!r}," f"location={self.location!r}" ")"

    @classmethod
    def _from_py_locked_package(cls, py_pkg: PyLockedPackage) -> LockedPackage:
        """
        Construct Rattler LockedPackage from FFI PyLockedPackage object.
        """
        pkg: LockedPackage
        if py_pkg.is_conda_binary:
            pkg = CondaLockedBinaryPackage.__new__(CondaLockedBinaryPackage)
        elif py_pkg.is_conda_source:
            pkg = CondaLockedSourcePackage.__new__(CondaLockedSourcePackage)
        elif py_pkg.is_pypi:
            pkg = PypiLockedPackage.__new__(PypiLockedPackage)
        else:
            raise TypeError(
                "Cannot create LockedPackage from PyLockedPackage, the type of the package is not supported."
            )

        pkg._package = py_pkg
        return pkg


class CondaLockedPackage(LockedPackage, ABC):
    """
    A locked conda package in a lock file.
    """

    @property
    def package_record(self) -> PackageRecord:
        """
        Returns the metadata of the package as recorded in the lock-file.
        """
        return PackageRecord._from_py_record(self._package.package_record)

    @property
    def version(self) -> Version:
        """
        Returns the version of the package as recorded in the lock-file.

        Examples
        --------
        ```python
        >>> from rattler import Platform, LockFile
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> lock_package = env.packages(Platform("osx-arm64"))[0]
        >>> lock_package.version
        Version("2024a")
        >>>
        ```
        """
        return Version._from_py_version(self._package.conda_version)

    def satisfies(self, spec: MatchSpec | str) -> bool:
        """
        Returns true if this package satisfies the given `spec`.

        Examples
        --------
        ```python
        >>> from rattler import LockFile, Platform
        >>> lock_file = LockFile.from_path("../test-data/test.lock")
        >>> env = lock_file.default_environment()
        >>> packages = env.packages(Platform("osx-arm64"))
        >>> packages[0].satisfies("tzdata >=2024a")
        True
        >>>
        ```
        """
        if isinstance(spec, str):
            spec = MatchSpec(spec)

        return self._package.conda_satisfies(spec._match_spec)


class PypiLockedPackage(LockedPackage):
    """
    A locked PyPI package in a lock file.
    """

    @property
    def version(self) -> str:
        """
        Returns the version of the package as recorded in the lock-file.
        """
        return self._package.pypi_version

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
        >>> data = pypi_packages[Platform("osx-arm64")][0]
        >>> data.requires_dist
        []
        >>>
        ```
        """
        return self._package.pypi_requires_dist

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
        >>> data = pypi_packages[Platform("osx-arm64")][0]
        >>> data.requires_python
        '>=3.7.0'
        >>>
        ```
        """
        return self._package.pypi_requires_python

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
        >>> data = pypi_packages[Platform("osx-arm64")][0]
        >>> data.is_editable
        False
        >>>
        ```
        """
        return self._package.pypi_is_editable

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
        >>> env_data = pypi_packages[Platform("osx-arm64")][0]
        >>> env_data.extras
        set()
        >>>
        ```
        """
        return self._package.pypi_extras

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
        >>> data = pypi_packages[Platform("osx-arm64")][0]
        >>> data.satisfies("charset-normalizer")
        True
        >>>
        ```
        """
        return self._package.pypi_satisfies(spec)

    @classmethod
    def _from_py_locked_package(cls, py_pkg: PyLockedPackage) -> PypiLockedPackage:
        """
        Construct Rattler LockedPackage from FFI PyLockedPackage object.
        """
        if py_pkg.is_pypi:
            pkg = PypiLockedPackage.__new__(PypiLockedPackage)
        else:
            raise TypeError(
                "Cannot create PypiLockedPackage from PyLockedPackage, the type of the package is not supported."
            )

        pkg._package = py_pkg
        return pkg


class CondaLockedSourcePackage(CondaLockedPackage):
    """
    A locked conda source package in a lock file.
    """


class CondaLockedBinaryPackage(CondaLockedPackage):
    """
    A locked conda binary package in a lock file.
    """

    def repo_data_record(self) -> RepoDataRecord:
        """
        Returns the metadata of the package as recorded in the lock-file including location information.
        """
        return RepoDataRecord._from_py_record(self._package.repo_data_record)
