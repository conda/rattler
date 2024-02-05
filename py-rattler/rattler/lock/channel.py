from __future__ import annotations

from rattler.rattler import PyLockChannel


class LockChannel:
    _channel: PyLockChannel

    def __init__(self, url: str) -> None:
        self._channel = PyLockChannel(url)

    @classmethod
    def _from_py_lock_channel(cls, channel: PyLockChannel) -> LockChannel:
        """
        Construct Rattler LockChannel from FFI PyLockChannel object.
        """
        chan = cls.__new__(cls)
        chan._channel = channel
        return chan
