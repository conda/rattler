from os import PathLike
from typing import Optional, Sequence, Tuple

from rattler.package.paths_json import PathsJson
from rattler.networking.client import Client
from rattler.rattler import download_bytes as py_download_bytes
from rattler.rattler import download_to_path as py_download_to_path
from rattler.rattler import download_to_writer as py_download_to_writer
from rattler.rattler import download_and_extract as py_download_and_extract
from rattler.rattler import extract as py_extract
from rattler.rattler import extract_tar_bz2 as py_extract_tar_bz2
from rattler.rattler import fetch_raw_package_file_from_url as py_fetch_raw_package_file_from_url
from rattler.rattler import fetch_raw_package_files_from_url as py_fetch_raw_package_files_from_url


def extract(path: PathLike[str], dest: PathLike[str]) -> Tuple[bytes, bytes]:
    """Extract a file to a destination."""
    return py_extract(path, dest)


def extract_tar_bz2(path: PathLike[str], dest: PathLike[str]) -> Tuple[bytes, bytes]:
    """Extract a tar.bz2 file to a destination."""
    return py_extract_tar_bz2(path, dest)


async def download_to_path(client: Client, url: str, dest: PathLike[str]) -> None:
    """
    Stream a package archive from a URL to a destination path.

    This method does not buffer the whole response in Python memory. Response
    bytes are fetched incrementally and written directly to `dest`.
    """
    await py_download_to_path(client._client, url, dest)


async def download_bytes(client: Client, url: str) -> bytes:
    """
    Download a package archive from a URL into memory.

    This is a convenience API. The full response body is buffered before the
    `bytes` object is returned, so peak memory use scales with the artifact
    size.
    """
    return await py_download_bytes(client._client, url)


async def download_to_writer(client: Client, url: str, writer: object) -> None:
    """
    Stream a package archive from a URL into a Python writer.

    The response body is fetched incrementally. For each chunk, `writer.write`
    is called with a `bytes` object. The writer must provide a synchronous
    `write(bytes)` method, for example `io.BytesIO()` or an open binary file.
    """
    await py_download_to_writer(client._client, url, writer)


async def download_and_extract(
    client: Client, url: str, dest: PathLike[str], expected_sha: Optional[bytes] = None
) -> Tuple[bytes, bytes]:
    """Download a file from a URL and extract it to a destination."""
    return await py_download_and_extract(client._client, url, dest, expected_sha)


async def fetch_raw_package_file_from_url(client: Client, url: str, path: str) -> bytes:
    """
    Fetch raw bytes for a file inside a remote `.conda` package using sparse
    range requests.
    """
    return await py_fetch_raw_package_file_from_url(client._client, url, path)


async def fetch_raw_package_files_from_url(
    client: Client, url: str, paths: Sequence[str]
) -> dict[str, bytes]:
    """
    Fetch raw bytes for multiple files inside a remote package in one archive scan.

    Duplicate paths are ignored after their first occurrence, and the returned
    dictionary preserves the first-seen path order.
    """
    return await py_fetch_raw_package_files_from_url(client._client, url, list(paths))


def _normalize_paths(paths: Sequence[str]) -> list[str]:
    seen: set[str] = set()
    normalized: list[str] = []
    for path in paths:
        if path not in seen:
            seen.add(path)
            normalized.append(path)
    return normalized


def _is_info_path(path: str) -> bool:
    return path == "info" or path.startswith("info/")


class RemotePackage:
    """Lazy async handle for reading files from one remote package."""

    def __init__(self, client: Client, url: str) -> None:
        self._client = client
        self._url = url
        self._closed = False
        self._paths: tuple[str, ...] | None = None
        self._path_set: set[str] | None = None
        self._bytes_cache: dict[str, bytes] = {}

    def _ensure_open(self) -> None:
        if self._closed:
            raise RuntimeError("RemotePackage is closed")

    def close(self) -> None:
        self._closed = True
        self._paths = None
        self._path_set = None
        self._bytes_cache.clear()

    async def __aenter__(self) -> "RemotePackage":
        self._ensure_open()
        return self

    async def __aexit__(self, exc_type: object, exc: object, tb: object) -> bool:
        self.close()
        return False

    async def paths(self) -> tuple[str, ...]:
        self._ensure_open()
        if self._paths is None:
            paths_json = await PathsJson.from_remote_url(self._client, self._url)
            package_paths = tuple(str(entry.relative_path) for entry in paths_json.paths)
            self._paths = package_paths
            self._path_set = set(package_paths)
        return self._paths

    async def exists(self, path: str) -> bool:
        self._ensure_open()
        if path in self._bytes_cache:
            return True
        if self._path_set is not None and not _is_info_path(path):
            return path in self._path_set
        if _is_info_path(path):
            try:
                await self.read_bytes(path)
            except FileNotFoundError:
                return False
            return True
        return path in await self.paths()

    async def read_bytes(self, path: str) -> bytes:
        self._ensure_open()
        cached = self._bytes_cache.get(path)
        if cached is not None:
            return cached
        if self._path_set is not None and not _is_info_path(path) and path not in self._path_set:
            raise FileNotFoundError(f"file '{path}' not found in package")

        data = await fetch_raw_package_file_from_url(self._client, self._url, path)
        self._bytes_cache[path] = data
        return data

    async def read_text(self, path: str, encoding: str = "utf-8") -> str:
        return (await self.read_bytes(path)).decode(encoding)

    async def read_many(self, paths: Sequence[str]) -> dict[str, bytes]:
        self._ensure_open()
        normalized_paths = _normalize_paths(paths)
        result: dict[str, bytes] = {}
        fetch_paths: list[str] = []
        missing_paths: list[str] = []

        for path in normalized_paths:
            cached = self._bytes_cache.get(path)
            if cached is not None:
                result[path] = cached
                continue
            if self._path_set is not None and not _is_info_path(path) and path not in self._path_set:
                missing_paths.append(path)
                continue
            fetch_paths.append(path)

        if missing_paths:
            joined = ", ".join(missing_paths)
            raise FileNotFoundError(f"file(s) not found in package: {joined}")

        if fetch_paths:
            fetched = await fetch_raw_package_files_from_url(self._client, self._url, fetch_paths)
            self._bytes_cache.update(fetched)
            result.update(fetched)

        return {path: result[path] for path in normalized_paths}

    async def read_files(self, paths: Sequence[str]) -> dict[str, bytes]:
        return await self.read_many(paths)


RemotePackageSession = RemotePackage


def open_remote_package(client: Client, url: str) -> RemotePackage:
    """Open a remote package for lazy file access."""
    return RemotePackage(client, url)
