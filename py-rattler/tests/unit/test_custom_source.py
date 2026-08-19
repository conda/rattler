"""Tests for custom RepoDataSource implementations."""

from typing import Any, List

import pytest

from rattler import (
    Channel,
    Gateway,
    PackageFormatSelection,
    PackageName,
    PackageRecord,
    Platform,
    RepoDataRecord,
    RepoDataSource,
    solve,
)


def _archive_type(file_name: str) -> str:
    """Determine the archive format of a record from its file name."""
    if file_name.endswith(".conda"):
        return "conda"
    if file_name.endswith(".tar.bz2"):
        return "tar_bz2"
    if file_name.endswith(".whl"):
        return "whl"
    return "unknown"


def _filter_by_format(
    records: List[RepoDataRecord], package_format_selection: PackageFormatSelection
) -> List[RepoDataRecord]:
    """Filter (and, for the `PREFER_*` variants, dedup) records by archive format.

    Mirrors the semantics of `PackageFormatSelection` used by sparse repodata: `.conda`
    is preferred over `.tar.bz2`, which is preferred over `.whl`, whenever a `PREFER_*`
    variant is selected.
    """
    if package_format_selection is PackageFormatSelection.ONLY_TAR_BZ2:
        allowed = {"tar_bz2"}
    elif package_format_selection is PackageFormatSelection.ONLY_CONDA:
        allowed = {"conda"}
    elif package_format_selection is PackageFormatSelection.PREFER_CONDA_WITH_WHL:
        allowed = {"conda", "tar_bz2", "whl"}
    else:  # BOTH or PREFER_CONDA
        allowed = {"conda", "tar_bz2"}

    filtered = [r for r in records if _archive_type(r.file_name) in allowed]

    if package_format_selection not in (
        PackageFormatSelection.PREFER_CONDA,
        PackageFormatSelection.PREFER_CONDA_WITH_WHL,
    ):
        return filtered

    preference = {"conda": 2, "tar_bz2": 1, "whl": 0}
    best: dict[Any, RepoDataRecord] = {}
    for record in filtered:
        key = (record.name.normalized, str(record.version), record.build)
        existing = best.get(key)
        if (
            existing is None
            or preference[_archive_type(record.file_name)] > preference[_archive_type(existing.file_name)]
        ):
            best[key] = record
    return list(best.values())


class MockRepoDataSource(RepoDataSource):
    """A mock implementation of the RepoDataSource protocol for testing."""

    def __init__(self, records_by_platform: dict[str, dict[str, List[RepoDataRecord]]]):
        """Initialize with a mapping of platform -> package_name -> records."""
        self._records = records_by_platform

    async def fetch_package_records(
        self, platform: Platform, name: PackageName, package_format_selection: PackageFormatSelection
    ) -> List[RepoDataRecord]:
        """Fetch records for a specific package name and platform."""
        platform_str = str(platform)
        name_str = name.normalized
        if platform_str in self._records and name_str in self._records[platform_str]:
            return _filter_by_format(self._records[platform_str][name_str], package_format_selection)
        return []

    def package_names(self, platform: Platform, package_format_selection: PackageFormatSelection) -> List[str]:
        """Return all available package names for the given platform."""
        platform_str = str(platform)
        if platform_str in self._records:
            return [
                name
                for name, records in self._records[platform_str].items()
                if _filter_by_format(records, package_format_selection)
            ]
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


def test_protocol_check() -> None:
    """Test that MockRepoDataSource is recognized as implementing RepoDataSource."""
    source = MockRepoDataSource({})
    assert isinstance(source, RepoDataSource)


def test_protocol_check_missing_method() -> None:
    """Test that objects missing methods are not recognized as RepoDataSource."""

    class IncompleteSource:
        async def fetch_package_records(self, platform: Any, name: Any) -> List[Any]:
            return []

        # Missing package_names method

    source = IncompleteSource()
    assert not isinstance(source, RepoDataSource)


@pytest.mark.asyncio
async def test_custom_source_query() -> None:
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
@pytest.mark.parametrize(
    "package_format,expected_results",
    [(PackageFormatSelection.ONLY_TAR_BZ2, 0), (PackageFormatSelection.PREFER_CONDA, 1)],
)
async def test_custom_source_query_package_format_selection(
    package_format: PackageFormatSelection, expected_results: int
) -> None:
    """Test querying with a custom RepoDataSource, filtering by package format."""
    # `create_test_record` always produces a `.conda` record, so `ONLY_TAR_BZ2` should
    # filter it out while `PREFER_CONDA` should keep it.
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
        package_format_selection=package_format,
    )

    assert len(results_snapshot(results)[0]) == expected_results


@pytest.mark.asyncio
async def test_custom_source_names() -> None:
    """Test querying package names from a custom RepoDataSource.

    `bar` has no records at all, so it has nothing matching any package format and
    is correctly excluded from the result.
    """
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

    assert sorted(n.normalized for n in names) == ["foo"]


@pytest.mark.asyncio
async def test_mixed_sources_query(conda_forge_channel: Channel) -> None:
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
async def test_custom_source_empty_results() -> None:
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
async def test_custom_source_multiple_platforms() -> None:
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


def test_invalid_source_type() -> None:
    """Test that invalid source types raise appropriate errors."""

    class NotASource:
        pass

    gateway = Gateway()

    with pytest.raises(TypeError, match="RepoDataSource"):
        import asyncio

        asyncio.run(
            gateway.query(
                sources=[NotASource()],  # type: ignore[list-item]
                platforms=["linux-64"],
                specs=["test"],
                recursive=False,
            )
        )


@pytest.mark.asyncio
async def test_custom_source_with_solve() -> None:
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
async def test_custom_source_backed_by_sparse_repodata() -> None:
    """Test a custom RepoDataSource that wraps SparseRepoData.

    This demonstrates how to create a custom source that loads records
    from local repodata files, which can be useful for offline scenarios
    or when you want to filter/transform repodata before solving.
    """
    import os

    from rattler import SparseRepoData

    # Load sparse repodata from the test data directory
    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    linux64_path = os.path.join(data_dir, "channels/dummy/linux-64/repodata.json")

    class SparseRepoDataSource(RepoDataSource):
        """A custom source that wraps SparseRepoData files."""

        def __init__(self, repodata_by_platform: dict[str, SparseRepoData]):
            self._repodata = repodata_by_platform

        async def fetch_package_records(
            self, platform: Platform, name: PackageName, package_format_selection: PackageFormatSelection
        ) -> List[RepoDataRecord]:
            platform_str = str(platform)
            if platform_str in self._repodata:
                return self._repodata[platform_str].load_records(name, package_format_selection)
            return []

        def package_names(self, platform: Platform, package_format_selection: PackageFormatSelection) -> List[str]:
            platform_str = str(platform)
            if platform_str in self._repodata:
                return self._repodata[platform_str].package_names(package_format_selection)
            return []

    # Create sparse repodata for linux-64
    linux64_data = SparseRepoData(
        channel=Channel("dummy"),
        subdir="linux-64",
        path=linux64_path,
    )

    # Wrap in our custom source
    source = SparseRepoDataSource({"linux-64": linux64_data})

    # Query using the custom source
    gateway = Gateway()
    results = await gateway.query(
        sources=[source],
        platforms=["linux-64"],
        specs=["foobar"],
        recursive=False,
    )

    # Verify we got results
    assert len(results) == 1
    assert len(results[0]) > 0

    # Test with solve - foobar depends on bors
    solved = await solve(
        sources=[source],
        specs=["foobar"],
        platforms=["linux-64"],
    )

    # Snapshot of solved packages with subdir prefix
    solved_snapshot = sorted(f"{r.subdir}/{r.name.normalized}-{r.version}-{r.build}" for r in solved)
    assert solved_snapshot == [
        "linux-64/bors-1.2.1-bla_1",
        "linux-64/foobar-2.1-bla_1",
    ]
