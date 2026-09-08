from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

from rattler.rattler import PyRemovedPackage


@dataclass(frozen=True)
class RemovedPackage:
    """A package that a channel lists under the ``removed`` key of its repodata.

    The archive may still be downloadable, but the channel no longer offers it
    for installation. Removed packages never appear among the records returned
    by :meth:`rattler.Gateway.query` or :class:`rattler.SparseRepoData`.
    """

    url: str
    """The URL the archive was served from. It is derived the same way as
    ``RepoDataRecord.url``, so it compares directly against a previously
    fetched record or the location of a lock file entry."""

    file_name: str
    """The file name of the archive, e.g. ``python-3.12.0-h0d6e5a4_0.conda``."""

    name: str
    """The package name parsed from the file name."""

    version: str
    """The version parsed from the file name."""

    build: str
    """The build string parsed from the file name."""

    channel: Optional[str]
    """The channel the package was removed from."""

    @classmethod
    def _from_py(cls, removed: PyRemovedPackage) -> RemovedPackage:
        return cls(
            url=removed.url,
            file_name=removed.file_name,
            name=removed.name,
            version=removed.version,
            build=removed.build,
            channel=removed.channel,
        )
