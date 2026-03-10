import io

import pytest
from pathlib import Path
from rattler.networking.middleware import MirrorMiddleware, OciMiddleware, GCSMiddleware
from rattler.package_streaming import (
    download_and_extract,
    download_bytes,
    download_to_path,
    download_to_writer,
    extract,
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
