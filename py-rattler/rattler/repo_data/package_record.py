from __future__ import annotations
import os
from typing import List, Optional
from rattler.package.package_name import PackageName

from rattler.rattler import PyRecord
from rattler.version.version import Version


class PackageRecord:
    """
    A single record in the Conda repodata. A single
    record refers to a single binary distribution
    of a package on a Conda channel.
    """

    _record: PyRecord

    @staticmethod
    def from_index_json(
        path: os.PathLike[str],
        size: Optional[int] = None,
        sha256: Optional[str] = None,
        md5: Optional[str] = None,
    ) -> PackageRecord:
        """
        Builds a PackageRecord from an `index.json`.
        These can be found in `info` directory inside an
        extracted package archive.

        Examples
        --------
        ```python
        >>> record = PackageRecord.from_index_json(
        ...     "../test-data/conda-meta/pysocks-1.7.1-pyh0701188_6.json"
        ... )
        >>> assert isinstance(record, PackageRecord)
        True
        >>>
        ```
        """
        return PackageRecord._from_py_record(
            PyRecord.from_index_json(path, size, sha256, md5)
        )

    @staticmethod
    def sort_topologically(records: List[PackageRecord]) -> List[PackageRecord]:
        """
        Sorts the records topologically.
        This function is deterministic, meaning that it will return the same result
        regardless of the order of records and of the depends vector inside the records.
        Note that this function only works for packages with unique names.

        Examples
        --------
        ```python
        >>> from os import listdir
        >>> from os.path import isfile, join
        >>> from rattler import PrefixRecord
        >>> records = [
        ...     PrefixRecord.from_path(join("../test-data/conda-meta/", f))
        ...     for f in listdir("../test-data/conda-meta")
        ...     if isfile(join("../test-data/conda-meta", f))
        ... ]
        >>> sorted = PackageRecord.sort_topologically(records)
        >>> sorted[0].name
        PackageName("vc")
        >>>
        ```
        """
        return [
            PackageRecord._from_py_record(p)
            for p in PyRecord.sort_topologically(records)
        ]

    @classmethod
    def _from_py_record(cls, py_record: PyRecord) -> PackageRecord:
        """
        Construct Rattler PackageRecord from FFI PyRecord object.
        """

        # quick sanity check
        assert isinstance(py_record, PyRecord)
        record = cls.__new__(cls)
        record._record = py_record
        # quick sanity check
        assert isinstance(record, PackageRecord)
        return record

    @property
    def arch(self) -> Optional[str]:
        """Optionally the architecture the package supports."""
        return self._record.arch

    @property
    def build(self) -> str:
        """The build string of the package."""
        return self._record.build

    @property
    def build_number(self) -> int:
        """The build number of the package."""
        return self._record.build_number

    @property
    def constrains(self) -> List[str]:
        """
        Additional constraints on packages.
        Constrains are different from depends in that packages
        specified in depends must be installed next to this package,
        whereas packages specified in constrains are not required to
        be installed, but if they are installed they must follow
        these constraints.
        """
        return self._record.constrains

    @property
    def depends(self) -> List[str]:
        """
        Specification of packages this package depends on.
        """
        return self._record.depends

    @property
    def features(self) -> Optional[str]:
        """
        Features are a deprecated way to specify different feature
        sets for the conda solver. This is not supported anymore and
        should not be used. Instead, mutex packages should be used
        to specify mutually exclusive features.
        """
        return self._record.features

    @property
    def legacy_bz2_md5(self) -> Optional[str]:
        """
        A deprecated md5 hash.
        """
        return self._record.legacy_bz2_md5

    @property
    def legacy_bz2_size(self) -> Optional[int]:
        """
        A deprecated package archive size.
        """
        return self._record.legacy_bz2_size

    @property
    def license(self) -> Optional[str]:
        """
        The specific license of the package.
        """
        return self._record.license

    @property
    def license_family(self) -> Optional[str]:
        """
        The license family.
        """
        return self._record.license_family

    @property
    def md5(self) -> Optional[str]:
        """
        Optionally a MD5 hash of the package archive.
        """
        return self._record.md5

    @property
    def name(self) -> PackageName:
        """
        The name of the package.
        """
        return PackageName._from_py_package_name(self._record.name)

    @property
    def platform(self) -> Optional[str]:
        """
        Optionally the platform the package supports.
        """
        return self._record.platform

    @property
    def sha256(self) -> Optional[str]:
        """
        Optionally a SHA256 hash of the package archive.
        """
        return self._record.sha256

    @property
    def size(self) -> Optional[int]:
        """
        Optionally the size of the package archive in bytes.
        """
        return self._record.size

    @property
    def subdir(self) -> str:
        """
        The subdirectory where the package can be found.
        """
        return self._record.subdir

    @property
    def timestamp(self) -> Optional[int]:
        """
        The date this entry was created.
        """
        return self._record.timestamp

    @property
    def track_features(self) -> List[str]:
        """
        Track features are nowadays only used to downweight
        packages (ie. give them less priority).
        To that effect, the number of track features is
        counted (number of commas) and the package is downweighted
        by the number of track_features.
        """
        return self._record.track_features

    @property
    def version(self) -> Version:
        """
        The version of the package.

        Examples
        --------
        ```python
        >>> record = PackageRecord.from_index_json(
        ...     "../test-data/conda-meta/pysocks-1.7.1-pyh0701188_6.json"
        ... )
        >>> record.version
        Version("1.7.1")
        >>>
        ```
        """
        return Version._from_py_version(self._record.version)

    def __str__(self) -> str:
        """
        Returns the string representation of the PackageRecord.

        Examples
        --------
        ```python
        >>> record = PackageRecord.from_index_json(
        ...     "../test-data/conda-meta/pysocks-1.7.1-pyh0701188_6.json"
        ... )
        >>> str(record)
        'pysocks=1.7.1=pyh0701188_6'
        >>>
        ```
        """
        return self._record.as_str()

    def __repr__(self) -> str:
        """
        Returns a representation of the PackageRecord.

        Examples
        --------
        ```python
        >>> record = PackageRecord.from_index_json(
        ...     "../test-data/conda-meta/pysocks-1.7.1-pyh0701188_6.json"
        ... )
        >>> record
        PackageRecord("pysocks=1.7.1=pyh0701188_6")
        >>>
        ```
        """
        return f'PackageRecord("{self.__str__()}")'
