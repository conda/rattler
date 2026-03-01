try:
    from rattler.rattler import (
        ActivationError,
        CacheDirError,
        ConversionError,
        ConvertSubdirError,
        DetectVirtualPackageError,
        EnvironmentCreationError,
        ExtractError,
        FetchRepoDataError,
        GatewayError,
        InvalidChannelError,
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
        ParsePlatformError,
        RequirementError,
        TransactionError,
        ValidatePackageRecordsError,
        VersionBumpError,
        VersionExtendError,
    )
except ImportError:
    # They are only redefined for documentation purposes
    # when there is no binary yet

    class ActivationError(Exception):  # type: ignore[no-redef]
        """Error that can occur when activating a conda environment"""

    class CacheDirError(Exception):  # type: ignore[no-redef]
        """Error that can occur when querying the cache directory"""

    class ConversionError(Exception):  # type: ignore[no-redef]
        """An error that can occur during conversion"""

    class ConvertSubdirError(Exception):  # type: ignore[no-redef]
        """An error that can occur when parsing a platform from a string."""

    class DetectVirtualPackageError(Exception):  # type: ignore[no-redef]
        """An error that can occur when trying to detect virtual packages"""

    class EnvironmentCreationError(Exception):  # type: ignore[no-redef]
        """An error that can occur when creating an environment."""

    class ExtractError(Exception):  # type: ignore[no-redef]
        """An error that can occur when extracting an archive."""

    class FetchRepoDataError(Exception):  # type: ignore[no-redef]
        """An error that can occur when fetching repo data"""

    class GatewayError(Exception):  # type: ignore[no-redef]
        """An error that can occur when querying the repodata gateway."""

    class InvalidChannelError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a channel."""

    class InvalidMatchSpecError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a MatchSpec"""

    class InvalidPackageNameError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a package name"""

    class InvalidUrlError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a URL"""

    class InvalidVersionError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a Version"""

    class InvalidVersionSpecError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a VersionSpec"""

    class IoError(Exception):  # type: ignore[no-redef]
        """An error that can occur during io operations"""

    class LinkError(Exception):  # type: ignore[no-redef]
        """An error that can occur when linking a package"""

    class PackageNameMatcherParseError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a package name matcher"""

    class ParseArchError(Exception):  # type: ignore[no-redef]
        """An error that can occur when parsing an arch from a string."""

    class ParseCondaLockError(Exception):  # type: ignore[no-redef]
        """An error that can occur when parsing a conda lock file"""

    class ParseExplicitEnvironmentSpecError(Exception):  # type: ignore[no-redef]
        """An error that can occur when parsing an explicit environment spec"""

    class ParsePlatformError(Exception):  # type: ignore[no-redef]
        """An error that can occur when parsing a platform from a string."""

    class RequirementError(Exception):  # type: ignore[no-redef]
        """An error that can occur when parsing a requirement"""

    class TransactionError(Exception):  # type: ignore[no-redef]
        """An error that can occur when executing a transaction"""

    class ValidatePackageRecordsError(Exception):  # type: ignore[no-redef]
        """An error when validating package records."""

    class VersionBumpError(Exception):  # type: ignore[no-redef]
        """An error that can occur when bumping a version."""

    class VersionExtendError(Exception):  # type: ignore[no-redef]
        """An error that can occur when extending a version."""


__all__ = [
    "ActivationError",
    "CacheDirError",
    "ConversionError",
    "ConvertSubdirError",
    "DetectVirtualPackageError",
    "EnvironmentCreationError",
    "ExtractError",
    "FetchRepoDataError",
    "GatewayError",
    "InvalidChannelError",
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
    "RequirementError",
    "TransactionError",
    "ValidatePackageRecordsError",
    "VersionBumpError",
    "VersionExtendError",
]
