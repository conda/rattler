from __future__ import annotations
import datetime
from typing import List, Optional
from rattler.match_spec.match_spec import MatchSpec

from rattler.rattler import py_solve, PySolverOptions
from rattler.repo_data.record import RepoDataRecord
from rattler.repo_data.sparse import SparseRepoData
from rattler.virtual_package.generic import GenericVirtualPackage


class SolverOptions:
    def __init__(self, timeout: datetime.timedelta):
        if not isinstance(timeout, datetime.timedelta):
            raise TypeError("timeout must be a datetime.timedelta")
        self.timeout = timeout

    def to_py_solver_options(self) -> PySolverOptions:
        return PySolverOptions(self.timeout.microseconds)


def solve(
    specs: List[MatchSpec],
    available_packages: List[SparseRepoData],
    locked_packages: Optional[List[RepoDataRecord]] = None,
    pinned_packages: Optional[List[RepoDataRecord]] = None,
    virtual_packages: Optional[List[GenericVirtualPackage]] = None,
    solver_options: Optional[SolverOptions] = None,
) -> List[RepoDataRecord]:
    """
    Resolve the dependencies and return the `RepoDataRecord`s
    that should be present in the environment.

    Arguments:
        specs: A list of matchspec to solve.
        available_packages: A list of RepoData to use for solving the `specs`.
        locked_packages: Records of packages that are previously selected.
                         If the solver encounters multiple variants of a single
                         package (identified by its name), it will sort the records
                         and select the best possible version. However, if there
                         exists a locked version it will prefer that variant instead.
                         This is useful to reduce the number of packages that are
                         updated when installing new packages. Usually you add the
                         currently installed packages or packages from a lock-file here.
        pinned_packages: Records of packages that are previously selected and CANNOT
                         be changed. If the solver encounters multiple variants of
                         a single package (identified by its name), it will sort the
                         records and select the best possible version. However, if
                         there is a variant available in the `pinned_packages` field it
                         will always select that version no matter what even if that
                         means other packages have to be downgraded.
        virtual_packages: A list of virtual packages considered active.

    Returns:
        Resolved list of `RepoDataRecord`s.
    """

    return [
        RepoDataRecord._from_py_record(solved_package)
        for solved_package in py_solve(
            [spec._match_spec for spec in specs],
            [package._sparse for package in available_packages],
            [package._record for package in locked_packages or []],
            [package._record for package in pinned_packages or []],
            [
                v_package._generic_virtual_package
                for v_package in virtual_packages or []
            ],
            solver_options.to_py_solver_options() if solver_options else None,
        )
    ]
