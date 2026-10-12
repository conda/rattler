"""Adversarial tests for the consent callback of `detect_virtual_packages`.

The channel fixtures mirror `test_virtual_package_detectors.py`: a
`noarch: generic` package whose executable prints a fixed report.
"""

import hashlib
import io
import json
import sys
import tarfile
from pathlib import Path
from typing import Any

import pytest

from rattler import Channel, ConsentRequest, Gateway, Subdir, detect_virtual_packages

pytestmark = pytest.mark.skipif(
    sys.platform == "win32", reason="the batch executable prints the report as-is only on Unix"
)

REPORT = json.dumps({"version": 1, "virtual_packages": {"__test_good": {"version": "1.2.3"}}})


def _detector_package(name: str) -> bytes:
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


async def _detect(tmp_path: Path, consent: Any) -> Any:
    channel = _write_channel(tmp_path / "channel", {"good-detect": ["__test_good"]})
    gateway = Gateway(cache_dir=tmp_path / "cache")
    registrations = await gateway.virtual_package_detectors([channel], [Subdir.current(), "noarch"])
    assert len(registrations.accepted) == 1
    return await detect_virtual_packages(
        registrations.accepted,
        gateway,
        Subdir.current(),
        Subdir.current(),
        [],
        consent,
        cache_dir=tmp_path / "cache",
    )


@pytest.mark.asyncio
async def test_a_raising_consent_callback_propagates_the_exception(tmp_path: Path) -> None:
    """An exception in the caller's callback must not be silently turned into
    a denial; the caller has to see it."""

    class Boom(RuntimeError):
        pass

    def consent(request: ConsentRequest) -> bool:
        raise Boom(f"no consent for {request.detector}")

    with pytest.raises(Boom):
        await _detect(tmp_path, consent)


@pytest.mark.asyncio
async def test_a_truthy_non_bool_return_value_allows_the_detector(tmp_path: Path) -> None:
    def consent(request: ConsentRequest) -> Any:
        return "yes"

    outcome = await _detect(tmp_path, consent)
    assert outcome.failures == [] and outcome.skipped == []
    (result,) = outcome.results
    assert str(result.virtual_package) == "__test_good=1.2.3=0"


@pytest.mark.asyncio
async def test_a_falsy_non_bool_return_value_denies_the_detector(tmp_path: Path) -> None:
    def consent(request: ConsentRequest) -> Any:
        return []

    outcome = await _detect(tmp_path, consent)
    assert outcome.results == []
    assert [skipped.reason for skipped in outcome.skipped] == ["consent-denied"]
    assert not (tmp_path / "cache" / "virtual-package-detectors" / "envs").exists()


@pytest.mark.asyncio
async def test_a_return_value_whose_truthiness_raises_propagates(tmp_path: Path) -> None:
    class Undecided:
        def __bool__(self) -> bool:
            raise ValueError("cannot decide")

    def consent(request: ConsentRequest) -> Any:
        return Undecided()

    with pytest.raises(ValueError, match="cannot decide"):
        await _detect(tmp_path, consent)


@pytest.mark.asyncio
async def test_a_consent_that_is_neither_bool_nor_callable_is_a_type_error(tmp_path: Path) -> None:
    """`1` is truthy but neither `True` nor a callable; it must be rejected
    up front instead of being interpreted as a denial or an approval."""
    with pytest.raises(TypeError):
        await _detect(tmp_path, 1)


@pytest.mark.asyncio
async def test_the_callback_sees_the_resolved_records_before_anything_is_installed(tmp_path: Path) -> None:
    envs = tmp_path / "cache" / "virtual-package-detectors" / "envs"
    seen: list[tuple[list[str], bool]] = []

    def consent(request: ConsentRequest) -> bool:
        seen.append(([record.name.normalized for record in request.records], envs.exists()))
        return True

    outcome = await _detect(tmp_path, consent)
    assert outcome.failures == []
    assert seen == [(["good-detect"], False)]
    assert envs.exists()
