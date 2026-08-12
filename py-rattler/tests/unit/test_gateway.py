import json
from pathlib import Path

import pytest

from rattler import Gateway, Channel, SourceConfig


def _write_repodata(
    channel: Path,
    subdir: str,
    revisions: dict[str, dict[str, object]],
) -> None:
    subdir_path = channel / subdir
    subdir_path.mkdir(parents=True, exist_ok=True)
    (subdir_path / "repodata.json").write_text(
        json.dumps(
            {
                "repodata_version": 1,
                "info": {"subdir": subdir, "repodata_revisions": revisions},
                "packages": {},
                "packages.conda": {
                    "demo-1.0-0.conda": {
                        "build": "0",
                        "build_number": 0,
                        "depends": [],
                        "md5": "82ecc40f09b9c44483e6b70cad2545d7",
                        "name": "demo",
                        "sha256": "eb65e866067865793b981c2ba74485f75bef441842b5998badc4ec66717685c7",
                        "size": 1234,
                        "subdir": subdir,
                        "timestamp": 1689209309623,
                        "version": "1.0",
                    }
                },
            }
        )
    )


@pytest.mark.asyncio
async def test_single_record_in_recursive_query(gateway: Gateway, conda_forge_channel: Channel) -> None:
    subdirs = await gateway.query(
        [conda_forge_channel], ["linux-64", "noarch"], ["python ==3.10.0 h543edf9_1_cpython"], recursive=True
    )

    python_records = [record for subdir in subdirs for record in subdir if record.name == "python"]
    assert len(python_records) == 1


@pytest.mark.asyncio
async def test_channel_notices(tmp_path: Path) -> None:
    noarch = tmp_path / "noarch"
    noarch.mkdir()
    (noarch / "repodata.json").write_text(
        json.dumps(
            {
                "packages": {
                    "demo-1.0-0.tar.bz2": {
                        "name": "demo",
                        "version": "1.0",
                        "build": "0",
                        "build_number": 0,
                        "depends": [],
                        "subdir": "noarch",
                    }
                }
            }
        )
    )
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

    gateway = Gateway()
    channel = Channel(str(tmp_path))
    notices = await gateway.channel_notices([channel])
    assert len(notices) == 1
    assert notices[0].id == "security-1"
    assert notices[0].level == "critical"
    assert notices[0].expires_at == "2099-01-01T00:00:00Z"

    result = await gateway.query([channel], ["noarch"], ["demo"], channel_notices=True)
    assert result.repodata is result
    assert result.notices == notices
    assert result.unsupported_repodata_revisions == []

    names = await gateway.names([channel], ["noarch"], channel_notices=True)
    assert names.names is names
    assert names.notices == notices
    assert names.unsupported_repodata_revisions == []


@pytest.mark.asyncio
async def test_unsupported_repodata_revisions_are_query_metadata(tmp_path: Path) -> None:
    gateway = Gateway()

    supported_channel_path = tmp_path / "supported"
    _write_repodata(supported_channel_path, "noarch", {"v3": {}})
    supported_channel = Channel(str(supported_channel_path))
    supported = await gateway.query([supported_channel], ["noarch"], ["demo"])

    assert supported.repodata is supported
    assert [[record.name for record in subdir] for subdir in supported] == [["demo"]]
    assert supported.unsupported_repodata_revisions == []

    unsupported_channel_path = tmp_path / "unsupported"
    _write_repodata(unsupported_channel_path, "noarch", {"v1": {}})
    unsupported_channel = Channel(str(unsupported_channel_path))
    unsupported = await gateway.query([unsupported_channel], ["noarch"], ["demo"])

    assert [[record.name for record in subdir] for subdir in unsupported] == [["demo"]]
    assert len(unsupported.unsupported_repodata_revisions) == 1
    report = unsupported.unsupported_repodata_revisions[0]
    assert report.channel == unsupported_channel.base_url
    assert report.subdir == "noarch"
    assert report.supported_revision == "v3"
    assert report.advertised_revision == "v1"
    assert report.message is None

    names = await gateway.names([unsupported_channel], ["noarch"])
    assert [name.source for name in names] == ["demo"]
    assert names.unsupported_repodata_revisions == [report]

    first_channel_path = tmp_path / "first"
    _write_repodata(first_channel_path, "linux-64", {"v1": {"message": "legacy layout"}})
    _write_repodata(first_channel_path, "noarch", {"v2": {}})
    second_channel_path = tmp_path / "second"
    _write_repodata(second_channel_path, "linux-64", {"v3": {}})
    _write_repodata(second_channel_path, "noarch", {"v4": {"message": "new layout"}})
    first_channel = Channel(str(first_channel_path))
    second_channel = Channel(str(second_channel_path))

    multiple = await gateway.query(
        [first_channel, second_channel],
        ["linux-64", "noarch"],
        ["demo"],
    )

    assert sum(len(subdir) for subdir in multiple) == 4
    reports = {
        (report.channel, report.subdir): (
            report.supported_revision,
            report.advertised_revision,
            report.message,
        )
        for report in multiple.unsupported_repodata_revisions
    }
    assert reports == {
        (first_channel.base_url, "linux-64"): ("v3", "v1", "legacy layout"),
        (first_channel.base_url, "noarch"): ("v3", "v2", None),
        (second_channel.base_url, "noarch"): ("v3", "v4", "new layout"),
    }


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
