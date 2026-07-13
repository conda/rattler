import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Generator

import pytest

from rattler import Channel, ChannelConfig, Client, Gateway, Platform


class DelayedHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        time.sleep(3)  # Delay for 3 seconds
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"{}")


def run_server(server: HTTPServer) -> None:
    server.serve_forever()


@pytest.fixture
def delayed_server() -> Generator[int, None, None]:
    server = HTTPServer(("localhost", 0), DelayedHandler)
    port = server.server_port
    thread = threading.Thread(target=run_server, args=(server,))
    thread.daemon = True
    thread.start()
    yield port
    server.shutdown()


@pytest.mark.asyncio
async def test_gateway_timeout(delayed_server: int) -> None:
    port = delayed_server
    channel_url = f"http://localhost:{port}/test-channel"

    # Create a gateway with a client that has a 1-second timeout
    # The server delays for 3 seconds, so this should timeout
    client = Client(timeout=1)
    gateway = Gateway(client=client)

    channel = Channel("test-channel", ChannelConfig(channel_url))

    with pytest.raises(Exception) as excinfo:
        await gateway.query([channel], [Platform("linux-64")], ["python"])

    # The exact error message might vary depending on how reqwest/pyo3 reports it,
    # but it should be a timeout related error.
    error_msg = str(excinfo.value).lower()
    assert "timeout" in error_msg or "timed out" in error_msg


@pytest.mark.asyncio
async def test_gateway_no_timeout(delayed_server: int) -> None:
    port = delayed_server
    channel_url = f"http://localhost:{port}/test-channel"

    # Create a gateway with a client that has a 5-second timeout
    # The server delays for 3 seconds, so this should succeed (or at least not timeout)
    client = Client(timeout=5)
    gateway = Gateway(client=client)

    channel = Channel("test-channel", ChannelConfig(channel_url))

    # It might still fail because we return empty {}, but it shouldn't be a timeout
    try:
        await gateway.query([channel], [Platform("linux-64")], ["python"])
    except Exception as e:
        assert "timeout" not in str(e).lower()
