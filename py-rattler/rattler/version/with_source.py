from __future__ import annotations

from typing import List, Optional, Tuple, Union

from rattler.version import Version

from rattler.rattler import PyVersionWithSource, InvalidVersionError


class VersionWithSource:
    """
    Holds a version and the string it was created from. This is useful if
    you want to retain the original string the version was created from.
    This might be useful in cases where you have multiple strings that
    are represented by the same [`Version`] but you still want to be able to
    distinguish them.

    The string `1.0` and `1.01` represent the same version. When you print
    the parsed version though it will come out as `1.0`. You loose the
    original representation. This struct stores the original source string.
    """

    def __init__(self, source: str, version: Optional[Version] = None):
        if isinstance(source, str):
            if version is None:
                version = Version(source)

            if isinstance(version, Version):
                # maybe we should use _inner for inner FFI objects everywhere?
                self._inner = PyVersionWithSource(version._version, source)
            else:
                raise TypeError(
                    "VersionWithSource constructor received unsupported type "
                    f" {type(version).__name__!r} for the `version` parameter"
                )
        else:
            raise TypeError(
                "VersionWithSource constructor received unsupported type "
                f" {type(version).__name__!r} for the `source` parameter"
            )

    @property
    def version(self) -> Version:
        """
        Returns the `Version` from current object.

        Examples
        --------
        >>> v = VersionWithSource("1.0.0")
        >>> v.version
        Version("1.0.0")
        >>> v2 = VersionWithSource("1.0.0", v.version)
        >>> v2.version
        Version("1.0.0")
        """
        return Version._from_py_version(self._inner.version())

    @property
    def epoch(self) -> Optional[int]:
        """
        Gets the epoch of the version or `None` if the epoch was not defined.

        Examples
        --------
        >>> v = VersionWithSource('2!1.0')
        >>> v.epoch
        2
        >>> v2 = VersionWithSource('2!1.0', v.version)
        >>> v2.epoch
        2
        """
        return self._inner.epoch()

    def bump(self) -> VersionWithSource:
        """
        Returns a new version where the last numerical segment of this version has
        been bumped.

        Examples
        --------
        >>> v = VersionWithSource('1.0')
        >>> v.bump()
        VersionWithSource("1.1")
        >>> v2 = VersionWithSource('1.0', v.version)
        >>> v2.bump()
        VersionWithSource("1.1")
        """
        return VersionWithSource._from_py_version_with_source(self._inner.bump())

    @property
    def has_local(self) -> bool:
        """
        Returns true if this version has a local segment defined.
        The local part of a version is the part behind the (optional) `+`.

        Examples
        --------
        >>> v = VersionWithSource('1.0+3.2-alpha0')
        >>> v.has_local
        True
        >>> v2 = VersionWithSource('1.0')
        >>> v2.has_local
        False
        >>> v3 = VersionWithSource('1.0+3.2-alpha0', v.version)
        >>> v3.has_local
        True
        >>> v4 = VersionWithSource('1.0', v2.version)
        >>> v4.has_local
        False
        """
        return self._inner.has_local()

    def segments(self) -> List[List[Union[str, int]]]:
        """
        Returns a list of segments of the version. It does not contain
        the local segment of the version.

        Examples
        --------
        >>> v = VersionWithSource("1.2dev.3-alpha4.5+6.8")
        >>> v.segments()
        [[1], [2, 'dev'], [3], [0, 'alpha', 4], [5]]
        >>> v2 = VersionWithSource("1.2dev.3-alpha4.5+6.8", v.version)
        >>> v2.segments()
        [[1], [2, 'dev'], [3], [0, 'alpha', 4], [5]]
        """
        return self._inner.segments()

    def local_segments(self) -> List[List[Union[str, int]]]:
        """
        Returns a list of local segments of the version. It does not
        contain the non-local segment of the version.

        Examples
        --------
        >>> v = VersionWithSource("1.2dev.3-alpha4.5+6.8")
        >>> v.local_segments()
        [[6], [8]]
        >>> v2 = VersionWithSource("1.2dev.3-alpha4.5+6.8", v.version)
        >>> v2.local_segments()
        [[6], [8]]
        """
        return self._inner.local_segments()

    def as_major_minor(self) -> Optional[Tuple[int, int]]:
        """
        Returns the major and minor segments from the version.
        Requires a minimum of 2 segments in version to be split
        into major and minor, returns `None` otherwise.

        Examples
        --------
        >>> v = VersionWithSource('1.0')
        >>> v.as_major_minor()
        (1, 0)
        >>> v2 = VersionWithSource('1.0', v.version)
        >>> v2.as_major_minor()
        (1, 0)
        """
        return self._inner.as_major_minor()

    @property
    def is_dev(self) -> bool:
        """
        Returns true if the version contains a component name "dev",
        dev versions are sorted before non-dev version.

        Examples
        --------
        >>> v = VersionWithSource('1.0.1dev')
        >>> v.is_dev
        True
        >>> v_non_dev = VersionWithSource('1.0.1')
        >>> v_non_dev >= v
        True
        >>> v2 = VersionWithSource('1.0.1dev', v.version)
        >>> v2.is_dev
        True
        >>> v2_non_dev = VersionWithSource('1.0.1', v_non_dev.version)
        >>> v2_non_dev >= v2
        True
        """
        return self._inner.is_dev()

    def starts_with(self, other: VersionWithSource) -> bool:
        """
        Checks if the version and local segment start
        same as other version.

        Examples
        --------
        >>> v1 = VersionWithSource('1.0.1')
        >>> v2 = VersionWithSource('1.0')
        >>> v1.starts_with(v2)
        True
        >>> v3 = VersionWithSource('1.0.1', v1.version)
        >>> v4 = VersionWithSource('1.0', v2.version)
        >>> v3.starts_with(v4)
        True
        """
        return self._inner.starts_with(other._inner)

    def compatible_with(self, other: VersionWithSource) -> bool:
        """
        Checks if this version is compatible with other version.
        Minor versions changes are compatible with older versions,
        major version changes are breaking and will not be compatible.

        Examples
        --------
        >>> v1 = VersionWithSource('1.0')
        >>> v2 = VersionWithSource('1.2')
        >>> v_major = VersionWithSource('2.0')
        >>> v1.compatible_with(v2)
        False
        >>> v2.compatible_with(v1)
        True
        >>> v_major.compatible_with(v2)
        False
        >>> v2.compatible_with(v_major)
        False
        """
        return self._inner.compatible_with(other._inner)

    def pop_segments(self, n: int = 1) -> VersionWithSource:
        """
        Pops `n` number of segments from the version and returns
        the new version. Raises `InvalidVersionError` if version
        becomes invalid due to the operation.

        Examples
        --------
        >>> v = VersionWithSource('2!1.0.1')
        >>> v.pop_segments() # `n` defaults to 1 if left empty
        VersionWithSource("2!1.0")
        >>> v.pop_segments(2) # old version is still usable
        VersionWithSource("2!1")
        >>> v.pop_segments(3) # doctest: +IGNORE_EXCEPTION_DETAIL
        Traceback (most recent call last):
        exceptions.InvalidVersionException: new Version must have atleast 1 valid
        segment
        """
        new_py_version = self._inner.pop_segments(n)
        if new_py_version:
            return self._from_py_version_with_source(new_py_version)
        else:
            raise InvalidVersionError("new Version must have atleast 1 valid segment")

    def with_segments(self, start: int, stop: int) -> VersionWithSource:
        """
        Returns new version with with segments ranging from `start` to `stop`.
        `stop` is exclusive. Raises `InvalidVersionError` if the provided range
        is invalid.

        Examples
        --------
        >>> v = VersionWithSource('2!1.2.3')
        >>> v.with_segments(0, 2)
        VersionWithSource("2!1.2")
        """
        new_py_version = self._inner.with_segments(start, stop)
        if new_py_version:
            return self._from_py_version_with_source(new_py_version)
        else:
            raise InvalidVersionError("Invalid segment range provided")

    @property
    def segment_count(self) -> int:
        """
        Returns the number of segments in the version.
        This does not include epoch or local segment of the version

        Examples
        --------
        >>> v = VersionWithSource('2!1.2.3')
        >>> v.segment_count
        3
        """
        return self._inner.segment_count()

    def strip_local(self) -> VersionWithSource:
        """
        Returns a new version with local segment stripped.

        Examples
        --------
        >>> v = VersionWithSource('1.2.3+4.alpha-5')
        >>> v.strip_local()
        VersionWithSource("1.2.3")
        """
        return self._from_py_version_with_source(self._inner.strip_local())

    @classmethod
    def _from_py_version_with_source(
        cls, py_version_with_source: PyVersionWithSource
    ) -> VersionWithSource:
        """Construct Rattler VersionWithSource from FFI PyVersionWithSource object."""
        version = cls.__new__(cls)
        version._inner = py_version_with_source
        return version

    def __str__(self):
        """
        Returns the string representation of the version

        Examples
        --------
        >>> str(VersionWithSource("1.2.3"))
        '1.2.3'
        """
        return self._inner.as_str()

    def __repr__(self):
        """
        Returns a representation of the version

        Examples
        --------
        >>> VersionWithSource("1.2.3")
        VersionWithSource("1.2.3")
        """
        return f'{type(self).__name__}("{self._inner.as_str()}")'

    def __hash__(self) -> int:
        """
        Computes the hash of this instance.

        Examples
        --------
        >>> hash(VersionWithSource("1.2.3")) == hash(VersionWithSource("1.2.3"))
        True
        >>> hash(VersionWithSource("1.2.3")) == hash(VersionWithSource("3.2.1"))
        False
        >>> hash(VersionWithSource("1")) == hash(VersionWithSource("1.0.0"))
        False
        """
        return self._inner.__hash__()

    def __eq__(self, other: VersionWithSource) -> bool:
        """
        Returns True if this instance represents the same version as `other`.

        Examples
        --------
        >>> VersionWithSource("1.2.3") == VersionWithSource("1.2.3")
        True
        >>> VersionWithSource("3.2.1") == VersionWithSource("1.2.3")
        False
        >>> VersionWithSource("1") == VersionWithSource("1.0.0")
        False
        """
        return self._inner == other._inner

    def __ne__(self, other: VersionWithSource) -> bool:
        """
        Returns True if this instance represents the same version as `other`.

        Examples
        --------
        >>> VersionWithSource("1.2.3") != VersionWithSource("1.2.3")
        False
        >>> VersionWithSource("3.2.1") != VersionWithSource("1.2.3")
        True
        >>> VersionWithSource("1") != VersionWithSource("1.0.0")
        True
        """
        return self._inner != other._inner

    def __gt__(self, other: VersionWithSource) -> bool:
        """
        Returns True if this instance should be ordered *after* `other`.

        Examples
        --------
        >>> VersionWithSource("1.2.3") > VersionWithSource("1.2.3")
        False
        >>> VersionWithSource("1.2.4") > VersionWithSource("1.2.3")
        True
        >>> VersionWithSource("1.2.3.1") > VersionWithSource("1.2.3")
        True
        >>> VersionWithSource("3.2.1") > VersionWithSource("1.2.3")
        True
        >>> VersionWithSource("1") > VersionWithSource("1.0.0")
        False
        """
        return self._inner > other._inner

    def __lt__(self, other: VersionWithSource) -> bool:
        """
        Returns True if this instance should be ordered *before* `other`.

        Examples
        --------
        >>> VersionWithSource("1.2.3") < VersionWithSource("1.2.3")
        False
        >>> VersionWithSource("1.2.3") < VersionWithSource("1.2.4")
        True
        >>> VersionWithSource("1.2.3") < VersionWithSource("1.2.3.1")
        True
        >>> VersionWithSource("3.2.1") < VersionWithSource("1.2.3")
        False
        >>> VersionWithSource("1") < VersionWithSource("1.0.0")
        True
        """
        return self._inner < other._inner

    def __ge__(self, other: VersionWithSource) -> bool:
        """
        Returns True if this instance should be ordered *after* or at the same location
        as `other`.

        Examples
        --------
        >>> VersionWithSource("1.2.3") >= VersionWithSource("1.2.3")
        True
        >>> VersionWithSource("1.2.4") >= VersionWithSource("1.2.3")
        True
        >>> VersionWithSource("1.2.3.1") >= VersionWithSource("1.2.3")
        True
        >>> VersionWithSource("3.2.1") >= VersionWithSource("1.2.3")
        True
        >>> VersionWithSource("1.2.3") >= VersionWithSource("3.2.1")
        False
        >>> VersionWithSource("1") >= VersionWithSource("1.0.0")
        False
        """
        return self._inner >= other._inner

    def __le__(self, other: VersionWithSource) -> bool:
        """
        Returns True if this instance should be ordered *before* or at the same
        location as `other`.

        Examples
        --------
        >>> VersionWithSource("1.2.3") <= VersionWithSource("1.2.3")
        True
        >>> VersionWithSource("1.2.3") <= VersionWithSource("1.2.4")
        True
        >>> VersionWithSource("1.2.3") <= VersionWithSource("1.2.3.1")
        True
        >>> VersionWithSource("3.2.1") <= VersionWithSource("1.2.3")
        False
        >>> VersionWithSource("1") <= VersionWithSource("1.0.0")
        True
        """
        return self._inner <= other._inner
