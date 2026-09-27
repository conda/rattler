"""Helpers for repodata revision metadata returned by Rattler."""

from __future__ import annotations

import datetime
from collections.abc import Mapping
from typing import TypedDict


class RepodataRevisionMetadata(TypedDict, total=False):
    """Metadata advertised for one ``vN`` repodata revision.

    The indexer derives package statistics from the records it writes. ``message``
    is the optional publisher-supplied description of the revision.
    """

    message: str
    n_packages: int
    oldest: datetime.datetime
    newest: datetime.datetime


_PyRepodataRevisionMetadata = tuple[str | None, int | None, int | None, int | None]


def _repodata_revisions_from_py(
    revisions: Mapping[str, _PyRepodataRevisionMetadata],
) -> dict[str, RepodataRevisionMetadata]:
    """Convert FFI revision metadata to Python's timestamp representation."""
    result: dict[str, RepodataRevisionMetadata] = {}
    for revision, (message, n_packages, oldest, newest) in revisions.items():
        metadata: RepodataRevisionMetadata = {}
        if message is not None:
            metadata["message"] = message
        if n_packages is not None:
            metadata["n_packages"] = n_packages
        if oldest is not None:
            metadata["oldest"] = datetime.datetime.fromtimestamp(oldest / 1000.0, tz=datetime.timezone.utc)
        if newest is not None:
            metadata["newest"] = datetime.datetime.fromtimestamp(newest / 1000.0, tz=datetime.timezone.utc)
        result[revision] = metadata
    return result
