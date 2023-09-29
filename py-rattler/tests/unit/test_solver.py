# type: ignore
import os.path
import subprocess

import pytest
from rattler import (
    Channel,
    ChannelConfig,
    fetch_repo_data,
    Platform,
    solve,
    MatchSpec,
    RepoDataRecord,
)


@pytest.fixture(scope="session")
def noarch_repo_data() -> None:
    port, repo_name = 8812, "test-repo-1"

    test_data_dir = os.path.join(
        os.path.dirname(__file__), "../../../test-data/test-server"
    )

    with subprocess.Popen(
        [
            "python",
            os.path.join(test_data_dir, "reposerver.py"),
            "-d",
            os.path.join(test_data_dir, "repo"),
            "-n",
            repo_name,
            "-p",
            str(port),
        ]
    ) as proc:
        yield port, repo_name
        proc.terminate()


@pytest.fixture(scope="session")
def linux64_repo_data() -> None:
    port, repo_name = 8813, "test-repo-2"

    test_data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")

    with subprocess.Popen(
        [
            "python",
            os.path.join(test_data_dir, "test-server/reposerver.py"),
            "-d",
            os.path.join(test_data_dir, "channels/dummy/"),
            "-n",
            repo_name,
            "-p",
            str(port),
        ]
    ) as proc:
        yield port, repo_name
        proc.terminate()


@pytest.mark.asyncio
async def test_solve(
    tmp_path,
    noarch_repo_data,
    linux64_repo_data,
):
    noarch_port, noarch_repo = noarch_repo_data
    linux64_port, linux64_repo = linux64_repo_data
    cache_dir = tmp_path / "test_repo_data_download"
    noarch_chan = Channel(
        noarch_repo, ChannelConfig(f"http://localhost:{noarch_port}/")
    )
    plat_noarch = Platform("noarch")
    linux64_chan = Channel(
        linux64_repo, ChannelConfig(f"http://localhost:{linux64_port}/")
    )
    plat_linux64 = Platform("linux-64")

    noarch_data = await fetch_repo_data(
        channels=[noarch_chan],
        platforms=[plat_noarch],
        cache_path=cache_dir,
        callback=None,
    )

    linux64_data = await fetch_repo_data(
        channels=[linux64_chan],
        platforms=[plat_linux64],
        cache_path=cache_dir,
        callback=None,
    )

    available_packages = [
        package.into_repo_data(noarch_chan) for package in noarch_data
    ] + [package.into_repo_data(linux64_chan) for package in linux64_data]

    solved_data = solve(
        [MatchSpec("test-package"), MatchSpec("foobar"), MatchSpec("baz")],
        available_packages,
        [],
        [],
        [],
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 4
