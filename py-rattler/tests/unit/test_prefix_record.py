import os
from pathlib import Path
from rattler import PrefixRecord, PrefixPaths, PrefixPathsEntry, PrefixPathType, FileMode


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
    assert r.noarch is None
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
