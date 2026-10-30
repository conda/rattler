from __future__ import annotations

import warnings
from typing import TYPE_CHECKING, Any

try:
    from rattler.rattler import (
        ActivationError,
        ActivationScriptFormatError,
        AttestationError,
        AuthenticationStorageError,
        CacheDirError,
        CanonicalMatchSpecError,
        ConfigError,
        ConversionError,
        ConvertSubdirError,
        DetectVirtualPackageError,
        EnvironmentCreationError,
        ExtractError,
        FetchRepoDataError,
        GatewayError,
        GatewayWarning,
        InstallerError,
        InvalidChannelError,
        InvalidHeaderNameError,
        InvalidHeaderValueError,
        InvalidMatchSpecError,
        InvalidPackageNameError,
        InvalidUrlError,
        InvalidVersionError,
        InvalidVersionSpecError,
        IoError,
        LinkError,
        PackageNameMatcherParseError,
        ParseArchError,
        ParseCondaLockError,
        ParseExplicitEnvironmentSpecError,
        ParseSubdirError,
        RequirementError,
        ShellError,
        SolverError,
        TransactionError,
        ValidatePackageRecordsError,
        VersionBumpError,
        VersionExtendError,
        VirtualPackageOverrideError,
    )
except ImportError:
    # They are only redefined for documentation purposes
    # when there is no binary yet

    class ActivationError(Exception):
        """Error that can occur when activating a conda environment"""

    class ActivationScriptFormatError(Exception):
        """Error that can occur when the activation script has an invalid format"""

    class AuthenticationStorageError(Exception):
        """Error that can occur when querying the authentication storage"""

    class CacheDirError(Exception):
        """Error that can occur when querying the cache directory"""

    class CanonicalMatchSpecError(Exception):
        """Error that can occur when a MatchSpec cannot be represented canonically"""

    class ConfigError(Exception):
        """An error that can occur when loading a configuration file"""

    class ConversionError(Exception):
        """An error that can occur during conversion"""

    class ConvertSubdirError(Exception):
        """An error that can occur when parsing a subdir from a string."""

    class DetectVirtualPackageError(Exception):
        """An error that can occur when trying to detect virtual packages"""

    class EnvironmentCreationError(Exception):
        """An error that can occur when creating an environment."""

    class ExtractError(Exception):
        """An error that can occur when extracting an archive."""

    class FetchRepoDataError(Exception):
        """An error that can occur when fetching repo data"""

    class GatewayError(Exception):
        """An error that can occur when querying the repodata gateway."""

    class GatewayWarning(UserWarning):
        """A non-fatal problem reported by the repodata gateway, e.g. while
        resolving CEP-42 channel relations."""

    class InstallerError(Exception):
        """An error that can occur when installing a package"""

    class AttestationError(Exception):
        """An error that can occur when verifying a Sigstore attestation"""

    class InvalidChannelError(Exception):
        """Error that can occur when parsing a channel."""

    class InvalidMatchSpecError(Exception):
        """Error that can occur when parsing a MatchSpec"""

    class InvalidHeaderNameError(Exception):
        """Error that can occur when an invalid header name is used"""

    class InvalidHeaderValueError(Exception):
        """Error that can occur when an invalid header value is used"""

    class InvalidPackageNameError(Exception):
        """Error that can occur when parsing a package name"""

    class InvalidUrlError(Exception):
        """Error that can occur when parsing a URL"""

    class InvalidVersionError(Exception):
        """Error that can occur when parsing a Version"""

    class InvalidVersionSpecError(Exception):
        """Error that can occur when parsing a VersionSpec"""

    class VirtualPackageOverrideError(Exception):
        """Error that can occur when a `CONDA_OVERRIDE_*` variable of a detector-provided virtual package is invalid"""

    class IoError(Exception):
        """An error that can occur during io operations"""

    class LinkError(Exception):
        """An error that can occur when linking a package"""

    class PackageNameMatcherParseError(Exception):
        """Error that can occur when parsing a package name matcher"""

    class ParseArchError(Exception):
        """An error that can occur when parsing an arch from a string."""

    class ParseCondaLockError(Exception):
        """An error that can occur when parsing a conda lock file"""

    class ParseExplicitEnvironmentSpecError(Exception):
        """An error that can occur when parsing an explicit environment spec"""

    class ParseSubdirError(Exception):
        """An error that can occur when parsing a platform from a string."""

    class RequirementError(Exception):
        """An error that can occur when parsing a requirement"""

    class ShellError(Exception):
        """An error that can occur when interacting with the shell"""

    class SolverError(Exception):
        """An error that can occur when solving for an environment"""

    class TransactionError(Exception):
        """An error that can occur when executing a transaction"""

    class ValidatePackageRecordsError(Exception):
        """An error when validating package records."""

    class VersionBumpError(Exception):
        """An error that can occur when bumping a version."""

    class VersionExtendError(Exception):
        """An error that can occur when extending a version."""


_DEPRECATED_ALIASES = {"ParsePlatformError": "ParseSubdirError"}

if TYPE_CHECKING:
    ParsePlatformError = ParseSubdirError
else:

    def __getattr__(name: str) -> Any:
        """Resolve the deprecated `ParsePlatformError` name.

        .. deprecated:: 0.27.0 Use `ParseSubdirError` instead.
        """
        if name in _DEPRECATED_ALIASES:
            replacement = _DEPRECATED_ALIASES[name]
            warnings.warn(
                f"`{name}` is deprecated, use `{replacement}` instead.",
                DeprecationWarning,
                stacklevel=2,
            )
            return globals()[replacement]
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


__all__ = [
    "ActivationError",
    "ActivationScriptFormatError",
    "AttestationError",
    "AuthenticationStorageError",
    "CacheDirError",
    "CanonicalMatchSpecError",
    "ConfigError",
    "ConversionError",
    "ConvertSubdirError",
    "DetectVirtualPackageError",
    "EnvironmentCreationError",
    "ExtractError",
    "FetchRepoDataError",
    "GatewayError",
    "GatewayWarning",
    "InstallerError",
    "InvalidChannelError",
    "InvalidHeaderNameError",
    "InvalidHeaderValueError",
    "InvalidMatchSpecError",
    "InvalidPackageNameError",
    "InvalidUrlError",
    "InvalidVersionError",
    "InvalidVersionSpecError",
    "IoError",
    "LinkError",
    "PackageNameMatcherParseError",
    "ParseArchError",
    "ParseCondaLockError",
    "ParseExplicitEnvironmentSpecError",
    "ParsePlatformError",
    "ParseSubdirError",
    "RequirementError",
    "ShellError",
    "SolverError",
    "TransactionError",
    "ValidatePackageRecordsError",
    "VersionBumpError",
    "VersionExtendError",
    "VirtualPackageOverrideError",
]
