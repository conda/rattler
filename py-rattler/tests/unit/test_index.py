import os
from pathlib import Path
import pytest
import shutil

from rattler import Platform, index


@pytest.fixture
def package_directory(tmp_path) -> Path:
    data_dir = Path(os.path.join(os.path.dirname(__file__), "../../../test-data/"))

    win_filename = "ruff-0.0.171-py310h298983d_0.conda"
    noarch_filename = "pytweening-1.0.4-pyhd8ed1ab_0.tar.bz2"
    win_subdir = tmp_path / "win-64"
    noarch_subdir = tmp_path / "noarch"
    win_subdir.mkdir()
    noarch_subdir.mkdir()
    shutil.copy(data_dir / win_filename, win_subdir / win_filename)
    shutil.copy(data_dir / noarch_filename, noarch_subdir / noarch_filename)
    return tmp_path


def test_index(package_directory):
    assert index(package_directory) == True

    assert set(os.listdir(package_directory)) == {"noarch", "win-64"}
    assert "repodata.json" in os.listdir(package_directory / "win-64")
    with open(package_directory / "win-64/repodata.json") as f:
        assert "ruff-0.0.171-py310h298983d_0" in f.read()
    assert "repodata.json" in os.listdir(package_directory / "noarch")
    with open(package_directory / "noarch/repodata.json") as f:
        assert "pytweening-1.0.4-pyhd8ed1ab_0" in f.read()


def test_index_specific_subdir_non_noarch(package_directory):
    assert index(package_directory, Platform("win-64")) == True

    assert "repodata.json" in os.listdir(package_directory / "win-64")
    with open(package_directory / "win-64/repodata.json") as f:
        assert "ruff-0.0.171-py310h298983d_0" in f.read()
    assert "repodata.json" in os.listdir(package_directory / "noarch")
    with open(package_directory / "noarch/repodata.json") as f:
        assert "pytweening-1.0.4-pyhd8ed1ab_0" in f.read()


def test_index_specific_subdir_noarch(package_directory):
    assert index(package_directory, Platform("noarch")) == True

    win_files = os.listdir(package_directory / "win-64")
    assert "repodata.json" not in win_files
    assert "ruff-0.0.171-py310h298983d_0.conda" in win_files
    assert "repodata.json" in os.listdir(package_directory / "noarch")
    with open(package_directory / "noarch/repodata.json") as f:
        assert "pytweening-1.0.4-pyhd8ed1ab_0" in f.read()
