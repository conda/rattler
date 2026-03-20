from __future__ import annotations
from pathlib import Path
from typing import Optional

from rattler.rattler import PyPrefixData, PyPackageName
from rattler.package.package_name import PackageName
from rattler.prefix.prefix_record import PrefixRecord


class PrefixData:
    @classmethod
    def _from_py_prefix_data(cls, py_prefix_data: PyPrefixData) -> PrefixData:
        """Construct Rattler PrefixData from FFI PyPrefixData object."""
        prefix_data = cls.__new__(cls)
        prefix_data._prefix_data = py_prefix_data
        return prefix_data

    def __init__(self, prefix_path: str | Path) -> None:
        self._prefix_data = PyPrefixData(str(prefix_path))

    def get(self, name: str | PackageName) -> Optional[PrefixRecord]:
        """
        Get record matching given name in target prefix. If not found, returns None.

        Examples
        --------
        ```python
        >>> from rattler.prefix.prefix_data import PrefixData
        >>> pd = PrefixData("../test-data/")
        >>> r = pd.get("requests")
        >>> r.name.normalized
        'requests'
        >>> pd.get("does-not-exist") is None
        True
        >>>
        ```
        """
        if isinstance(name, PackageName):
            name = name.normalized
        if pyrecord := self._prefix_data.get(PyPackageName(name)):
            return PrefixRecord._from_py_record(pyrecord)
        return None
