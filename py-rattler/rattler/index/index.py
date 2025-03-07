from __future__ import annotations

import os
from typing import Optional

from rattler.platform import Platform
from rattler.rattler import py_index_fs, py_index_s3


async def index_fs(
    channel_directory: os.PathLike[str],
    target_platform: Optional[Platform] = None,
    repodata_patch: Optional[str] = None,
    force: bool = False,
    max_parallel: int = 128,
) -> None:
    """
    Indexes dependencies in the `channel_directory` for one or more subdirectories within said directory.
    Will generate repodata.json files in each subdirectory containing metadata about each present package,
    or if `target_platform` is specified will only consider the subdirectory corresponding to this platform.
    Will always index the "noarch" subdirectory, and thus this subdirectory should always be present, because
    conda channels at a minimum must include this subdirectory.

    Arguments:
        channel_directory: A `os.PathLike[str]` that is the directory containing subdirectories
                           of dependencies to index.
        target_platform(optional): A `Platform` to index dependencies for.
        repodata_patch(optional): The name of the conda package (expected to be in the `noarch` subdir) that should be used for repodata patching.
        force: Whether to forcefully re-index all subdirs.
        max_parallel: The maximum number of packages to process in-memory simultaneously.
    """
    await py_index_fs(
        channel_directory,
        target_platform._inner if target_platform else target_platform,
        repodata_patch,
        force,
        max_parallel,
    )


async def index_s3(
    channel_url: str,
    region: str,
    endpoint_url: str,
    force_path_style: bool = False,
    access_key_id: Optional[str] = None,
    secret_access_key: Optional[str] = None,
    session_token: Optional[str] = None,
    target_platform: Optional[Platform] = None,
    repodata_patch: Optional[str] = None,
    force: bool = False,
    max_parallel: int = 128,
) -> None:
    """
    Indexes dependencies in the `channel_url` for one or more subdirectories in the S3 directory.
    Will generate repodata.json files in each subdirectory containing metadata about each present package,
    or if `target_platform` is specified will only consider the subdirectory corresponding to this platform.
    Will always index the "noarch" subdirectory, and thus this subdirectory should always be present, because
    conda channels at a minimum must include this subdirectory.

    Arguments:
        channel_url: An S3 URL (e.g., s3://my-bucket/my-channel that containins the subdirectories
                     of dependencies to index.
        region: The region of the S3 bucket.
        endpoint_url: The endpoint URL of the S3 bucket.
        force_path_style: Whether to use path-style addressing for S3.
        access_key_id(optional): The access key ID to use for authentication.
        secret_access_key(optional): The secret access key to use for authentication.
        session_token(optional): The session token to use for authentication.
        target_platform(optional): A `Platform` to index dependencies for.
        repodata_patch(optional): The name of the conda package (expected to be in the `noarch` subdir) that should be used for repodata patching.
        force: Whether to forcefully re-index all subdirs.
        max_parallel: The maximum number of packages to process in-memory simultaneously.
    """
    await py_index_s3(
        channel_url,
        region,
        endpoint_url,
        force_path_style,
        access_key_id,
        secret_access_key,
        session_token,
        target_platform._inner if target_platform else target_platform,
        repodata_patch,
        force,
        max_parallel,
    )
