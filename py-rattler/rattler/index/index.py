from __future__ import annotations
import os
from typing import Optional

from rattler.platform import Platform
from rattler.rattler import py_index


def index(
    channel_directory: os.PathLike[str],
    target_platform: Optional[Platform] = None,
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
    """
    py_index(
        channel_directory,
        target_platform._inner if target_platform else target_platform,
    )
