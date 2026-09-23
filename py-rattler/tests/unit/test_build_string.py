import json
from pathlib import Path

import pytest

from rattler import BuildString, GenericVirtualPackage, IndexJson, PackageName, PackageRecord, Version


def record(build: str | BuildString) -> PackageRecord:
    return PackageRecord("demo", "1.0", build, 0, "noarch")


def index_json(build: str = "0") -> IndexJson:
    return IndexJson.from_str(json.dumps({"name": "demo", "version": "1.0", "build": build, "build_number": 0}))


@pytest.mark.parametrize("value", ["0", "py_0", "Ab9_.+", "a" * 64])
def test_checked_build_strings(value: str) -> None:
    build = BuildString(value)
    assert str(build) == value
    assert repr(build) == f"BuildString({value!r})"
    assert build == BuildString(value)
    assert hash(build) == hash(BuildString(value))
    assert build != BuildString("other")
    for argument in (value, build):
        package = record(argument)
        assert package.build == build
        assert isinstance(package.build, BuildString)
        package.build = argument
        assert package.build == build
        index = index_json()
        index.build = argument
        assert index.build == build
        assert isinstance(index.build, BuildString)
        virtual = GenericVirtualPackage(PackageName("__demo"), Version("1"), argument)
        assert virtual.build_string == build
        assert isinstance(virtual.build_string, BuildString)


@pytest.mark.parametrize("value", ["", "py3-none-any", "é", "a" * 65])
def test_invalid_build_strings_require_explicit_unchecked(value: str) -> None:
    with pytest.raises(ValueError):
        BuildString(value)
    with pytest.raises(ValueError):
        record(value)
    with pytest.raises(ValueError):
        GenericVirtualPackage(PackageName("__demo"), Version("1"), value)

    unchecked = BuildString.new_unchecked(value)
    assert str(unchecked) == value
    assert record(unchecked).build == unchecked
    virtual = GenericVirtualPackage(PackageName("__demo"), Version("1"), unchecked)
    assert virtual.build_string == unchecked
    for target in (record("0"), index_json()):
        with pytest.raises(ValueError):
            target.build = value
        assert target.build == BuildString("0")
        target.build = unchecked
        assert target.build == unchecked
    assert json.loads(record(unchecked).to_json())["build"] == value


@pytest.mark.parametrize("value", ["", "py3-none-any"])
def test_reading_and_copying_legacy_metadata_does_not_validate(value: str, tmp_path: Path) -> None:
    index = index_json(value)
    assert index.build == BuildString.new_unchecked(value)
    path = tmp_path / "index.json"
    path.write_text(
        json.dumps({"name": "demo", "version": "1.0", "build": value, "build_number": 0, "subdir": "noarch"})
    )
    package = PackageRecord.from_index_json(path)
    assert package.build == index.build
    for build in (index.build, package.build):
        copied = record(build)
        assert copied.build == build
        for target in (copied, index_json()):
            target.build = build
            target.build = target.build
            assert str(target.build) == value
        virtual = GenericVirtualPackage(PackageName("__demo"), Version("1"), build)
        assert record(virtual.build_string).build == build
    # Changing a record does not mutate previously returned build objects.
    build = package.build
    package.build = "0"
    assert str(build) == value
