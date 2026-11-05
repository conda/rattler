import asyncio
import gc
import json
from pathlib import Path

import pytest

from rattler import Channel, PackageFormatSelection, SparseRepoData, solve, solve_with_sparse_repodata


def test_from_bytes_owns_input_and_loads_records() -> None:
    data = json.dumps(
        {
            "info": {"subdir": "linux-64"},
            "packages.conda": {
                "example-1.0-0.conda": {
                    "name": "example",
                    "version": "1.0",
                    "build": "0",
                    "build_number": 0,
                    "subdir": "linux-64",
                    "depends": [],
                    "license": "café",
                }
            },
        },
        ensure_ascii=False,
    ).encode()
    repo = SparseRepoData.from_bytes(Channel("https://example.org/channel"), "linux-64", data)
    del data
    gc.collect()
    records = repo.load_records("example")
    assert len(records) == 1
    assert records[0].file_name == "example-1.0-0.conda"
    assert records[0].license == "café"
    assert records[0].url == "https://example.org/channel/linux-64/example-1.0-0.conda"
    repo.close()


def test_from_bytes_close_keeps_loaded_records_alive() -> None:
    data = json.dumps(
        {
            "packages": {
                "example-1.0-0.tar.bz2": {
                    "name": "example",
                    "version": "1.0",
                    "build": "0",
                    "build_number": 0,
                    "subdir": "linux-64",
                    "depends": [],
                }
            }
        }
    ).encode()
    with SparseRepoData.from_bytes(Channel("https://example.org/channel"), "linux-64", data) as repo:
        records = repo.load_records("example")
    repo.close()
    assert records[0].file_name == "example-1.0-0.tar.bz2"
    with pytest.raises(ValueError, match="closed"):
        repo.load_records("example")


@pytest.mark.parametrize("data", ["{}", bytearray(b"{}"), memoryview(b"{}"), {}, None])
def test_from_bytes_rejects_non_bytes_input(data: object) -> None:
    with pytest.raises(TypeError, match="data"):
        SparseRepoData.from_bytes(Channel("https://example.org/channel"), "linux-64", data)  # type: ignore[arg-type]


def test_from_bytes_rejects_invalid_channel_type() -> None:
    with pytest.raises(TypeError, match="channel"):
        SparseRepoData.from_bytes("channel", "linux-64", b"{}")  # type: ignore[arg-type]


def test_from_bytes_rejects_invalid_subdir_type() -> None:
    with pytest.raises(TypeError, match="subdir"):
        SparseRepoData.from_bytes(Channel("https://example.org/channel"), 1, b"{}")  # type: ignore[arg-type]


@pytest.mark.parametrize("data", [b"", b"{", b"\xff"])
def test_from_bytes_rejects_malformed_json(data: bytes) -> None:
    with pytest.raises(OSError):
        SparseRepoData.from_bytes(Channel("https://example.org/channel"), "linux-64", data)


@pytest.mark.parametrize("package_format", list(PackageFormatSelection))
def test_from_bytes_matches_file_records(tmp_path: Path, package_format: PackageFormatSelection) -> None:
    record = {
        "name": "example",
        "version": "1.0",
        "build": "0",
        "build_number": 0,
        "subdir": "linux-64",
        "depends": [],
        "license": "café",
        "timestamp": 1700000000000,
        "track_features": "one two",
        "md5": "a" * 32,
    }
    data = json.dumps(
        {
            "info": {"subdir": "linux-64", "base_url": "https://mirror.example.org/packages/"},
            "packages": {"example-1.0-0.tar.bz2": record},
            "packages.conda": {"example-1.0-0.conda": record},
        },
        ensure_ascii=False,
    ).encode()
    path = tmp_path / "repodata.json"
    path.write_bytes(data)
    channel = Channel("https://example.org/channel")
    with (
        SparseRepoData(channel, "linux-64", path) as file_repo,
        SparseRepoData.from_bytes(channel, "linux-64", data) as bytes_repo,
    ):
        assert bytes_repo.subdir == "linux-64"
        assert bytes_repo.package_names(package_format) == ["example"]
        expected = [record.to_json() for record in file_repo.load_records("example", package_format)]
        actual = [record.to_json() for record in bytes_repo.load_records("example", package_format)]
        assert actual == expected
        assert bytes_repo.record_count(package_format) == len(expected)


def test_from_bytes_supports_sparse_and_gateway_solves(tmp_path: Path) -> None:
    def payload(name: str, depends: list[str]) -> bytes:
        return json.dumps(
            {
                "packages.conda": {
                    f"{name}-1.0-0.conda": {
                        "name": name,
                        "version": "1.0",
                        "build": "0",
                        "build_number": 0,
                        "subdir": "linux-64",
                        "depends": depends,
                    }
                }
            }
        ).encode()

    path = tmp_path / "repodata.json"
    path.write_bytes(payload("dependency", []))

    async def check_solves() -> None:
        with (
            SparseRepoData.from_bytes(
                Channel("https://example.org/first"), "linux-64", payload("example", ["dependency >=1"])
            ) as first,
            SparseRepoData(Channel("https://example.org/second"), "linux-64", path) as second,
        ):
            for _ in range(2):
                sparse_result = await solve_with_sparse_repodata(["example"], [first, second], virtual_packages=[])
                gateway_result = await solve(
                    sources=[first, second],
                    specs=["example"],
                    platforms=["linux-64"],
                    virtual_packages=[],
                    channel_relations="disabled",
                )
                assert sorted(record.name.normalized for record in sparse_result) == ["dependency", "example"]
                assert sorted(record.to_json() for record in gateway_result) == sorted(
                    record.to_json() for record in sparse_result
                )

    asyncio.run(check_solves())


@pytest.mark.parametrize(
    "base_url",
    [
        "https://example.org/channel/linux-64?ignored=query",
        "https://example.org/channel/linux-64#ignored-fragment",
    ],
)
@pytest.mark.parametrize(
    ("section", "filename", "wheel_url"),
    [
        ("packages.conda", "example-1.0-0.conda", None),
        ("packages", "example-1.0-0.tar.bz2", None),
        ("v3.whl", "example-1.0-0", "../wheels/example.whl"),
        ("v3.whl", "example-1.0-0", "https://cdn.example.org/example.whl"),
        ("v3.whl", "example-1.0-0", "//other-bucket/wheels/example.whl"),
    ],
)
def test_from_bytes_preserves_base_url_resolution(
    tmp_path: Path, base_url: str, section: str, filename: str, wheel_url: str | None
) -> None:
    record: dict[str, object] = {"name": "example", "version": "1.0", "build": "0", "build_number": 0}
    repodata: dict[str, object] = {"info": {"subdir": "linux-64", "base_url": base_url}}
    if wheel_url is not None:
        record["url"] = wheel_url
        repodata["v3"] = {"whl": {filename: record}}
    else:
        repodata[section] = {filename: record}
    data = json.dumps(repodata).encode()
    path = tmp_path / "repodata.json"
    path.write_bytes(data)
    channel = Channel("https://example.org/channel")
    with (
        SparseRepoData(channel, "linux-64", path) as file_repo,
        SparseRepoData.from_bytes(channel, "linux-64", data) as bytes_repo,
    ):
        expected = file_repo.load_records("example", PackageFormatSelection.PREFER_CONDA_WITH_WHL)
        actual = bytes_repo.load_records("example", PackageFormatSelection.PREFER_CONDA_WITH_WHL)
        assert len(actual) == 1
        assert [record.to_json() for record in actual] == [record.to_json() for record in expected]
        if wheel_url is None:
            assert actual[0].url == f"https://example.org/channel/linux-64/{filename}"
