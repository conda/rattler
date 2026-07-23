import pytest
from rattler import Version, VersionSpec, VersionWithSource


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


def test_version_spec_simple() -> None:
    """Test simple version specifications."""
    spec = VersionSpec(">=1.2.3")
    assert spec.matches(Version("1.2.3"))
    assert spec.matches(Version("1.5.0"))
    assert spec.matches(Version("2.0.0"))
    assert not spec.matches(Version("1.2.2"))
    assert not spec.matches(Version("1.0.0"))


def test_version_spec_range() -> None:
    """Test version specification with range."""
    spec = VersionSpec(">=1.2.3,<2.0.0")
    assert spec.matches(Version("1.2.3"))
    assert spec.matches(Version("1.5.0"))
    assert spec.matches(Version("1.99.99"))
    assert not spec.matches(Version("2.0.0"))
    assert not spec.matches(Version("2.1.0"))
    assert not spec.matches(Version("1.2.2"))


def test_version_spec_exact() -> None:
    """Test exact version specification."""
    spec = VersionSpec("==1.2.3")
    assert spec.matches(Version("1.2.3"))
    assert not spec.matches(Version("1.2.4"))
    assert not spec.matches(Version("1.2.2"))


def test_version_spec_or() -> None:
    """Test version specification with OR operator."""
    spec = VersionSpec(">=1.2.3|<1.0.0")
    assert spec.matches(Version("1.2.3"))
    assert spec.matches(Version("1.5.0"))
    assert spec.matches(Version("0.5.0"))
    assert not spec.matches(Version("1.0.0"))
    assert not spec.matches(Version("1.2.2"))


def test_version_spec_any() -> None:
    """Test wildcard version specification."""
    spec = VersionSpec("*")
    assert spec.matches(Version("0.0.1"))
    assert spec.matches(Version("1.2.3"))
    assert spec.matches(Version("999.999.999"))


def test_version_spec_strict_mode() -> None:
    """Test version specification with strict parsing."""
    # This should work in lenient mode
    spec_lenient = VersionSpec(">=1.2.3", strict=False)
    assert spec_lenient.matches(Version("1.2.3"))

    # This should work in strict mode
    spec_strict = VersionSpec(">=1.2.3", strict=True)
    assert spec_strict.matches(Version("1.2.3"))


def test_version_spec_equality() -> None:
    """Test version specification equality."""
    spec1 = VersionSpec(">=1.2.3,<2.0.0")
    spec2 = VersionSpec(">=1.2.3,<2.0.0")
    spec3 = VersionSpec(">=1.2.3")

    assert spec1 == spec2
    assert spec1 != spec3
    assert not (spec1 == spec3)


def test_version_spec_hash() -> None:
    """Test version specification hashing."""
    spec1 = VersionSpec(">=1.2.3,<2.0.0")
    spec2 = VersionSpec(">=1.2.3,<2.0.0")
    spec3 = VersionSpec(">=1.2.3")

    assert hash(spec1) == hash(spec2)
    assert hash(spec1) != hash(spec3)


def test_version_spec_str() -> None:
    """Test version specification string representation."""
    spec = VersionSpec(">=1.2.3,<2.0.0")
    assert str(spec) == ">=1.2.3,<2.0.0"

    spec_any = VersionSpec("*")
    assert str(spec_any) == "*"


def test_version_spec_repr() -> None:
    """Test version specification repr."""
    spec = VersionSpec(">=1.2.3")
    assert repr(spec) == 'VersionSpec(">=1.2.3")'
