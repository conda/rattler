from os import PathLike
from typing import Optional, Tuple

from rattler.networking.client import Client
from rattler.rattler import download as py_download
from rattler.rattler import download_and_extract as py_download_and_extract
from rattler.rattler import extract as py_extract
from rattler.rattler import extract_tar_bz2 as py_extract_tar_bz2
from rattler.rattler import fetch_raw_package_file_from_url as py_fetch_raw_package_file_from_url


def extract(path: PathLike[str], dest: PathLike[str]) -> Tuple[bytes, bytes]:
    """Extract a file to a destination."""
    return py_extract(path, dest)


def extract_tar_bz2(path: PathLike[str], dest: PathLike[str]) -> Tuple[bytes, bytes]:
    """Extract a tar.bz2 file to a destination."""
    return py_extract_tar_bz2(path, dest)


async def download(client: Client, url: str, dest: PathLike[str]) -> None:
    """Download a package archive from a URL to a destination path."""
    await py_download(client._client, url, dest)


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
