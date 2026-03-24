import io

import pytest
import rattler.package_streaming as package_streaming
from pathlib import Path
from rattler.networking.middleware import MirrorMiddleware, OciMiddleware, GCSMiddleware
from rattler.package_streaming import (
    download_and_extract,
    download_bytes,
    download_to_path,
    download_to_writer,
    extract,
    fetch_raw_package_files_from_url,
    open_remote_package,
)
from rattler.networking.client import Client


def get_test_data() -> Path:
    return (Path(__file__).parent / "../../../test-data/test-server/repo/noarch/test-package-0.1-0.tar.bz2").absolute()


def test_extract(tmpdir: Path) -> None:
    dest = Path(tmpdir) / "extract"

    extract(get_test_data(), dest)

    # sanity check that paths exist
    assert (dest / "info").exists()
    assert (dest / "info" / "index.json").exists()
    assert (dest / "info" / "paths.json").exists()


@pytest.mark.asyncio
async def test_download_to_path(tmpdir: Path) -> None:
    destination = Path(tmpdir) / "download" / "boltons.conda"
    extract_dest = Path(tmpdir) / "download_extract"
    client = Client.default_client()

    await download_to_path(
        client,
        "https://repo.prefix.dev/conda-forge/noarch/boltons-24.0.0-pyhd8ed1ab_0.conda",
        destination,
    )

    assert destination.exists()
    assert destination.stat().st_size > 0

    extract(destination, extract_dest)

    assert (extract_dest / "info").exists()
    assert (extract_dest / "info" / "index.json").exists()


@pytest.mark.asyncio
async def test_download_bytes(tmpdir: Path) -> None:
    destination = Path(tmpdir) / "download_bytes.conda"
    extract_dest = Path(tmpdir) / "download_bytes_extract"
    client = Client.default_client()

    bytes_data = await download_bytes(
        client,
        "https://repo.prefix.dev/conda-forge/noarch/boltons-24.0.0-pyhd8ed1ab_0.conda",
    )

    assert bytes_data

    destination.write_bytes(bytes_data)
    extract(destination, extract_dest)

    assert (extract_dest / "info").exists()
    assert (extract_dest / "info" / "index.json").exists()


@pytest.mark.asyncio
async def test_download_to_writer(tmpdir: Path) -> None:
    destination = Path(tmpdir) / "download_to_writer.conda"
    extract_dest = Path(tmpdir) / "download_to_writer_extract"
    client = Client.default_client()
    writer = io.BytesIO()

    await download_to_writer(
        client,
        "https://repo.prefix.dev/conda-forge/noarch/boltons-24.0.0-pyhd8ed1ab_0.conda",
        writer,
    )

    bytes_data = writer.getvalue()
    assert bytes_data

    destination.write_bytes(bytes_data)
    extract(destination, extract_dest)

    assert (extract_dest / "info").exists()
    assert (extract_dest / "info" / "index.json").exists()


@pytest.mark.asyncio
async def test_download_and_extract(tmpdir: Path) -> None:
    dest = Path(tmpdir) / "download_and_extract"
    dest.mkdir(parents=True, exist_ok=True)
    client = Client()

    await download_and_extract(
        client, "https://repo.prefix.dev/conda-forge/noarch/boltons-24.0.0-pyhd8ed1ab_0.conda", dest
    )

    # sanity check that paths exist
    assert (dest / "info").exists()
    assert (dest / "info" / "index.json").exists()
    assert (dest / "info" / "paths.json").exists()
    assert (dest / "site-packages/boltons-24.0.0.dist-info").exists()


@pytest.mark.asyncio
async def test_fetch_raw_package_files_from_url() -> None:
    client = Client.default_client()

    files = await fetch_raw_package_files_from_url(
        client,
        "https://repo.prefix.dev/conda-forge/noarch/boltons-24.0.0-pyhd8ed1ab_0.conda",
        ["info/index.json", "info/about.json", "info/index.json"],
    )

    assert list(files) == ["info/index.json", "info/about.json"]
    assert files["info/index.json"]
    assert files["info/about.json"]


@pytest.mark.asyncio
async def test_open_remote_package_lazy_api() -> None:
    client = Client.default_client()

    async with open_remote_package(
        client,
        "https://repo.prefix.dev/conda-forge/noarch/boltons-24.0.0-pyhd8ed1ab_0.conda",
    ) as package:
        paths = await package.paths()
        assert "site-packages/boltons/cacheutils.py" in paths
        assert await package.exists("info/index.json")
        assert not await package.exists("does/not/exist")

        raw = await package.read_bytes("info/index.json")
        text = await package.read_text("info/index.json")
        files = await package.read_many(["info/index.json", "info/paths.json", "info/index.json"])

        assert raw
        assert text
        assert list(files) == ["info/index.json", "info/paths.json"]
        assert files["info/index.json"] == raw
        assert files["info/paths.json"]

    with pytest.raises(RuntimeError):
        await package.paths()


@pytest.mark.asyncio
async def test_open_remote_package_caches_paths(monkeypatch: pytest.MonkeyPatch) -> None:
    class FakeEntry:
        def __init__(self, path: str) -> None:
            self.relative_path = path

    class FakePathsJson:
        def __init__(self) -> None:
            self.paths = [FakeEntry("info/index.json"), FakeEntry("lib/libfoo.so")]

    calls = 0

    async def fake_from_remote_url(client: object, url: str) -> FakePathsJson:
        nonlocal calls
        calls += 1
        return FakePathsJson()

    monkeypatch.setattr(package_streaming.PathsJson, "from_remote_url", fake_from_remote_url)

    package = open_remote_package(Client.default_client(), "https://example.invalid/test.conda")

    assert await package.paths() == ("info/index.json", "lib/libfoo.so")
    assert await package.paths() == ("info/index.json", "lib/libfoo.so")
    assert await package.exists("lib/libfoo.so")
    assert not await package.exists("missing")
    assert calls == 1


@pytest.mark.asyncio
async def test_open_remote_package_read_many_uses_cache(monkeypatch: pytest.MonkeyPatch) -> None:
    file_calls: list[str] = []
    many_calls: list[list[str]] = []

    async def fake_fetch_file(client: object, url: str, path: str) -> bytes:
        file_calls.append(path)
        return f"single:{path}".encode()

    async def fake_fetch_files(client: object, url: str, paths: list[str]) -> dict[str, bytes]:
        many_calls.append(paths)
        return {path: f"many:{path}".encode() for path in paths}

    monkeypatch.setattr(package_streaming, "fetch_raw_package_file_from_url", fake_fetch_file)
    monkeypatch.setattr(package_streaming, "fetch_raw_package_files_from_url", fake_fetch_files)

    package = open_remote_package(Client.default_client(), "https://example.invalid/test.conda")

    first = await package.read_bytes("info/index.json")
    second = await package.read_bytes("info/index.json")
    many = await package.read_many(["info/index.json", "info/about.json", "info/about.json"])

    assert first == b"single:info/index.json"
    assert second == b"single:info/index.json"
    assert many == {
        "info/index.json": b"single:info/index.json",
        "info/about.json": b"many:info/about.json",
    }
    assert file_calls == ["info/index.json"]
    assert many_calls == [["info/about.json"]]


@pytest.mark.asyncio
# @pytest.mark.xfail(reason="Github currently requires a PAT to get a token?")
async def test_download_from_oci(tmpdir: Path) -> None:
    dest = Path(tmpdir) / "destination"
    # Note: the order of middlewares matters here! The OCI middleware must come after the mirror middleware.
    client = Client(
        [
            # TODO somehow these URLs are very susceptible to missing last /
            # Maybe we can use the new ChannelURL type or one of these.
            MirrorMiddleware(
                {"https://conda.anaconda.org/conda-forge/": ["oci://ghcr.io/channel-mirrors/conda-forge/"]}
            ),
            OciMiddleware(),
        ]
    )

    expected_sha = bytes.fromhex("e44d07932306392372411ab1261670a552f96077f925af00c1559a18a73a1bdc")
    await download_and_extract(
        client, "https://conda.anaconda.org/conda-forge/noarch/boltons-24.0.0-pyhd8ed1ab_0.conda", dest, expected_sha
    )

    # sanity check that paths exist
    assert (dest / "info").exists()
    assert (dest / "info" / "index.json").exists()
    assert (dest / "info" / "paths.json").exists()
    assert (dest / "site-packages/boltons-24.0.0.dist-info").exists()


def test_instantiate_gcs_middleware() -> None:
    _client = Client([GCSMiddleware()])
