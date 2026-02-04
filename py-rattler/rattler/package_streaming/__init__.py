from typing import Tuple, Optional
from os import PathLike
from rattler.networking.client import Client

from rattler.rattler import (
    extract as py_extract,
    extract_tar_bz2 as py_extract_tar_bz2,
    download_and_extract as py_download_and_extract,
)


def extract(
    path: PathLike[str],
    dest: PathLike[str],
    cas_root: Optional[PathLike[str]] = None,
) -> Tuple[bytes, bytes]:
    """Extract a file to a destination.

    Args:
        path: Path to the archive file to extract.
        dest: Destination directory for extracted contents.
        cas_root: Optional path to a Content Addressable Store (CAS) root directory.
            When provided, file contents are deduplicated by storing them in the CAS
            and creating hardlinks in the destination.

    Returns:
        A tuple of (sha256, md5) hashes of the extracted archive.
    """
    return py_extract(path, dest, cas_root)


def extract_tar_bz2(
    path: PathLike[str],
    dest: PathLike[str],
    cas_root: Optional[PathLike[str]] = None,
) -> Tuple[bytes, bytes]:
    """Extract a tar.bz2 file to a destination.

    Args:
        path: Path to the tar.bz2 archive file to extract.
        dest: Destination directory for extracted contents.
        cas_root: Optional path to a Content Addressable Store (CAS) root directory.
            When provided, file contents are deduplicated by storing them in the CAS
            and creating hardlinks in the destination.

    Returns:
        A tuple of (sha256, md5) hashes of the extracted archive.
    """
    return py_extract_tar_bz2(path, dest, cas_root)


async def download_and_extract(
    client: Client,
    url: str,
    dest: PathLike[str],
    expected_sha: Optional[bytes] = None,
    cas_root: Optional[PathLike[str]] = None,
) -> Tuple[bytes, bytes]:
    """Download a file from a URL and extract it to a destination.

    Args:
        client: The HTTP client to use for downloading.
        url: URL of the archive to download.
        dest: Destination directory for extracted contents.
        expected_sha: Optional expected SHA256 hash of the archive for verification.
        cas_root: Optional path to a Content Addressable Store (CAS) root directory.
            When provided, file contents are deduplicated by storing them in the CAS
            and creating hardlinks in the destination.

    Returns:
        A tuple of (sha256, md5) hashes of the extracted archive.
    """
    return await py_download_and_extract(client._client, url, dest, expected_sha, cas_root)
