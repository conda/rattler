from __future__ import annotations
import os
from typing import List, Optional

from rattler.networking.authenticated_client import AuthenticatedClient
from rattler.platform.platform import Platform
from rattler.prefix.prefix_record import PrefixRecord
from rattler.repo_data.record import RepoDataRecord

from rattler.rattler import py_link


async def link(
    dependencies: List[RepoDataRecord],
    target_prefix: os.PathLike[str],
    cache_dir: os.PathLike[str],
    installed_packages: Optional[List[PrefixRecord]] = None,
    platform: Optional[Platform] = None,
) -> None:
    """
    Create an environment by downloading and linking the `dependencies` in
    the `target_prefix` directory.

    Arguments:
        dependencies: A list of solved `RepoDataRecord`s.
        target_prefix: Path to the directory where the environment should
                       be created.
        cache_dir: Path to directory where the dependencies will be
                   downloaded and cached.
        installed_packages: A list of `PrefixRecord`s which are
                                      already installed in the
                                      `target_prefix`. This can be obtained
                                      by loading `PrefixRecord`s from
                                      `{target_prefix}/conda-meta/`.
        platform: Target platform to create and link the
                            environment. Defaults to current platform.
    """
    platform = platform or Platform.current()
    client = AuthenticatedClient()

    await py_link(
        dependencies,
        target_prefix,
        cache_dir,
        installed_packages or [],
        platform._inner,
        client._client,
    )
