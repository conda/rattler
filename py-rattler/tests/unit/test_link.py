# type: ignore
import os
import pytest

from rattler import Channel, SparseRepoData, MatchSpec, solve, link


@pytest.mark.asyncio
async def test_link(tmp_path):
    cache_dir = tmp_path / "cache"
    env_dir = tmp_path / "env"

    linux64_chan = Channel("conda-forge")
    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    linux64_path = os.path.join(data_dir, "channels/conda-forge/linux-64/repodata.json")
    linux64_data = SparseRepoData(
        channel=linux64_chan,
        subdir="linux-64",
        path=linux64_path,
    )

    solved_data = solve(
        [MatchSpec("xtensor")],
        [linux64_data],
    )

    await link(solved_data, env_dir, cache_dir)

    assert os.path.exists(env_dir / "include/xtensor.hpp")
    assert os.path.exists(env_dir / "include/xtensor")
    assert os.path.exists(env_dir / "include/xtl")
