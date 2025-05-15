from rattler import PackageRecord, NoArchType


def test_platform_arch() -> None:
    record = PackageRecord(name="x", version="1", build="0", build_number=0, subdir="linux-64")
    assert record.platform == "linux"
    assert record.arch == "x86_64"


def test_platform_explicit() -> None:
    record = PackageRecord(name="x", version="1", build="0", build_number=0, subdir="linux-64", platform="windows")
    assert record.platform == "windows"
    assert record.arch == "x86_64"


def test_platform_arch_unknown_subdir() -> None:
    record = PackageRecord(name="x", version="1", build="0", build_number=0, subdir="doesntexist")
    assert record.platform is None
    assert record.arch is None


def test_noarch_python() -> None:
    record = PackageRecord(
        name="x", version="1", build="0", build_number=0, subdir="noarch", noarch=NoArchType("python")
    )
    assert record.noarch.python
    assert not record.noarch.generic
    assert not record.noarch.none


def test_noarch_none() -> None:
    record = PackageRecord(name="x", version="1", build="0", build_number=0, subdir="noarch", noarch=None)
    assert record.noarch.none
    assert not record.noarch.python
    assert not record.noarch.generic


def test_noarch_literal_python() -> None:
    record = PackageRecord(name="x", version="1", build="0", build_number=0, subdir="noarch", noarch="python")
    assert record.noarch.python
    assert not record.noarch.generic
    assert not record.noarch.none


def test_noarch_literal_true() -> None:
    record = PackageRecord(name="x", version="1", build="0", build_number=0, subdir="noarch", noarch=True)
    assert not record.noarch.python
    assert record.noarch.generic
    assert not record.noarch.none
