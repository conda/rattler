try:
    from rattler.rattler import (
        InvalidVersionError,
        InvalidMatchSpecError,
        InvalidPackageNameError,
        InvalidUrlError,
        InvalidChannelError,
        ActivationError,
        ParsePlatformError,
        ParseArchError,
        TransactionError,
        LinkError,
        IoError,
        DetectVirtualPackageError,
        CacheDirError,
        FetchRepoDataError,
        SolverError,
        ConvertSubdirError,
        VersionBumpError,
        EnvironmentCreationError,
        ExtractError,
        GatewayError,
        ValidatePackageRecordsException,
    )
except ImportError:
    # They are only redefined for documentation purposes
    # when there is no binary yet

    class InvalidVersionError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a Version"""

    class InvalidMatchSpecError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a MatchSpec"""

    class InvalidPackageNameError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a package name"""

    class InvalidUrlError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a URL"""

    class InvalidChannelError(Exception):  # type: ignore[no-redef]
        """Error that can occur when parsing a channel."""

    class ActivationError(Exception):  # type: ignore[no-redef]
        """Error that can occur when activating a conda environment"""

    class ParsePlatformError(Exception):  # type: ignore[no-redef]
        """An error that can occur when parsing a platform from a string."""

    class ParseArchError(Exception):  # type: ignore[no-redef]
        """An error that can occur when parsing an arch from a string."""

    class TransactionError(Exception):  # type: ignore[no-redef]
        """An error that can occur when executing a transaction"""

    class LinkError(Exception):  # type: ignore[no-redef]
        """An error that can occur when linking a package"""

    class IoError(Exception):  # type: ignore[no-redef]
        """An error that can occur during io operations"""

    class DetectVirtualPackageError(Exception):  # type: ignore[no-redef]
        """An error that can occur when trying to detect virtual packages"""

    class CacheDirError(Exception):  # type: ignore[no-redef]
        """An error that can occur when querying the cache directory"""

    class FetchRepoDataError(Exception):  # type: ignore[no-redef]
        """An error that can occur when fetching repo data"""

    class SolverError(Exception):  # type: ignore[no-redef]
        """An error that can occur when trying to solve an environment"""

    class ConvertSubdirError(Exception):  # type: ignore[no-redef]
        """An error that can occur when parsing a platform from a string."""

    class VersionBumpError(Exception):  # type: ignore[no-redef]
        """An error that can occur when bumping a version."""

    class EnvironmentCreationError(Exception):  # type: ignore[no-redef]
        """An error that can occur when creating an environment."""

    class ExtractError(Exception):  # type: ignore[no-redef]
        """An error that can occur when extracting an archive."""

    class GatewayError(Exception):  # type: ignore[no-redef]
        """An error that can occur when querying the repodata gateway."""

    class ValidatePackageRecordsException(Exception):  # type: ignore[no-redef]
        """An error when validating package records."""


__all__ = [
    "ActivationError",
    "CacheDirError",
    "DetectVirtualPackageError",
    "FetchRepoDataError",
    "InvalidChannelError",
    "InvalidMatchSpecError",
    "InvalidPackageNameError",
    "InvalidUrlError",
    "InvalidVersionError",
    "IoError",
    "LinkError",
    "ParseArchError",
    "ParsePlatformError",
    "SolverError",
    "TransactionError",
    "ConvertSubdirError",
    "VersionBumpError",
    "EnvironmentCreationError",
    "ExtractError",
    "GatewayError",
    "ValidatePackageRecordsException",
]
