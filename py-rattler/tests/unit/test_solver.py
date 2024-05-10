import pytest

from rattler import (
    solve,
    ChannelPriority,
    RepoDataRecord,
    Channel,
    Gateway,
)


@pytest.mark.asyncio
async def test_solve(gateway: Gateway, conda_forge_channel: Channel) -> None:
    solved_data = await solve(
        [conda_forge_channel],
        ["linux-64"],
        ["python", "sqlite"],
        gateway,
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 19


@pytest.mark.asyncio
async def test_solve_channel_priority_disabled(
    gateway: Gateway, pytorch_channel: Channel, conda_forge_channel: Channel
) -> None:
    solved_data = await solve(
        [conda_forge_channel, pytorch_channel],
        ["linux-64"],
        ["pytorch-cpu=0.4.1=py36_cpu_1"],
        gateway,
        channel_priority=ChannelPriority.Disabled,
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert (
        list(filter(lambda r: r.file_name.startswith("pytorch-cpu-0.4.1-py36_cpu_1"), solved_data))[0].channel
        == pytorch_channel.base_url
    )
    assert len(solved_data) == 32
