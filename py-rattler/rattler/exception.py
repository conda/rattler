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


__all__ = [
    "InvalidVersionError",
    "InvalidMatchSpecError",
    "InvalidPackageNameError",
    "InvalidUrlError",
    "InvalidChannelError",
    "ActivationError",
    "ParsePlatformError",
    "ParseArchError",
]
