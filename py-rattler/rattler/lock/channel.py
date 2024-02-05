from __future__ import annotations
from typing import Self

from rattler.rattler import PyLockChannel


class LockChannel:
    _channel: PyLockChannel

    @classmethod
    def _from_py_lock_channel(cls, channel: PyLockChannel) -> Self:
        """
        Construct Rattler LockChannel from FFI PyLockChannel object.
        """
        chan = cls.__new__(cls)
        chan._channel = channel
        return chan
