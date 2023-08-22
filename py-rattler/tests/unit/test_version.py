from rattler import Version


def test_version_comparision():
    assert Version("1.0") < Version("2.0")
    assert Version("1.0") <= Version("2.0")
    assert Version("2.0") > Version("1.0")
    assert Version("2.0") >= Version("1.0")
    assert Version("1.0.0") == Version("1.0")
    assert Version("1.0") != Version("2.0")


def test_bump():
    assert Version("1.0").bump() == Version("1.1")
    assert Version("1.a").bump() == Version("1.1a")
    assert Version("1dev").bump() == Version("2dev")
    assert Version("1dev0").bump() == Version("1dev1")
    assert Version("1!0").bump() == Version("1!1")
    assert Version("1.2-alpha.3-beta-dev0").bump() == Version("1.2-alpha.3-beta-dev1")


def test_epoch():
    assert Version("1!1.0").epoch == 1
    assert Version("1.0").epoch is None


def test_dev():
    assert Version("1.2-alpha.3-beta-dev0").dev is True
    assert Version("1.2-alpha.3").dev is False


def test_local():
    assert Version("1.0+1.2").local is True
    assert Version("1.0").local is False


def test_as_major_minor():
    assert Version("2.3.4").as_major_minor() == (2, 3)
    assert Version("1.2-alpha.3-beta-dev0").as_major_minor() == (1, 2)


def test_starts_with():
    assert Version("1.0.6").starts_with(Version("1.0")) is True
    assert Version("1.0.6").starts_with(Version("1.6")) is False


def test_compatible_with():
    assert Version("1.6").compatible_with(Version("1.5")) is True
    assert Version("1.6").compatible_with(Version("1.7")) is False
    assert Version("1.6").compatible_with(Version("2.0")) is False


def test_pop_segments():
    assert Version("1.6.0").pop_segments() == Version("1.6")
    assert Version("1.6.0").pop_segments(2) == Version("1")
    assert Version("1.6.0").pop_segments(3) is None


def test_strip_local():
    assert Version("1.6+2.0").strip_local() == Version("1.6")
    assert Version("1.6").strip_local() == Version("1.6")
