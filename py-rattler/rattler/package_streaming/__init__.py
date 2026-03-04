from enum import Enum
from os import PathLike
from typing import Literal, Optional, Tuple, Union, overload

from rattler.networking.client import Client
from rattler.package import AboutJson, IndexJson, PathsJson, RunExportsJson
from rattler.rattler import (
    PyPackageFile,
    download_and_extract as py_download_and_extract,
)
from rattler.rattler import (
    extract as py_extract,
)
from rattler.rattler import (
    extract_tar_bz2 as py_extract_tar_bz2,
)
from rattler.rattler import (
    fetch_package_file_from_url as py_fetch_package_file_from_url,
)

PackageFileResult = Union[IndexJson, AboutJson, PathsJson, RunExportsJson]


class PackageFile(Enum):
    INDEX_JSON = PyPackageFile.Index
    ABOUT_JSON = PyPackageFile.About
    PATHS_JSON = PyPackageFile.Paths
    RUN_EXPORTS_JSON = PyPackageFile.RunExports


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


@overload
async def fetch_package_file_from_url(
    client: Client, url: str, package_file: Literal[PackageFile.INDEX_JSON]
) -> IndexJson: ...


@overload
async def fetch_package_file_from_url(
    client: Client, url: str, package_file: Literal[PackageFile.ABOUT_JSON]
) -> AboutJson: ...


@overload
async def fetch_package_file_from_url(
    client: Client, url: str, package_file: Literal[PackageFile.PATHS_JSON]
) -> PathsJson: ...


@overload
async def fetch_package_file_from_url(
    client: Client, url: str, package_file: Literal[PackageFile.RUN_EXPORTS_JSON]
) -> RunExportsJson: ...


@overload
async def fetch_package_file_from_url(client: Client, url: str, package_file: PackageFile) -> PackageFileResult: ...


async def fetch_package_file_from_url(client: Client, url: str, package_file: PackageFile) -> PackageFileResult:
    """
    Fetch a specific package file from a remote package using HTTP range requests.

    For `.conda` packages, this function fetches only the minimal bytes needed from the package.
    For `.tar.bz2` packages, it falls back to downloading the entire package.
    """
    raw_result = await py_fetch_package_file_from_url(client._client, url, package_file.value)
    if package_file is PackageFile.INDEX_JSON:
        return IndexJson._from_py_index_json(raw_result)
    if package_file is PackageFile.ABOUT_JSON:
        return AboutJson._from_py_about_json(raw_result)
    if package_file is PackageFile.PATHS_JSON:
        return PathsJson._from_py_paths_json(raw_result)
    return RunExportsJson._from_py_run_exports_json(raw_result)

