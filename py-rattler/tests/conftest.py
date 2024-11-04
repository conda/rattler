import os
from pathlib import Path

import pytest
import requests
from pytest import TempPathFactory

from rattler import Gateway, Channel


@pytest.fixture(scope="session")
def gateway() -> Gateway:
    return Gateway()


@pytest.fixture
def test_data_dir() -> str:
    return os.path.normpath(os.path.join(os.path.dirname(__file__), "../../test-data"))


@pytest.fixture
def conda_forge_channel(test_data_dir: str, conda_forge_repo_data: Path) -> Channel:
    return Channel(str(conda_forge_repo_data))


@pytest.fixture
def pytorch_channel(test_data_dir: str) -> Channel:
    return Channel(os.path.join(test_data_dir, "channels/pytorch"))


@pytest.fixture
def dummy_channel(test_data_dir: str) -> Channel:
    return Channel(os.path.join(test_data_dir, "channels/dummy"))


@pytest.fixture(scope="session")
def package_file_ruff(tmp_path_factory: TempPathFactory) -> Path:
    destination = tmp_path_factory.getbasetemp() / "ruff-0.0.171-py310h298983d_0.conda"

    r = requests.get("https://conda.anaconda.org/conda-forge/win-64/ruff-0.0.171-py310h298983d_0.conda")
    with open(destination, "wb") as f:
        f.write(r.content)

    return destination


@pytest.fixture(scope="session")
def package_file_pytweening(tmp_path_factory: TempPathFactory) -> Path:
    destination = tmp_path_factory.getbasetemp() / "pytweening-1.0.4-pyhd8ed1ab_0.tar.bz2"

    r = requests.get("https://conda.anaconda.org/conda-forge/noarch/pytweening-1.0.4-pyhd8ed1ab_0.tar.bz2")
    with open(destination, "wb") as f:
        f.write(r.content)

    return destination


@pytest.fixture(scope="session")
def conda_forge_repo_data(tmp_path_factory: TempPathFactory) -> Path:
    destination = tmp_path_factory.mktemp("conda-forge-repo-data", numbered=False)

    for subdir in ["noarch", "linux-64"]:
        subdir_path = destination / subdir
        repodata_json_path = subdir_path / "repodata.json"
        if os.path.exists(repodata_json_path):
            continue

        subdir_path.mkdir()

        r = requests.get(f"https://rattler-test.pixi.run/test-data/channels/conda-forge/{subdir}/repodata.json")
        with open(repodata_json_path, "wb") as f:
            f.write(r.content)

    return destination
