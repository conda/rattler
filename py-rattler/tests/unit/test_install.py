import os
from pathlib import Path

import pytest

from rattler import solve, install, Gateway, Channel


@pytest.mark.asyncio
async def test_install(gateway: Gateway, conda_forge_channel: Channel, tmp_path: Path) -> None:
    cache_dir = tmp_path / "cache"
    env_dir = tmp_path / "env"

    solved_data = await solve(
        [conda_forge_channel],
        ["conda-forge-pinning"],
        platforms=["noarch"],
        gateway=gateway,
    )

    await install(solved_data, env_dir, cache_dir)

    assert os.path.exists(env_dir / "conda_build_config.yaml")
    assert os.path.exists(env_dir / "share/conda-forge/migrations/pypy37.yaml")
    assert os.path.exists(env_dir / "share/conda-forge/migrations/pypy37-windows.yaml")


@pytest.mark.asyncio
async def test_reinstall(gateway: Gateway, conda_forge_channel: Channel, tmp_path: Path) -> None:
    cache_dir = tmp_path / "cache"
    env_dir = tmp_path / "env"

    solved_data = await solve(
        [conda_forge_channel],
        ["conda-forge-pinning"],
        platforms=["noarch"],
        gateway=gateway,
    )

    await install(solved_data, env_dir, cache_dir)

    assert os.path.exists(env_dir / "conda_build_config.yaml")
    assert os.path.exists(env_dir / "share/conda-forge/migrations/pypy37.yaml")
    assert os.path.exists(env_dir / "share/conda-forge/migrations/pypy37-windows.yaml")

    # Remove a file and re-install
    os.remove(env_dir / "share" / "conda-forge" / "migrations" / "pypy37.yaml")
    await install(solved_data, env_dir, cache_dir, reinstall_packages={"conda-forge-pinning"})
    assert os.path.exists(env_dir / "share" / "conda-forge" / "migrations" / "pypy37.yaml")
