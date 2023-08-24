from __future__ import annotations

from typing import Optional

from rattler.rattler import PyVersion


class Version:
    def __init__(self, version: str):
        if isinstance(version, str):
            self._version = PyVersion(version)
        else:
            raise TypeError(
                "Version constructor received unsupported type "
                f" {type(version).__name__!r} for the `version` parameter"
            )

    @classmethod
    def _from_py_version(cls, py_version: PyVersion) -> Version:
        """Construct Rattler version from FFI PyVersion object."""
        version = cls.__new__(cls)
        version._version = py_version
        return version

    def __str__(self) -> str:
        return self._version.as_str()

    def __repr__(self) -> str:
        return self.__str__()

    @property
    def epoch(self) -> Optional[str]:
        """
        Gets the epoch of the version or `None` if the epoch was not defined.

        Examples
        --------
        >>> v = Version('2!1.0')
        >>> v.epoch
        2
        """
        return self._version.epoch()

    def bump(self) -> Version:
        """
        Returns a new version where the last numerical segment of this version has
        been bumped.

        Examples
        --------
        >>> v = Version('1.0')
        >>> v.bump()
        1.1
        """
        return Version._from_py_version(self._version.bump())

    def __eq__(self, other: Version) -> bool:
        return self._version.equals(other._version)

    def __ne__(self, other: Version) -> bool:
        return self._version.not_equal(other._version)

    def __gt__(self, other: Version) -> bool:
        return self._version.greater_than(other._version)

    def __lt__(self, other: Version) -> bool:
        return self._version.less_than(other._version)

    def __ge__(self, other: Version) -> bool:
        return self._version.greater_than_equals(other._version)

    def __le__(self, other: Version) -> bool:
        return self._version.less_than_equals(other._version)
