import os

import pytest

from rattler import Gateway, Channel


@pytest.fixture(scope="session")
def gateway() -> Gateway:
    return Gateway()

@pytest.fixture
def conda_forge_channel() -> Channel:
    data_dir = os.path.join(os.path.dirname(__file__), "../../test-data/channels/conda-forge")
    return Channel(data_dir)

@pytest.fixture
def pytorch_channel() ->Channel:
    data_dir = os.path.join(os.path.dirname(__file__), "../../test-data/channels/pytorch")
    return Channel(data_dir)
