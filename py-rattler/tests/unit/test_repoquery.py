from typing import List, Optional

import pytest
from rattler import (
    GenericVirtualPackage,
    PackageName,
    PackageRecord,
    RepoDataRecord,
    Version,
    who_needs,
)


def record(
    name: str,
    version: str,
    build: str,
    depends: List[str],
    constrains: Optional[List[str]] = None,
) -> RepoDataRecord:
    return RepoDataRecord(
        PackageRecord(
            name=name,
            version=version,
            build=build,
            build_number=0,
            subdir="linux-64",
            depends=depends,
            constrains=constrains or [],
        ),
        f"{name}-{version}-{build}.conda",
        f"https://example.com/{name}-{version}-{build}.conda",
        "https://example.com/test-channel",
    )


@pytest.fixture
def records() -> List[RepoDataRecord]:
    return [
        record("python", "3.13.0", "0", []),
        record("old-lib", "1.0.0", "0", ["python >=3.8,<3.10"]),
        record("numpy", "2.1.0", "0", ["python >=3.10"]),
        record("pandas-stubs", "2.2.0", "0", [], ["pandas >=2.2"]),
        record("cuda-tool", "1.0.0", "0", ["__cuda >=12"]),
        record("cpython-lib", "1.0.0", "0", ["python 3.13.* *_cpython"]),
    ]


def test_who_needs_by_name(records: List[RepoDataRecord]) -> None:
    dependents = who_needs(records, "python")
    assert [d.record.name.normalized for d in dependents] == [
        "old-lib",
        "numpy",
        "cpython-lib",
    ]
    assert dependents[0].dependency == "python >=3.8,<3.10"
    assert dependents[0].kind == "depends"
    assert dependents[0].run_export_kind is None

    # A PackageName target behaves the same as a str.
    assert [d.record.name.normalized for d in who_needs(records, PackageName("python"))] == [
        "old-lib",
        "numpy",
        "cpython-lib",
    ]


def test_who_needs_by_record(records: List[RepoDataRecord]) -> None:
    # Only dependents whose match spec matches the concrete record are
    # reported: old-lib requires <3.10 and cpython-lib a *_cpython build.
    python = record("python", "3.13.1", "h123_0", [])
    assert [d.record.name.normalized for d in who_needs(records, python)] == ["numpy"]

    python_cpython = record("python", "3.13.1", "h123_0_cpython", [])
    assert [d.record.name.normalized for d in who_needs(records, python_cpython)] == [
        "numpy",
        "cpython-lib",
    ]


def test_who_needs_constrains(records: List[RepoDataRecord]) -> None:
    dependents = who_needs(records, "pandas")
    assert [(d.record.name.normalized, d.kind) for d in dependents] == [("pandas-stubs", "constrains")]


def test_who_needs_virtual_package(records: List[RepoDataRecord]) -> None:
    # Name-based matching works for virtual packages, which have no
    # records of their own.
    assert [d.record.name.normalized for d in who_needs(records, "__cuda")] == ["cuda-tool"]

    # A concrete virtual package matches against the dependency's version
    # constraint.
    cuda = GenericVirtualPackage(PackageName("__cuda"), Version("12.4"), "0")
    assert [d.record.name.normalized for d in who_needs(records, cuda)] == ["cuda-tool"]
    old_cuda = GenericVirtualPackage(PackageName("__cuda"), Version("11.8"), "0")
    assert who_needs(records, old_cuda) == []


def test_who_needs_invalid_target(records: List[RepoDataRecord]) -> None:
    with pytest.raises(TypeError, match="expected a str"):
        who_needs(records, 42)  # type: ignore[arg-type]
