import json
from pathlib import Path

import pytest

from rattler import Gateway, Channel, SourceConfig


@pytest.mark.asyncio
async def test_single_record_in_recursive_query(gateway: Gateway, conda_forge_channel: Channel) -> None:
    subdirs = await gateway.query(
        [conda_forge_channel], ["linux-64", "noarch"], ["python ==3.10.0 h543edf9_1_cpython"], recursive=True
    )

    python_records = [record for subdir in subdirs for record in subdir if record.name == "python"]
    assert len(python_records) == 1


@pytest.mark.asyncio
async def test_channel_notices(tmp_path: Path) -> None:
    (tmp_path / "notices.json").write_text(
        json.dumps(
            {
                "notices": [
                    {
                        "id": "security-1",
                        "message": "Please update demo",
                        "level": "critical",
                        "created_at": "2025-01-01T00:00:00Z",
                        "expires_at": "2099-01-01T00:00:00Z",
                    }
                ]
            }
        )
    )

    gateway = Gateway(channel_notices=True)
    notices = await gateway.channel_notices([Channel(str(tmp_path))])
    assert len(notices) == 1
    assert notices[0].id == "security-1"
    assert notices[0].level == "critical"
    assert notices[0].expires_at == "2099-01-01T00:00:00Z"


def test_init_per_channel_config_key() -> None:
    test_source_config = SourceConfig()

    # build an incorrect per_channel_config & check for TypeError
    channel = Channel("https://conda.anaconda.org/conda-forge")
    # per_channel_config uses a Channel object as the key — this is what caused the original bug
    per_channel_config = {channel: test_source_config}
    with pytest.raises(TypeError):
        Gateway(per_channel_config=per_channel_config)  # type: ignore[arg-type]

    # build right config & make sure gateway object initializes
    right_config = {"http://test-config-key.com": test_source_config}
    test_gateway = Gateway(per_channel_config=right_config)
    assert test_gateway is not None
