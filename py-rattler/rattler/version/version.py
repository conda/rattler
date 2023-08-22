from __future__ import annotations

from typing import Optional, Tuple

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

    @property
    def local(self) -> bool:
        """
        Returns true if this version has a local segment defined.

        Examples
        --------
        >>> v = Version('1.0')
        >>> v.local
        False
        """
        return self._version.has_local()

    def as_major_minor(self) -> Optional[Tuple[int, int]]:
        """
        Returns the major and minor segments from the version.

        Examples
        --------
        >>> v = Version('1.0')
        >>> v.as_major_minor()
        (1, 0)
        """
        return self._version.as_major_minor()

    @property
    def dev(self) -> bool:
        """
        Returns true if the version contains a component name "dev".

        Examples
        --------
        >>> v = Version('1.0.1dev')
        >>> v.dev
        True
        """
        return self._version.is_dev()

    def starts_with(self, other: Version) -> bool:
        """
        Checks if the version and local segment start
        same as other version.

        Examples
        --------
        >>> v1 = Version('1.0.1')
        >>> v2 = Version('1.0')
        >>> v1.starts_with(v2)
        True
        """
        return self._version.starts_with(other._version)

    def compatible_with(self, other: Version) -> bool:
        """
        Checks if this version is compatible with other version.
        """
        return self._version.compatible_with(other._version)

    def pop_segments(self, n: int = 1) -> Optional[Version]:
        """
        Pops `n` number of segments from the version and returns
        the new version. Returns `None` if the version becomes
        invalid due to the operation.
        """
        new_py_version = self._version.pop_segments(n)
        if new_py_version:
            # maybe it should raise an exception instead?
            return self._from_py_version(new_py_version)

    def with_segments(self, start: int, stop: int) -> Optional[Version]:
        """
        Returns new version with with segments ranging from `start` to `stop`.
        `stop` is exclusive.
        """
        new_py_version = self._version.with_segments(start, stop)
        if new_py_version:
            return self._from_py_version(new_py_version)
        else:
            # maybe it should raise an exception instead?
            return None

    def segment_count(self) -> int:
        """
        Returns the number of segments in the version.
        """
        return self._version.segment_count()

    def strip_local(self) -> Version:
        """
        Returns a new version with local segment stripped.
        """
        return self._from_py_version(self._version.strip_local())

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
