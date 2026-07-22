from enum import Enum

from rattler.rattler import PyChannelPriority


class ChannelPriority(Enum):
    """
    Defines how priority of channels functions during solves. If strict, the channel that the package is first
    found in will be used as the only channel for that package. If flexible, higher-priority channels are
    preferred but packages may still be taken from lower-priority channels when required to find a solution
    (matching conda's "flexible" channel priority). If disabled, then packages can be retrieved from
    any channel as package version takes precedence.
    """

    Strict = PyChannelPriority.Strict
    Flexible = PyChannelPriority.Flexible
    Disabled = PyChannelPriority.Disabled
