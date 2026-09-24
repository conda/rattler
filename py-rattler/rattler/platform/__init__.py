from __future__ import annotations

from typing import TYPE_CHECKING, Any

from rattler.platform import subdir as _subdir
from rattler.platform.arch import Arch
from rattler.platform.subdir import Subdir, SubdirLiteral

__all__ = ["Arch", "Platform", "PlatformLiteral", "Subdir", "SubdirLiteral"]


_DEPRECATED_ALIASES = {"Platform": "Subdir", "PlatformLiteral": "SubdirLiteral"}

if TYPE_CHECKING:
    Platform = Subdir
    PlatformLiteral = SubdirLiteral
else:

    def __getattr__(name: str) -> Any:
        """Forward the deprecated `Platform` names to `rattler.platform.subdir`, which warns."""
        if name in _DEPRECATED_ALIASES:
            return getattr(_subdir, name)
        raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
