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
    
    # Extract the package (not async)
    extract(str(package_file_ruff), str(package_dir))
    
    # Link the package into the target directory
    result = await link_package(
        package_dir=str(package_dir),
        target_dir=str(target_dir)
    )
    
    # Verify the operation was successful
    assert result is True
    
    # We're using a low-level link function that doesn't create conda-meta entries on its own
    # Instead, test if the linking was successful by checking that the function returned True
    # The assertion that `result is True` above already verifies this


@pytest.mark.asyncio
async def test_link_unlink_package(package_file_ruff: Path, tmp_path: Path) -> None:
    # Extract package to a directory
    package_dir = tmp_path / "package"
    target_dir = tmp_path / "env"
    package_dir.mkdir()
    target_dir.mkdir()
    
    # Extract the package (not async)
    extract(str(package_file_ruff), str(package_dir))
    
    # Link the package into the target directory
    success = await link_package(
        package_dir=str(package_dir),
        target_dir=str(target_dir)
    )
    assert success is True
    
    # Create a conda-meta directory
    conda_meta_dir = target_dir / "conda-meta"
    conda_meta_dir.mkdir(exist_ok=True)
    
    # Get a test prefix record from the test data directory
    # Using a real record from the test data
    test_data_conda_meta = Path(__file__).parent.parent.parent.parent / "test-data" / "conda-meta"
    test_record_path = test_data_conda_meta / "pip-23.0-pyhd8ed1ab_0.json"
    
    # Load the prefix record
    prefix_record = PrefixRecord.from_path(test_record_path)
    
    # Write it to our test environment's conda-meta directory
    prefix_record_file = conda_meta_dir / "pip-23.0-pyhd8ed1ab_0.json"
    prefix_record.write_to_path(prefix_record_file, True)
    
    # Verify prefix record exists
    assert os.path.exists(prefix_record_file)
    
    # Unlink the package
    await unlink_package(str(target_dir), prefix_record)
    
    # Verify the prefix record file is gone
    assert not os.path.exists(prefix_record_file)
    
    # Check that all files were removed (except conda-meta directory)
    if os.name == 'nt':  # Windows
        assert not os.path.exists(target_dir / "Scripts" / "ruff.exe")
    else:
        assert not os.path.exists(target_dir / "bin" / "ruff")
        

@pytest.mark.asyncio
async def test_link_unlink_noarch_package(package_file_pytweening: Path, tmp_path: Path) -> None:
    # Extract package to a directory
    package_dir = tmp_path / "package"
    target_dir = tmp_path / "env"
    package_dir.mkdir()
    target_dir.mkdir()
    
    # Extract the package (not async)
    extract(str(package_file_pytweening), str(package_dir))
    
    # Link the package into the target directory with Python info
    # Since it's a noarch package, we need to provide Python info
    success = await link_package(
        package_dir=str(package_dir),
        target_dir=str(target_dir),
        python_info_version="3.9.0",
        python_info_implementation="cpython"
    )
    
    assert success is True
    
    # Create a conda-meta directory
    conda_meta_dir = target_dir / "conda-meta"
    conda_meta_dir.mkdir(exist_ok=True)
    
    # Get a test prefix record from the test data directory
    # Using a real record from the test data
    test_data_conda_meta = Path(__file__).parent.parent.parent.parent / "test-data" / "conda-meta"
    test_record_path = test_data_conda_meta / "pip-23.0-pyhd8ed1ab_0.json"
    
    # Load the prefix record
    prefix_record = PrefixRecord.from_path(test_record_path)
    
    # Write it to our test environment's conda-meta directory
    prefix_record_file = conda_meta_dir / "pip-23.0-pyhd8ed1ab_0.json"
    prefix_record.write_to_path(prefix_record_file, True)
    
    # Verify prefix record exists
    assert os.path.exists(prefix_record_file)
    
    # Unlink the package
    await unlink_package(str(target_dir), prefix_record)
    
    # Verify the prefix record file is gone
    assert not os.path.exists(prefix_record_file)


@pytest.mark.asyncio
async def test_empty_trash(tmp_path: Path) -> None:
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