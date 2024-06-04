# type: ignore
import os
from pathlib import Path
import pytest
import shutil

from rattler import Platform, index


@pytest.fixture
def package_directory(tmp_path, package_file_ruff: Path, package_file_pytweening: Path) -> Path:
    win_subdir = tmp_path / "win-64"
    noarch_subdir = tmp_path / "noarch"
    win_subdir.mkdir()
    noarch_subdir.mkdir()
    shutil.copy(package_file_ruff, win_subdir)
    shutil.copy(package_file_pytweening, noarch_subdir)
    return tmp_path


def test_index(package_directory):
    index(package_directory)

    assert set(os.listdir(package_directory)) == {"noarch", "win-64"}
    assert "repodata.json" in os.listdir(package_directory / "win-64")
    with open(package_directory / "win-64/repodata.json") as f:
        assert "ruff-0.0.171-py310h298983d_0" in f.read()
    assert "repodata.json" in os.listdir(package_directory / "noarch")
    with open(package_directory / "noarch/repodata.json") as f:
        assert "pytweening-1.0.4-pyhd8ed1ab_0" in f.read()


def test_index_specific_subdir_non_noarch(package_directory):
    index(package_directory, Platform("win-64"))

    assert "repodata.json" in os.listdir(package_directory / "win-64")
    with open(package_directory / "win-64/repodata.json") as f:
        assert "ruff-0.0.171-py310h298983d_0" in f.read()
    assert "repodata.json" in os.listdir(package_directory / "noarch")
    with open(package_directory / "noarch/repodata.json") as f:
        assert "pytweening-1.0.4-pyhd8ed1ab_0" in f.read()


def test_index_specific_subdir_noarch(package_directory):
    index(package_directory, Platform("noarch"))

    win_files = os.listdir(package_directory / "win-64")
    assert "repodata.json" not in win_files
    assert "ruff-0.0.171-py310h298983d_0.conda" in win_files
    assert "repodata.json" in os.listdir(package_directory / "noarch")
    with open(package_directory / "noarch/repodata.json") as f:
        assert "pytweening-1.0.4-pyhd8ed1ab_0" in f.read()
