from rattler.version import Version, VersionWithSource
from rattler.match_spec import MatchSpec, NamelessMatchSpec
from rattler.repo_data import (
    PackageRecord,
    RepoData,
    RepoDataRecord,
    PatchInstructions,
    SparseRepoData,
    Gateway,
    SourceConfig,
)
from rattler.channel import Channel, ChannelConfig, ChannelPriority
from rattler.networking import Client, fetch_repo_data
from rattler.virtual_package import GenericVirtualPackage, VirtualPackage, VirtualPackageOverrides, Override
from rattler.package import (
    PackageName,
    AboutJson,
    RunExportsJson,
    PathsJson,
    PathsEntry,
    PathType,
    PrefixPlaceholder,
    FileMode,
    IndexJson,
)
from rattler.package.no_arch_type import NoArchType
from rattler.prefix import PrefixRecord, PrefixPaths, PrefixPathsEntry, PrefixPathType
from rattler.platform import Platform
from rattler.utils.rattler_version import get_rattler_version as _get_rattler_version
from rattler.install import install
from rattler.index import index
from rattler.lock import (
    LockFile,
    Environment,
    LockChannel,
    PackageHashes,
    LockedPackage,
    PypiPackageData,
    PypiPackageEnvironmentData,
)
from rattler.solver import solve, solve_with_sparse_repodata

__version__ = _get_rattler_version()
del _get_rattler_version

__all__ = [
    "Version",
    "VersionWithSource",
    "MatchSpec",
    "NamelessMatchSpec",
    "PackageRecord",
    "Channel",
    "ChannelConfig",
    "ChannelPriority",
    "Client",
    "PatchInstructions",
    "RepoDataRecord",
    "RepoData",
    "fetch_repo_data",
    "GenericVirtualPackage",
    "VirtualPackage",
    "VirtualPackageOverrides",
    "Override",
    "PackageName",
    "PrefixRecord",
    "PrefixPaths",
    "PrefixPathsEntry",
    "PrefixPathType",
    "SparseRepoData",
    "LockFile",
    "Environment",
    "LockChannel",
    "PackageHashes",
    "LockedPackage",
    "PypiPackageData",
    "PypiPackageEnvironmentData",
    "solve",
    "solve_with_sparse_repodata",
    "Platform",
    "install",
    "index",
    "AboutJson",
    "RunExportsJson",
    "PathsJson",
    "PathsEntry",
    "PathType",
    "PrefixPlaceholder",
    "FileMode",
    "IndexJson",
    "Gateway",
    "SourceConfig",
    "NoArchType",
]
