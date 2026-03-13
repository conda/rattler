from rattler import PackageRecord, WhlPackageRecord


def test_whl_package_record_construction_with_url() -> None:
    record = PackageRecord(
        name="numpy",
        version="1.24.0",
        build="cp39-cp39-linux_x86_64",
        build_number=0,
        subdir="linux-64",
    )
    whl_record = WhlPackageRecord(
        record,
        "https://example.com/wheels/numpy-1.24.0-cp39-cp39-linux_x86_64.whl",
    )
    assert whl_record.url == "https://example.com/wheels/numpy-1.24.0-cp39-cp39-linux_x86_64.whl"
    assert whl_record.name == record.name
    assert whl_record.version == record.version


def test_whl_package_record_construction_with_relative_path() -> None:
    record = PackageRecord(
        name="requests",
        version="2.28.0",
        build="py3-none-any",
        build_number=0,
        subdir="noarch",
    )
    whl_record = WhlPackageRecord(record, "subdir/requests-2.28.0-py3-none-any.whl")
    assert whl_record.url == "subdir/requests-2.28.0-py3-none-any.whl"
    assert whl_record.name == record.name


def test_whl_package_record_properties() -> None:
    record = PackageRecord(
        name="foo",
        version="1.0",
        build="py_0",
        build_number=0,
        subdir="noarch",
    )
    url = "https://pypi.org/simple/foo-1.0-py3-none-any.whl"
    whl_record = WhlPackageRecord(record, url)

    assert isinstance(whl_record, PackageRecord)
    assert whl_record.name == record.name
    assert whl_record.version == record.version
    assert whl_record.build == record.build
    assert whl_record.subdir == record.subdir
    assert whl_record.url == url


def test_whl_package_record_repr() -> None:
    record = PackageRecord(
        name="pkg",
        version="1.0",
        build="0",
        build_number=0,
        subdir="noarch",
    )
    whl_record = WhlPackageRecord(record, "https://example.com/pkg.whl")
    repr_str = repr(whl_record)
    assert "WhlPackageRecord" in repr_str
    assert "https://example.com/pkg.whl" in repr_str
