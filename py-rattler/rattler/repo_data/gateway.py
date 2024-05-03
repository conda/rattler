from __future__ import annotations

import typing
from dataclasses import dataclass

from rattler.rattler import PyGateway, PySourceConfig

from rattler import Channel

if typing.TYPE_CHECKING:
    import os
    from typing import Optional

CacheAction = typing.Literal["cache-or-fetch", "use-cache-only", "force-cache-only", "no-cache"]


@dataclass
class SourceConfig:
    """
    Describes properties about a channel.

    This can be used to configure the Gateway to handle channels in a certain
    way.
    """

    jlap_enabled: bool = True
    zstd_enabled: bool = True
    bz2_enabled: bool = True
    cache_action: CacheAction = "cache-or-fetch"

    def _into_py(self) -> PySourceConfig:
        return PySourceConfig(
            jlap_enabled=self.jlap_enabled,
            zstd_enabled=self.zstd_enabled,
            bz2_enabled=self.bz2_enabled,
            cache_action=self.cache_action,
        )


class Gateway:
    """
    An object that manages repodata and allows efficiently querying different
    channels for it.
    """

    def __init__(
            self,
            cache_dir: Optional[os.PathLike[str]] = None,
            default_config: Optional[SourceConfig] = None,
            per_channel_config: Optional[dict[Channel | str, SourceConfig]] = None,
            max_concurrent_requests: int = 100
    ) -> None:
        default_config = default_config or SourceConfig()

        self._gateway = PyGateway(
            cache_dir=cache_dir,
            default_config=default_config._into_py(),
            per_channel_config={
                channel._channel if isinstance(channel, Channel) else Channel(
                    channel)._channel: config._into_py()
                for channel, config in (per_channel_config or {}).items()
            },
            max_concurrent_requests=max_concurrent_requests
        )
