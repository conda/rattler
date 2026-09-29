import http.server
import os
import threading

import pytest

from rattler.package_streaming import FileRanges, PackageArchive


@pytest.fixture
def conda_package(test_data_dir: str) -> str:
    return os.path.join(test_data_dir, "clobber/clobber-fd-1-0.1.0-h4616a5c_0.conda")


@pytest.fixture
def tar_bz2_package(test_data_dir: str) -> str:
    return os.path.join(test_data_dir, "clobber/clobber-1-0.1.0-h4616a5c_0.tar.bz2")


@pytest.fixture
def sparse_package(test_data_dir: str) -> str:
    return os.path.join(test_data_dir, "sparse/sparse-test-1.0.0-0.conda")


# Slices covering every shape: head, tail, open-ended, negative on both ends,
# mixed signs, overlapping, unordered, past the end, inverted and empty.
RANGE_SLICES = [
    slice(0, 4096),
    slice(-4096, None),
    slice(100, None),
    slice(-8, -4),
    slice(100, -4),
    slice(-8, 4),
    slice(None, 16),
    slice(None, None),
    slice(50_000, 60_000),
    slice(55_000, 70_000),
    slice(10, 20),
    slice(0, 4096),
    slice(1_000_000, 2_000_000),
    slice(-1_000_000, None),
    slice(20, 10),
    slice(0, 0),
]


@pytest.mark.asyncio
async def test_read_files(conda_package: str) -> None:
    archive = await PackageArchive.from_path(conda_package)
    assert archive.access == "local"

    files = await archive.read_files(["info/index.json", "clobber", "missing"])
    assert files["clobber"] == b"clobber-fd-1\n"
    assert files["info/index.json"] is not None
    assert files["missing"] is None

    assert await archive.read_file("clobber") == b"clobber-fd-1\n"
    assert await archive.read_file("missing") is None


@pytest.mark.asyncio
async def test_typed_metadata(conda_package: str) -> None:
    archive = await PackageArchive.from_path(conda_package)
    index = await archive.index_json()
    assert index.name.normalized == "clobber-fd-1"
    about = await archive.about_json()
    assert about is not None


@pytest.mark.asyncio
async def test_stream(conda_package: str) -> None:
    archive = await PackageArchive.from_path(conda_package)

    names = [entry.name async for entry in archive.stream("info")]
    assert "info/index.json" in names

    async for entry in archive.stream("pkg"):
        if entry.name == "clobber":
            assert await entry.read() == b"clobber-fd-1\n"
            break


@pytest.mark.asyncio
async def test_tar_bz2(tar_bz2_package: str) -> None:
    archive = await PackageArchive.from_path(tar_bz2_package)

    files = await archive.read_files(["info/index.json", "clobber.txt"])
    assert files["info/index.json"] is not None
    assert files["clobber.txt"] is not None

    names = [entry.name async for entry in archive.stream("pkg")]
    assert "clobber.txt" in names
    assert not any(name.startswith("info/") for name in names)


@pytest.mark.asyncio
async def test_list_files(conda_package: str) -> None:
    archive = await PackageArchive.from_path(conda_package)
    info = await archive.list_files("info")
    assert "info/index.json" in info
    content = await archive.list_files("pkg")
    assert content == ["clobber"]


@pytest.mark.asyncio
async def test_run_exports_json_absent(conda_package: str) -> None:
    archive = await PackageArchive.from_path(conda_package)
    assert await archive.run_exports_json() is None


@pytest.mark.asyncio
async def test_symlinks_surfaced_not_followed(test_data_dir: str) -> None:
    archive = await PackageArchive.from_path(os.path.join(test_data_dir, "sparse/symlink-test-1.0.0-0.conda"))

    files = await archive.list_files("pkg")
    assert "lib/liblink.so" in files and "lib/libreal.so.1" in files
    assert "lib/libhard.so" in files

    assert await archive.read_file("lib/libreal.so.1") == b"real library bytes"
    with pytest.raises(OSError, match="links are not followed"):
        await archive.read_file("lib/liblink.so")

    async for entry in archive.stream("pkg"):
        if entry.name == "lib/liblink.so":
            assert entry.is_link and entry.is_symlink
            assert not entry.is_hardlink and not entry.is_file
            assert entry.link_target == "libreal.so.1"
            with pytest.raises(OSError, match="links are not followed"):
                await entry.read()

    async for entry in archive.stream("pkg"):
        if entry.name == "lib/libhard.so":
            assert entry.is_link and entry.is_hardlink
            assert not entry.is_symlink and not entry.is_file
            assert entry.link_target == "lib/libreal.so.1"


@pytest.mark.asyncio
async def test_invalid_archive_paths(conda_package: str) -> None:
    archive = await PackageArchive.from_path(conda_package)
    for path in ["", "../clobber", "/clobber"]:
        with pytest.raises(OSError, match="invalid package-relative archive path"):
            await archive.read_file(path)


@pytest.mark.asyncio
async def test_from_url_spooled_fallback(conda_package: str) -> None:
    """python's http.server has no Range support, exercising the fallback."""
    directory = os.path.dirname(conda_package)
    handler = lambda *args: http.server.SimpleHTTPRequestHandler(*args, directory=directory)  # noqa: E731
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        from rattler.networking.client import Client

        url = f"http://127.0.0.1:{server.server_port}/{os.path.basename(conda_package)}"
        archive = await PackageArchive.from_url(Client(), url)
        assert archive.access == "spooled"
        assert await archive.read_file("clobber") == b"clobber-fd-1\n"
    finally:
        server.shutdown()
        server.server_close()


@pytest.mark.asyncio
async def test_remote_fallback_policy(conda_package: str) -> None:
    """Callers can reject or limit a full-download fallback."""
    directory = os.path.dirname(conda_package)
    handler = lambda *args: http.server.SimpleHTTPRequestHandler(*args, directory=directory)  # noqa: E731
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        from rattler.networking.client import Client

        url = f"http://127.0.0.1:{server.server_port}/{os.path.basename(conda_package)}"
        with pytest.raises(OSError, match="does not support sparse"):
            await PackageArchive.from_url(Client(), url, sparse="require")
        with pytest.raises(OSError, match="exceeds the configured 1 byte limit"):
            await PackageArchive.from_url(Client(), url, max_spool_size=1)
    finally:
        server.shutdown()
        server.server_close()


@pytest.mark.asyncio
async def test_read_file_ranges(sparse_package: str) -> None:
    archive = await PackageArchive.from_path(sparse_package)
    for path in ["lib/blob.bin", "bin/first-file.txt", "info/index.json"]:
        expected = await archive.read_file(path)
        assert expected is not None
        result = await archive.read_file_ranges(path, RANGE_SLICES)
        assert isinstance(result, FileRanges)
        assert result.size == len(expected)
        assert result.chunks == [expected[s] for s in RANGE_SLICES]

    # An empty range list only returns the size.
    result = await archive.read_file_ranges("lib/blob.bin", [])
    assert result is not None
    assert result.size == 150_000 and result.chunks == []

    assert await archive.read_file_ranges("lib/missing.bin", [slice(0, 4)]) is None
    with pytest.raises(OSError, match="invalid package-relative archive path"):
        await archive.read_file_ranges("../blob.bin", [slice(0, 4)])


@pytest.mark.asyncio
async def test_read_file_ranges_tar_bz2(tar_bz2_package: str) -> None:
    archive = await PackageArchive.from_path(tar_bz2_package)
    expected = await archive.read_file("clobber.txt")
    assert expected is not None
    result = await archive.read_file_ranges("clobber.txt", [slice(0, 3), slice(-3, None), slice(1, -1)])
    assert result is not None
    assert result.size == len(expected)
    assert result.chunks == [expected[:3], expected[-3:], expected[1:-1]]


@pytest.mark.asyncio
async def test_read_file_ranges_rejects_bad_slices(sparse_package: str) -> None:
    archive = await PackageArchive.from_path(sparse_package)
    with pytest.raises(ValueError, match="step"):
        await archive.read_file_ranges("lib/blob.bin", [slice(0, 10, 2)])
    with pytest.raises(ValueError, match="step"):
        await archive.read_file_ranges("lib/blob.bin", [slice(None, None, -1)])
    with pytest.raises(TypeError, match="slice"):
        await archive.read_file_ranges("lib/blob.bin", [(0, 10)])  # type: ignore[list-item]
    with pytest.raises(TypeError):
        await archive.read_file_ranges("lib/blob.bin", [slice("a", "b")])


@pytest.mark.asyncio
async def test_read_file_ranges_links_not_followed(test_data_dir: str) -> None:
    archive = await PackageArchive.from_path(os.path.join(test_data_dir, "sparse/symlink-test-1.0.0-0.conda"))
    for link in ["lib/liblink.so", "lib/libhard.so"]:
        with pytest.raises(OSError, match="links are not followed"):
            await archive.read_file_ranges(link, [slice(0, 4)])
        with pytest.raises(OSError, match="links are not followed"):
            await archive.read_file_ranges(link, [])
    result = await archive.read_file_ranges("lib/libreal.so.1", [slice(-5, None)])
    assert result is not None
    assert result.chunks == [b"bytes"]


@pytest.mark.asyncio
async def test_entry_read_ranges(sparse_package: str) -> None:
    archive = await PackageArchive.from_path(sparse_package)
    blob = await archive.read_file("lib/blob.bin")
    assert blob is not None

    seen = []
    async for entry in archive.stream("pkg"):
        seen.append(entry.name)
        if entry.name == "lib/blob.bin":
            result = await entry.read_ranges(RANGE_SLICES)
            assert result.size == len(blob) == entry.size
            assert result.chunks == [blob[s] for s in RANGE_SLICES]
            # Like read(), a ranged read consumes the entry.
            with pytest.raises(RuntimeError, match="already been read"):
                await entry.read_ranges([slice(0, 4)])
            with pytest.raises(RuntimeError, match="already been read"):
                await entry.read()
            stale = entry
        elif entry.name == "share/last-file.txt":
            # The stream is still positioned correctly after a partial read.
            assert await entry.read() == b"last payload file\n"
    assert seen == ["bin/first-file.txt", "lib/blob.bin", "share/last-file.txt"]
    with pytest.raises(RuntimeError, match="stream has advanced"):
        await stale.read_ranges([slice(0, 4)])

    async for entry in archive.stream("pkg"):
        if entry.name == "lib/blob.bin":
            with pytest.raises(ValueError, match="step"):
                await entry.read_ranges([slice(0, 10, 2)])
            # A rejected range list does not consume the entry.
            assert (await entry.read_ranges([])).chunks == []
