from __future__ import annotations
from typing import List, Tuple
from rattler.match_spec.match_spec import MatchSpec

from rattler.rattler import py_solve
from rattler.repo_data.record import RepoDataRecord
from rattler.virtual_package.generic import GenericVirtualPackage
from rattler.repo_data.repo_data import RepoData
from rattler.channel import Channel


def solve(
    specs: List[MatchSpec],
    available_packages: List[Tuple[RepoData, Channel]],
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
            [
                (repo_data._repo_data, channel._channel)
                for (repo_data, channel) in available_packages
            ],
            [package._record for package in locked_packages],
            [package._record for package in pinned_packages],
            [v_package._generic_virtual_package for v_package in virtual_packages],
        )
    ]
