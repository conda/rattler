from __future__ import annotations
import datetime
from typing import List, Optional, Literal, Sequence

from rattler import Channel, Platform, VirtualPackage, SparseRepoData
from rattler.match_spec.match_spec import MatchSpec

from rattler.channel import ChannelPriority
from rattler.rattler import py_solve, PyMatchSpec, py_solve_with_sparse_repodata

from rattler.platform.platform import PlatformLiteral
from rattler.repo_data.gateway import Gateway
from rattler.repo_data.record import RepoDataRecord
from rattler.virtual_package.generic import GenericVirtualPackage

SolveStrategy = Literal["highest", "lowest", "lowest-direct"]
"""Defines the strategy to use when multiple versions of a package are available during solving."""


async def solve(
    channels: Sequence[Channel | str],
    specs: Sequence[MatchSpec | str],
    gateway: Gateway = Gateway(),
    platforms: Optional[Sequence[Platform | PlatformLiteral]] = None,
    locked_packages: Optional[Sequence[RepoDataRecord]] = None,
    pinned_packages: Optional[Sequence[RepoDataRecord]] = None,
    virtual_packages: Optional[Sequence[GenericVirtualPackage | VirtualPackage]] = None,
    timeout: Optional[datetime.timedelta] = None,
    channel_priority: ChannelPriority = ChannelPriority.Strict,
    exclude_newer: Optional[datetime.datetime] = None,
    strategy: SolveStrategy = "highest",
    constraints: Optional[Sequence[MatchSpec | str]] = None,
) -> List[RepoDataRecord]:
    """
    Resolve the dependencies and return the `RepoDataRecord`s
    that should be present in the environment.

    Arguments:
        channels: The channels to query for the packages.
        specs: A list of matchspec to solve.
        platforms: The platforms to query for the packages. If `None` the current platform and
                `noarch` is used.
        gateway: The gateway to use for acquiring repodata.
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
        channel_priority: (Default = ChannelPriority.Strict) When `ChannelPriority.Strict`
                 the channel that the package is first found in will be used as
                 the only channel for that package. When `ChannelPriority.Disabled`
                 it will search for every package in every channel.
        timeout:    The maximum time the solver is allowed to run.
        exclude_newer: Exclude any record that is newer than the given datetime.
        strategy: The strategy to use when multiple versions of a package are available.

            * `"highest"`: Select the highest compatible version of all packages.
            * `"lowest"`: Select the lowest compatible version of all packages.
            * `"lowest-direct"`: Select the lowest compatible version for all
              direct dependencies but the highest compatible version of transitive
              dependencies.
        constraints: Additional constraints that should be satisfied by the solver.
            Packages included in the `constraints` are not necessarily installed,
            but they must be satisfied by the solution.

    Returns:
        Resolved list of `RepoDataRecord`s.
    """

    platforms = platforms if platforms is not None else [Platform.current(), Platform("noarch")]

    return [
        RepoDataRecord._from_py_record(solved_package)
        for solved_package in await py_solve(
            channels=[
                channel._channel if isinstance(channel, Channel) else Channel(channel)._channel for channel in channels
            ],
            platforms=[
                platform._inner if isinstance(platform, Platform) else Platform(platform)._inner
                for platform in platforms
            ],
            specs=[spec._match_spec if isinstance(spec, MatchSpec) else PyMatchSpec(str(spec), True) for spec in specs],
            gateway=gateway._gateway,
            locked_packages=[package._record for package in locked_packages or []],
            pinned_packages=[package._record for package in pinned_packages or []],
            virtual_packages=[
                v_package.into_generic()._generic_virtual_package
                if isinstance(v_package, VirtualPackage)
                else v_package._generic_virtual_package
                for v_package in virtual_packages or []
            ],
            channel_priority=channel_priority.value,
            timeout=int(timeout / datetime.timedelta(microseconds=1)) if timeout else None,
            exclude_newer_timestamp_ms=int(exclude_newer.replace(tzinfo=datetime.timezone.utc).timestamp() * 1000)
            if exclude_newer
            else None,
            strategy=strategy,
            constraints=[
                constraint._match_spec if isinstance(constraint, MatchSpec) else PyMatchSpec(str(constraint), True)
                for constraint in constraints
            ]
            if constraints is not None
            else [],
        )
    ]


async def solve_with_sparse_repodata(
    specs: Sequence[MatchSpec | str],
    sparse_repodata: Sequence[SparseRepoData],
    locked_packages: Optional[Sequence[RepoDataRecord]] = None,
    pinned_packages: Optional[Sequence[RepoDataRecord]] = None,
    virtual_packages: Optional[Sequence[GenericVirtualPackage | VirtualPackage]] = None,
    timeout: Optional[datetime.timedelta] = None,
    channel_priority: ChannelPriority = ChannelPriority.Strict,
    exclude_newer: Optional[datetime.datetime] = None,
    strategy: SolveStrategy = "highest",
    constraints: Optional[Sequence[MatchSpec | str]] = None,
) -> List[RepoDataRecord]:
    """
    Resolve the dependencies and return the `RepoDataRecord`s
    that should be present in the environment.

    This function is similar to `solve` but instead of querying for repodata
    with a `Gateway` object this function allows you to manually pass in the
    repodata.

    Arguments:
        specs: A list of matchspec to solve.
        sparse_repodata: The repodata to query for the packages.
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
        channel_priority: (Default = ChannelPriority.Strict) When `ChannelPriority.Strict`
                 the channel that the package is first found in will be used as
                 the only channel for that package. When `ChannelPriority.Disabled`
                 it will search for every package in every channel.
        timeout:    The maximum time the solver is allowed to run.
        exclude_newer: Exclude any record that is newer than the given datetime.
        strategy: The strategy to use when multiple versions of a package are available.

            * `"highest"`: Select the highest compatible version of all packages.
            * `"lowest"`: Select the lowest compatible version of all packages.
            * `"lowest-direct"`: Select the lowest compatible version for all
              direct dependencies but the highest compatible version of transitive
              dependencies.
        constraints: Additional constraints that should be satisfied by the solver.
            Packages included in the `constraints` are not necessarily installed,
            but they must be satisfied by the solution.

    Returns:
        Resolved list of `RepoDataRecord`s.
    """

    return [
        RepoDataRecord._from_py_record(solved_package)
        for solved_package in await py_solve_with_sparse_repodata(
            specs=[spec._match_spec if isinstance(spec, MatchSpec) else PyMatchSpec(str(spec), True) for spec in specs],
            sparse_repodata=[package._sparse for package in sparse_repodata],
            locked_packages=[package._record for package in locked_packages or []],
            pinned_packages=[package._record for package in pinned_packages or []],
            virtual_packages=[
                v_package.into_generic()._generic_virtual_package
                if isinstance(v_package, VirtualPackage)
                else v_package._generic_virtual_package
                for v_package in virtual_packages or []
            ],
            channel_priority=channel_priority.value,
            timeout=int(timeout / datetime.timedelta(microseconds=1)) if timeout else None,
            exclude_newer_timestamp_ms=int(exclude_newer.replace(tzinfo=datetime.timezone.utc).timestamp() * 1000)
            if exclude_newer
            else None,
            strategy=strategy,
            constraints=[
                constraint._match_spec if isinstance(constraint, MatchSpec) else PyMatchSpec(str(constraint), True)
                for constraint in constraints
            ]
            if constraints is not None
            else [],
        )
    ]
