from __future__ import annotations

from rattler.rattler import PyBuildString


class BuildString:
    """An opaque conda build string, validated according to CEP26.

    Values must contain 1–64 ASCII letters, digits, or ``_``, ``.``, ``+``.
    Invalid values raise ``ValueError``. Use :meth:`new_unchecked` explicitly
    for legacy metadata or wheel tags that do not follow CEP26.

    >>> str(BuildString("py_0"))
    'py_0'
    >>> str(BuildString.new_unchecked("py3-none-any"))
    'py3-none-any'
    """

    def __init__(self, value: str) -> None:
        self._build = PyBuildString(value)

    @classmethod
    def new_unchecked(cls, value: str) -> BuildString:
        """Construct a build string without validation, preserving the value verbatim."""
        return cls._from_py_build_string(PyBuildString.new_unchecked(value))

    @classmethod
    def _from_py_build_string(cls, value: PyBuildString) -> BuildString:
        """Wrap an existing build string without revalidating legacy metadata."""
        result = cls.__new__(cls)
        result._build = value
        return result

    @staticmethod
    def _to_py(value: str | BuildString) -> PyBuildString:
        return value._build if isinstance(value, BuildString) else PyBuildString(value)

    def __str__(self) -> str:
        return str(self._build)

    def __repr__(self) -> str:
        return f"BuildString({str(self)!r})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, BuildString):
            return NotImplemented
        return str(self) == str(other)

    def __hash__(self) -> int:
        return hash(str(self))
