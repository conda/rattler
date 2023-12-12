from __future__ import annotations
from typing import Optional, TYPE_CHECKING

from rattler.rattler import py_index

if TYPE_CHECKING:
    import os
    from rattler.platform import Platform


def index(
    channel_directory: os.PathLike[str],
    target_platform: Optional[Platform] = None,
) -> bool:
    """
    TODO(blowry): add docstring
    :param channel_directory:
    :param target_platform:
    :return:
    """

    return py_index(
        channel_directory,
        target_platform._inner if target_platform else target_platform,
    )
