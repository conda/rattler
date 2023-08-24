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
