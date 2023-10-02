from __future__ import annotations
from typing import List
from rattler.match_spec.match_spec import MatchSpec

from rattler.rattler import py_solve
from rattler.repo_data.record import RepoDataRecord
from rattler.repo_data.sparse import SparseRepoData
from rattler.virtual_package.generic import GenericVirtualPackage


def solve(
    specs: List[MatchSpec],
    available_packages: List[SparseRepoData],
    locked_packages: List[RepoDataRecord],
    pinned_packages: List[RepoDataRecord],
    virtual_packages: List[GenericVirtualPackage],
) -> List[RepoDataRecord]:
    """
    Resolve the dependencies and return the [`RepoDataRecord`]s
    that should be present in the environment.
    """
    return [
        RepoDataRecord._from_py_record(solved_package)
        for solved_package in py_solve(
            [spec._match_spec for spec in specs],
            available_packages,
            [package._record for package in locked_packages],
            [package._record for package in pinned_packages],
            [v_package._generic_virtual_package for v_package in virtual_packages],
        )
    ]
