from __future__ import annotations

from typing import List, Literal, Optional, Sequence, Union

from rattler.package.package_name import PackageName
from rattler.rattler import PyDependent, py_who_needs
from rattler.repo_data.package_record import PackageRecord
from rattler.repo_data.record import RepoDataRecord
from rattler.virtual_package.generic import GenericVirtualPackage

DependencyKind = Literal["depends", "constrains", "run_export"]
RunExportKind = Literal["weak", "strong", "noarch", "weak_constrains", "strong_constrains"]


class Dependent:
    """
    A record that references the package queried through `who_needs`.
    """

    _dependent: PyDependent

    @property
    def record(self) -> RepoDataRecord:
        """The record that references the queried package."""
        return RepoDataRecord._from_py_record(self._dependent.record)

    @property
    def dependency(self) -> str:
        """
        The dependency string through which the record references the
        queried package.
        """
        return self._dependent.dependency

    @property
    def kind(self) -> DependencyKind:
        """The field of the record the dependency comes from."""
        return self._dependent.kind

    @property
    def run_export_kind(self) -> Optional[RunExportKind]:
        """
        The run export field the dependency comes from, for `run_export`
        kinds.
        """
        return self._dependent.run_export_kind

    @classmethod
    def _from_py_dependent(cls, py_dependent: PyDependent) -> Dependent:
        dependent = cls.__new__(cls)
        dependent._dependent = py_dependent
        return dependent

    def __repr__(self) -> str:
        return self._dependent.__repr__()


def who_needs(
    records: Sequence[RepoDataRecord],
    target: Union[str, PackageName, PackageRecord, GenericVirtualPackage],
) -> List[Dependent]:
    """
    Returns all records in `records` that reference the package described
    by `target` - its reverse dependencies.

    A record references the target when one of its `depends` or
    `constrains` entries, or one of its run exports, matches the target;
    the `kind` of each result tells which field matched, so callers
    interested in only some of the kinds can simply filter the result.

    How dependencies are matched depends on the target: a package name (or
    `str`) reports every record with a dependency entry on that name,
    while a concrete `PackageRecord` or `GenericVirtualPackage` only
    reports dependents whose dependency match spec matches it.

    Note that reverse dependency lookup requires the *complete* set of
    records of the queried channels and platforms - any record not passed
    in here is invisible to the search. Use a wildcard `Gateway` query
    (spec `*`) to obtain them.

    Examples
    --------
    ```python
    >>> from rattler import PackageRecord, RepoDataRecord, who_needs
    >>> def record(name, version, build, depends):
    ...     return RepoDataRecord(
    ...         PackageRecord(
    ...             name=name,
    ...             version=version,
    ...             build=build,
    ...             build_number=0,
    ...             subdir="linux-64",
    ...             depends=depends,
    ...         ),
    ...         f"{name}-{version}-{build}.conda",
    ...         f"https://example.com/{name}-{version}-{build}.conda",
    ...         "https://example.com/test-channel",
    ...     )
    >>> records = [
    ...     record("python", "3.13.0", "0", []),
    ...     record("old-lib", "1.0.0", "0", ["python >=3.8,<3.10"]),
    ...     record("numpy", "2.1.0", "0", ["python >=3.10"]),
    ... ]
    >>> [d.record.name.normalized for d in who_needs(records, "python")]
    ['old-lib', 'numpy']
    >>> python = record("python", "3.13.1", "h123_0", [])
    >>> [d.record.name.normalized for d in who_needs(records, python)]
    ['numpy']
    >>>
    ```
    """
    if isinstance(target, str):
        target = PackageName(target)

    if isinstance(target, PackageName):
        py_target = target._name
    elif isinstance(target, GenericVirtualPackage):
        py_target = target._generic_virtual_package
    elif isinstance(target, PackageRecord):
        py_target = target._record
    else:
        raise TypeError(
            "expected a str, PackageName, PackageRecord, or GenericVirtualPackage "
            f"as the target, not {type(target).__name__}"
        )

    return [
        Dependent._from_py_dependent(py_dependent)
        for py_dependent in py_who_needs([record._record for record in records], py_target)
    ]
