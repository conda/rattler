import json
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

import pytest

from rattler import Channel, Config, Gateway, SourceConfig


@pytest.mark.asyncio
async def test_single_record_in_recursive_query(gateway: Gateway, conda_forge_channel: Channel) -> None:
    subdirs = await gateway.query(
        [conda_forge_channel], ["linux-64", "noarch"], ["python ==3.10.0 h543edf9_1_cpython"], recursive=True
    )

    python_records = [record for subdir in subdirs for record in subdir if record.name == "python"]
    assert len(python_records) == 1


@pytest.mark.asyncio
async def test_channel_notices(tmp_path: Path) -> None:
    noarch = tmp_path / "noarch"
    noarch.mkdir()
    (noarch / "repodata.json").write_text(
        json.dumps(
            {
                "packages": {
                    "demo-1.0-0.tar.bz2": {
                        "name": "demo",
                        "version": "1.0",
                        "build": "0",
                        "build_number": 0,
                        "depends": [],
                        "subdir": "noarch",
                    }
                }
            }
        )
    )
    (tmp_path / "notices.json").write_text(
        json.dumps(
            {
                "notices": [
                    {
                        "id": "security-1",
                        "message": "Please update demo",
                        "level": "critical",
                        "created_at": "2025-01-01T00:00:00Z",
                        "expires_at": "2099-01-01T00:00:00Z",
                    }
                ]
            }
        )
    )

    gateway = Gateway()
    channel = Channel(str(tmp_path))
    notices = await gateway.channel_notices([channel])
    assert len(notices) == 1
    assert notices[0].id == "security-1"
    assert notices[0].level == "critical"
    assert notices[0].expires_at == "2099-01-01T00:00:00Z"

    result = await gateway.query([channel], ["noarch"], ["demo"], channel_notices=True)
    assert result.repodata is result
    assert result.notices == notices

    names = await gateway.names([channel], ["noarch"], channel_notices=True)
    assert names.names is names
    assert names.notices == notices


def test_init_per_channel_config_key() -> None:
    test_source_config = SourceConfig()

    # build an incorrect per_channel_config & check for TypeError
    channel = Channel("https://conda.anaconda.org/conda-forge")
    # per_channel_config uses a Channel object as the key — this is what caused the original bug
    per_channel_config = {channel: test_source_config}
    with pytest.raises(TypeError):
        Gateway(per_channel_config=per_channel_config)  # type: ignore[arg-type]

    # build right config & make sure gateway object initializes
    right_config = {"http://test-config-key.com": test_source_config}
    test_gateway = Gateway(per_channel_config=right_config)
    assert test_gateway is not None


@pytest.mark.asyncio
async def test_gateway_from_config_applies_mirrors_and_repodata_config() -> None:
    requested_paths: list[str] = []

    class RepodataHandler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            requested_paths.append(self.path)
            if self.path == "/mirror/noarch/repodata.json":
                body = json.dumps({"packages": {}}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            else:
                self.send_response(404)
                self.end_headers()

        def log_message(self, format: str, *args: object) -> None:
            pass

    server = HTTPServer(("127.0.0.1", 0), RepodataHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        config = Config.from_toml(f"""\
            [mirrors]
            "https://upstream.invalid/channel" = ["http://127.0.0.1:{server.server_port}/mirror"]

            [repodata-config]
            disable-zstd = true
            disable-bzip2 = true
            disable-sharded = true
        """)
        gateway = Gateway.from_config(config)

        records = await gateway.query([Channel("https://upstream.invalid/channel")], ["noarch"], ["demo"])

        assert records == [[]]
        assert requested_paths == ["/mirror/noarch/repodata.json"]
    finally:
        server.shutdown()
        thread.join()
