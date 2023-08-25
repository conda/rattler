from __future__ import annotations

from typing import List, Optional, Tuple

from rattler.rattler import PyVersion, InvalidVersionError


class Version:
    """
    This class implements an order relation between version strings.
    Version strings can contain the usual alphanumeric characters
    (A-Za-z0-9), separated into segments by dots and underscores.
    Empty segments (i.e. two consecutive dots, a leading/trailing
    underscore) are not permitted. An optional epoch number - an
    integer followed by '!' - can precede the actual version string
    (this is useful to indicate a change in the versioning scheme itself).
    Version comparison is case-insensitive.
    """

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
    def has_local(self) -> bool:
        """
        Returns true if this version has a local segment defined.
        The local part of a version is the part behind the (optional) `+`.

        Examples
        --------
        >>> v = Version('1.0+3.2-alpha0')
        >>> v.has_local
        True
        >>> v2 = Version('1.0')
        >>> v.has_local
        False
        """
        return self._version.has_local()

    def segments(self) -> List[List[str]]:
        """
        Returns a list of segments of the version. It does not contain
        the local segment of the version.

        Examples
        --------
        >>> v = Version("1.2dev.3-alpha4.5+6.8")
        >>> v.segments()
        [['1'], ['2', 'dev'], ['3'], ['0', 'alpha', '4'], ['5']]
        """
        return self._version.segments()

    def local_segments(self) -> List[List[str]]:
        """
        Returns a list of local segments of the version. It does not
        contain the non-local segment of the version.

        Examples
        --------
        >>> v = Version("1.2dev.3-alpha4.5+6.8")
        >>> v.local_segments()
        [['6'], ['8']]
        """
        return self._version.local_segments()

    def as_major_minor(self) -> Optional[Tuple[int, int]]:
        """
        Returns the major and minor segments from the version.
        Requires a minimum of 2 segments in version to be split
        into major and minor, returns `None` otherwise.

        Examples
        --------
        >>> v = Version('1.0')
        >>> v.as_major_minor()
        (1, 0)
        """
        return self._version.as_major_minor()

    @property
    def is_dev(self) -> bool:
        """
        Returns true if the version contains a component name "dev",
        dev versions are sorted before non-dev version.

        Examples
        --------
        >>> v = Version('1.0.1dev')
        >>> v.is_dev
        True
        >>> v_non_dev = Version('1.0.1')
        >>> v_non_dev >= v
        False
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
        Minor versions changes are compatible with older versions,
        major version changes are breaking and will not be compatible.

        Examples
        --------
        >>> v1 = Version('1.0')
        >>> v2 = Version('1.2')
        >>> v_major = Version('2.0')
        >>> v1.compatible_with(v2)
        False
        >>> v2.compatible_with(v1)
        True
        >>> v_major.compatible_with(v2)
        False
        >>> v2.compatible_with(v_major)
        False
        """
        return self._version.compatible_with(other._version)

    def pop_segments(self, n: int = 1) -> Version:
        """
        Pops `n` number of segments from the version and returns
        the new version. Raises `InvalidVersionError` if version
        becomes invalid due to the operation.

        Examples
        --------
        >>> v = Version('2!1.0.1')
        >>> v.pop_segments() # `n` defaults to 1 if left empty
        2!1.0
        >>> v.pop_segments(2) # old version is still usable
        2!1
        >>> v.pop_segments(3) # doctest: +IGNORE_EXCEPTION_DETAIL
        Traceback (most recent call last):
        exceptions.InvalidVersionException: new Version must have atleast 1 valid
        segment
        """
        new_py_version = self._version.pop_segments(n)
        if new_py_version:
            return self._from_py_version(new_py_version)
        else:
            raise InvalidVersionError("new Version must have atleast 1 valid segment")

    def with_segments(self, start: int, stop: int) -> Version:
        """
        Returns new version with with segments ranging from `start` to `stop`.
        `stop` is exclusive. Raises `InvalidVersionError` if the provided range
        is invalid.

        Examples
        --------
        >>> v = Version('2!1.2.3')
        >>> v.with_segments(0, 2)
        2!1.2
        """
        new_py_version = self._version.with_segments(start, stop)
        if new_py_version:
            return self._from_py_version(new_py_version)
        else:
            raise InvalidVersionError("Invalid segment range provided")

    @property
    def segment_count(self) -> int:
        """
        Returns the number of segments in the version.
        This does not include epoch or local segment of the version

        Examples
        --------
        >>> v = Version('2!1.2.3')
        >>> v.segment_count
        3
        """
        return self._version.segment_count()

    def strip_local(self) -> Version:
        """
        Returns a new version with local segment stripped.

        Examples
        --------
        >>> v = Version('1.2.3+4.alpha-5')
        >>> v.strip_local()
        1.2.3
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
