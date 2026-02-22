from rattler import PackageRecord, NoArchType
import json


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


def test_package_record_setters_and_serialization() -> None:
    """
    Verify that PackageRecord properties can be updated and that
    these updates are correctly serialized to JSON by the Rust backend.
    """
    # 1. Create a basic record
    record = PackageRecord(
        name="test-pkg",
        version="1.0.0",
        build="py_0",
        build_number=0,
        subdir="linux-64",
    )

    # 2. Update properties (Currently untested in the official repo)
    record.license = "MIT"
    record.depends = ["python >=3.9"]
    record.size = 2048

    # 3. Verify the Python getters return the updated values
    assert record.license == "MIT"
    assert record.depends == ["python >=3.9"]
    assert record.size == 2048

    # 4. Verify the Rust bridge: Does to_json() see our changes?
    json_data = json.loads(record.to_json())
    assert json_data["license"] == "MIT"
    assert json_data["depends"] == ["python >=3.9"]
    assert json_data["size"] == 2048


def test_package_record_hash_properties() -> None:
    """
    Verify that cryptographic hashes (md5, sha256) are correctly handled
    as bytes across the Python-Rust boundary.
    """
    record = PackageRecord(name="hash-test", version="1.0", build="0", build_number=0, subdir="noarch")

    sample_md5 = bytes.fromhex("5e5a97795de72f8cc3baf3d9ea6327a2")
    sample_sha = bytes.fromhex("4e50b3d90a351c9d47d239d3f90fce4870df2526e4f7fef35203ab3276a6dfc9")

    record.md5 = sample_md5
    record.sha256 = sample_sha

    assert record.md5 == sample_md5
    assert record.sha256 == sample_sha
    assert isinstance(record.sha256, bytes)


def test_package_record_topological_sort() -> None:
    """
    Verify that PackageRecord.sort_topologically correctly delegates to the
    Rust backend to sort records based on their 'depends' requirements.
    """
    record_dep = PackageRecord(name="dependency-pkg", version="1.0", build="0", build_number=0, subdir="noarch")
    record_main = PackageRecord(
        name="main-pkg", version="1.0", build="0", build_number=0, subdir="noarch", depends=["dependency-pkg"]
    )

    # Pass them into the sorter in the WRONG order
    unsorted_records = [record_main, record_dep]
    sorted_records = PackageRecord.sort_topologically(unsorted_records)

    # Assert that Rust correctly sorted the dependency first
    assert len(sorted_records) == 2
    assert sorted_records[0].name.normalized == "dependency-pkg"
    assert sorted_records[1].name.normalized == "main-pkg"
