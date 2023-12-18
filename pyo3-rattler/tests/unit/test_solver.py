# type: ignore
import os.path

from rattler import (
    solve,
    Channel,
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
