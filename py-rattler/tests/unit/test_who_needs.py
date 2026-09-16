import pytest
from rattler import (
    Channel,
    Gateway,
    GenericVirtualPackage,
    PackageName,
    Platform,
    Version,
)
from rattler.repo_data import Dependent


def summary(dependents: list[Dependent]) -> list[tuple[str, str, str]]:
    """The dependents as sorted `(package, kind, dependency)` triples."""
    return sorted((d.record.name.normalized, d.kind, d.dependency) for d in dependents)


def extras(dependents: list[Dependent]) -> list[str]:
    """The sorted names of the extras the dependents come from."""
    return sorted(d.extra for d in dependents if d.extra is not None)


@pytest.mark.asyncio
async def test_who_needs_by_name(gateway: Gateway, dummy_channel: Channel) -> None:
    # A name target reports every dependency naming the package,
    # regardless of its version constraints, across `depends` and
    # `constrains`.
    dependents = await gateway.who_needs([dummy_channel], ["linux-64"], "bors")
    assert summary(dependents) == [
        ("foo", "constrains", "bors <2.0"),
        # Both foobar builds in the subdir depend on bors.
        ("foobar", "depends", "bors <2.0"),
        ("foobar", "depends", "bors <2.0"),
    ]
    assert all(d.run_export_kind is None and d.extra is None for d in dependents)

    # A PackageName target behaves the same as a str.
    by_name = await gateway.who_needs([dummy_channel], ["linux-64"], PackageName("bors"))
    assert summary(by_name) == summary(dependents)


@pytest.mark.asyncio
async def test_who_needs_by_record(gateway: Gateway, dummy_channel: Channel) -> None:
    # Only dependents whose match spec matches the concrete record are
    # reported: every edge on bors requires <2.0.
    records = await gateway.query([dummy_channel], ["linux-64"], ["bors"], recursive=False)
    by_version = {str(record.version): record for record in records[0]}

    dependents = await gateway.who_needs([dummy_channel], ["linux-64"], by_version["1.1"])
    assert {name for name, _, _ in summary(dependents)} == {"foo", "foobar"}

    assert await gateway.who_needs([dummy_channel], ["linux-64"], by_version["2.1"]) == []


@pytest.mark.asyncio
async def test_who_needs_virtual_package(gateway: Gateway, dummy_channel: Channel) -> None:
    # Name-based matching works for virtual packages, which have no
    # records of their own.
    dependents = await gateway.who_needs([dummy_channel], ["linux-64"], "__cuda")
    assert summary(dependents) == [("cuda-version", "constrains", "__cuda >=12.1")]

    # A concrete virtual package matches against the dependency's version
    # constraint.
    cuda = GenericVirtualPackage(PackageName("__cuda"), Version("12.5"), "0")
    assert summary(await gateway.who_needs([dummy_channel], ["linux-64"], cuda)) == summary(dependents)
    old_cuda = GenericVirtualPackage(PackageName("__cuda"), Version("11.8"), "0")
    assert await gateway.who_needs([dummy_channel], ["linux-64"], old_cuda) == []


@pytest.mark.asyncio
async def test_who_needs_extra_depends(gateway: Gateway, test_data_dir: str) -> None:
    # Dependencies declared under an optional feature are reported with the
    # name of the extra in `extra`.
    channel = Channel(f"{test_data_dir}/channels/dummy-optional-dependencies")
    dependents = await gateway.who_needs([channel], ["noarch"], "bar")
    assert summary(dependents) == [
        ("conflicting-extras", "extra_depends", "bar <2"),
        ("conflicting-extras", "extra_depends", "bar >=2"),
        ("foo", "extra_depends", "bar <2"),
    ]
    assert extras(dependents) == ["extra1", "extra2", "with-bar"]

    # A concrete target still has to satisfy the extra's constraints:
    # bar 1 matches extra1's `bar <2` but not extra2's `bar >=2`.
    records = await gateway.query([channel], ["noarch"], ["bar"], recursive=False)
    bar_1 = next(record for record in records[0] if str(record.version) == "1")
    assert extras(await gateway.who_needs([channel], ["noarch"], bar_1)) == ["extra1", "with-bar"]


@pytest.mark.asyncio
async def test_who_needs_multi_platform(gateway: Gateway, conda_forge_channel: Channel) -> None:
    # A multi-platform call (with a duplicate platform thrown in) returns
    # exactly the union of the single-platform calls. The order of records
    # within a platform is not deterministic, so compare as multisets.
    combined = await gateway.who_needs([conda_forge_channel], ["linux-64", "noarch", "linux-64"], "python_abi")
    assert combined
    assert all(
        PackageName("python_abi").normalized in d.dependency and d.kind in ("depends", "constrains") for d in combined
    )

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
