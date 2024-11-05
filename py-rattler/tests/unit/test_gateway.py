import pytest

from rattler import Gateway, Channel

@pytest.mark.asyncio
async def test_single_record_in_recursive_query(gateway: Gateway, conda_forge_channel: Channel) -> None:
    records = await gateway.query(
        [conda_forge_channel],
        ["linux-64", "noarch"],
        ["python 3.10.0 h543edf9_1_cpython"],
        recursive=True)

    python_records = [record for record in records if record.name == "python"]
    assert len(python_records) == 1
