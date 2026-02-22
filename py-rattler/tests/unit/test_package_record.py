import json
import random
import datetime

from rattler import NoArchType, PackageRecord, PackageName, VersionWithSource


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
    Verify that ALL PackageRecord properties can be updated and that
    these updates are correctly serialized to JSON by the Rust backend.
    """
    record = PackageRecord(
        name="test-pkg",
        version="1.0.0",
        build="py_0",
        build_number=0,
        subdir="linux-64",
    )

    record.arch = "x86_64"
    record.build = "py_1"
    record.build_number = 1
    record.constrains = ["scipy >1.0"]
    record.depends = ["python >=3.9", "numpy"]
    record.features = "test-feature"
    record.legacy_bz2_md5 = b"1234" * 4
    record.legacy_bz2_size = 1024
    record.license = "MIT"
    record.license_family = "MIT_Family"
    record.md5 = b"1234" * 4
    record.noarch = NoArchType("python")
    record.platform = "linux"
    record.sha256 = b"5678" * 8
    record.size = 2048
    record.subdir = "noarch"
    record.name = PackageName("new-test-pkg")
    record.version = VersionWithSource("2.0.0")
    ts = datetime.datetime(2023, 1, 1, 0, 0, tzinfo=datetime.timezone.utc)
    record.timestamp = ts
    record.track_features = ["track-me"]
    record.python_site_packages_path = "lib/python3.9/site-packages"
    json_data = json.loads(record.to_json())

    #  To verify the names match the expected order. NoArch is stored as specific keys in JSON usually, and checking type logic here via getters
    assert json_data["arch"] == "x86_64"
    assert json_data["build"] == "py_1"
    assert json_data["build_number"] == 1
    assert json_data["constrains"] == ["scipy >1.0"]
    assert json_data["depends"] == ["python >=3.9", "numpy"]
    assert json_data["features"] == "test-feature"
    assert json_data["legacy_bz2_size"] == 1024
    assert json_data["license"] == "MIT"
    assert json_data["license_family"] == "MIT_Family"
    assert record.noarch.python
    assert json_data["platform"] == "linux"
    assert json_data["size"] == 2048
    assert json_data["subdir"] == "noarch"
    assert json_data["timestamp"] == int(ts.timestamp() * 1000)
    assert json_data["track_features"] == "track-me"
    assert json_data["name"] == "new-test-pkg"
    assert json_data["version"] == "2.0.0"
    assert record.python_site_packages_path == "lib/python3.9/site-packages"
    assert record.md5 == b"1234" * 4
    assert record.sha256 == b"5678" * 8
    assert record.legacy_bz2_md5 == b"1234" * 4


def test_package_record_topological_sort_robust() -> None:
    """
    Verify topological sort using a shuffled chain of 5 dependencies.
    Chain: pkg_e -> pkg_d -> pkg_c -> pkg_b -> pkg_a
    """
    # Making a chain of records
    pkg_a = PackageRecord(name="pkg_a", version="1", build="0", build_number=0, subdir="noarch")

    pkg_b = PackageRecord(name="pkg_b", version="1", build="0", build_number=0, subdir="noarch", depends=["pkg_a"])

    pkg_c = PackageRecord(name="pkg_c", version="1", build="0", build_number=0, subdir="noarch", depends=["pkg_b"])

    pkg_d = PackageRecord(name="pkg_d", version="1", build="0", build_number=0, subdir="noarch", depends=["pkg_c"])

    pkg_e = PackageRecord(name="pkg_e", version="1", build="0", build_number=0, subdir="noarch", depends=["pkg_d"])

    # Expected order: A (no deps), B (needs A), C (needs B)...
    expected_order = ["pkg_a", "pkg_b", "pkg_c", "pkg_d", "pkg_e"]

    records = [pkg_a, pkg_b, pkg_c, pkg_d, pkg_e]

    # Run the test multiple times with random shuffles and to verify the names match the expected order
    for _ in range(5):
        random.shuffle(records)
        sorted_records = PackageRecord.sort_topologically(records)
        sorted_names = [r.name.normalized for r in sorted_records]
        assert sorted_names == expected_order
