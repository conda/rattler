from __future__ import annotations
from pathlib import Path
from typing import Optional, Union, List, TYPE_CHECKING


if TYPE_CHECKING:
    from os import PathLike
    from rattler.channel import Channel
    from rattler.repo_data import PatchInstructions, RepoDataRecord

from rattler.rattler import PyChannelInfo, PyChannelRelations, PyRepoData


class ChannelRelations:
    """
    Relationships to other channels declared inside ``repodata.json`` under
    ``info.channel_relations`` as specified by
    [CEP-42](https://github.com/conda/ceps/blob/main/cep-0042.md).

    Both ``base`` and ``overrides`` are optional relative-path channel
    references (e.g. ``"../conda-forge"``).
    """

    def __init__(
        self,
        base: Optional[str] = None,
        overrides: Optional[str] = None,
        _inner: Optional[PyChannelRelations] = None,
    ) -> None:
        if _inner is not None:
            self._inner = _inner
        else:
            self._inner = PyChannelRelations(base=base, overrides=overrides)

    @classmethod
    def _from_inner(cls, py_channel_relations: PyChannelRelations) -> ChannelRelations:
        return cls(_inner=py_channel_relations)

    @property
    def base(self) -> Optional[str]:
        """A reference to a channel with higher priority than the declaring channel."""
        return self._inner.base

    @base.setter
    def base(self, value: Optional[str]) -> None:
        self._inner.base = value

    @property
    def overrides(self) -> Optional[str]:
        """A reference to a channel with lower priority than the declaring channel."""
        return self._inner.overrides

    @overrides.setter
    def overrides(self, value: Optional[str]) -> None:
        self._inner.overrides = value

    def __repr__(self) -> str:
        return f"ChannelRelations(base={self.base!r}, overrides={self.overrides!r})"


class ChannelInfo:
    """
    The ``info`` section of a ``repodata.json`` file.
    """

    def __init__(self, py_channel_info: PyChannelInfo) -> None:
        self._inner = py_channel_info

    @property
    def subdir(self) -> Optional[str]:
        """The channel's subdirectory (e.g. ``"linux-64"``)."""
        return self._inner.subdir

    @subdir.setter
    def subdir(self, value: Optional[str]) -> None:
        self._inner.subdir = value

    @property
    def base_url(self) -> Optional[str]:
        """The base URL for all package URLs in this channel, if any."""
        return self._inner.base_url

    @base_url.setter
    def base_url(self, value: Optional[str]) -> None:
        self._inner.base_url = value

    @property
    def channel_relations(self) -> Optional[ChannelRelations]:
        """
        Channel relations declared by this channel, see
        [CEP-42](https://github.com/conda/ceps/blob/main/cep-0042.md).

        ``None`` when the channel does not declare any relations.
        """
        relations = self._inner.channel_relations
        if relations is None:
            return None
        return ChannelRelations._from_inner(relations)

    @channel_relations.setter
    def channel_relations(self, value: Optional[ChannelRelations]) -> None:
        self._inner.channel_relations = None if value is None else value._inner

    def __repr__(self) -> str:
        return (
            f"ChannelInfo(subdir={self.subdir!r}, base_url={self.base_url!r}, "
            f"channel_relations={self.channel_relations!r})"
        )


class RepoData:
    def __init__(self, path: Union[str, PathLike[str]]) -> None:
        if not isinstance(path, (str, Path)):
            raise TypeError(
                f"RepoData constructor received unsupported type  {type(path).__name__!r} for the `path` parameter"
            )

        self._repo_data = PyRepoData.from_path(path)

    @property
    def info(self) -> Optional[ChannelInfo]:
        """
        Returns the channel info contained in the repodata, if any.
        """
        info = self._repo_data.info
        if info is None:
            return None
        return ChannelInfo(info)

    def apply_patches(self, instructions: PatchInstructions) -> None:
        """
        Apply a patch to a repodata file.
        Note that we currently do not handle revoked instructions.
        """
        self._repo_data.apply_patches(instructions._patch_instructions)

    def into_repo_data(self, channel: Channel) -> List[RepoDataRecord]:
        """
        Builds a `List[RepoDataRecord]` from the packages in a
        `RepoData` given the source of the data.

        Examples
        --------
        ```python
        >>> from rattler import Channel
        >>> repo_data = RepoData("../test-data/test-server/repo/noarch/repodata.json")
        >>> repo_data.into_repo_data(Channel("test"))
        [...]
        >>>
        ```
        """
        from rattler.repo_data import RepoDataRecord

        return [
            RepoDataRecord._from_py_record(record)
            for record in PyRepoData.repo_data_to_records(self._repo_data, channel._channel)
        ]

    @classmethod
    def _from_py_repo_data(cls, py_repo_data: PyRepoData) -> RepoData:
        """
        Construct Rattler RepoData from FFI PyRepoData object.
        """
        repo_data = cls.__new__(cls)
        repo_data._repo_data = py_repo_data
        return repo_data

    def __repr__(self) -> str:
        """
        Returns a representation of the RepoData.

        Examples
        --------
        ```python
        >>> repo_data = RepoData("../test-data/test-server/repo/noarch/repodata.json")
        >>> repo_data
        RepoData()
        >>>
        ```
        """
        return f"{type(self).__name__}()"
