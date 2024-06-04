import pytest
from rattler import Version, VersionWithSource


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


def test_compare_with_source() -> None:
    """Tests that comparing a Version with a VersionWithSource works as expected."""
    assert Version("1.0") == VersionWithSource("1.00")
