import warnings

import pytest
from rattler import Subdir


def test_deprecated_platform_aliases() -> None:
    """The pre-rename names still resolve, but warn and yield the same objects."""
    import rattler
    import rattler.platform
    import rattler.exceptions

    with pytest.warns(DeprecationWarning, match="`Platform` is deprecated"):
        assert rattler.Platform is Subdir
    with pytest.warns(DeprecationWarning, match="`Platform` is deprecated"):
        assert rattler.platform.Platform is Subdir
    with pytest.warns(DeprecationWarning, match="`PlatformLiteral` is deprecated"):
        assert rattler.platform.PlatformLiteral is rattler.platform.SubdirLiteral
    with pytest.warns(DeprecationWarning, match="`ParsePlatformError` is deprecated"):
        assert rattler.exceptions.ParsePlatformError is rattler.exceptions.ParseSubdirError


def test_importing_rattler_does_not_warn() -> None:
    """Only touching a deprecated name warns, not importing the package."""
    import importlib

    import rattler

    with warnings.catch_warnings():
        warnings.simplefilter("error", DeprecationWarning)
        importlib.reload(rattler)


def test_unknown_attribute_still_raises() -> None:
    import rattler

    with pytest.raises(AttributeError):
        rattler.NotAThing  # type: ignore[attr-defined]
