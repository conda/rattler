import os
from pathlib import Path

import pytest

from rattler import solve, install, Gateway, Channel


class RecordingDelegate:
    """Captures install progress events for assertions."""

    def __init__(self) -> None:
        self.events: list[tuple[str, str]] = []

    def on_unlink_start(self, package_name: str) -> None:
        self.events.append(("unlink_start", package_name))

    def on_unlink_complete(self, package_name: str) -> None:
        self.events.append(("unlink_complete", package_name))

    def on_link_start(self, package_name: str) -> None:
        self.events.append(("link_start", package_name))

    def on_link_complete(self, package_name: str) -> None:
        self.events.append(("link_complete", package_name))


class FailingDelegate:
    """A delegate that raises on link_start to test error propagation."""

    def on_link_start(self, package_name: str) -> None:
        raise RuntimeError(f"delegate error on {package_name}")


class PartialDelegate:
    """A delegate that only implements a subset of progress callbacks."""

    def __init__(self) -> None:
        self.link_starts: list[str] = []

    def on_link_start(self, package_name: str) -> None:
        self.link_starts.append(package_name)


class FailingUnlinkDelegate:
    """A delegate that raises on unlink_start to test unlink error propagation."""

    def on_unlink_start(self, package_name: str) -> None:
        raise RuntimeError(f"unlink delegate error on {package_name}")


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


@pytest.mark.asyncio
async def test_install_with_progress_delegate(gateway: Gateway, conda_forge_channel: Channel, tmp_path: Path) -> None:
    cache_dir = tmp_path / "cache"
    env_dir = tmp_path / "env"

    solved_data = await solve(
        [conda_forge_channel],
        ["conda-forge-pinning"],
        platforms=["noarch"],
        gateway=gateway,
    )

    delegate = RecordingDelegate()
    await install(solved_data, env_dir, cache_dir, progress_delegate=delegate)

    # Should have received link_start and link_complete for the package.
    link_starts = [name for ev, name in delegate.events if ev == "link_start"]
    link_completes = [name for ev, name in delegate.events if ev == "link_complete"]
    assert len(link_starts) > 0
    assert len(link_completes) > 0
    assert "conda-forge-pinning" in link_starts
    assert "conda-forge-pinning" in link_completes


@pytest.mark.asyncio
async def test_reinstall_with_progress_delegate(gateway: Gateway, conda_forge_channel: Channel, tmp_path: Path) -> None:
    cache_dir = tmp_path / "cache"
    env_dir = tmp_path / "env"

    solved_data = await solve(
        [conda_forge_channel],
        ["conda-forge-pinning"],
        platforms=["noarch"],
        gateway=gateway,
    )

    # First install (no delegate).
    await install(solved_data, env_dir, cache_dir)

    # Reinstall with delegate — should see both unlink and link events.
    delegate = RecordingDelegate()
    await install(
        solved_data,
        env_dir,
        cache_dir,
        reinstall_packages={"conda-forge-pinning"},
        progress_delegate=delegate,
    )

    unlink_starts = [name for ev, name in delegate.events if ev == "unlink_start"]
    unlink_completes = [name for ev, name in delegate.events if ev == "unlink_complete"]
    link_starts = [name for ev, name in delegate.events if ev == "link_start"]
    link_completes = [name for ev, name in delegate.events if ev == "link_complete"]
    assert "conda-forge-pinning" in unlink_starts
    assert "conda-forge-pinning" in unlink_completes
    assert "conda-forge-pinning" in link_starts
    assert "conda-forge-pinning" in link_completes


@pytest.mark.asyncio
async def test_install_delegate_exception_aborts(gateway: Gateway, conda_forge_channel: Channel, tmp_path: Path) -> None:
    cache_dir = tmp_path / "cache"
    env_dir = tmp_path / "env"

    solved_data = await solve(
        [conda_forge_channel],
        ["conda-forge-pinning"],
        platforms=["noarch"],
        gateway=gateway,
    )

    delegate = FailingDelegate()
    with pytest.raises(RuntimeError, match="delegate error"):
        await install(solved_data, env_dir, cache_dir, progress_delegate=delegate)


@pytest.mark.asyncio
async def test_install_with_partial_progress_delegate(gateway: Gateway, conda_forge_channel: Channel, tmp_path: Path) -> None:
    cache_dir = tmp_path / "cache"
    env_dir = tmp_path / "env"

    solved_data = await solve(
        [conda_forge_channel],
        ["conda-forge-pinning"],
        platforms=["noarch"],
        gateway=gateway,
    )

    delegate = PartialDelegate()
    await install(solved_data, env_dir, cache_dir, progress_delegate=delegate)

    assert "conda-forge-pinning" in delegate.link_starts
    assert os.path.exists(env_dir / "conda_build_config.yaml")


@pytest.mark.asyncio
async def test_reinstall_unlink_delegate_exception_aborts(gateway: Gateway, conda_forge_channel: Channel, tmp_path: Path) -> None:
    cache_dir = tmp_path / "cache"
    env_dir = tmp_path / "env"

    solved_data = await solve(
        [conda_forge_channel],
        ["conda-forge-pinning"],
        platforms=["noarch"],
        gateway=gateway,
    )

    await install(solved_data, env_dir, cache_dir)

    delegate = FailingUnlinkDelegate()
    with pytest.raises(RuntimeError, match="unlink delegate error"):
        await install(
            solved_data,
            env_dir,
            cache_dir,
            reinstall_packages={"conda-forge-pinning"},
            progress_delegate=delegate,
        )
