import json
from pathlib import Path

import pytest

from rattler import EntryPoint, LinkJson, NoArchLinks, PythonEntryPoints


@pytest.fixture
def test_data_dir() -> Path:
    return Path(__file__).parent.parent.parent.parent / "test-data"


def test_entry_point_construct_and_parse() -> None:
    ep = EntryPoint(command="run-task", module="my_app.cli", function="main")
    assert ep.command == "run-task"
    assert ep.module == "my_app.cli"
    assert ep.function == "main"
    assert str(ep) == "run-task = my_app.cli:main"

    ep.command = "task-runner"
    assert ep.command == "task-runner"
    ep.module = "my_app.runner"
    assert ep.module == "my_app.runner"
    ep.function = "run"
    assert ep.function == "run"
    assert str(ep) == "task-runner = my_app.runner:run"

    parsed = EntryPoint.from_str("jupyter-lab = jupyterlab.labapp:main")
    assert parsed.command == "jupyter-lab"
    assert parsed.module == "jupyterlab.labapp"
    assert parsed.function == "main"
    assert parsed == EntryPoint("jupyter-lab", "jupyterlab.labapp", "main")

    with pytest.raises(ValueError):
        EntryPoint.from_str("invalid_entry_point_string")


def test_link_json_python_noarch(test_data_dir: Path) -> None:
    link_path = test_data_dir / "link-json" / "jupyterlab-link.json"
    link_json = LinkJson.from_path(link_path)

    assert LinkJson.package_path() == Path("info/link.json")
    assert link_json.package_metadata_version == 1

    noarch = link_json.noarch
    assert noarch.is_python is True
    assert noarch.is_generic is False

    eps = noarch.entry_points
    assert eps is not None
    assert len(eps) == 4
    assert [e.command for e in eps] == [
        "jlpm",
        "jupyter-lab",
        "jupyter-labextension",
        "jupyter-labhub",
    ]
    assert [e.module for e in eps] == [
        "jupyterlab.jlpmapp",
        "jupyterlab.labapp",
        "jupyterlab.labextensions",
        "jupyterlab.labhubapp",
    ]
    assert all(e.function == "main" for e in eps)

    pep = noarch.python_entry_points
    assert pep is not None
    assert len(pep.entry_points) == 4

    # Test JSON serialization and roundtrip
    json_str = link_json.to_json()
    assert isinstance(json_str, str)
    data = json.loads(json_str)
    assert data["package_metadata_version"] == 1
    assert data["noarch"]["type"] == "python"
    assert len(data["noarch"]["entry_points"]) == 4

    roundtrip = LinkJson.from_str(json_str)
    assert roundtrip.package_metadata_version == link_json.package_metadata_version
    assert roundtrip.noarch.is_python is True
    assert roundtrip.noarch.entry_points == eps


def test_link_json_generic_noarch(test_data_dir: Path) -> None:
    link_path = test_data_dir / "link-json" / "tzdata-link.json"
    link_json = LinkJson.from_path(link_path)

    assert link_json.package_metadata_version == 1
    noarch = link_json.noarch
    assert noarch.is_generic is True
    assert noarch.is_python is False
    assert noarch.entry_points is None
    assert noarch.python_entry_points is None

    json_str = link_json.to_json()
    data = json.loads(json_str)
    assert data["noarch"]["type"] == "generic"


def test_link_json_construct() -> None:
    # Generic
    generic_links = NoArchLinks.generic()
    assert generic_links.is_generic is True
    lj_generic = LinkJson(noarch=generic_links, package_metadata_version=1)
    assert lj_generic.noarch.is_generic is True
    assert lj_generic.package_metadata_version == 1

    # Python with entry points
    ep1 = EntryPoint("cmd1", "mod1", "fn1")
    ep2 = EntryPoint("cmd2", "mod2", "fn2")
    py_links = NoArchLinks.python([ep1, ep2])
    assert py_links.is_python is True
    assert py_links.entry_points == [ep1, ep2]

    lj_python = LinkJson(noarch=py_links, package_metadata_version=2)
    assert lj_python.package_metadata_version == 2
    assert lj_python.noarch.is_python is True
    assert lj_python.noarch.entry_points == [ep1, ep2]

    # Mutability
    lj_python.package_metadata_version = 3
    assert lj_python.package_metadata_version == 3

    lj_python.noarch = generic_links
    assert lj_python.noarch.is_generic is True


def test_python_entry_points() -> None:
    ep = EntryPoint("tool", "tool.cli", "run")
    pep = PythonEntryPoints([ep])
    assert pep.entry_points == [ep]

    ep2 = EntryPoint("other", "other.cli", "start")
    pep.entry_points = [ep, ep2]
    assert pep.entry_points == [ep, ep2]
