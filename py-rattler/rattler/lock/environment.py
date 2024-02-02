from __future__ import annotations
import os
from typing import Dict, List, Optional, Self, Tuple
from rattler.platform.platform import Platform

from rattler.rattler import PyEnvironment
from rattler.repo_data.record import RepoDataRecord


class Environment:
    """
    Information about a specific environment in the lock-file.
    """

    _env: PyEnvironment

    def platforms(self) -> List[Platform]:
        """
        Returns all the platforms for which we have a locked-down environment.
        """
        return [Platform._from_py_platform(p) for p in self._env.platforms()]

    def channels(self) -> List[LockChannel]:
        """
        Returns the channels that are used by this environment.
        Note that the order of the channels is significant.
        The first channel is the highest priority channel.
        """
        return [LockChannel._from_py_lock_channel(c) for c in self._env.channels()]

    def packages(self, platform: Platform) -> Optional[List[LockPackage]]:
        """
        Returns all the packages for a specific platform in this environment.
        """
        if packages := self._env.packages(platform._inner):
            return [LockPackage._from_py_lock_package(p) for p in packages]
        return None

    def packages_by_platform(self) -> List[Tuple[Platform, List[LockPackage]]]:
        """
        Returns a list of all packages and platforms defined for this environment.
        """
        return [
            (
                Platform._from_py_platform(platform),
                [LockPackage._from_py_lock_package(p) for p in packages],
            )
            for (platform, packages) in self._env.packages_by_platform()
        ]

    def pypi_packages(
        self,
    ) -> Dict[str, List[Tuple[PypiPackageData, PypiPackageEnvironmentData]]]:
        """
        Returns all pypi packages for all platforms.
        """
        return {
            str(Platform._from_py_platform(platform)): [
                (
                    PypiPackageData._from_py_pypi_package_data(pkg_data),
                    PypiPackageEnvironmentData._from_py_pypi_package_environment_data(
                        env_data
                    ),
                )
                for (pkg_data, env_data) in pypi_tup
            ]
            for (platform, pypi_tup) in self._env.pypi_packages().items()
        }

    def conda_repodata_records(self) -> Dict[str, List[RepoDataRecord]]:
        """
        Returns all conda packages for all platforms.
        """
        return { self._env.conda_repodata_records() }

    @classmethod
    def _from_py_environment(cls, py_environment: PyEnvironment) -> Self:
        """
        Construct Rattler Environment from FFI PyEnvironment object.
        """
        env = cls.__new__(cls)
        env._env = py_environment
        return env
