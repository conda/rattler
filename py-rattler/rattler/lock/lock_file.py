from __future__ import annotations
import os
from typing import List, Optional, Tuple
from rattler.lock.config import (
    CondaPackageConfig,
    LockFileChannelConfig,
    PypiPackageConfig,
)
from rattler.lock.environment import Environment

from rattler.rattler import PyLockFile


class LockFile:
    """
    Represents a lock-file for both Conda packages and Pypi packages.
    Lock-files can store information for multiple platforms and for multiple environments.
    """

    _lock_file: PyLockFile

    def __init__(
        self,
        channels: Optional[List[LockFileChannelConfig]] = None,
        conda_packages: Optional[List[CondaPackageConfig]] = None,
        pypi_packages: Optional[List[PypiPackageConfig]] = None,
    ) -> None:
        self._lock_file = PyLockFile(
            [c._inner for c in channels or []],
            [pkg._inner for pkg in conda_packages or []],
            [pkg._inner for pkg in pypi_packages or []],
        )

    @staticmethod
    def from_path(path: os.PathLike[str]) -> LockFile:
        """
        Parses a rattler-lock file from a file.
        """
        return LockFile._from_py_lock_file(PyLockFile.from_path(path))

    def to_path(self, path: os.PathLike[str]) -> None:
        """
        Writes the rattler-lock to a file.
        """
        return self._lock_file.to_path(path)

    def environments(self) -> List[Tuple[str, Environment]]:
        """
        Returns an iterator over all environments defined in the lock-file.
        """
        return [
            (name, Environment._from_py_environment(e))
            for (name, e) in self._lock_file.environments()
        ]

    def environment(self, name: str) -> Optional[Environment]:
        """
        Returns the environment with the given name.
        """
        return Environment._from_py_environment(self._lock_file.environment(name))

    def default_environment(self) -> Optional[Environment]:
        """
        Returns the environment with the default name as defined by [`DEFAULT_ENVIRONMENT_NAME`].
        """
        return Environment._from_py_environment(self._lock_file.default_environment())

    @classmethod
    def _from_py_lock_file(cls, py_lock_file: PyLockFile) -> LockFile:
        """
        Construct Rattler LockFile from FFI PyLockFile object.
        """
        lock_file = cls.__new__(cls)
        lock_file._lock_file = py_lock_file
        return lock_file
