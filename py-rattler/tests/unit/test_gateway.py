import pytest

from rattler import Gateway, Channel, SourceConfig


@pytest.mark.asyncio
async def test_single_record_in_recursive_query(gateway: Gateway, conda_forge_channel: Channel) -> None:
    subdirs = await gateway.query(
        [conda_forge_channel], ["linux-64", "noarch"], ["python ==3.10.0 h543edf9_1_cpython"], recursive=True
    )

    python_records = [record for subdir in subdirs for record in subdir if record.name == "python"]
    assert len(python_records) == 1

def test_init_per_channel_config_key():
    test_source_config = SourceConfig()
    # build an incorrect per_channel_config & check for TypeError
    bad_config = {
        123: test_source_config
    }
    with pytest.raises(TypeError):
        Gateway(per_channel_config=bad_config)

    # build right config & make sure gateway object initializes
    right_config= {
        "http://test-config-key.com": test_source_config
    }
    test_gateway = Gateway(per_channel_config=right_config)
    assert test_gateway is not None
