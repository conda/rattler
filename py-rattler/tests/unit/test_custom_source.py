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
