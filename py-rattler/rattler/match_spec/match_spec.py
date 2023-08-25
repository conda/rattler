from __future__ import annotations

from rattler.match_spec import NamelessMatchSpec
from rattler.rattler import PyMatchSpec
from rattler.repo_data import PackageRecord


class MatchSpec:
    def __init__(self, spec: str):
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

    @staticmethod
    def from_nameless(spec: NamelessMatchSpec, name: str) -> MatchSpec:
        """
        Constructs a MatchSpec from a NamelessMatchSpec
        and a name.
        """
        return MatchSpec._from_py_match_spec(
            PyMatchSpec.from_nameless(spec._nameless_match_spec, name)
        )

    def __str__(self) -> str:
        return self._match_spec.as_str()

    def __repr__(self) -> str:
        return self.__str__()
