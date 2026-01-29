"""Tests for custom RepoDataSource implementations."""

from typing import List

import pytest

from rattler import (
    Channel,
    Gateway,
    PackageName,
    PackageRecord,
    Platform,
    RepoDataRecord,
    RepoDataSource,
    solve,
)


class MockRepoDataSource(RepoDataSource):
    """A mock implementation of the RepoDataSource protocol for testing."""

    def __init__(self, records_by_platform: dict[str, dict[str, List[RepoDataRecord]]]):
        """Initialize with a mapping of platform -> package_name -> records."""
        self._records = records_by_platform

    async def fetch_package_records(self, platform: Platform, name: PackageName) -> List[RepoDataRecord]:
        """Fetch records for a specific package name and platform."""
        platform_str = str(platform)
        name_str = name.normalized
        if platform_str in self._records and name_str in self._records[platform_str]:
            return self._records[platform_str][name_str]
        return []

    def package_names(self, platform: Platform) -> List[str]:
        """Return all available package names for the given platform."""
        platform_str = str(platform)
        if platform_str in self._records:
            return list(self._records[platform_str].keys())
        return []


def create_test_record(name: str, version: str, platform: str) -> RepoDataRecord:
    """Create a test RepoDataRecord."""
    pkg_record = PackageRecord(
        name=name,
        version=version,
        build="test_0",
        build_number=0,
        subdir=platform,
    )
    return RepoDataRecord(
        package_record=pkg_record,
        file_name=f"{name}-{version}-test_0.conda",
        url=f"https://example.com/{platform}/{name}-{version}-test_0.conda",
        channel="https://example.com",
    )


def record_snapshot(record: RepoDataRecord) -> str:
    """Convert a record to a snapshot string for comparison."""
    return f"{record.name.normalized}={record.version}={record.build}"


def results_snapshot(results: List[List[RepoDataRecord]]) -> List[List[str]]:
    """Convert query results to snapshot format."""
    return [[record_snapshot(r) for r in source_results] for source_results in results]


def test_protocol_check():
    """Test that MockRepoDataSource is recognized as implementing RepoDataSource."""
    source = MockRepoDataSource({})
    assert isinstance(source, RepoDataSource)


def test_protocol_check_missing_method():
    """Test that objects missing methods are not recognized as RepoDataSource."""

    class IncompleteSource:
        async def fetch_package_records(self, platform, name):
            return []

        # Missing package_names method

    source = IncompleteSource()
    assert not isinstance(source, RepoDataSource)


@pytest.mark.asyncio
async def test_custom_source_query():
    """Test querying with a custom RepoDataSource."""
    source = MockRepoDataSource(
        {
            "linux-64": {"test-package": [create_test_record("test-package", "1.0.0", "linux-64")]},
        }
    )

    gateway = Gateway()
    results = await gateway.query(
        sources=[source],
        platforms=["linux-64"],
        specs=["test-package"],
        recursive=False,
    )

    assert results_snapshot(results) == [["test-package=1.0.0=test_0"]]


@pytest.mark.asyncio
async def test_custom_source_names():
    """Test querying package names from a custom RepoDataSource."""
    source = MockRepoDataSource(
        {
            "linux-64": {
                "foo": [create_test_record("foo", "1.0.0", "linux-64")],
                "bar": [],
            },
        }
    )

    gateway = Gateway()
    names = await gateway.names(
        sources=[source],
        platforms=["linux-64"],
    )

    assert sorted(n.normalized for n in names) == ["bar", "foo"]


@pytest.mark.asyncio
async def test_mixed_sources_query(conda_forge_channel: Channel):
    """Test querying with both channels and custom sources."""
    custom_source = MockRepoDataSource(
        {
            "linux-64": {"custom-only-pkg": [create_test_record("custom-only-pkg", "2.0.0", "linux-64")]},
        }
    )

    gateway = Gateway()
    results = await gateway.query(
        sources=[conda_forge_channel, custom_source],
        platforms=["linux-64"],
        specs=["custom-only-pkg"],
        recursive=False,
    )

    # Channel has no results, custom source has the record
    assert results_snapshot(results) == [[], ["custom-only-pkg=2.0.0=test_0"]]


@pytest.mark.asyncio
async def test_custom_source_empty_results():
    """Test that custom sources handle empty results correctly."""
    source = MockRepoDataSource({})

    gateway = Gateway()
    results = await gateway.query(
        sources=[source],
        platforms=["linux-64"],
        specs=["nonexistent-package"],
        recursive=False,
    )

    assert results_snapshot(results) == [[]]


@pytest.mark.asyncio
async def test_custom_source_multiple_platforms():
    """Test custom source with multiple platforms."""
    source = MockRepoDataSource(
        {
            "linux-64": {"multi-plat": [create_test_record("multi-plat", "1.0.0", "linux-64")]},
            "noarch": {"multi-plat": [create_test_record("multi-plat", "1.0.0", "noarch")]},
        }
    )

    gateway = Gateway()
    results = await gateway.query(
        sources=[source],
        platforms=["linux-64", "noarch"],
        specs=["multi-plat"],
        recursive=False,
    )

    # One result list per platform
    assert results_snapshot(results) == [
        ["multi-plat=1.0.0=test_0"],
        ["multi-plat=1.0.0=test_0"],
    ]


def test_invalid_source_type():
    """Test that invalid source types raise appropriate errors."""

    class NotASource:
        pass

    gateway = Gateway()

    with pytest.raises(TypeError, match="RepoDataSource"):
        import asyncio

        asyncio.run(
            gateway.query(
                sources=[NotASource()],
                platforms=["linux-64"],
                specs=["test"],
                recursive=False,
            )
        )


@pytest.mark.asyncio
async def test_custom_source_with_solve():
    """Test using a custom RepoDataSource with the solve function."""
    # Create a simple package with no dependencies
    source = MockRepoDataSource(
        {
            "linux-64": {
                "my-package": [create_test_record("my-package", "1.0.0", "linux-64")],
            },
        }
    )

    # Solve using the custom source
    solved = await solve(
        sources=[source],
        specs=["my-package"],
        platforms=["linux-64"],
    )

    assert len(solved) == 1
    assert record_snapshot(solved[0]) == "my-package=1.0.0=test_0"


@pytest.mark.asyncio
async def test_custom_source_backed_by_sparse_repodata():
    """Test a custom RepoDataSource that wraps SparseRepoData.

    This demonstrates how to create a custom source that loads records
    from local repodata files for multiple platforms (linux-64 and noarch),
    which can be useful for offline scenarios or when you want to
    filter/transform repodata before solving.
    """
    import os

    from rattler import SparseRepoData

    # Load sparse repodata from the test data directory
    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    linux64_path = os.path.join(data_dir, "channels/conda-forge/linux-64/repodata.json")
    noarch_path = os.path.join(data_dir, "channels/conda-forge/noarch/repodata.json")

    class SparseRepoDataSource(RepoDataSource):
        """A custom source that wraps SparseRepoData files."""

        def __init__(self, repodata_by_platform: dict[str, SparseRepoData]):
            self._repodata = repodata_by_platform

        async def fetch_package_records(self, platform: Platform, name: PackageName) -> List[RepoDataRecord]:
            platform_str = str(platform)
            if platform_str in self._repodata:
                return self._repodata[platform_str].load_records(name)
            return []

        def package_names(self, platform: Platform) -> List[str]:
            platform_str = str(platform)
            if platform_str in self._repodata:
                return self._repodata[platform_str].package_names()
            return []

    # Create sparse repodata for linux-64 and noarch
    linux64_data = SparseRepoData(
        channel=Channel("conda-forge"),
        subdir="linux-64",
        path=linux64_path,
    )
    noarch_data = SparseRepoData(
        channel=Channel("conda-forge"),
        subdir="noarch",
        path=noarch_path,
    )

    # Wrap both in our custom source
    source = SparseRepoDataSource(
        {
            "linux-64": linux64_data,
            "noarch": noarch_data,
        }
    )

    # Query using the custom source with both platforms
    gateway = Gateway()
    results = await gateway.query(
        sources=[source],
        platforms=["linux-64", "noarch"],
        specs=["python"],
        recursive=False,
    )

    # Verify we got results from both platforms
    # results[0] is linux-64, results[1] is noarch
    assert len(results) == 2
    assert len(results[0]) > 0  # linux-64 has python
    # noarch may or may not have python records

    # Test with solve - python has dependencies from both linux-64 and noarch
    solved = await solve(
        sources=[source],
        specs=["python"],
        platforms=["linux-64", "noarch"],
    )

    # Snapshot of solved packages with subdir prefix
    solved_snapshot = sorted(f"{r.subdir}/{r.name.normalized}-{r.version}-{r.build}" for r in solved)
    assert solved_snapshot == [
        "linux-64/_libgcc_mutex-0.1-conda_forge",
        "linux-64/_openmp_mutex-4.5-2_gnu",
        "linux-64/bzip2-1.0.8-h7f98852_4",
        "linux-64/ca-certificates-2022.6.15-ha878542_0",
        "linux-64/ld_impl_linux-64-2.36.1-hea4e1c9_2",
        "linux-64/libffi-3.4.2-h7f98852_5",
        "linux-64/libgcc-ng-12.1.0-h8d9b700_16",
        "linux-64/libgomp-12.1.0-h8d9b700_16",
        "linux-64/libnsl-2.0.0-h7f98852_0",
        "linux-64/libsqlite-3.39.2-h753d276_1",
        "linux-64/libuuid-2.32.1-h7f98852_1000",
        "linux-64/libzlib-1.2.12-h166bdaf_2",
        "linux-64/ncurses-6.3-h27087fc_1",
        "linux-64/openssl-3.0.5-h166bdaf_1",
        "linux-64/python-3.10.5-ha86cf86_0_cpython",
        "linux-64/readline-8.1.2-h0f457ee_0",
        "linux-64/sqlite-3.39.2-h4ff8645_1",
        "linux-64/tk-8.6.12-h27826a3_0",
        "linux-64/xz-5.2.6-h166bdaf_0",
        "noarch/tzdata-2021e-he74cb21_0",
    ]
