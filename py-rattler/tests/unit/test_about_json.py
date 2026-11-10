import json
from pathlib import Path

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
