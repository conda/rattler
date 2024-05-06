import os
from pathlib import Path

import pytest

from rattler import solve, link, Gateway, Channel


@pytest.mark.asyncio
async def test_link(gateway: Gateway, conda_forge_channel: Channel, tmp_path: Path) -> None:
    cache_dir = tmp_path / "cache"
    env_dir = tmp_path / "env"

    solved_data = await solve(
        [conda_forge_channel],
        ["linux-64"],
        ["xtensor"],
        gateway,
    )

    await link(solved_data, env_dir, cache_dir)

    assert os.path.exists(env_dir / "include/xtensor.hpp")
    assert os.path.exists(env_dir / "include/xtensor")
    assert os.path.exists(env_dir / "include/xtl")
