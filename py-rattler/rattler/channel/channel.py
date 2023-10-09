from __future__ import annotations
from typing import Optional

from rattler.rattler import PyChannel
from rattler.channel.channel_config import ChannelConfig


class Channel:
    def __init__(
        self, name: str, channel_configuration: ChannelConfig = ChannelConfig()
    ) -> None:
        """
        Create a new channel.

        ```python
        >>> channel = Channel("conda-forge", ChannelConfig())
        >>> channel
        Channel(name="conda-forge", base_url="https://conda.anaconda.org/conda-forge/")
        >>>
        ```
        """
        self._channel = PyChannel(name, channel_configuration._channel_configuration)

    @property
    def name(self) -> Optional[str]:
        """
        Return the name of this channel.

        Examples
        --------
        ```python
        >>> channel = Channel("conda-forge", ChannelConfig())
        >>> channel.name
        'conda-forge'
        >>>
        ```
        """
        return self._channel.name

    @property
    def base_url(self) -> str:
        """
        Return the base URL of this channel.

        Examples
        --------
        ```python
        >>> channel = Channel("conda-forge", ChannelConfig())
        >>> channel.base_url
        'https://conda.anaconda.org/conda-forge/'
        >>>
        ```
        """
        return self._channel.base_url

    def __repr__(self) -> str:
        """
        Return a string representation of this channel.

        Examples
        --------
        ```python
        >>> channel = Channel("conda-forge")
        >>> channel
        Channel(name="conda-forge", base_url="https://conda.anaconda.org/conda-forge/")
        >>>
        ```
        """
        return f'Channel(name="{self.name}", base_url="{self.base_url}")'
