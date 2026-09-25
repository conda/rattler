import hashlib
import io
import json
import sys
import tarfile
from pathlib import Path

import pytest

from rattler import (
    Channel,
    Config,
    ConsentRequest,
    Gateway,
    GenericVirtualPackage,
    PackageName,
    RepoData,
    Subdir,
    Version,
    detect_virtual_packages,
)

REPORT = json.dumps(
    {
        "version": 1,
        "virtual_packages": {"__test_good": {"version": "1.2.3"}, "__test_absent": None},
    }
)


def _detector_package(name: str) -> bytes:
    """A `noarch: generic` package whose executable prints REPORT."""
    files = {
        f"bin/{name}": f"#!/bin/sh\necho '{REPORT}'\n".encode(),
        f"Scripts/{name}.bat": f"@echo off\r\necho {REPORT}\r\n".encode(),
    }
    files["info/index.json"] = json.dumps(
        {
            "name": name,
            "version": "1.0.0",
            "build": "0",
            "build_number": 0,
            "depends": [],
            "noarch": "generic",
            "subdir": "noarch",
            "timestamp": 1700000000000,
        }
    ).encode()
    files["info/paths.json"] = json.dumps(
        {
            "paths": [
                {
                    "_path": path,
                    "path_type": "hardlink",
                    "sha256": hashlib.sha256(contents).hexdigest(),
                    "size_in_bytes": len(contents),
                }
                for path, contents in files.items()
                if not path.startswith("info/")
            ],
            "paths_version": 1,
        }
    ).encode()
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:bz2") as archive:
        for path, contents in files.items():
            info = tarfile.TarInfo(path)
            info.size = len(contents)
            info.mode = 0o755
            archive.addfile(info, io.BytesIO(contents))
    return buffer.getvalue()


def _write_channel(root: Path, registrations: dict[str, list[str]]) -> Channel:
    noarch = root / "noarch"
    noarch.mkdir(parents=True)
    packages = {}
    for name in registrations:
        archive = _detector_package(name)
        file_name = f"{name}-1.0.0-0.tar.bz2"
        (noarch / file_name).write_bytes(archive)
        packages[file_name] = {
            "name": name,
            "version": "1.0.0",
            "build": "0",
            "build_number": 0,
            "depends": [],
            "noarch": "generic",
            "subdir": "noarch",
            "sha256": hashlib.sha256(archive).hexdigest(),
            "md5": hashlib.md5(archive).hexdigest(),
            "size": len(archive),
        }
    (noarch / "repodata.json").write_text(
        json.dumps(
            {
                "info": {"subdir": "noarch", "virtual_package_detectors": registrations},
                "packages": packages,
                "packages.conda": {},
            }
        )
    )
    return Channel(str(root))


def test_channel_info_exposes_raw_registrations(tmp_path: Path) -> None:
    _write_channel(tmp_path, {"good-detect": ["__test_good", "__test_absent"]})
    repodata = RepoData.from_path(tmp_path / "noarch" / "repodata.json")
    assert repodata.info is not None
    assert repodata.info.virtual_package_detectors == {"good-detect": ["__test_good", "__test_absent"]}


@pytest.mark.asyncio
async def test_gateway_collects_registrations(tmp_path: Path) -> None:
    channel = _write_channel(
        tmp_path,
        {"good-detect": ["__test_good", "__test_absent"], "other-detect": ["__test_other"]},
    )
    registrations = await Gateway(cache_dir=tmp_path / "cache").virtual_package_detectors([channel], Subdir.current())
    assert registrations.rejected == []
    assert [r.detector.normalized for r in registrations.accepted] == ["good-detect", "other-detect"]
    good = registrations.accepted[0]
    assert [name.normalized for name in good.virtual_packages] == ["__test_good", "__test_absent"]
    assert good.origin == channel.base_url
    assert [c.base_url for c in good.resolution_channels] == [channel.base_url]


@pytest.mark.asyncio
async def test_conflicting_registrations_are_rejected_in_channel_order(tmp_path: Path) -> None:
    first = _write_channel(tmp_path / "first", {"a-detect": ["__test_x"]})
    second = _write_channel(tmp_path / "second", {"b-detect": ["__test_x", "__test_y"]})
    registrations = await Gateway(cache_dir=tmp_path / "cache").virtual_package_detectors(
        [first, second], Subdir.current()
    )
    assert [r.detector.normalized for r in registrations.accepted] == ["a-detect"]
    (rejected,) = registrations.rejected
    assert rejected.detector.normalized == "b-detect"
    assert rejected.conflicting_name.normalized == "__test_x"
    assert rejected.conflict == "name"
    assert rejected.accepted_detector.normalized == "a-detect"
    assert rejected.accepted_origin == first.base_url


@pytest.mark.asyncio
@pytest.mark.skipif(sys.platform == "win32", reason="the batch executable prints the report as-is only on Unix")
async def test_detects_with_a_consent_callback(tmp_path: Path) -> None:
    channel = _write_channel(tmp_path / "channel", {"good-detect": ["__test_good", "__test_absent"]})
    gateway = Gateway(cache_dir=tmp_path / "cache")
    registrations = await gateway.virtual_package_detectors([channel], Subdir.current())
    asked: list[ConsentRequest] = []

    def consent(request: ConsentRequest) -> bool:
        asked.append(request)
        return True

    outcome = await detect_virtual_packages(
        registrations.accepted,
        gateway,
        Subdir.current(),
        Subdir.current(),
        [],
        consent,
        cache_dir=tmp_path / "cache",
    )
    assert outcome.failures == [] and outcome.skipped == []
    assert len(asked) == 1
    assert asked[0].detector.normalized == "good-detect"
    assert [record.name.normalized for record in asked[0].records] == ["good-detect"]
    assert len(asked[0].digest) == 64

    results = {result.name.normalized: result for result in outcome.results}
    assert str(results["__test_good"].virtual_package) == "__test_good=1.2.3=0"
    assert results["__test_good"].source == "detector"
    assert results["__test_good"].from_cache is False
    assert results["__test_absent"].absent

    merged = outcome.merge([GenericVirtualPackage(PackageName("__test_good"), Version("0.1"), "0")])
    assert [str(package.version) for package in merged] == ["1.2.3"]

    # Denying consent skips the detector without asking again for the cache.
    denied = await detect_virtual_packages(
        registrations.accepted,
        gateway,
        Subdir.current(),
        Subdir.current(),
        [],
        False,
        cache_dir=tmp_path / "cache",
    )
    assert [skipped.reason for skipped in denied.skipped] == ["consent-denied"]


def test_config_exposes_consent() -> None:
    config = Config.from_toml(
        """
        [virtual-package-detectors]
        timeout-seconds = 60

        [virtual-package-detectors.consent."https://conda.anaconda.org/conda-forge"]
        mpi-detect = "allow"
        """
    )
    section = config.virtual_package_detectors
    assert section["timeout-seconds"] == 60
    assert section["consent"]["https://conda.anaconda.org/conda-forge"] == {"mpi-detect": "allow"}
    assert Config().virtual_package_detectors == {}
