from rattler.channel import Channel, ChannelConfig, ChannelPriority
from rattler.config import Config, RunPostLinkScripts, TlsRootCerts
from rattler.index import index
from rattler.install import InstallerReporter, install
from rattler.lock import (
    CondaLockedBinaryPackage,
    CondaLockedPackage,
    CondaLockedSourcePackage,
    Environment,
    LockChannel,
    LockedPackage,
    LockFile,
    LockPlatform,
    PackageHashes,
    PypiLockedPackage,
)
from rattler.match_spec import MatchSpec, NamelessMatchSpec
from rattler.networking import Client, fetch_repo_data
from rattler.package import (
    AboutJson,
    FileMode,
    IndexJson,
    NoArchLiteral,
    NoArchType,
    PackageName,
    PathsEntry,
    PathsJson,
    PathType,
    PrefixPlaceholder,
    RunExportsJson,
)
from rattler.platform import Platform
from rattler.prefix import Link, LinkType, PrefixPaths, PrefixPathsEntry, PrefixPathType, PrefixRecord
from rattler.repo_data import (
    ChannelInfo,
    ChannelNotice,
    ChannelRelations,
    Dependent,
    Gateway,
    GatewayNamesResult,
    GatewayQueryResult,
    PackageFormatSelection,
    PackageRecord,
    PatchInstructions,
    RemovedPackage,
    RepoData,
    RepoDataRecord,
    RepodataRevisionMetadata,
    RepoDataSource,
    SourceConfig,
    SparseRepoData,
    WhlPackageRecord,
)
from rattler.sigstore import (
    CertificateClaims,
    ChannelCheck,
    Issuer,
    Publisher,
    TrustedRoot,
    VerificationMode,
    VerificationOutcome,
    VerificationPolicy,
    VerifiedAttestation,
    VerifiedChecks,
    verify_attestation,
)
from rattler.solver import solve, solve_with_sparse_repodata
from rattler.utils.rattler_version import get_rattler_version as _get_rattler_version
from rattler.version import Version, VersionSpec, VersionWithSource
from rattler.virtual_package import GenericVirtualPackage, Override, VirtualPackage, VirtualPackageOverrides

__version__ = _get_rattler_version()
del _get_rattler_version

__all__ = [
    "AboutJson",
    "Channel",
    "CertificateClaims",
    "ChannelCheck",
    "ChannelConfig",
    "ChannelInfo",
    "ChannelNotice",
    "ChannelPriority",
    "ChannelRelations",
    "Client",
    "CondaLockedBinaryPackage",
    "CondaLockedPackage",
    "CondaLockedSourcePackage",
    "Config",
    "Dependent",
    "Environment",
    "FileMode",
    "Gateway",
    "GatewayNamesResult",
    "GatewayQueryResult",
    "GenericVirtualPackage",
    "IndexJson",
    "InstallerReporter",
    "Issuer",
    "Link",
    "LinkType",
    "LockChannel",
    "LockFile",
    "LockPlatform",
    "LockedPackage",
    "MatchSpec",
    "NamelessMatchSpec",
    "NoArchLiteral",
    "NoArchType",
    "Override",
    "PackageFormatSelection",
    "PackageHashes",
    "PackageName",
    "PackageRecord",
    "PatchInstructions",
    "PathType",
    "PathsEntry",
    "PathsJson",
    "Platform",
    "PrefixPathType",
    "PrefixPaths",
    "PrefixPathsEntry",
    "PrefixPlaceholder",
    "PrefixRecord",
    "Publisher",
    "PypiLockedPackage",
    "RemovedPackage",
    "RepoData",
    "RepoDataRecord",
    "RepoDataSource",
    "RepodataRevisionMetadata",
    "RunExportsJson",
    "RunPostLinkScripts",
    "SourceConfig",
    "SparseRepoData",
    "TlsRootCerts",
    "TrustedRoot",
    "VerificationMode",
    "VerificationOutcome",
    "VerificationPolicy",
    "VerifiedAttestation",
    "VerifiedChecks",
    "Version",
    "VersionSpec",
    "VersionWithSource",
    "VirtualPackage",
    "VirtualPackageOverrides",
    "WhlPackageRecord",
    "fetch_repo_data",
    "index",
    "install",
    "solve",
    "solve_with_sparse_repodata",
    "verify_attestation",
]

# PTY support - only available on Unix platforms
try:
    from rattler.pty import PtyProcess, PtyProcessOptions, PtySession  # noqa: F401

    __all__.extend(["PtyProcess", "PtyProcessOptions", "PtySession"])
except ImportError:
    pass
