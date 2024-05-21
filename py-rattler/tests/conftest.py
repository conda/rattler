import os

import pytest

from rattler import Gateway, Channel


@pytest.fixture(scope="session")
def gateway() -> Gateway:
    return Gateway()


@pytest.fixture
def test_data_dir() -> str:
    return os.path.normpath(os.path.join(os.path.dirname(__file__), "../../test-data"))


@pytest.fixture
def conda_forge_channel(test_data_dir: str) -> Channel:
    return Channel(os.path.join(test_data_dir, "channels/conda-forge"))


@pytest.fixture
def pytorch_channel(test_data_dir: str) -> Channel:
    return Channel(os.path.join(test_data_dir, "channels/pytorch"))


@pytest.fixture
def dummy_channel(test_data_dir: str) -> Channel:
    return Channel(os.path.join(test_data_dir, "channels/dummy"))