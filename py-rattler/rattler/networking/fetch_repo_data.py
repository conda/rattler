from __future__ import annotations
from typing import Callable, List, Optional, Union, TYPE_CHECKING


from rattler.rattler import py_fetch_repo_data
from rattler.repo_data import RepoData

if TYPE_CHECKING:
    import os
    from rattler.channel import Channel
    from rattler.platform import Platform


async def fetch_repo_data(
    *,
    channels: List[Channel],
    platforms: List[Platform],
    cache_path: Union[str, os.PathLike[str]],
    callback: Optional[Callable[[int, int], None]],
) -> List[RepoData]:
    """
    Returns a list of RepoData for given channels and platform.
    """
    repo_data_list = await py_fetch_repo_data(
        [channel._channel for channel in channels],
        [platform._inner for platform in platforms],
        cache_path,
        callback,
    )

    return [RepoData._from_py_repo_data(repo_data) for repo_data in repo_data_list]
