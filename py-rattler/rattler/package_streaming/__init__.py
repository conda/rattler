from os import PathLike
from typing import Optional, Tuple

from rattler.networking.client import Client
from rattler.package import AboutJson, IndexJson
from rattler.rattler import (
    download_and_extract as py_download_and_extract,
)
from rattler.rattler import (
    extract as py_extract,
)
from rattler.rattler import (
    extract_tar_bz2 as py_extract_tar_bz2,
)
from rattler.rattler import (
    fetch_about_json_from_url as py_fetch_about_json_from_url,
)
from rattler.rattler import (
    fetch_index_json_from_url as py_fetch_index_json_from_url,
)


def extract(path: PathLike[str], dest: PathLike[str]) -> Tuple[bytes, bytes]:
    """Extract a file to a destination."""
    return py_extract(path, dest)


def extract_tar_bz2(path: PathLike[str], dest: PathLike[str]) -> Tuple[bytes, bytes]:
    """Extract a tar.bz2 file to a destination."""
    return py_extract_tar_bz2(path, dest)


async def download_and_extract(
    client: Client, url: str, dest: PathLike[str], expected_sha: Optional[bytes] = None
) -> Tuple[bytes, bytes]:
    """Download a file from a URL and extract it to a destination."""
    return await py_download_and_extract(client._client, url, dest, expected_sha)


async def fetch_index_json_from_url(client: Client, url: str) -> IndexJson:
    """
    Fetch the IndexJson from a remote `.conda` or `.tar.bz2` package using HTTP range requests.

    For `.conda` packages, this function fetches only the minimal bytes needed from the package,
    typically just the `info/` section which is located at the end of the archive.
    For `.tar.bz2` packages, it falls back to downloading the entire package.

    If the server doesn't support range requests, the function falls back to
    downloading the entire package.

    Args:
        client: The HTTP client to use for requests.
        url: The URL of the package.

    Returns:
        The parsed IndexJson from the package.
    """
    return await py_fetch_index_json_from_url(client._client, url)


async def fetch_about_json_from_url(client: Client, url: str) -> AboutJson:
    """
    Fetch the AboutJson from a remote `.conda` or `.tar.bz2` package using HTTP range requests.

    For `.conda` packages, this function fetches only the minimal bytes needed from the package,
    typically just the `info/` section which is located at the end of the archive.
    For `.tar.bz2` packages, it falls back to downloading the entire package.

    If the server doesn't support range requests, the function falls back to
    downloading the entire package.

    Args:
        client: The HTTP client to use for requests.
        url: The URL of the package.

    Returns:
        The parsed AboutJson from the package.
    """
    return await py_fetch_about_json_from_url(client._client, url)
