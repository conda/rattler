import json
from pathlib import Path

import pytest

from rattler import Gateway, Channel, PackageFormatSelection, SourceConfig, SparseRepoData


@pytest.mark.asyncio
async def test_removed_packages(tmp_path: Path) -> None:
    noarch = tmp_path / "noarch"
    noarch.mkdir()
    record = {"name": "demo", "build": "0", "build_number": 0, "depends": [], "subdir": "noarch"}
    (noarch / "repodata.json").write_text(
        json.dumps(
            {
                "info": {"subdir": "noarch"},
                "packages": {
                    "demo-1.0-0.tar.bz2": {**record, "version": "1.0"},
                    "demo-2.0-0.tar.bz2": {**record, "version": "2.0"},
                },
                "removed": ["demo-2.0-0.tar.bz2", "other-1.0-0.tar.bz2"],
            }
        )
    )
    channel = Channel(str(tmp_path))

    # The gateway hides removed records and reports them for every fetched name.
    result = await Gateway().query([channel], ["noarch"], ["demo"])
    assert [record.file_name for record in result[0]] == ["demo-1.0-0.tar.bz2"]
    assert len(result.removed) == 1
    (removed,) = result.removed[0]
    assert removed.file_name == "demo-2.0-0.tar.bz2"
    assert (removed.name, removed.version, removed.build) == ("demo", "2.0", "0")
    assert removed.url.endswith("/noarch/demo-2.0-0.tar.bz2")
    assert removed.channel is not None

    # Sparse repodata exposes the same information, for one name or all.
    sparse = SparseRepoData(channel, "noarch", noarch / "repodata.json")
    assert [record.file_name for record in sparse.load_records("demo")] == ["demo-1.0-0.tar.bz2"]
    assert [removed.file_name for removed in sparse.load_removed()] == ["demo-2.0-0.tar.bz2", "other-1.0-0.tar.bz2"]
    assert [removed.file_name for removed in sparse.load_removed("other")] == ["other-1.0-0.tar.bz2"]


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

    names = await gateway.names([channel], ["noarch"], channel_notices=True)
    assert names.names is names
    assert names.notices == notices


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


def _write_v3_repodata_with_wheel(root: Path) -> None:
    """A noarch subdir advertising the CEP 48 v3 revision with one `.conda` and one `.whl` record."""
    noarch = root / "noarch"
    noarch.mkdir()
    (noarch / "repodata.json").write_text(
        json.dumps(
            {
                "repodata_version": 2,
                "info": {
                    "subdir": "noarch",
                    "repodata_revisions": {
                        "v3": {
                            "message": "wheels ahead",
                            "n_packages": 1,
                            "oldest": 1768249989851,
                            "newest": 1773851561010,
                        }
                    },
                },
                "packages": {},
                "packages.conda": {
                    "demo-1.0-0.conda": {
                        "name": "demo",
                        "version": "1.0",
                        "build": "0",
                        "build_number": 0,
                        "depends": [],
                        "subdir": "noarch",
                        "noarch": "generic",
                    }
                },
                "v3": {
                    "whl": {
                        "six-1.9.0-py3_none_any_0": {
                            "name": "six",
                            "version": "1.9.0",
                            "build": "py3_none_any_0",
                            "build_number": 0,
                            "depends": [],
                            "subdir": "noarch",
                            "noarch": "python",
                            "url": "https://files.pythonhosted.org/packages/six-1.9.0-py2.py3-none-any.whl",
                        }
                    }
                },
            }
        )
    )


@pytest.mark.asyncio
async def test_repodata_revisions(tmp_path: Path) -> None:
    _write_v3_repodata_with_wheel(tmp_path)
    gateway = Gateway()
    channel = Channel(str(tmp_path))

    revisions = await gateway.repodata_revisions(channel, "noarch")
    assert list(revisions) == ["v3"]
    assert revisions["v3"]["message"] == "wheels ahead"
    assert revisions["v3"]["n_packages"] == 1
    assert revisions["v3"]["oldest"].year == 2026

    # Subdirs the channel does not publish are empty, not an error.
    assert await gateway.repodata_revisions(channel, "linux-64") == {}


@pytest.mark.asyncio
async def test_package_format_selection(tmp_path: Path) -> None:
    _write_v3_repodata_with_wheel(tmp_path)
    channel = Channel(str(tmp_path))

    # By default only conda records are returned.
    records = await Gateway().query([channel], ["noarch"], ["demo", "six"], recursive=False)
    assert sorted(record.file_name for record in records[0]) == ["demo-1.0-0.conda"]

    gateway = Gateway(
        default_config=SourceConfig(package_format_selection=PackageFormatSelection.PREFER_CONDA_WITH_WHL)
    )
    records = await gateway.query([channel], ["noarch"], ["demo", "six"], recursive=False)
    assert sorted(record.file_name for record in records[0]) == [
        "demo-1.0-0.conda",
        "six-1.9.0-py3_none_any_0.whl",
    ]
    wheel = next(record for record in records[0] if record.file_name.endswith(".whl"))
    assert wheel.url == "https://files.pythonhosted.org/packages/six-1.9.0-py2.py3-none-any.whl"

    names = await gateway.names([channel], ["noarch"])
    assert sorted(name.normalized for name in names) == ["demo", "six"]
