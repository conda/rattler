import json
from pathlib import Path

from rattler import IndexJson, PathsJson, RunExportsJson


def test_index_json_to_json(test_data_dir: str) -> None:
    path = Path(test_data_dir) / "conda-22.11.1-py38haa244fe_1-index.json"
    index = IndexJson.from_path(path)
    json_str = index.to_json()
    data = json.loads(json_str)

    assert data["name"] == "conda"
    assert data["version"] == "22.11.1"
    assert data["build"] == "py38haa244fe_1"

    roundtripped = IndexJson.from_str(json_str)
    assert roundtripped.name == index.name
    assert roundtripped.version == index.version
    assert roundtripped.build == index.build


def test_paths_json_to_json(test_data_dir: str) -> None:
    path = Path(test_data_dir) / "conda-22.9.0-py38haa244fe_2-paths.json"
    paths = PathsJson.from_path(path)
    json_str = paths.to_json()
    data = json.loads(json_str)

    assert data["paths_version"] == 1
    assert len(data["paths"]) > 0

    roundtripped = PathsJson.from_str(json_str)
    assert roundtripped.paths_version == paths.paths_version
    assert len(roundtripped.paths) == len(paths.paths)


def test_run_exports_json_is_empty_and_to_json(test_data_dir: str) -> None:
    empty = RunExportsJson()
    assert empty.is_empty()

    path = Path(test_data_dir) / "python-3.10.6-h2c4edbf_0_cpython-run_exports.json"
    run_exports = RunExportsJson.from_path(path)
    assert not run_exports.is_empty()

    json_str = run_exports.to_json()
    data = json.loads(json_str)
    assert data["weak"] == ["python_abi 3.10.* *_cp310"]

    roundtripped = RunExportsJson.from_str(json_str)
    assert roundtripped.weak == run_exports.weak
    assert not roundtripped.is_empty()
