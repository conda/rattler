import pytest
from rattler import Version
from rattler.exceptions import VersionBumpError


def test_version_dash_normalisation() -> None:
    assert Version("1.0-").segments() == [[1], [0, "_"]]
    assert Version("1.0_").segments() == [[1], [0, "_"]]
    assert Version("1.0dev-+2.3").segments() == [[1], [0, "dev", "_"]]
    assert Version("1.0dev_").segments() == [[1], [0, "dev", "_"]]

    assert Version("1.0dev-+2.3").local_segments() == [[2], [3]]
    assert Version("1.0dev+3.4-dev").local_segments() == [[3], [4], [0, "dev"]]
    assert Version("1.0dev+3.4-").local_segments() == [[3], [4, "_"]]

    with pytest.raises(Exception):
        Version("1-.0dev-")

    with pytest.raises(Exception):
        Version("1-.0dev+3.4-")


def test_bump() -> None:
    assert Version("0.5.5").bump_major() == Version("1.5.5")
    assert Version("0.5.5").bump_minor() == Version("0.6.5")
    assert Version("0.5.5").bump_patch() == Version("0.5.6")
    assert Version("0.5.5").bump_last() == Version("0.5.6")


def test_bump_fail() -> None:
    with pytest.raises(VersionBumpError):
        Version("1").bump_minor()

    with pytest.raises(VersionBumpError):
        Version("1.5").bump_patch()
