from __future__ import annotations
import os
from typing import List, Optional

from rattler.package.package_name import PackageName
from rattler.rattler import PyRecord
from rattler.prefix.prefix_paths import PrefixPaths
from rattler.repo_data.record import RepoDataRecord
from rattler.version.version import Version


class PrefixRecord(RepoDataRecord):
    @classmethod
    def _from_py_record(cls, py_record: PyRecord) -> PrefixRecord:
        """Construct Rattler PrefixRecord from FFI PyRecord object."""

        # quick sanity check
        assert py_record.is_prefix_record
        record = cls.__new__(cls)
        record._record = py_record
        return record

    @classmethod
    def create_from_record(
        cls,
        name: str | PackageName,
        version: str | Version,
        build_number: int,
        build_string: str,
        channel: str,
        subdir: str,
        md5: Optional[str] = None,
        sha256: Optional[str] = None,
        size: Optional[int] = None,
        depends: Optional[List[str]] = None,
        constrains: Optional[List[str]] = None,
        track_features: Optional[List[str]] = None,
        features: Optional[List[str]] = None,
        noarch: Optional[str] = None,
        platform: Optional[str] = None,
        arch: Optional[str] = None,
        timestamp: Optional[int] = None,
    ) -> PrefixRecord:
        """Creates a new PrefixRecord with the specified attributes.

        Parameters
        ----------
        name : str | PackageName
            The name of the package
        version : str | Version
            The version of the package
        build_number : int
            The build number of the package
        build_string : str
            The build string of the package
        channel : str
            The channel the package is from
        subdir : str
            The subdirectory of the channel
        md5 : Optional[str]
            The MD5 hash of the package
        sha256 : Optional[str]
            The SHA256 hash of the package
        size : Optional[int]
            The size of the package in bytes
        depends : Optional[List[str]]
            Package dependencies
        constrains : Optional[List[str]]
            Package constraints
        track_features : Optional[List[str]]
            Features to track
        features : Optional[List[str]]
            Package features
        noarch : Optional[str]
            Noarch specification
        platform : Optional[str]
            Platform specification
        arch : Optional[str]
            Architecture specification
        timestamp : Optional[int]
            Package timestamp

        Returns
        -------
        PrefixRecord
            A new PrefixRecord instance
        """
        record = PyRecord.create(
            name,
            version,
            build_number,
            build_string,
            channel,
        )

        if subdir:
            record.subdir = subdir
        if md5:
            record.md5 = md5
        if sha256:
            record.sha256 = sha256
        if size:
            record.size = size
        if depends:
            record.depends = depends
        if constrains:
            record.constrains = constrains
        if track_features:
            record.track_features = track_features
        if features:
            record.features = features
        if noarch:
            record.noarch = noarch
        if platform:
            record.platform = platform
        if arch:
            record.arch = arch
        if timestamp:
            record.timestamp = timestamp

        return cls._from_py_record(record)

    @staticmethod
    def from_path(path: os.PathLike[str]) -> PrefixRecord:
        """
        Parses a PrefixRecord from a file.

        Examples
        --------
        ```python
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> assert isinstance(r, PrefixRecord)
        >>>
        ```
        """
        return PrefixRecord._from_py_record(PyRecord.from_path(path))

    def create(
        name: str,
        version: str,
        build_number: int,
        build_string: str,
        channel: str,
    ) -> PrefixRecord:
        """
        Creates a new PrefixRecord instance.
        """
        return PrefixRecord._from_py_record(
            PyRecord.create(
                name,
                version,
                build_number,
                build_string,
                channel,
            )
        )

    def write_to_path(self, path: os.PathLike[str], pretty: bool) -> None:
        """
        Writes the contents of this instance to the file at the specified location.
        """
        self._record.write_to_path(path, pretty)

    @property
    def package_tarball_full_path(self) -> Optional[os.PathLike[str]]:
        """
        The path to where the archive of the package was stored on disk.

        Examples
        --------
        ```python
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> r.package_tarball_full_path
        'C:\\\\Users\\\\bas\\\\micromamba\\\\pkgs\\\\requests-2.28.2-pyhd8ed1ab_0.tar.bz2'
        >>>
        ```
        """
        return self._record.package_tarball_full_path

    @property
    def extracted_package_dir(self) -> Optional[os.PathLike[str]]:
        """
        The path that contains the extracted package content.

        Examples
        --------
        ```python
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> r.extracted_package_dir
        'C:\\\\Users\\\\bas\\\\micromamba\\\\pkgs\\\\requests-2.28.2-pyhd8ed1ab_0'
        >>>
        ```
        """
        return self._record.extracted_package_dir

    @property
    def files(self) -> List[os.PathLike[str]]:
        """
        A sorted list of all files included in this package

        Examples
        --------
        ```python
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> r.files # doctest:+ELLIPSIS
        [...]
        >>>
        ```
        """
        return self._record.files

    @property
    def paths_data(self) -> PrefixPaths:
        """
        Information about how files have been linked when installing the package.

        Examples
        --------
        ```python
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> r.paths_data # doctest:+ELLIPSIS
        PrefixPaths(paths=[...])
        >>>
        ```
        """
        return PrefixPaths._from_py_prefix_paths(self._record.paths_data)

    @property
    def requested_spec(self) -> Optional[str]:
        """
        The spec that was used when this package was installed.
        Note that this field is not currently updated if another
        spec was used. If this package was not directly requested by the
        user but was instead installed as a dependency of another package
        `None` will be returned.

        Examples
        --------
        ```python
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> r.requested_spec
        ''
        >>>
        ```
        """
        return self._record.requested_spec

    def __repr__(self) -> str:
        """
        Returns a representation of the version

        Examples
        --------
        ```python
        >>> r = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/requests-2.28.2-pyhd8ed1ab_0.json"
        ... )
        >>> r
        PrefixRecord(name="requests", version="2.28.2")
        >>>
        ```
        """
        return f'PrefixRecord(name="{self.name.normalized}", version="{self.version}")'
