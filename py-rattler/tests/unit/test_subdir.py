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


def test_deprecated_all_matches_known() -> None:
    """`Subdir.all` still works but points at the known subdirs."""
    with pytest.warns(DeprecationWarning, match="`Subdir.all` is deprecated"):
        assert list(Subdir.all()) == list(Subdir.known())


def test_arbitrary_subdirs() -> None:
    """A subdir rattler has never heard of parses and answers from its name."""
    subdir = Subdir("linux-esp32s3")
    assert not subdir.is_known
    assert str(subdir) == "linux-esp32s3"
    assert subdir.only_platform == "linux"
    assert str(subdir.arch) == "esp32s3"
    assert subdir.is_linux
    assert subdir.is_unix
    assert not subdir.is_windows

    # An os rattler does not know is not assumed to be unix-like.
    assert not Subdir("espidf-xtensa").is_unix

    # Names that violate CEP 26 are still rejected.
    with pytest.raises(Exception):
        Subdir("Linux-64")
