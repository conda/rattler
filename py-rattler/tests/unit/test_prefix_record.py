import os
from pathlib import Path
from rattler import (
    PrefixRecord,
    PrefixPaths,
    PrefixPathsEntry,
    PrefixPathType,
    FileMode,
    PackageRecord,
    RepoDataRecord,
    VersionWithSource,
)


def test_load_prefix_record() -> None:
    r = PrefixRecord.from_path(
        Path(__file__).parent / ".." / ".." / ".." / "test-data" / "conda-meta" / "tk-8.6.12-h8ffe710_0.json"
    )
    assert r.arch == "x86_64"
    assert r.build == "h8ffe710_0"
    assert r.build_number == 0
    assert r.channel == "https://conda.anaconda.org/conda-forge/win-64"
    assert len(r.constrains) == 0
    assert len(r.depends) == 2
    assert str(r.extracted_package_dir) == "C:\\Users\\bas\\micromamba\\envs\\conda\\pkgs\\tk-8.6.12-h8ffe710_0"
    assert r.features is None
    assert r.file_name == "tk-8.6.12-h8ffe710_0.tar.bz2"
    assert len(r.files) == len(r.paths_data.paths) == 1099
    assert r.subdir == "win-64"
    assert r.noarch.none
    paths = r.paths_data
    assert isinstance(paths, PrefixPaths)
    paths_with_placeholder = 0
    assert paths.paths_version == 1
    for entry in paths.paths:
        assert isinstance(entry, PrefixPathsEntry)
        assert entry.relative_path is not None
        assert isinstance(entry.path_type, PrefixPathType)
        if entry.prefix_placeholder is not None:
            paths_with_placeholder += 1
            assert isinstance(entry.file_mode, FileMode)
            assert entry.file_mode.text or entry.file_mode.binary
            assert entry.sha256_in_prefix.hex() is not None
        else:
            assert entry.file_mode.unknown
            assert entry.sha256.hex() is not None
        assert entry.size_in_bytes > 0

        # check that it implements os.PathLike
        isinstance(entry, os.PathLike)

    assert paths_with_placeholder == 3


def test_create_prefix_record() -> None:
    r = PrefixRecord.from_path(
        Path(__file__).parent / ".." / ".." / ".." / "test-data" / "conda-meta" / "tk-8.6.12-h8ffe710_0.json"
    )

    r.arch = "foobar"
    assert r.arch == "foobar"
    r.version = VersionWithSource("1.0.23")
    assert r.version == VersionWithSource("1.0.23")

    sha256 = "c505c9636f910d737b3a304ca2daff88fef1a92450d4dcd2f1a9d735eb1fa4d6"

    r.sha256 = bytes.fromhex(sha256)
    assert r.sha256.hex() == sha256

    package_record = PackageRecord(
        name="foobar",
        version="1.0",
        build_number=1,
        build="foo_1",
        platform="win-64",
        subdir="win",
        arch="x86_64",
    )
    print(package_record.to_json())

    repodata_record = RepoDataRecord(
        package_record,
        file_name="foobar.tar.bz2",
        url="https://foobar.com/foobar.tar.bz2",
        channel="https://foobar.com/win-64",
    )

    assert repodata_record.url == "https://foobar.com/foobar.tar.bz2"
    assert repodata_record.channel == "https://foobar.com/win-64"
    assert repodata_record.file_name == "foobar.tar.bz2"

    paths_data = PrefixPaths()

    r = PrefixRecord(
        repodata_record,
        paths_data=paths_data,
    )

    r.requested_spec = "foo"
    assert r.requested_spec == "foo"

    print(r.to_json())


def test_prefix_paths() -> None:
    prefix_path_type = PrefixPathType("hardlink")
    assert prefix_path_type.hardlink

    # create a paths entry
    prefix_paths_entry = PrefixPathsEntry(
        Path("foo/bar/baz"),
        prefix_path_type,
        prefix_placeholder="placeholder_foo_bar",
        file_mode=FileMode("binary"),
        sha256=bytes.fromhex("c505c9636f910d737b3a304ca2daff88fef1a92450d4dcd2f1a9d735eb1fa4d6"),
        sha256_in_prefix=bytes.fromhex("c505c9636f910d737b3a304ca2daff88fef1a92450d4dcd2f1a9d735eb1fa4d6"),
        size_in_bytes=1024,
    )

    assert str(prefix_paths_entry.relative_path) == "foo/bar/baz"
    assert prefix_paths_entry.path_type.hardlink
    assert prefix_paths_entry.prefix_placeholder == "placeholder_foo_bar"
    assert prefix_paths_entry.file_mode.binary
    assert prefix_paths_entry.sha256.hex() == "c505c9636f910d737b3a304ca2daff88fef1a92450d4dcd2f1a9d735eb1fa4d6"
    assert (
        prefix_paths_entry.sha256_in_prefix.hex() == "c505c9636f910d737b3a304ca2daff88fef1a92450d4dcd2f1a9d735eb1fa4d6"
    )
    assert prefix_paths_entry.size_in_bytes == 1024
