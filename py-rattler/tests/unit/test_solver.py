import datetime
import os

import pytest

from rattler import (
    solve,
    ChannelPriority,
    RepoDataRecord,
    Channel,
    Gateway,
    SparseRepoData,
    MatchSpec,
    solve_with_records,
    solve_with_sparse_repodata,
)


@pytest.mark.asyncio
async def test_solve(gateway: Gateway, conda_forge_channel: Channel) -> None:
    solved_data = await solve(
        [conda_forge_channel],
        ["python", "sqlite"],
        platforms=["linux-64"],
        gateway=gateway,
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 19


@pytest.mark.asyncio
async def test_solve_exclude_newer(gateway: Gateway, dummy_channel: Channel) -> None:
    """Tests the exclude_newer parameter of the solve function.

    The exclude_newer parameter is used to exclude any record that is newer than
    the given datetime. This test case will exclude the record with version
    4.0.2 because of the `exclude_newer` argument.
    """
    solved_data = await solve(
        [dummy_channel],
        ["foo"],
        platforms=["linux-64"],
        gateway=gateway,
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 1
    assert str(solved_data[0].version) == "4.0.2"

    # Now solve again but with a datetime that is before the version 4.0.2
    solved_data = await solve(
        [dummy_channel],
        ["foo"],
        platforms=["linux-64"],
        gateway=gateway,
        exclude_newer=datetime.datetime.fromisoformat("2021-01-01"),
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 1
    assert str(solved_data[0].version) == "3.0.2"


@pytest.mark.asyncio
async def test_solve_lowest(gateway: Gateway, dummy_channel: Channel) -> None:
    solved_data = await solve(
        [dummy_channel],
        ["foobar"],
        platforms=["linux-64"],
        gateway=gateway,
        strategy="lowest",
    )

    assert isinstance(solved_data, list)
    assert len(solved_data) == 2

    assert solved_data[0].name.normalized == "foobar"
    assert str(solved_data[0].version) == "2.0"

    assert solved_data[1].name.normalized == "bors"
    assert str(solved_data[1].version) == "1.0"


@pytest.mark.asyncio
async def test_solve_lowest_direct(gateway: Gateway, dummy_channel: Channel) -> None:
    solved_data = await solve(
        [dummy_channel],
        ["foobar"],
        platforms=["linux-64"],
        gateway=gateway,
        strategy="lowest-direct",
    )

    assert isinstance(solved_data, list)
    assert len(solved_data) == 2

    assert solved_data[0].name.normalized == "foobar"
    assert str(solved_data[0].version) == "2.0"

    assert solved_data[1].name.normalized == "bors"
    assert str(solved_data[1].version) == "1.2.1"


@pytest.mark.asyncio
async def test_solve_channel_priority_disabled(
    gateway: Gateway, pytorch_channel: Channel, conda_forge_channel: Channel
) -> None:
    solved_data = await solve(
        [conda_forge_channel, pytorch_channel],
        ["pytorch-cpu ==0.4.1 py36_cpu_1"],
        platforms=["linux-64"],
        gateway=gateway,
        channel_priority=ChannelPriority.Disabled,
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert (
        list(filter(lambda r: r.file_name.startswith("pytorch-cpu-0.4.1-py36_cpu_1"), solved_data))[0].channel
        == pytorch_channel.base_url
    )
    assert len(solved_data) == 32


@pytest.mark.asyncio
async def test_solve_constraints(gateway: Gateway, dummy_channel: Channel) -> None:
    solved_data = await solve(
        [dummy_channel],
        ["foobar"],
        constraints=["bors <=1", "nonexisting"],
        platforms=["linux-64"],
        gateway=gateway,
    )

    assert isinstance(solved_data, list)
    assert len(solved_data) == 2

    assert solved_data[0].file_name == "foobar-2.1-bla_1.tar.bz2"
    assert solved_data[1].file_name == "bors-1.0-bla_1.tar.bz2"


@pytest.mark.asyncio
async def test_solve_with_repodata() -> None:
    linux64_chan = Channel("conda-forge")
    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    linux64_path = os.path.join(data_dir, "channels/dummy/linux-64/repodata.json")
    linux64_data = SparseRepoData(
        channel=linux64_chan,
        subdir="linux-64",
        path=linux64_path,
    )

    solved_data = await solve_with_sparse_repodata(
        [MatchSpec("foobar")],
        [linux64_data],
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 2


@pytest.mark.asyncio
async def test_conditional_root_requirement_satisfied(gateway: Gateway, dummy_channel: Channel) -> None:
    """Test that a conditional root requirement is included when the condition is satisfied."""
    from rattler import GenericVirtualPackage, MatchSpec, PackageName, Version

    solved_data = await solve(
        [dummy_channel],
        [MatchSpec("foo; if __unix", experimental_conditionals=True)],
        platforms=["linux-64"],
        gateway=gateway,
        virtual_packages=[GenericVirtualPackage(PackageName("__unix"), Version("0"), "0")],
    )

    assert isinstance(solved_data, list)
    assert len(solved_data) > 0
    # Foo should be included because __unix virtual package exists
    package_names = [r.name.normalized for r in solved_data]
    assert "foo" in package_names


@pytest.mark.asyncio
async def test_conditional_root_requirement_not_satisfied(gateway: Gateway, dummy_channel: Channel) -> None:
    """Test that a conditional root requirement is excluded when the condition is not satisfied."""
    from rattler import GenericVirtualPackage, MatchSpec, PackageName, Version

    solved_data = await solve(
        [dummy_channel],
        [MatchSpec("foo; if __win", experimental_conditionals=True)],
        platforms=["linux-64"],
        gateway=gateway,
        virtual_packages=[GenericVirtualPackage(PackageName("__unix"), Version("0"), "0")],
    )

    assert isinstance(solved_data, list)
    # Foo should NOT be included because __win virtual package does not exist
    package_names = [r.name.normalized for r in solved_data]
    assert "foo" not in package_names


@pytest.mark.asyncio
async def test_conditional_root_requirement_with_logic(gateway: Gateway, dummy_channel: Channel) -> None:
    """Test that a conditional root requirement with AND logic is evaluated correctly."""
    from rattler import GenericVirtualPackage, MatchSpec, PackageName, Version

    solved_data = await solve(
        [dummy_channel],
        [MatchSpec("foo; if __unix and __linux", experimental_conditionals=True)],
        platforms=["linux-64"],
        gateway=gateway,
        virtual_packages=[
            GenericVirtualPackage(PackageName("__unix"), Version("0"), "0"),
            GenericVirtualPackage(PackageName("__linux"), Version("0"), "0"),
        ],
    )

    assert isinstance(solved_data, list)
    assert len(solved_data) > 0
    # Foo should be included because both __unix and __linux virtual packages exist
    package_names = [r.name.normalized for r in solved_data]
    assert "foo" in package_names


@pytest.mark.asyncio
async def test_solve_with_sparse_repodata_conditional_dependencies() -> None:
    """Test that solve_with_sparse_repodata handles conditional dependencies in repodata.

    This is a regression test for https://github.com/conda/rattler/issues/1917
    The solver should properly resolve packages with conditional dependencies like
    "osx-dependency; if __osx" when using sparse repodata.
    """
    from rattler import GenericVirtualPackage, PackageName, Version

    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    noarch_path = os.path.join(data_dir, "channels/conditional-repodata/noarch/repodata.json")
    noarch_chan = Channel("conditional-repodata")
    noarch_data = SparseRepoData(
        channel=noarch_chan,
        subdir="noarch",
        path=noarch_path,
    )

    # Test 1: Platform-conditional dependency with __linux virtual package
    solved_data = await solve_with_sparse_repodata(
        [MatchSpec("package")],
        [noarch_data],
        virtual_packages=[
            GenericVirtualPackage(PackageName("__linux"), Version("0"), "0"),
        ],
    )

    assert isinstance(solved_data, list)
    package_names = [r.name.normalized for r in solved_data]
    assert "package" in package_names
    assert "linux-dependency" in package_names
    # Should NOT include osx or win dependencies
    assert "osx-dependency" not in package_names
    assert "win-dependency" not in package_names


@pytest.mark.asyncio
async def test_solve_with_sparse_repodata_version_conditional_dependencies() -> None:
    """Test that solve_with_sparse_repodata handles version-conditional dependencies.

    This is a regression test for https://github.com/conda/rattler/issues/1917
    The solver should properly resolve conditional dependencies like
    "package; if side-dependency=0.2" when the condition is satisfied.
    """
    from rattler import GenericVirtualPackage, PackageName, Version

    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    noarch_path = os.path.join(data_dir, "channels/conditional-repodata/noarch/repodata.json")
    noarch_chan = Channel("conditional-repodata")
    noarch_data = SparseRepoData(
        channel=noarch_chan,
        subdir="noarch",
        path=noarch_path,
    )

    # Test: Version-conditional dependency - when side-dependency=0.2 is requested,
    # "package" should be included due to the conditional "package; if side-dependency=0.2"
    solved_data = await solve_with_sparse_repodata(
        [MatchSpec("conditional-dependency"), MatchSpec("side-dependency=0.2")],
        [noarch_data],
        virtual_packages=[
            GenericVirtualPackage(PackageName("__linux"), Version("0"), "0"),
        ],
    )

    assert isinstance(solved_data, list)
    package_names = [r.name.normalized for r in solved_data]
    assert "conditional-dependency" in package_names
    assert "side-dependency" in package_names
    # "package" should be included because side-dependency=0.2 satisfies the condition
    assert "package" in package_names
    # linux-dependency should also be included because __linux is in virtual packages
    assert "linux-dependency" in package_names


@pytest.mark.asyncio
async def test_solve_with_records(gateway: Gateway, dummy_channel: Channel) -> None:
    """Test solve_with_records by first fetching records via gateway, then solving with them directly."""
    # First, solve using the gateway to get records
    initial_solved = await solve(
        [dummy_channel],
        ["foobar"],
        platforms=["linux-64"],
        gateway=gateway,
    )

    # Now we need to get all available records from the channel
    # We can use the gateway to query for all packages we might need
    all_records = await solve(
        [dummy_channel],
        ["foobar", "bors"],  # Request all packages that might be needed
        platforms=["linux-64"],
        gateway=gateway,
    )

    # Use solve_with_records with the records we got
    # Passing records as a single "channel" (list of records)
    solved_data = await solve_with_records(
        [MatchSpec("foobar")],
        [all_records],  # List of list of records - outer list is channels
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 2

    # Check we got the expected packages
    package_names = [r.name.normalized for r in solved_data]
    assert "foobar" in package_names
    assert "bors" in package_names


@pytest.mark.asyncio
async def test_solve_with_records_from_sparse_repodata() -> None:
    """Test solve_with_records using records loaded from SparseRepoData."""
    linux64_chan = Channel("conda-forge")
    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    linux64_path = os.path.join(data_dir, "channels/dummy/linux-64/repodata.json")
    linux64_data = SparseRepoData(
        channel=linux64_chan,
        subdir="linux-64",
        path=linux64_path,
    )

    # First solve with sparse repodata to get the records
    sparse_solved = await solve_with_sparse_repodata(
        [MatchSpec("foobar")],
        [linux64_data],
    )

    # Now solve with the records directly
    # We need to provide all available records, not just the solved ones
    # Get all records by solving for all packages we might need
    all_records = await solve_with_sparse_repodata(
        [MatchSpec("foobar"), MatchSpec("bors")],
        [linux64_data],
    )

    solved_data = await solve_with_records(
        [MatchSpec("foobar")],
        [all_records],
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 2

    # Verify the results match what we got from sparse repodata
    assert len(solved_data) == len(sparse_solved)
    solved_names = sorted([r.name.normalized for r in solved_data])
    sparse_names = sorted([r.name.normalized for r in sparse_solved])
    assert solved_names == sparse_names
