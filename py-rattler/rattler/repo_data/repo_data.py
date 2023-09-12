from __future__ import annotations
import os
from typing import Self, Union, List, TYPE_CHECKING

if TYPE_CHECKING:
    from rattler.channel import Channel
    from rattler.repo_data import PatchInstructions, RepoDataRecord


from rattler.rattler import PyRepoData


class RepoData:
    def __init__(self, path: Union[str, os.PathLike]) -> None:
        if not isinstance(path, (str, os.PathLike)):
            raise TypeError(
                "RepoData constructor received unsupported type "
                f" {type(path).__name__!r} for the `path` parameter"
            )

        self._repo_data = PyRepoData.from_path(path)

    def apply_patches(self, instructions: PatchInstructions) -> None:
        """
        Apply a patch to a repodata file.
        Note that we currently do not handle revoked instructions.
        """
        self._repo_data.apply_patches(instructions._instructions)

    def into_repo_data(self, channel: Channel) -> List[RepoDataRecord]:
        """
        Builds a `List[RepoDataRecord]` from the packages in a
        `RepoData` given the source of the data.
        """
        return PyRepoData.repo_data_to_records(self._repo_data, channel)

    @classmethod
    def _from_py_repo_data(cls, py_repo_data: PyRepoData) -> Self:
        """
        Construct Rattler RepoData from FFI PyRepoData object.
        """
        repo_data = cls.__new__(cls)
        repo_data._repo_data = py_repo_data
        return repo_data

    def __repr__(self):
        return f"{type(self).__name__}()"
