import json
from pathlib import Path

import pytest

from rattler import AboutJson


def test_about_json_extra_metadata() -> None:
    payload = {
        "extra": {
            "flow_id": "2024.08.13",
            "nested": {"enabled": True, "values": [1, "two", None]},
        }
    }
    about = AboutJson.from_str(json.dumps(payload))

    assert about.extra == payload["extra"]

    updated = {"owner": "prefix.dev", "build": {"number": 7}, "tags": ["a", "b"]}
    about.extra = updated

    assert about.extra == updated


def test_about_json_extra_conversion() -> None:
    about = AboutJson.from_str("{}")

    about.extra = {
        "flag": False,
        "count": 2**63,
        "ratio": 0.5,
        "missing": None,
        "pair": (1, True),
        "order": {"z": 1, "a": 2},
    }

    assert about.extra == {
        "flag": False,
        "count": 2**63,
        "ratio": 0.5,
        "missing": None,
        "pair": [1, True],
        "order": {"z": 1, "a": 2},
    }
    assert type(about.extra["flag"]) is bool
    assert list(about.extra["order"]) == ["z", "a"]

    with pytest.raises(TypeError, match="keys must be strings"):
        about.extra = {"nested": {1: "one"}}
    with pytest.raises(TypeError, match="unsupported type 'object'"):
        about.extra = {"value": object()}
    with pytest.raises(ValueError):
        about.extra = ["not", "a", "mapping"]  # ty: ignore[invalid-assignment]


def test_about_json_to_json(test_data_dir: str) -> None:
    path = Path(test_data_dir) / "dummy-about.json"
    about = AboutJson.from_path(path)
    json_str = about.to_json()
    data = json.loads(json_str)

    assert data["channels"] == ["https://conda.anaconda.org/conda-forge"]
    assert data["description"] == "A dummy description."

    # Verify roundtrip
    roundtripped = AboutJson.from_str(json_str)
    assert roundtripped.channels == about.channels
    assert roundtripped.description == about.description
