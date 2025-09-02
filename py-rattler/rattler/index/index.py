from __future__ import annotations

from dataclasses import dataclass
import os
from typing import Optional

from rattler.platform import Platform
from rattler.rattler import py_index_fs, py_index_s3


@dataclass
class S3Credentials:
    """Credentials for accessing an S3 backend."""

    # The endpoint URL of the S3 backend
    endpoint_url: str

    # The region of the S3 backend
    region: str

    # The access key ID for the S3 bucket.
    access_key_id: Optional[str] = None

    # The secret access key for the S3 bucket.
    secret_access_key: Optional[str] = None

    # The session token for the S3 bucket.
    session_token: Optional[str] = None


async def index_fs(
    channel_directory: os.PathLike[str],
    target_platform: Optional[Platform] = None,
    repodata_patch: Optional[str] = None,
    write_zst: bool = True,
    write_shards: bool = True,
    force: bool = False,
    max_parallel: int | None = None,
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
        target_platform: A `Platform` to index dependencies for.
        repodata_patch: The name of the conda package (expected to be in the `noarch` subdir) that should be used for repodata patching.
        write_zst: Whether to write repodata.json.zst.
        write_shards: Whether to write sharded repodata.
        force: Whether to forcefully re-index all subdirs.
        max_parallel: The maximum number of packages to process in-memory simultaneously.
    """
    await py_index_fs(
        channel_directory,
        target_platform._inner if target_platform else target_platform,
        repodata_patch,
        write_zst,
        write_shards,
        force,
        max_parallel,
    )


async def index_s3(
    channel_url: str,
    credentials: Optional[S3Credentials] = None,
    force_path_style: Optional[bool] = False,
    target_platform: Optional[Platform] = None,
    repodata_patch: Optional[str] = None,
    write_zst: bool = True,
    write_shards: bool = True,
    force: bool = False,
    max_parallel: int | None = None,
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
        force_path_style: Whether to use path-style addressing for S3.
        credentials: The credentials to use for accessing the S3 bucket. If not provided, will use the default
                     credentials from the environment.
        target_platform: A `Platform` to index dependencies for.
        repodata_patch: The name of the conda package (expected to be in the `noarch` subdir) that should be used for repodata patching.
        write_zst: Whether to write repodata.json.zst.
        write_shards: Whether to write sharded repodata.
        force: Whether to forcefully re-index all subdirs.
        max_parallel: The maximum number of packages to process in-memory simultaneously.
    """
    await py_index_s3(
        channel_url,
        credentials,
        force_path_style,
        target_platform._inner if target_platform else target_platform,
        repodata_patch,
        write_zst,
        write_shards,
        force,
        max_parallel,
    )
