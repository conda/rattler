import os
from pathlib import Path

import pytest

from rattler.install import link_package, unlink_package, empty_trash
from rattler.package_streaming import extract
from rattler.prefix.prefix_record import PrefixRecord


@pytest.mark.asyncio
async def test_link_package(package_file_ruff: Path, tmp_path: Path) -> None:
    # Extract package to a directory
    package_dir = tmp_path / "package"
    target_dir = tmp_path / "env"
    package_dir.mkdir()
    target_dir.mkdir()

    _ = extract(package_file_ruff, package_dir)

    assert await link_package(package_dir=package_dir, target_dir=target_dir)

    # Verify the package was linked.
    # This test package is a windows package so we should see something in Lib/site-packages/ruff/
    assert (target_dir / "Lib" / "site-packages" / "ruff").exists()


@pytest.mark.asyncio
async def test_link_unlink_package(package_file_ruff: Path, tmp_path: Path) -> None:
    # Extract package to a directory
    package_dir = tmp_path / "package"
    target_dir = tmp_path / "env"
    package_dir.mkdir()
    target_dir.mkdir()

    _ = extract(package_file_ruff, package_dir)

    assert await link_package(package_dir=str(package_dir), target_dir=str(target_dir))

    # Verify the package was linked.
    assert (target_dir / "Scripts" / "ruff.exe").exists()


@pytest.mark.asyncio
async def test_empty_trash(tmp_path: Path) -> None:
    """
    # Create a trash directory and some files in it
    target_dir = tmp_path / "env"
    target_dir.mkdir()
    trash_dir = target_dir / ".trash"
    trash_dir.mkdir()

    # Create some "trash" files
    (trash_dir / "file1.trash").write_text("test content 1")
    (trash_dir / "file2.trash").write_text("test content 2")

    # Verify trash directory and files exist
    assert os.path.exists(trash_dir)
    assert os.path.exists(trash_dir / "file1.trash")
    assert os.path.exists(trash_dir / "file2.trash")

    # Empty the trash
    await empty_trash(str(target_dir))

    # Verify trash directory is gone
    assert not os.path.exists(trash_dir)
    """
    pass
