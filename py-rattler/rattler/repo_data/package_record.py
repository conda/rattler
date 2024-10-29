from __future__ import annotations
import os
from typing import List, Optional, TYPE_CHECKING
import datetime

from rattler import VersionWithSource
from rattler.match_spec.match_spec import MatchSpec
from rattler.package.no_arch_type import NoArchType
from rattler.package.package_name import PackageName
from rattler.platform.platform import Platform
from rattler.rattler import PyRecord

if TYPE_CHECKING:
    import networkx as nx
else:
    try:
        import networkx as nx
    except ImportError:
        nx = None


class PackageRecord:
    """
    A single record in the Conda repodata. A single
    record refers to a single binary distribution
    of a package on a Conda channel.
    """

    _record: PyRecord

    def matches(self, spec: MatchSpec) -> bool:
        """
        Match a [`PackageRecord`] against a [`MatchSpec`].
        """
        return spec.matches(self)

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
        >>>
        ```
        """
        return PackageRecord._from_py_record(PyRecord.from_index_json(path, size, sha256, md5))

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
        PackageName("python_abi")
        >>>
        ```
        """
        return [PackageRecord._from_py_record(p) for p in PyRecord.sort_topologically(records)]

    @staticmethod
    def to_graph(records: List[PackageRecord]) -> nx.DiGraph:  # type: ignore[type-arg]
        """
        Converts a list of PackageRecords to a DAG (`networkx.DiGraph`).
        The nodes in the graph are the PackageRecords and the edges are the dependencies.

        Examples
        --------
        ```python
        import rattler
        import asyncio
        import networkx as nx
        from matplotlib import pyplot as plt

        records = asyncio.run(rattler.solve(['main'], ['python'], platforms=['osx-arm64', 'noarch']))
        graph = rattler.PackageRecord.to_graph(records)

        nx.draw(graph, with_labels=True, font_weight='bold')
        plt.show()
        ```
        """
        if nx is None:
            raise ImportError("networkx is not installed")

        names_to_records = {record.name: record for record in records}

        graph = nx.DiGraph()  # type: ignore[var-annotated]
        for record in records:
            graph.add_node(record)
            for dep in record.depends:
                name = dep.split(" ")[0]
                graph.add_edge(record, names_to_records[PackageName(name)])

        return graph

    @classmethod
    def _from_py_record(cls, py_record: PyRecord) -> PackageRecord:
        """
        Construct Rattler PackageRecord from FFI PyRecord object.
        """

        # quick sanity check
        assert py_record.is_package_record
        record = cls.__new__(cls)
        record._record = py_record
        return record

    def __init__(
        self,
        name: str | PackageName,
        version: str | VersionWithSource,
        build: str,
        build_number: int,
        subdir: str | Platform,
        arch: Optional[str],
        platform: Optional[str],
        noarch: Optional[NoArchType] = None,
        depends: List[str] = None,
        constrains: List[str] = None,
        sha256: Optional[bytes] = None,
        md5: Optional[bytes] = None,
        size: Optional[int] = None,
        features: Optional[List[str]] = None,
        legacy_bz2_md5: Optional[bytes] = None,
        legacy_bz2_size: Optional[int] = None,
        license: Optional[str] = None,
        license_family: Optional[str] = None,
    ) -> None:
        # Convert Platform to str
        if isinstance(subdir, Platform):
            arch = subdir.arch
            platform = subdir.only_platform
            subdir = subdir.__str__()

        # convert str to PackageName
        if isinstance(name, str):
            name = PackageName(name)

        # convert str to VersionWithSource
        if isinstance(version, str):
            version = VersionWithSource(version)

        self._record = PyRecord.create(
            name._name,
            (version._version, version._source),
            build,
            build_number,
            subdir,
            arch,
            platform,
            noarch,
        )

        if constrains is not None:
            self._record.constrains = constrains
        if depends is not None:
            self._record.depends = depends
        if sha256 is not None:
            self._record.sha256 = sha256
        if md5 is not None:
            self._record.md5 = md5
        if size is not None:
            self._record.size = size
        if features is not None:
            self._record.features = features
        if legacy_bz2_md5 is not None:
            self._record.legacy_bz2_md5 = legacy_bz2_md5
        if legacy_bz2_size is not None:
            self._record.legacy_bz2_size = legacy_bz2_size
        if license is not None:
            self._record.license = license
        if license_family is not None:
            self._record.license_family = license_family

    @property
    def arch(self) -> Optional[str]:
        """
        Optionally the architecture the package supports.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.arch
        'x86_64'
        >>>
        ```
        """
        return self._record.arch

    @arch.setter
    def arch(self, value: Optional[str]) -> None:
        self._record.arch = value

    @property
    def build(self) -> str:
        """
        The build string of the package.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.build
        'hcfcfb64_0'
        >>>
        ```
        """
        return self._record.build

    @build.setter
    def build(self, value: str) -> None:
        self._record.build = value

    @property
    def build_number(self) -> int:
        """
        The build number of the package.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.build_number
        0
        >>>
        ```
        """
        return self._record.build_number

    @build_number.setter
    def build_number(self, value: int) -> None:
        self._record.build_number = value

    @property
    def constrains(self) -> List[str]:
        """
        Additional constraints on packages.
        Constrains are different from depends in that packages
        specified in depends must be installed next to this package,
        whereas packages specified in constrains are not required to
        be installed, but if they are installed they must follow
        these constraints.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.constrains
        []
        >>>
        ```
        """
        return self._record.constrains

    @constrains.setter
    def constrains(self, value: List[str]) -> None:
        self._record.constrains = value

    @property
    def depends(self) -> List[str]:
        """
        Specification of packages this package depends on.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.depends
        ['ucrt >=10.0.20348.0', 'vc >=14.2,<15', 'vs2015_runtime >=14.29.30139']
        >>>
        ```
        """
        return self._record.depends

    @depends.setter
    def depends(self, value: List[str]) -> None:
        self._record.depends = value

    @property
    def features(self) -> Optional[str]:
        """
        Features are a deprecated way to specify different feature
        sets for the conda solver. This is not supported anymore and
        should not be used. Instead, mutex packages should be used
        to specify mutually exclusive features.
        """
        return self._record.features

    @features.setter
    def features(self, value: Optional[str]) -> None:
        self._record.features = value

    @property
    def legacy_bz2_md5(self) -> Optional[bytes]:
        """
        A deprecated md5 hash.
        """
        return self._record.legacy_bz2_md5

    @legacy_bz2_md5.setter
    def legacy_bz2_md5(self, value: Optional[bytes]) -> None:
        self._record.legacy_bz2_md5 = value

    @property
    def legacy_bz2_size(self) -> Optional[int]:
        """
        A deprecated package archive size.
        """
        return self._record.legacy_bz2_size

    @legacy_bz2_size.setter
    def legacy_bz2_size(self, value: Optional[int]) -> None:
        self._record.legacy_bz2_size = value

    @property
    def license(self) -> Optional[str]:
        """
        The specific license of the package.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.license
        'Unlicense'
        >>>
        ```
        """
        return self._record.license

    @license.setter
    def license(self, value: Optional[str]) -> None:
        self._record.license = value

    @property
    def license_family(self) -> Optional[str]:
        """
        The license family.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/pip-23.0-pyhd8ed1ab_0.json"
        ... )
        >>> record.license_family
        'MIT'
        >>>
        ```

        """
        return self._record.license_family

    @license_family.setter
    def license_family(self, value: Optional[str]) -> None:
        self._record.license_family = value

    @property
    def md5(self) -> Optional[bytes]:
        """
        Optionally a MD5 hash of the package archive.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.md5.hex()
        '5e5a97795de72f8cc3baf3d9ea6327a2'
        >>>
        ```
        """
        return self._record.md5

    @md5.setter
    def md5(self, value: Optional[bytes]) -> None:
        self._record.md5 = value

    @property
    def name(self) -> PackageName:
        """
        The name of the package.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.name
        PackageName("libsqlite")
        >>>
        ```
        """
        return PackageName._from_py_package_name(self._record.name)

    @name.setter
    def name(self, value: PackageName) -> None:
        self._record.name = value._name

    @property
    def noarch(self) -> Optional[str]:
        """
        The noarch type of the package.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.noarch
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/pip-23.0-pyhd8ed1ab_0.json"
        ... )
        >>> record.noarch
        'python'
        >>>
        ```
        """
        noarchtype = NoArchType._from_py_no_arch_type(self._record.noarch)
        if noarchtype.python:
            return "python"
        if noarchtype.generic:
            return "generic"
        return None

    @noarch.setter
    def noarch(self, value: Optional[str]) -> None:
        if value == "python":
            self._record.noarch = NoArchType.python()._noarch_type
        elif value == "generic":
            self._record.noarch = NoArchType.generic()._noarch_type
        else:
            self._record.noarch = NoArchType.none()._noarch_type

    @property
    def platform(self) -> Optional[str]:
        """
        Optionally the platform the package supports.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.platform
        'win32'
        >>>
        ```
        """
        return self._record.platform

    @platform.setter
    def platform(self, value: Optional[str]) -> None:
        self._record.platform = value

    @property
    def sha256(self) -> Optional[bytes]:
        """
        Optionally a SHA256 hash of the package archive.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.sha256.hex()
        '4e50b3d90a351c9d47d239d3f90fce4870df2526e4f7fef35203ab3276a6dfc9'
        >>>
        """
        return self._record.sha256

    @sha256.setter
    def sha256(self, value: Optional[bytes]) -> None:
        self._record.sha256 = value

    @property
    def size(self) -> Optional[int]:
        """
        Optionally the size of the package archive in bytes.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.size
        669941
        >>>
        ```
        """
        return self._record.size

    @size.setter
    def size(self, value: Optional[int]) -> None:
        self._record.size = value

    @property
    def subdir(self) -> str:
        """
        The subdirectory where the package can be found.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.subdir
        'win-64'
        >>>
        ```
        """
        return self._record.subdir

    @subdir.setter
    def subdir(self, value: str) -> None:
        self._record.subdir = value

    @property
    def timestamp(self) -> Optional[datetime.datetime]:
        """
        The date this entry was created.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.timestamp
        datetime.datetime(2022, 11, 17, 15, 7, 19, 781000, tzinfo=datetime.timezone.utc)
        >>>
        ```
        """
        if self._record.timestamp:
            return datetime.datetime.fromtimestamp(self._record.timestamp / 1000.0, tz=datetime.timezone.utc)

        return self._record.timestamp

    @timestamp.setter
    def timestamp(self, value: Optional[datetime.datetime]) -> None:
        if value is not None:
            self._record.timestamp = int(value.timestamp() * 1000)
        else:
            self._record.timestamp = None

    @property
    def track_features(self) -> List[str]:
        """
        Track features are nowadays only used to downweight
        packages (ie. give them less priority).
        To that effect, the number of track features is
        counted (number of commas) and the package is downweighted
        by the number of track_features.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.track_features
        []
        >>>
        ```
        """
        return self._record.track_features

    @track_features.setter
    def track_features(self, value: List[str]) -> None:
        self._record.track_features = value

    @property
    def version(self) -> VersionWithSource:
        """
        The version of the package.

        Examples
        --------
        ```python
        >>> from rattler import PrefixRecord
        >>> record = PrefixRecord.from_path(
        ...     "../test-data/conda-meta/libsqlite-3.40.0-hcfcfb64_0.json"
        ... )
        >>> record.version
        VersionWithSource(version="3.40.0", source="3.40.0")
        >>>
        ```
        """
        return VersionWithSource._from_py_version(*self._record.version)

    @version.setter
    def version(self, value: VersionWithSource) -> None:
        self._record.version = (value._version, value._source)

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

    def to_json(self):
        """
        Convert the PackageRecord to a JSON serializable object.
        """
        return self._record.to_json()
