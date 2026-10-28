from pathlib import Path

from rattler import PathsEntry, PathsJson, PathType


def test_paths_json_sort() -> None:
    test_data_dir = Path(__file__).parent.parent.parent.parent / "test-data"
    paths_file = test_data_dir / "conda-22.9.0-py38haa244fe_2-paths.json"

    paths_json = PathsJson.from_path(paths_file)

    # Set custom unsorted paths
    paths_json.paths = [
        PathsEntry(
            relative_path="z/file",
            no_link=False,
            path_type=PathType("hardlink"),
            prefix_placeholder=None,
            sha256=None,
            size_in_bytes=None,
        ),
        PathsEntry(
            relative_path="a/file",
            no_link=False,
            path_type=PathType("hardlink"),
            prefix_placeholder=None,
            sha256=None,
            size_in_bytes=None,
        ),
        PathsEntry(
            relative_path="m/file",
            no_link=False,
            path_type=PathType("hardlink"),
            prefix_placeholder=None,
            sha256=None,
            size_in_bytes=None,
        ),
    ]

    # Verify initially unsorted
    assert [p.relative_path for p in paths_json.paths] == [
        Path("z/file"),
        Path("a/file"),
        Path("m/file"),
    ]

    # Sort in-place
    paths_json.sort()

    # Verify sorted
    assert [p.relative_path for p in paths_json.paths] == [
        Path("a/file"),
        Path("m/file"),
        Path("z/file"),
    ]
