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
        ["pytorch-cpu 0.4.1 py36_cpu_1"],
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
