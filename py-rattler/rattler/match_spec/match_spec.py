from __future__ import annotations
from typing import TYPE_CHECKING

from rattler.rattler import PyMatchSpec

if TYPE_CHECKING:
    from rattler.match_spec import NamelessMatchSpec
    from rattler.repo_data import PackageRecord


class MatchSpec:
    """
    A `MatchSpec` is a query language for conda packages.
    It can be composed of any of the attributes of `PackageRecord`.

    `MatchSpec` can be composed of keyword arguments, where keys are
    any of the attributes of `PackageRecord`. Values for keyword arguments
    are exact values the attributes should match against. Many fields can
    be matched against non-exact values by including wildcard `*` and `>`/`<`
    ranges where supported. Any non-specified field is the equivalent of a
    full wildcard match.

    MatchSpecs can also be composed using a single positional argument, with optional
    keyword arguments. Keyword arguments also override any conflicting information
    provided in the positional argument. Conda has historically had several string
    representations for equivalent MatchSpecs.

    A series of rules are now followed for creating the canonical string
    representation of a MatchSpec instance. The canonical string representation can
    generically be represented by:

    `(channel(/subdir):(namespace):)name(version(build))[key1=value1,key2=value2]`

    where `()` indicate optional fields.

    The rules for constructing a canonical string representation are:

    1. `name` (i.e. "package name") is required, but its value can be '*'. Its
    position is always outside the key-value brackets.
    2. If `version` is an exact version, it goes outside the key-value brackets and
    is prepended by `==`. If `version` is a "fuzzy" value (e.g. `1.11.*`), it goes
    outside the key-value brackets with the `.*` left off and is prepended by `=`.
    Otherwise `version` is included inside key-value brackets.
    3. If `version` is an exact version, and `build` is an exact value, `build` goes
    outside key-value brackets prepended by a `=`.  Otherwise, `build` goes inside
    key-value brackets. `build_string` is an alias for `build`.
    4. The `namespace` position is being held for a future feature. It is currently
    ignored.
    5. If `channel` is included and is an exact value, a `::` separator is used between
    `channel` and `name`.  `channel` can either be a canonical channel name or a
    channel url. In the canonical string representation, the canonical channel name
    will always be used.
    6. If `channel` is an exact value and `subdir` is an exact value, `subdir` is
    appended to `channel` with a `/` separator.  Otherwise, `subdir` is included in
    the key-value brackets.
    7. Key-value brackets can be delimited by comma, space, or comma+space. Value can
    optionally be wrapped in single or double quotes, but must be wrapped if `value`
    contains a comma, space, or equal sign.  The canonical format uses comma delimiters
    and single quotes.
    8. When constructing a `MatchSpec` instance from a string, any key-value pair given
    inside the key-value brackets overrides any matching parameter given outside the
    brackets.

    When `MatchSpec` attribute values are simple strings, the are interpreted using the
    following conventions:
    - If the string begins with `^` and ends with `$`, it is converted to a regex.
    - If the string contains an asterisk (`*`), it is transformed from a glob to a
    regex.
    - Otherwise, an exact match to the string is sought.

    To fully-specify a package with a full, exact spec, the following fields must be
    given as exact values:
    - channel
    - subdir
    - name
    - version
    - build
    """

    def __init__(self, spec: str) -> None:
        if isinstance(spec, str):
            self._match_spec = PyMatchSpec(spec)
        else:
            raise TypeError(
                "MatchSpec constructor received unsupported type"
                f" {type(spec).__name__!r} for the 'spec' parameter"
            )

    @classmethod
    def _from_py_match_spec(cls, py_match_spec: PyMatchSpec) -> MatchSpec:
        """
        Construct py-rattler MatchSpec from PyMatchSpec FFI object.
        """
        match_spec = cls.__new__(cls)
        match_spec._match_spec = py_match_spec

        return match_spec

    def matches(self, record: PackageRecord) -> bool:
        """Match a MatchSpec against a PackageRecord."""
        return self._match_spec.matches(record._package_record)

    @classmethod
    def from_nameless(cls, spec: NamelessMatchSpec, name: str) -> MatchSpec:
        """
        Constructs a MatchSpec from a NamelessMatchSpec
        and a name.

        Examples
        --------
        ```python
        >>> from rattler import NamelessMatchSpec
        >>> spec = NamelessMatchSpec('3.4')
        >>> MatchSpec.from_nameless(spec, "foo")
        MatchSpec("foo ==3.4")
        >>> MatchSpec.from_nameless(spec, "$foo") # doctest: +IGNORE_EXCEPTION_DETAIL
        Traceback (most recent call last):
        exceptions.InvalidPackageNameException
        >>>
        ```
        """
        return cls._from_py_match_spec(
            PyMatchSpec.from_nameless(spec._nameless_match_spec, name)
        )

    def __str__(self) -> str:
        """
        Returns a string representation of the MatchSpec.

        Examples
        --------
        ```python
        >>> from rattler import NamelessMatchSpec
        >>> spec = NamelessMatchSpec('3.4')
        >>> str(MatchSpec.from_nameless(spec, "foo"))
        'foo ==3.4'
        >>>
        ```
        """
        return self._match_spec.as_str()

    def __repr__(self) -> str:
        """
        Returns a representation of the MatchSpec.

        Examples
        --------
        ```python
        >>> from rattler import NamelessMatchSpec
        >>> spec = NamelessMatchSpec('3.4')
        >>> MatchSpec.from_nameless(spec, "foo")
        MatchSpec("foo ==3.4")
        >>>
        ```
        """
        return f'MatchSpec("{self._match_spec.as_str()}")'
