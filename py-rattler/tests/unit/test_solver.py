import datetime
import json
import os
from pathlib import Path

import pytest

from rattler import (
    solve,
    ChannelPriority,
    RepoDataRecord,
    Channel,
    Gateway,
    SparseRepoData,
    MatchSpec,
    solve_with_sparse_repodata,
    PackageFormatSelection,
)


@pytest.mark.asyncio
async def test_solve(gateway: Gateway, conda_forge_channel: Channel) -> None:
    solved_data = await solve(
        [conda_forge_channel],
        ["python", "sqlite"],
        platforms=["linux-64"],
        gateway=gateway,
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 19


@pytest.mark.asyncio
async def test_solve_exclude_newer(gateway: Gateway, dummy_channel: Channel) -> None:
    """Tests the exclude_newer parameter of the solve function.

    The exclude_newer parameter is used to exclude any record that is newer than
    the given datetime. This test case will exclude the record with version
    4.0.2 because of the `exclude_newer` argument.
    """
    solved_data = await solve(
        [dummy_channel],
        ["foo"],
        platforms=["linux-64"],
        gateway=gateway,
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 1
    assert str(solved_data[0].version) == "4.0.2"

    # Now solve again but with a datetime that is before the version 4.0.2
    solved_data = await solve(
        [dummy_channel],
        ["foo"],
        platforms=["linux-64"],
        gateway=gateway,
        exclude_newer=datetime.datetime.fromisoformat("2021-01-01"),
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 1
    assert str(solved_data[0].version) == "3.0.2"


@pytest.mark.asyncio
async def test_solve_lowest(gateway: Gateway, dummy_channel: Channel) -> None:
    solved_data = await solve(
        [dummy_channel],
        ["foobar"],
        platforms=["linux-64"],
        gateway=gateway,
        strategy="lowest",
    )

    assert isinstance(solved_data, list)
    assert len(solved_data) == 2

    assert solved_data[0].name.normalized == "foobar"
    assert str(solved_data[0].version) == "2.0"

    assert solved_data[1].name.normalized == "bors"
    assert str(solved_data[1].version) == "1.0"


@pytest.mark.asyncio
async def test_solve_lowest_direct(gateway: Gateway, dummy_channel: Channel) -> None:
    solved_data = await solve(
        [dummy_channel],
        ["foobar"],
        platforms=["linux-64"],
        gateway=gateway,
        strategy="lowest-direct",
    )

    assert isinstance(solved_data, list)
    assert len(solved_data) == 2

    assert solved_data[0].name.normalized == "foobar"
    assert str(solved_data[0].version) == "2.0"

    assert solved_data[1].name.normalized == "bors"
    assert str(solved_data[1].version) == "1.2.1"


@pytest.mark.asyncio
async def test_solve_channel_priority_disabled(
    gateway: Gateway, pytorch_channel: Channel, conda_forge_channel: Channel
) -> None:
    solved_data = await solve(
        [conda_forge_channel, pytorch_channel],
        ["pytorch-cpu ==0.4.1 py36_cpu_1"],
        platforms=["linux-64"],
        gateway=gateway,
        channel_priority=ChannelPriority.Disabled,
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert (
        list(filter(lambda r: r.file_name.startswith("pytorch-cpu-0.4.1-py36_cpu_1"), solved_data))[0].channel
        == pytorch_channel.base_url
    )
    assert len(solved_data) == 32


@pytest.mark.asyncio
async def test_solve_constraints(gateway: Gateway, dummy_channel: Channel) -> None:
    solved_data = await solve(
        [dummy_channel],
        ["foobar"],
        constraints=["bors <=1", "nonexisting"],
        platforms=["linux-64"],
        gateway=gateway,
    )

    assert isinstance(solved_data, list)
    assert len(solved_data) == 2

    assert solved_data[0].file_name == "foobar-2.1-bla_1.tar.bz2"
    assert solved_data[1].file_name == "bors-1.0-bla_1.tar.bz2"


@pytest.mark.asyncio
async def test_solve_with_repodata() -> None:
    linux64_chan = Channel("conda-forge")
    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    linux64_path = os.path.join(data_dir, "channels/dummy/linux-64/repodata.json")
    linux64_data = SparseRepoData(
        channel=linux64_chan,
        subdir="linux-64",
        path=linux64_path,
    )

    solved_data = await solve_with_sparse_repodata(
        [MatchSpec("foobar")],
        [linux64_data],
    )

    assert isinstance(solved_data, list)
    assert isinstance(solved_data[0], RepoDataRecord)
    assert len(solved_data) == 2


def python_repodata(tmp_path: Path, python_version: str = "3.12.0") -> SparseRepoData:
    repodata_path = tmp_path / "noarch" / "repodata.json"
    repodata_path.parent.mkdir()
    repodata_path.write_text(
        json.dumps(
            {
                "info": {"subdir": "noarch"},
                "packages": {
                    f"python-{python_version}-0.tar.bz2": {
                        "build": "0",
                        "build_number": 0,
                        "depends": [],
                        "name": "python",
                        "subdir": "noarch",
                        "version": python_version,
                    },
                    "pip-1.0-0.tar.bz2": {
                        "build": "0",
                        "build_number": 0,
                        "depends": [],
                        "name": "pip",
                        "subdir": "noarch",
                        "version": "1.0",
                    },
                    "standalone-1.0-0.tar.bz2": {
                        "build": "0",
                        "build_number": 0,
                        "depends": [],
                        "name": "standalone",
                        "subdir": "noarch",
                        "version": "1.0",
                    },
                    "application-1.0-0.tar.bz2": {
                        "name": "application",
                        "version": "1.0",
                        "build": "0",
                        "build_number": 0,
                        "depends": ["python"],
                        "subdir": "noarch",
                    },
                },
            }
        )
    )
    return SparseRepoData(
        channel=Channel(str(tmp_path)),
        subdir="noarch",
        path=repodata_path,
    )


@pytest.mark.asyncio
async def test_solve_adds_pip_to_cached_python(tmp_path: Path) -> None:
    python_repodata(tmp_path)
    channel = Channel(str(tmp_path))
    gateway = Gateway()

    without_pip = await solve(
        [channel],
        ["application"],
        platforms=["noarch"],
        gateway=gateway,
    )
    with_pip = await solve(
        [channel],
        ["application"],
        platforms=["noarch"],
        gateway=gateway,
        add_pip_as_python_dependency=True,
    )

    assert {record.name.normalized for record in without_pip} == {
        "application",
        "python",
    }
    assert {record.name.normalized for record in with_pip} == {
        "application",
        "python",
        "pip",
    }
    python_record = next(record for record in with_pip if record.name.normalized == "python")
    assert "pip" in python_record.depends


@pytest.mark.parametrize(
    ("python_version", "add_pip", "expected_names"),
    [
        ("2.7.18", True, {"python", "pip"}),
        ("3.12.0", True, {"python", "pip"}),
        ("4.0.0", True, {"python"}),
        ("3.12.0", False, {"python"}),
    ],
)
@pytest.mark.asyncio
async def test_solve_with_sparse_repodata_adds_pip_to_python(
    tmp_path: Path,
    python_version: str,
    add_pip: bool,
    expected_names: set[str],
) -> None:
    sparse_repodata = python_repodata(tmp_path, python_version)

    solved_data = await solve_with_sparse_repodata(
        ["python"],
        [sparse_repodata],
        add_pip_as_python_dependency=add_pip,
    )

    assert {record.name.normalized for record in solved_data} == expected_names
    python_record = next(record for record in solved_data if record.name.normalized == "python")
    assert ("pip" in python_record.depends) is (add_pip and python_version[0] in "23")


@pytest.mark.asyncio
async def test_add_pip_does_not_patch_locked_python(tmp_path: Path) -> None:
    sparse_repodata = python_repodata(tmp_path)
    locked_python = sparse_repodata.load_records("python")[0]

    solved_data = await solve_with_sparse_repodata(
        ["python", "standalone"],
        [sparse_repodata],
        locked_packages=[locked_python],
        add_pip_as_python_dependency=True,
    )

    assert {record.name.normalized for record in solved_data} == {"python", "standalone"}
    python_record = next(record for record in solved_data if record.name.normalized == "python")
    assert "pip" not in python_record.depends


@pytest.mark.asyncio
async def test_conditional_root_requirement_satisfied(gateway: Gateway, dummy_channel: Channel) -> None:
    """Test that a conditional root requirement is included when the condition is satisfied."""
    from rattler import GenericVirtualPackage, MatchSpec, PackageName, Version

    solved_data = await solve(
        [dummy_channel],
        [MatchSpec('foo[when="__unix"]')],
        platforms=["linux-64"],
        gateway=gateway,
        virtual_packages=[GenericVirtualPackage(PackageName("__unix"), Version("0"), "0")],
    )

    assert isinstance(solved_data, list)
    assert len(solved_data) > 0
    # Foo should be included because __unix virtual package exists
    package_names = [r.name.normalized for r in solved_data]
    assert "foo" in package_names


@pytest.mark.asyncio
async def test_conditional_root_requirement_not_satisfied(gateway: Gateway, dummy_channel: Channel) -> None:
    """Test that a conditional root requirement is excluded when the condition is not satisfied."""
    from rattler import GenericVirtualPackage, MatchSpec, PackageName, Version

    solved_data = await solve(
        [dummy_channel],
        [MatchSpec('foo[when="__win"]')],
        platforms=["linux-64"],
        gateway=gateway,
        virtual_packages=[GenericVirtualPackage(PackageName("__unix"), Version("0"), "0")],
    )

    assert isinstance(solved_data, list)
    # Foo should NOT be included because __win virtual package does not exist
    package_names = [r.name.normalized for r in solved_data]
    assert "foo" not in package_names


@pytest.mark.asyncio
async def test_conditional_root_requirement_with_logic(gateway: Gateway, dummy_channel: Channel) -> None:
    """Test that a conditional root requirement with AND logic is evaluated correctly."""
    from rattler import GenericVirtualPackage, MatchSpec, PackageName, Version

    solved_data = await solve(
        [dummy_channel],
        [MatchSpec('foo[when="__unix and __linux"]')],
        platforms=["linux-64"],
        gateway=gateway,
        virtual_packages=[
            GenericVirtualPackage(PackageName("__unix"), Version("0"), "0"),
            GenericVirtualPackage(PackageName("__linux"), Version("0"), "0"),
        ],
    )

    assert isinstance(solved_data, list)
    assert len(solved_data) > 0
    # Foo should be included because both __unix and __linux virtual packages exist
    package_names = [r.name.normalized for r in solved_data]
    assert "foo" in package_names


@pytest.mark.asyncio
async def test_solve_with_sparse_repodata_conditional_dependencies() -> None:
    """Test that solve_with_sparse_repodata handles conditional dependencies in repodata.

    This is a regression test for https://github.com/conda/rattler/issues/1917
    The solver should properly resolve packages with conditional dependencies like
    `osx-dependency[when="__osx"]` when using sparse repodata.
    """
    from rattler import GenericVirtualPackage, PackageName, Version

    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    noarch_path = os.path.join(data_dir, "channels/conditional-repodata/noarch/repodata.json")
    noarch_chan = Channel("conditional-repodata")
    noarch_data = SparseRepoData(
        channel=noarch_chan,
        subdir="noarch",
        path=noarch_path,
    )

    # Test: Platform-conditional dependency with __linux virtual package
    solved_data = await solve_with_sparse_repodata(
        [MatchSpec("package")],
        [noarch_data],
        virtual_packages=[
            GenericVirtualPackage(PackageName("__linux"), Version("0"), "0"),
        ],
    )

    assert isinstance(solved_data, list)
    package_names = [r.name.normalized for r in solved_data]
    assert "package" in package_names
    assert "linux-dependency" in package_names
    # Should NOT include osx or win dependencies
    assert "osx-dependency" not in package_names
    assert "win-dependency" not in package_names


@pytest.mark.asyncio
async def test_solve_with_sparse_repodata_version_conditional_dependencies() -> None:
    """Test that solve_with_sparse_repodata handles version-conditional dependencies.

    This is a regression test for https://github.com/conda/rattler/issues/1917
    The solver should properly resolve conditional dependencies like
    `package[when="side-dependency=0.2"]` when the condition is satisfied.
    """
    from rattler import GenericVirtualPackage, PackageName, Version

    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    noarch_path = os.path.join(data_dir, "channels/conditional-repodata/noarch/repodata.json")
    noarch_chan = Channel("conditional-repodata")
    noarch_data = SparseRepoData(
        channel=noarch_chan,
        subdir="noarch",
        path=noarch_path,
    )

    # Test: Version-conditional dependency - when side-dependency=0.2 is requested,
    # "package" should be included due to the conditional `package[when="side-dependency=0.2"]`
    solved_data = await solve_with_sparse_repodata(
        [MatchSpec("conditional-dependency"), MatchSpec("side-dependency=0.2")],
        [noarch_data],
        virtual_packages=[
            GenericVirtualPackage(PackageName("__linux"), Version("0"), "0"),
        ],
    )

    assert isinstance(solved_data, list)
    package_names = [r.name.normalized for r in solved_data]
    assert "conditional-dependency" in package_names
    assert "side-dependency" in package_names
    # "package" should be included because side-dependency=0.2 satisfies the condition
    assert "package" in package_names
    # linux-dependency should also be included because __linux is in virtual packages
    assert "linux-dependency" in package_names


@pytest.mark.asyncio
async def test_solve_with_sparse_repodata_with_wheels() -> None:
    """
    Test that solve when repodata includes `packages.whl` works as expected.
    """
    from rattler import GenericVirtualPackage, PackageName, Version

    chn = Channel("with-wheels")

    data_dir = os.path.join(os.path.dirname(__file__), "../../../test-data/")
    noarch_path = os.path.join(data_dir, "channels/with-wheels/noarch/repodata.json")
    noarch_data = SparseRepoData(
        channel=chn,
        subdir="noarch",
        path=noarch_path,
    )
    linux_64_path = os.path.join(data_dir, "channels/with-wheels/linux-64/repodata.json")
    linux_64_data = SparseRepoData(
        channel=chn,
        subdir="linux-64",
        path=linux_64_path,
    )

    # Test: Version-conditional dependency - when side-dependency=0.2 is requested,
    # "package" should be included due to the conditional "package; if side-dependency=0.2"
    solved_data = await solve_with_sparse_repodata(
        [MatchSpec("starlette")],
        [noarch_data, linux_64_data],
        virtual_packages=[
            GenericVirtualPackage(PackageName("__unix"), Version("15"), "0"),
            GenericVirtualPackage(PackageName("__linux"), Version("0"), "0"),
        ],
        package_format_selection=PackageFormatSelection.PREFER_CONDA_WITH_WHL,
    )

    whl_files = sum(r.file_name.endswith(".whl") for r in solved_data)
    conda_files = sum(r.file_name.endswith(".conda") for r in solved_data)
    tar_bz2_files = sum(r.file_name.endswith(".tar.bz2") for r in solved_data)

    assert whl_files == 2
    assert conda_files == 21
    assert tar_bz2_files == 4

    assert isinstance(solved_data, list)
    package_names = [r.name.normalized for r in solved_data]
    # solve needs to include these two packages
    assert "starlette" in package_names
    assert "python" in package_names
