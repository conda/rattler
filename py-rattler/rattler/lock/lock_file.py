from __future__ import annotations
import os
from typing import List, Optional, Tuple
from rattler.lock.environment import Environment

from rattler.rattler import PyLockFile


class LockFile:
    """
    Represents a lock-file for both Conda packages and Pypi packages.
    Lock-files can store information for multiple platforms and for multiple environments.
    """

    _lock_file: PyLockFile

    @staticmethod
    def from_path(path: os.PathLike[str]) -> LockFile:
        """
        Parses a rattler-lock file from a file.

        Examples
        --------
        ```python
        >>> lock_file = LockFile.from_path("./pixi.lock")
        >>> lock_file
        LockFile()
        >>>
        ```
        """
        return LockFile._from_py_lock_file(PyLockFile.from_path(path))

    def to_path(self, path: os.PathLike[str]) -> None:
        """
        Writes the rattler-lock to a file.

        Examples
        --------
        ```python
        >>> lock_file = LockFile()
        >>> lock_file.to_path("/tmp/test.lock")
        >>>
        ```
        """
        return self._lock_file.to_path(path)

    def environments(self) -> List[Tuple[str, Environment]]:
        """
        Returns an iterator over all environments defined in the lock-file.

        Examples
        --------
        ```python
        >>> from rattler import LockFileChannelConfig, Channel, ChannelConfig
        >>> lock_file = LockFile()
        >>> lock_file.environments()
        []
        >>> default_channel_config = LockFileChannelConfig("default", [Channel("conda-forge").to_lock_channel()])
        >>> test_channel_config = LockFileChannelConfig("test", [Channel("conda-forge", ChannelConfig("http://localhost:8912/")).to_lock_channel()])
        >>> lock_file2 = LockFile([default_channel_config, test_channel_config])
        >>> lock_file2.environments()
        [('default', Environment()), ('test', Environment())]
        >>>
        ```
        """
        return [
            (name, Environment._from_py_environment(e))
            for (name, e) in self._lock_file.environments()
        ]

    def environment(self, name: str) -> Optional[Environment]:
        """
        Returns the environment with the given name.

        Examples
        --------
        ```python
        >>> from rattler import LockFileChannelConfig, Channel, ChannelConfig
        >>> default_channel_config = LockFileChannelConfig("default", [Channel("conda-forge").to_lock_channel()])
        >>> test_channel_config = LockFileChannelConfig("test", [Channel("conda-forge", ChannelConfig("http://localhost:8912/")).to_lock_channel()])
        >>> lock_file = LockFile([default_channel_config, test_channel_config])
        >>> lock_file.environment("test")
        Environment()
        >>> lock_file.environment("doesnt-exist")
        >>>
        ```
        """
        if env := self._lock_file.environment(name):
            return Environment._from_py_environment(env)
        return None

    def default_environment(self) -> Optional[Environment]:
        """
        Returns the environment with the default name as defined by [`DEFAULT_ENVIRONMENT_NAME`].

        Examples
        --------
        ```python
        >>> from rattler import LockFileChannelConfig, Channel, ChannelConfig
        >>> default_channel_config = LockFileChannelConfig("default", [Channel("conda-forge").to_lock_channel()])
        >>> test_channel_config = LockFileChannelConfig("test", [Channel("conda-forge", ChannelConfig("http://localhost:8912/")).to_lock_channel()])
        >>> lock_file = LockFile([default_channel_config, test_channel_config])
        >>> lock_file.default_environment()
        Environment()
        >>>
        ```
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

    def __repr__(self) -> str:
        """
        Returns a representation of the LockFile.

        Examples
        --------
        ```python
        >>> lock_file = LockFile()
        >>> lock_file
        LockFile()
        >>>
        ```
        """
        return "LockFile()"
