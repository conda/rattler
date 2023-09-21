from __future__ import annotations
import os
from typing import List, Optional, Self

from rattler.rattler import PyPrefixRecord
from rattler.prefix.prefix_paths import PrefixPaths


class PrefixRecord:
    def __init__(self, source: str) -> None:
        if not isinstance(source, str):
            raise TypeError(
                "PrefixRecord constructor received unsupported type "
                f" {type(source).__name__!r} for the `source` parameter"
            )
        self._record = PyPrefixRecord(source)

    @classmethod
    def _from_py_prefix_record(cls, py_prefix_record: PyPrefixRecord) -> Self:
        """Construct Rattler PrefixRecord from FFI PyPrefixRecord object."""
        record = cls.__new__(cls)
        record._record = py_prefix_record
        return record

    @staticmethod
    def from_path(path: os.PathLike[str]) -> PrefixRecord:
        return PrefixRecord._from_py_prefix_record(PyPrefixRecord.from_path(path))

    def write_to_path(self, path: os.PathLike[str], pretty: bool) -> None:
        self._record.write_to_path(path, pretty)

    @property
    def package_tarball_full_path(self) -> Optional[os.PathLike[str]]:
        return self._record.package_tarball_full_path

    @property
    def extracted_package_dir(self) -> Optional[os.PathLike[str]]:
        return self._record.extracted_package_dir

    @property
    def files(self) -> List[os.PathLike[str]]:
        return self._record.files

    @property
    def paths_data(self) -> PrefixPaths:
        return PrefixPaths._from_py_prefix_paths(self._record.paths_data)

    @property
    def requested_spec(self) -> Optional[str]:
        return self._record.requested_spec
