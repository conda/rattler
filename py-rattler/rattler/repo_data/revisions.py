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


def _repodata_revision_metadata_from_py(metadata: _PyRepodataRevisionMetadata) -> RepodataRevisionMetadata:
    """Convert the FFI metadata of one revision to Python's timestamp representation."""
    message, n_packages, oldest, newest = metadata
    result: RepodataRevisionMetadata = {}
    if message is not None:
        result["message"] = message
    if n_packages is not None:
        result["n_packages"] = n_packages
    if oldest is not None:
        result["oldest"] = datetime.datetime.fromtimestamp(oldest / 1000.0, tz=datetime.timezone.utc)
    if newest is not None:
        result["newest"] = datetime.datetime.fromtimestamp(newest / 1000.0, tz=datetime.timezone.utc)
    return result


def _repodata_revisions_from_py(
    revisions: Mapping[str, _PyRepodataRevisionMetadata],
) -> dict[str, RepodataRevisionMetadata]:
    """Convert FFI revision metadata to Python's timestamp representation."""
    return {revision: _repodata_revision_metadata_from_py(metadata) for revision, metadata in revisions.items()}
