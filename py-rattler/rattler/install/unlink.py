from __future__ import annotations
import os

from rattler.prefix.prefix_record import PrefixRecord
from rattler.rattler import py_unlink_package, py_empty_trash


async def unlink_package(
    target_prefix: str | os.PathLike[str],
    prefix_record: PrefixRecord,
) -> None:
    """
    Unlinks a package from a prefix at a low-level.

    This function removes all files of a package from a prefix
    and cleans up empty directories.

    Arguments:
        target_prefix: Path to the environment prefix
        prefix_record: Prefix record for the package to unlink
    """
    await py_unlink_package(
        target_prefix=str(target_prefix),
        prefix_record=prefix_record,
    )


async def empty_trash(
    target_prefix: str | os.PathLike[str],
) -> None:
    """
    Empties the trash directory in the prefix.

    This function removes files in the .trash directory that are
    not currently in use by other processes.

    Arguments:
        target_prefix: Path to the environment prefix
    """
    await py_empty_trash(
        target_prefix=str(target_prefix),
    )
