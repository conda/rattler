# type: ignore
import os.path

from rattler import (
    solve,
    Channel,
    ChannelPriority,
    MatchSpec,
    RepoDataRecord,
    SparseRepoData,
)


def test_solve():
    linux64_chan = Channel("conda-forge")
    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    linux64_path = os.path.join(data_dir, "channels/conda-forge/linux-64/repodata.json")
    linux64_data = SparseRepoData(
        channel=linux64_chan,
        subdir="linux-64",
        path=linux64_path,
    )

    solved_data = solve(
        [MatchSpec("python"), MatchSpec("sqlite")],
        [linux64_data],
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 19


def test_solve_channel_priority_disabled():
    cf_chan = Channel("conda-forge")
    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    cf_path = os.path.join(data_dir, "channels/conda-forge/linux-64/repodata.json")
    cf_data = SparseRepoData(
        channel=cf_chan,
        subdir="linux-64",
        path=cf_path,
    )

    pytorch_chan = Channel("pytorch")
    pytorch_path = os.path.join(data_dir, "channels/pytorch/linux-64/repodata.json")
    pytorch_data = SparseRepoData(
        channel=pytorch_chan,
        subdir="linux-64",
        path=pytorch_path,
    )

    solved_data = solve(
        [MatchSpec("pytorch-cpu=0.4.1=py36_cpu_1")],
        [cf_data, pytorch_data],
        channel_priority=ChannelPriority.Disabled,
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert list(filter(lambda r: r.file_name.startswith("pytorch-cpu-0.4.1-py36_cpu_1"), solved_data))[0].channel == "https://conda.anaconda.org/pytorch/"
    assert len(solved_data) == 32
