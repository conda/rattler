from __future__ import annotations
from typing import Optional, Tuple
from rattler.lock.pypi import PypiPackageData, PypiPackageEnvironmentData

from rattler.rattler import PyLockPackage
from rattler.repo_data.record import RepoDataRecord


class LockPackage:
    _package: PyLockPackage

    def satisfies(self, spec: str) -> bool:
        return self._package.satisfies(spec)

    def as_conda(self) -> Optional[RepoDataRecord]:
        """
        Returns this instance as a [`RepoDataRecord`] if this instance represents a conda package.
        """
        if pkg := self._package.as_conda():
            return RepoDataRecord._from_py_record(pkg)
        return None

    def as_pypi(self) -> Optional[Tuple[PypiPackageData, PypiPackageEnvironmentData]]:
        """
        Returns this instance as a tuple of [`PypiPackageData`] & [`PypiPackageEnvironmentData`]
        if this instance represents a pypi package.
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
        """
        return self._package.is_conda

    @property
    def is_pypi(self) -> bool:
        """
        Returns true if this package represents a conda package.
        """
        return self._package.is_pypi

    @property
    def name(self) -> str:
        """
        Returns the name of the package.
        """
        return self._package.name

    @property
    def url(self) -> str:
        """
        Returns the URL of the package
        """
        return self._package.url

    @property
    def version(self) -> str:
        """
        Returns the version string of the package.
        """
        return self._package.version

    @classmethod
    def _from_py_lock_package(cls, lock_pkg: PyLockPackage) -> LockPackage:
        """
        Construct Rattler LockPackage from FFI PyLockPackage object.
        """
        pkg = cls.__new__(cls)
        pkg._package = lock_pkg
        return pkg

    def __repr__(self) -> str:
        """
        Returns a representation of the LockPackage.
        """
        return "LockPackage()"
