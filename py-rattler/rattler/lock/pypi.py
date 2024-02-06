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
        """
        return self._data.satisfies(spec)

    @property
    def name(self) -> str:
        """
        The name of the package.
        """
        return self._data.name

    @property
    def version(self) -> str:
        """
        The version of the package.
        """
        return self._data.version

    @property
    def url(self) -> str:
        """
        The URL that points to where the artifact can be downloaded from.
        """
        return self._data.url

    @property
    def hash(self) -> Optional[PackageHashes]:
        """
        Hashes of the file pointed to by `url`.
        """
        return PackageHashes._from_py_package_hashes(self._data.hash)

    @property
    def requires_dist(self) -> List[str]:
        """
        A list of dependencies on other packages.
        """
        return self._data.requires_dist

    @property
    def requires_python(self) -> Optional[str]:
        """
        The python version that this package requires.
        """
        return self._data.requires_python

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
        """
        return self._data.extras

    @classmethod
    def _from_py_pypi_env_data(
        cls, env_data: PyPypiPackageEnvironmentData
    ) -> PypiPackageEnvironmentData:
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
