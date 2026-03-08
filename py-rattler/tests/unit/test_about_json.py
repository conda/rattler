import json

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
