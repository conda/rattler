from rattler.version import Version, VersionWithSource
from rattler.match_spec import MatchSpec, NamelessMatchSpec
from rattler.repo_data import (
    PackageRecord,
    RepoData,
    RepoDataRecord,
    PatchInstructions,
    SparseRepoData,
)
from rattler.channel import Channel, ChannelConfig
from rattler.networking import AuthenticatedClient, fetch_repo_data
from rattler.virtual_package import GenericVirtualPackage, VirtualPackage
from rattler.package import PackageName
from rattler.prefix import PrefixRecord, PrefixPaths
from rattler.solver import solve
from rattler.platform import Platform
from rattler.utils.rattler_version import get_rattler_version as _get_rattler_version
from rattler.linker import link

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
    "AuthenticatedClient",
    "PatchInstructions",
    "RepoDataRecord",
    "RepoData",
    "fetch_repo_data",
    "GenericVirtualPackage",
    "VirtualPackage",
    "PackageName",
    "PrefixRecord",
    "PrefixPaths",
    "SparseRepoData",
    "solve",
    "Platform",
    "link",
]
