from __future__ import annotations
from dataclasses import dataclass
from typing import List

from rattler.lock.channel import LockChannel
from rattler.lock.pypi import PypiPackageData, PypiPackageEnvironmentData
from rattler.platform.platform import Platform
from rattler.repo_data.record import RepoDataRecord

from rattler.rattler import (
    PyLockFileChannelConfig,
    PyCondaPackageConfig,
    PyPypiPackageConfig,
)


class LockFileChannelConfig:
    """
    A config class used to set channels in lockfiles.
    """

    _inner: PyLockFileChannelConfig

    def __init__(self, env: str, channels: List[LockChannel]) -> None:
        self._inner = PyLockFileChannelConfig(env, [c._channel for c in channels])


class CondaPackageConfig:
    """
    A config data class used to set conda packages in lockfiles.
    """

    _inner: PyCondaPackageConfig

    def __init__(
        self, env: str, platform: Platform, locked_package: RepoDataRecord
    ) -> None:
        self._inner = PyCondaPackageConfig(env, platform._inner, locked_package._record)


@dataclass
class PypiPackageConfig:
    """
    A config data class used to set pypi packages in lockfiles.
    """

    _inner: PyPypiPackageConfig

    def __init__(
        self,
        env: str,
        platform: Platform,
        locked_package: PypiPackageData,
        env_data: PypiPackageEnvironmentData,
    ) -> None:
        self._inner = PyPypiPackageConfig(
            env, platform._inner, locked_package._data, env_data._data
        )
