from __future__ import annotations

from rattler.rattler import PyChannelConfig


class ChannelConfig:
    def __init__(self, channel_alias: str = "https://conda.anaconda.org/") -> None:
        """
        Create a new channel configuration.

        Examples
        --------
        ```python
        >>> channel_config = ChannelConfig()
        >>> channel_config
        ChannelConfig(channel_alias="https://conda.anaconda.org/")
        >>> channel_config = ChannelConfig("https://repo.prefix.dev/")
        >>> channel_config
        ChannelConfig(channel_alias="https://repo.prefix.dev/")
        >>>
        ```
        """
        self._channel_configuration = PyChannelConfig(channel_alias)

    def __repr__(self) -> str:
        """
        Return a string representation of this channel configuration.

        Examples
        --------
        ```python
        >>> channel_config = ChannelConfig()
        >>> channel_config
        ChannelConfig(channel_alias="https://conda.anaconda.org/")
        >>>
        ```
        """
        alias = self._channel_configuration.channel_alias
        return f'ChannelConfig(channel_alias="{alias}")'
