from typing import List, Optional

import pytest
from rattler import (
    Channel,
    Gateway,
    GenericVirtualPackage,
    MatchSpec,
    PackageName,
    PackageRecord,
    Platform,
    RepoDataRecord,
    Version,
    who_needs,
)
from rattler.repo_data import Dependent


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


@pytest.mark.asyncio
async def test_gateway_who_needs(gateway: Gateway, conda_forge_channel: Channel) -> None:
    # The same lookup through the gateway: the wildcard query and the
    # reverse dependency scan run entirely in Rust, and only the matching
    # records cross into Python.
    dependents = await gateway.who_needs([conda_forge_channel], ["linux-64", "noarch"], "python_abi")
    assert dependents
    assert all(
        PackageName("python_abi").normalized in d.dependency and d.kind in ("depends", "constrains") for d in dependents
    )

    # The result matches running the pure who_needs over a materialized
    # wildcard query.
    wildcard = MatchSpec("*", exact_names_only=False)
    query_result = await gateway.query([conda_forge_channel], ["linux-64", "noarch"], [wildcard], recursive=False)
    all_records = [record for subdir_records in query_result for record in subdir_records]
    direct = who_needs(all_records, "python_abi")
    assert {d.record.name.normalized for d in dependents} == {d.record.name.normalized for d in direct}


@pytest.mark.asyncio
async def test_gateway_who_needs_multi_platform(gateway: Gateway, conda_forge_channel: Channel) -> None:
    # A multi-platform call (with a duplicate platform thrown in) returns
    # exactly the union of the single-platform calls. The order of records
    # within a platform is not deterministic, so compare as multisets.
    combined = await gateway.who_needs([conda_forge_channel], ["linux-64", "noarch", "linux-64"], "python_abi")

    def key(dependent: Dependent) -> tuple[str, str, str, str, str, str]:
        record = dependent.record
        return (
            record.name.normalized,
            str(record.version),
            record.build,
            record.subdir,
            dependent.dependency,
            dependent.kind,
        )

    per_platform = [
        key(dependent)
        for platform in (Platform("linux-64"), Platform("noarch"))
        for dependent in await gateway.who_needs([conda_forge_channel], [platform], "python_abi")
    ]
    assert per_platform  # the fixture channel must actually exercise this
    assert sorted(key(dependent) for dependent in combined) == sorted(per_platform)
