from __future__ import annotations

from typing import Any, Literal, Optional, Union

from rattler.package.package_name import PackageName
from rattler.rattler import PyDependent
from rattler.repo_data.package_record import PackageRecord
from rattler.repo_data.record import RepoDataRecord
from rattler.virtual_package.generic import GenericVirtualPackage

DependencyKind = Literal["depends", "constrains", "extra_depends", "run_export"]
RunExportKind = Literal["weak", "strong", "noarch", "weak_constrains", "strong_constrains"]


class Dependent:
    """
    A record that references the package queried through
    `Gateway.who_needs`.
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

    @property
    def extra(self) -> Optional[str]:
        """
        The name of the optional feature the dependency comes from, for
        `extra_depends` kinds. The reference only applies when that extra
        is enabled.
        """
        return self._dependent.extra

    @classmethod
    def _from_py_dependent(cls, py_dependent: PyDependent) -> Dependent:
        dependent = cls.__new__(cls)
        dependent._dependent = py_dependent
        return dependent

    def __repr__(self) -> str:
        return self._dependent.__repr__()


def _target_to_py(
    target: Union[str, PackageName, PackageRecord, GenericVirtualPackage],
) -> Any:
    """Converts a who_needs target into its inner PyO3 object."""
    if isinstance(target, str):
        target = PackageName(target)

    if isinstance(target, PackageName):
        return target._name
    if isinstance(target, GenericVirtualPackage):
        return target._generic_virtual_package
    if isinstance(target, PackageRecord):
        return target._record
    raise TypeError(
        "expected a str, PackageName, PackageRecord, or GenericVirtualPackage "
        f"as the target, not {type(target).__name__}"
    )
