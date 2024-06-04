# type: ignore
import os.path

import pytest
from xprocess import ProcessStarter
from rattler import Channel, ChannelConfig, fetch_repo_data, SparseRepoData, PackageName
from rattler.platform import Platform
from rattler.repo_data.record import RepoDataRecord


@pytest.fixture(scope="session")
def serve_repo_data(xprocess) -> None:
    port, repo_name = 8912, "test-repo"

    test_data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/test-server")

    class Starter(ProcessStarter):
        # startup pattern
        pattern = f"Server started at localhost:{port}"

        # command to start process
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
        ]

    # ensure process is running and return its logfile
    xprocess.ensure("reposerver", Starter)

    yield port, repo_name

    # clean up whole process tree afterwards
    xprocess.getinfo("reposerver").terminate()


@pytest.mark.asyncio
async def test_fetch_repo_data(
    tmp_path,
    serve_repo_data,
):
    port, repo = serve_repo_data
    cache_dir = tmp_path / "test_repo_data_download"
    chan = Channel(repo, ChannelConfig(f"http://localhost:{port}/"))
    plat = Platform("noarch")

    result = await fetch_repo_data(
        channels=[chan],
        platforms=[plat],
        cache_path=cache_dir,
        callback=None,
    )
    assert isinstance(result, list)

    repodata = result[0]
    assert isinstance(repodata, SparseRepoData)

    package = PackageName(repodata.package_names()[0])
    repodata_record = repodata.load_records(package)[0]
    assert isinstance(repodata_record, RepoDataRecord)

    assert repodata_record.channel == f"http://localhost:{port}/{repo}/"
    assert repodata_record.file_name == "test-package-0.1-0.tar.bz2"
    assert repodata_record.name == PackageName("test-package")
    assert repodata_record.url == f"http://localhost:{port}/test-repo/noarch/test-package-0.1-0.tar.bz2"
