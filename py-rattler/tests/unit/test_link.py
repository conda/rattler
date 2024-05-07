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
        ["noarch"],
        ["conda-forge-pinning"],
        gateway,
    )

    await link(solved_data, env_dir, cache_dir)

    assert os.path.exists(env_dir / "conda_build_config.yaml")
    assert os.path.exists(env_dir / "share/conda-forge/migrations/pypy37.yaml")
    assert os.path.exists(env_dir / "share/conda-forge/migrations/pypy37-windows.yaml")
