# type: ignore
import os.path

import pytest
from xprocess import ProcessStarter

from rattler import Channel, ChannelConfig, fetch_repo_data
from rattler.networking import AddHeadersMiddleware, Client
from rattler.platform import Platform


@pytest.fixture(scope="module")
def serve_bearer_repo(xprocess) -> None:
    """Start a test server that requires bearer authentication."""
    port = 8913
    repo_name = "bearer-repo"
    bearer_token = "test-secret-token"

    test_data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/test-server")

    class Starter(ProcessStarter):
        pattern = f"Server started at localhost:{port}"
        args = [
            "python",
            "-u",
            os.path.join(test_data_dir, "reposerver.py"),
            "-d",
            os.path.join(test_data_dir, "repo"),
            "-n",
            repo_name,
            "-p",
            str(port),
            "--bearer",
            bearer_token,
        ]

    xprocess.ensure("bearer_reposerver", Starter)
    yield port, repo_name, bearer_token
    xprocess.getinfo("bearer_reposerver").terminate()


@pytest.mark.asyncio
async def test_add_headers_middleware_with_bearer_auth(
    tmp_path,
    serve_bearer_repo,
) -> None:
    """Test that AddHeadersMiddleware correctly adds bearer token for authentication."""
    port, repo, token = serve_bearer_repo
    cache_dir = tmp_path / "test_bearer_auth"

    # Track callback invocations
    callback_calls = []

    def header_callback(host: str, path: str) -> dict[str, str] | None:
        callback_calls.append((host, path))
        # Add bearer token for our test server
        if f"localhost:{port}" in host or host == "localhost":
            return {"Authorization": f"Bearer {token}"}
        return None

    client = Client([AddHeadersMiddleware(header_callback)])
    chan = Channel(repo, ChannelConfig(f"http://localhost:{port}/"))
    plat = Platform("noarch")

    result = await fetch_repo_data(
        channels=[chan],
        platforms=[plat],
        cache_path=cache_dir,
        callback=None,
        client=client,
    )

    # Verify that the request succeeded (which means auth worked)
    assert isinstance(result, list)
    assert len(result) > 0

    # Verify callback was invoked
    assert len(callback_calls) > 0
    # Check that host contains localhost
    assert any("localhost" in host for host, _ in callback_calls)


@pytest.mark.asyncio
async def test_add_headers_middleware_wrong_token_fails(
    tmp_path,
    serve_bearer_repo,
) -> None:
    """Test that requests fail when the wrong token is provided."""
    port, repo, _ = serve_bearer_repo
    cache_dir = tmp_path / "test_wrong_token"

    def header_callback(host: str, path: str) -> dict[str, str] | None:
        # Provide wrong token
        return {"Authorization": "Bearer wrong-token"}

    client = Client([AddHeadersMiddleware(header_callback)])
    chan = Channel(repo, ChannelConfig(f"http://localhost:{port}/"))
    plat = Platform("noarch")

    # This should fail because the token is wrong
    with pytest.raises(Exception):
        await fetch_repo_data(
            channels=[chan],
            platforms=[plat],
            cache_path=cache_dir,
            callback=None,
            client=client,
        )
