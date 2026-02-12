"""Tests for LockFile creation with LockPlatform combinations and round-tripping."""

import tempfile
from pathlib import Path


from rattler import (
    LockFile,
    LockPlatform,
    LockChannel,
    Platform,
    PackageRecord,
    RepoDataRecord,
)


# Path to test data relative to the repo root
TEST_DATA_DIR = Path(__file__).parent.parent.parent.parent / "test-data"


def _create_repo_data_record(index_json_path: Path, platform_subdir: str) -> RepoDataRecord:
    """Helper to create a RepoDataRecord from an index.json file."""
    package_record = PackageRecord.from_index_json(index_json_path)
    file_name = index_json_path.stem + ".conda"
    url = f"https://conda.anaconda.org/conda-forge/{platform_subdir}/{file_name}"
    channel = f"https://conda.anaconda.org/conda-forge/{platform_subdir}"
    return RepoDataRecord(package_record, file_name, url, channel)


class TestLockPlatformCreation:
    """Tests for creating LockPlatform with various combinations of data."""

    def test_platform_name_only(self) -> None:
        """Test creating a LockPlatform with just a name."""
        platform = LockPlatform("linux-64")
        assert platform.name == "linux-64"
        assert platform.subdir == Platform("linux-64")
        assert platform.virtual_packages == []

    def test_platform_with_explicit_subdir(self) -> None:
        """Test creating a LockPlatform with name and explicit subdir."""
        platform = LockPlatform("linux-64", subdir=Platform("linux-64"))
        assert platform.name == "linux-64"
        assert platform.subdir == Platform("linux-64")
        assert platform.virtual_packages == []

    def test_platform_with_virtual_packages(self) -> None:
        """Test creating a LockPlatform with name and virtual packages."""
        virtual_packages = ["__glibc=2.17", "__cuda=11.0"]
        platform = LockPlatform("linux-64", virtual_packages=virtual_packages)
        assert platform.name == "linux-64"
        assert platform.virtual_packages == virtual_packages

    def test_platform_with_all_fields(self) -> None:
        """Test creating a LockPlatform with all fields specified."""
        virtual_packages = ["__glibc=2.31"]
        platform = LockPlatform(
            "linux-64",
            subdir=Platform("linux-64"),
            virtual_packages=virtual_packages,
        )
        assert platform.name == "linux-64"
        assert platform.subdir == Platform("linux-64")
        assert platform.virtual_packages == virtual_packages

    def test_platform_with_empty_virtual_packages(self) -> None:
        """Test creating a LockPlatform with an empty virtual packages list."""
        platform = LockPlatform("linux-64", virtual_packages=[])
        assert platform.name == "linux-64"
        assert platform.virtual_packages == []

    def test_platform_with_many_virtual_packages(self) -> None:
        """Test creating a LockPlatform with many virtual packages."""
        virtual_packages = [
            "__glibc=2.31",
            "__cuda=11.0",
            "__linux=5.4",
            "python=3.11",
        ]
        platform = LockPlatform("linux-64", virtual_packages=virtual_packages)
        assert platform.virtual_packages == virtual_packages

    def test_platform_str_and_repr(self) -> None:
        """Test string representations of LockPlatform."""
        platform = LockPlatform("linux-64")
        assert str(platform) == "linux-64"
        assert "linux-64" in repr(platform)


class TestLockFileCreation:
    """Tests for creating LockFile with various platform combinations."""

    def test_lockfile_with_single_platform_name_only(self) -> None:
        """Test creating a LockFile with a single platform (name only)."""
        platform = LockPlatform("linux-64")
        lock_file = LockFile([platform])

        platforms = lock_file.platforms()
        assert len(platforms) == 1
        assert platforms[0].name == "linux-64"

    def test_lockfile_with_single_platform_explicit_subdir(self) -> None:
        """Test creating a LockFile with a single platform (explicit subdir)."""
        platform = LockPlatform("linux-64", subdir=Platform("linux-64"))
        lock_file = LockFile([platform])

        platforms = lock_file.platforms()
        assert len(platforms) == 1
        assert platforms[0].name == "linux-64"

    def test_lockfile_with_single_platform_virtual_packages(self) -> None:
        """Test creating a LockFile with a single platform (with virtual packages)."""
        virtual_packages = ["__glibc=2.17"]
        platform = LockPlatform("linux-64", virtual_packages=virtual_packages)
        lock_file = LockFile([platform])

        platforms = lock_file.platforms()
        assert len(platforms) == 1
        assert platforms[0].virtual_packages == virtual_packages

    def test_lockfile_with_single_platform_all_fields(self) -> None:
        """Test creating a LockFile with a single platform (all fields)."""
        virtual_packages = ["__glibc=2.31", "__cuda=11.0"]
        platform = LockPlatform(
            "linux-64",
            subdir=Platform("linux-64"),
            virtual_packages=virtual_packages,
        )
        lock_file = LockFile([platform])

        platforms = lock_file.platforms()
        assert len(platforms) == 1
        assert platforms[0].name == "linux-64"
        assert platforms[0].virtual_packages == virtual_packages

    def test_lockfile_with_multiple_platforms_all_combinations(self) -> None:
        """Test creating a LockFile with multiple platforms using all combinations."""
        # Platform with name only
        p1 = LockPlatform("linux-64")

        # Platform with explicit subdir
        p2 = LockPlatform("osx-arm64", subdir=Platform("osx-arm64"))

        # Platform with virtual packages
        p3 = LockPlatform("win-64", virtual_packages=["__win=10.0"])

        # Platform with all fields
        p4 = LockPlatform(
            "osx-64",
            subdir=Platform("osx-64"),
            virtual_packages=["__osx=10.15"],
        )

        lock_file = LockFile([p1, p2, p3, p4])
        platforms = lock_file.platforms()

        assert len(platforms) == 4

        # Find each platform by name and verify
        platform_dict = {p.name: p for p in platforms}

        assert "linux-64" in platform_dict
        assert platform_dict["linux-64"].virtual_packages == []

        assert "osx-arm64" in platform_dict

        assert "win-64" in platform_dict
        assert platform_dict["win-64"].virtual_packages == ["__win=10.0"]

        assert "osx-64" in platform_dict
        assert platform_dict["osx-64"].virtual_packages == ["__osx=10.15"]

    def test_lockfile_with_all_standard_platforms(self) -> None:
        """Test creating a LockFile with all standard platform names."""
        platform_names = [
            "linux-64",
            "linux-aarch64",
            "osx-64",
            "osx-arm64",
            "win-64",
        ]

        platforms = [LockPlatform(name) for name in platform_names]
        lock_file = LockFile(platforms)

        result_platforms = lock_file.platforms()
        assert len(result_platforms) == len(platform_names)

        result_names = {p.name for p in result_platforms}
        assert result_names == set(platform_names)


class TestLockFileRoundTrip:
    """Tests for round-tripping LockFile through serialization."""

    def _roundtrip(self, lock_file: LockFile) -> LockFile:
        """Helper to round-trip a lock file through file serialization."""
        with tempfile.NamedTemporaryFile(suffix=".lock", delete=False) as f:
            path = Path(f.name)

        try:
            lock_file.to_path(path)
            return LockFile.from_path(path)
        finally:
            path.unlink(missing_ok=True)

    def _assert_platforms_match(self, original: LockFile, parsed: LockFile) -> None:
        """Helper to verify platforms match between two lock files."""
        original_platforms = sorted(original.platforms(), key=lambda p: p.name)
        parsed_platforms = sorted(parsed.platforms(), key=lambda p: p.name)

        assert len(original_platforms) == len(parsed_platforms)

        for orig, pars in zip(original_platforms, parsed_platforms):
            assert orig.name == pars.name
            assert orig.subdir == pars.subdir
            assert orig.virtual_packages == pars.virtual_packages

    def test_roundtrip_platform_name_only(self) -> None:
        """Test round-tripping a LockFile with platform name only."""
        platform = LockPlatform("linux-64")
        lock_file = LockFile([platform])

        parsed = self._roundtrip(lock_file)
        self._assert_platforms_match(lock_file, parsed)

    def test_roundtrip_platform_with_explicit_subdir(self) -> None:
        """Test round-tripping a LockFile with explicit subdir."""
        platform = LockPlatform("linux-64", subdir=Platform("linux-64"))
        lock_file = LockFile([platform])

        parsed = self._roundtrip(lock_file)
        self._assert_platforms_match(lock_file, parsed)

    def test_roundtrip_platform_with_virtual_packages(self) -> None:
        """Test round-tripping a LockFile with virtual packages."""
        virtual_packages = ["__glibc=2.17", "__cuda=11.0"]
        platform = LockPlatform("linux-64", virtual_packages=virtual_packages)
        lock_file = LockFile([platform])

        parsed = self._roundtrip(lock_file)
        self._assert_platforms_match(lock_file, parsed)

        # Explicitly verify virtual packages
        parsed_platform = parsed.platforms()[0]
        assert parsed_platform.virtual_packages == virtual_packages

    def test_roundtrip_platform_with_all_fields(self) -> None:
        """Test round-tripping a LockFile with all fields."""
        virtual_packages = ["__glibc=2.31"]
        platform = LockPlatform(
            "linux-64",
            subdir=Platform("linux-64"),
            virtual_packages=virtual_packages,
        )
        lock_file = LockFile([platform])

        parsed = self._roundtrip(lock_file)
        self._assert_platforms_match(lock_file, parsed)

    def test_roundtrip_multiple_platforms_all_combinations(self) -> None:
        """Test round-tripping a LockFile with multiple platforms using all combinations."""
        # Platform with name only
        p1 = LockPlatform("linux-64")

        # Platform with explicit subdir
        p2 = LockPlatform("osx-arm64", subdir=Platform("osx-arm64"))

        # Platform with virtual packages
        p3 = LockPlatform("win-64", virtual_packages=["__win=10.0"])

        # Platform with all fields
        p4 = LockPlatform(
            "osx-64",
            subdir=Platform("osx-64"),
            virtual_packages=["__osx=10.15"],
        )

        lock_file = LockFile([p1, p2, p3, p4])

        parsed = self._roundtrip(lock_file)
        self._assert_platforms_match(lock_file, parsed)

    def test_roundtrip_with_empty_virtual_packages(self) -> None:
        """Test round-tripping a LockFile with empty virtual packages."""
        platform = LockPlatform("linux-64", virtual_packages=[])
        lock_file = LockFile([platform])

        parsed = self._roundtrip(lock_file)
        self._assert_platforms_match(lock_file, parsed)

    def test_roundtrip_with_many_virtual_packages(self) -> None:
        """Test round-tripping a LockFile with many virtual packages."""
        virtual_packages = [
            "__glibc=2.31",
            "__cuda=11.0",
            "__linux=5.4",
            "python=3.11",
        ]
        platform = LockPlatform("linux-64", virtual_packages=virtual_packages)
        lock_file = LockFile([platform])

        parsed = self._roundtrip(lock_file)
        self._assert_platforms_match(lock_file, parsed)

    def test_roundtrip_all_standard_platforms(self) -> None:
        """Test round-tripping a LockFile with all standard platforms."""
        platform_names = [
            "linux-64",
            "linux-aarch64",
            "osx-64",
            "osx-arm64",
            "win-64",
        ]

        platforms = [LockPlatform(name) for name in platform_names]
        lock_file = LockFile(platforms)

        parsed = self._roundtrip(lock_file)
        self._assert_platforms_match(lock_file, parsed)

        # Verify all platforms are present
        parsed_names = {p.name for p in parsed.platforms()}
        assert parsed_names == set(platform_names)

    def test_roundtrip_preserves_platform_order(self) -> None:
        """Test that round-tripping preserves platform data correctly."""
        platforms = [
            LockPlatform("linux-64", virtual_packages=["__glibc=2.17"]),
            LockPlatform("osx-arm64", virtual_packages=["__osx=11.0"]),
            LockPlatform("win-64", virtual_packages=["__win=10.0"]),
        ]
        lock_file = LockFile(platforms)

        parsed = self._roundtrip(lock_file)

        # Verify each platform's data is preserved
        parsed_dict = {p.name: p for p in parsed.platforms()}

        assert parsed_dict["linux-64"].virtual_packages == ["__glibc=2.17"]
        assert parsed_dict["osx-arm64"].virtual_packages == ["__osx=11.0"]
        assert parsed_dict["win-64"].virtual_packages == ["__win=10.0"]


class TestLockFileRoundTripWithPackages:
    """Tests for round-tripping LockFile with conda and pypi packages."""

    def _roundtrip(self, lock_file: LockFile) -> LockFile:
        """Helper to round-trip a lock file through file serialization."""
        with tempfile.NamedTemporaryFile(suffix=".lock", delete=False) as f:
            path = Path(f.name)

        try:
            lock_file.to_path(path)
            return LockFile.from_path(path)
        finally:
            path.unlink(missing_ok=True)

    def test_roundtrip_with_conda_packages_single_env(self) -> None:
        """Test round-tripping with conda packages in a single environment."""
        platform = LockPlatform("linux-64")
        lock_file = LockFile([platform])

        # Set up channel
        lock_file.set_channels("default", [LockChannel("https://conda.anaconda.org/conda-forge/")])

        # Add some conda packages
        tzdata_record = _create_repo_data_record(
            TEST_DATA_DIR / "conda-meta" / "tzdata-2024a-h0c530f3_0.json",
            "noarch",
        )
        lock_file.add_conda_package("default", platform, tzdata_record)

        # Round-trip
        parsed = self._roundtrip(lock_file)

        # Verify environment exists
        env = parsed.default_environment()
        assert env is not None

        # Verify packages
        platforms = env.platforms()
        assert len(platforms) == 1

        packages = env.packages(platforms[0])
        assert packages is not None
        assert len(packages) == 1
        assert packages[0].name == "tzdata"

    def test_roundtrip_with_conda_packages_multiple_packages(self) -> None:
        """Test round-tripping with multiple conda packages."""
        platform = LockPlatform("osx-arm64", virtual_packages=["__osx=11.0"])
        lock_file = LockFile([platform])

        lock_file.set_channels("default", [LockChannel("https://conda.anaconda.org/conda-forge/")])

        # Add multiple packages
        packages_to_add = [
            "tzdata-2024a-h0c530f3_0.json",
            "libzlib-1.2.13-h53f4e23_5.json",
            "libffi-3.4.2-h3422bc3_5.json",
        ]

        for pkg_file in packages_to_add:
            record = _create_repo_data_record(
                TEST_DATA_DIR / "conda-meta" / pkg_file,
                "osx-arm64",
            )
            lock_file.add_conda_package("default", platform, record)

        # Round-trip
        parsed = self._roundtrip(lock_file)

        env = parsed.default_environment()
        assert env is not None

        platforms = env.platforms()
        packages = env.packages(platforms[0])
        assert packages is not None
        assert len(packages) == 3

        package_names = {p.name for p in packages}
        assert package_names == {"tzdata", "libzlib", "libffi"}

    def test_roundtrip_with_pypi_packages(self) -> None:
        """Test round-tripping with pypi packages."""
        platform = LockPlatform("linux-64")
        lock_file = LockFile([platform])

        lock_file.set_channels("default", [LockChannel("https://conda.anaconda.org/conda-forge/")])

        # Add pypi packages
        lock_file.add_pypi_package(
            "default",
            platform,
            "requests",
            "2.31.0",
            "https://files.pythonhosted.org/packages/requests-2.31.0-py3-none-any.whl",
        )
        lock_file.add_pypi_package(
            "default",
            platform,
            "urllib3",
            "2.0.4",
            "https://files.pythonhosted.org/packages/urllib3-2.0.4-py3-none-any.whl",
        )

        # Round-trip
        parsed = self._roundtrip(lock_file)

        env = parsed.default_environment()
        assert env is not None

        # Check pypi packages
        pypi_packages = env.pypi_packages()
        assert "linux-64" in pypi_packages
        assert len(pypi_packages["linux-64"]) == 2

        pypi_names = {p.name for p in pypi_packages["linux-64"]}
        assert pypi_names == {"requests", "urllib3"}

    def test_roundtrip_with_mixed_conda_and_pypi_packages(self) -> None:
        """Test round-tripping with both conda and pypi packages."""
        platform = LockPlatform("linux-64", virtual_packages=["__glibc=2.17"])
        lock_file = LockFile([platform])

        lock_file.set_channels("default", [LockChannel("https://conda.anaconda.org/conda-forge/")])

        # Add conda packages
        tzdata_record = _create_repo_data_record(
            TEST_DATA_DIR / "conda-meta" / "tzdata-2024a-h0c530f3_0.json",
            "noarch",
        )
        lock_file.add_conda_package("default", platform, tzdata_record)

        libzlib_record = _create_repo_data_record(
            TEST_DATA_DIR / "conda-meta" / "libzlib-1.2.13-h53f4e23_5.json",
            "linux-64",
        )
        lock_file.add_conda_package("default", platform, libzlib_record)

        # Add pypi packages
        lock_file.add_pypi_package(
            "default",
            platform,
            "numpy",
            "1.26.0",
            "https://files.pythonhosted.org/packages/numpy-1.26.0-cp311-cp311-linux_x86_64.whl",
        )
        lock_file.add_pypi_package(
            "default",
            platform,
            "pandas",
            "2.1.0",
            "https://files.pythonhosted.org/packages/pandas-2.1.0-cp311-cp311-linux_x86_64.whl",
        )

        # Round-trip
        parsed = self._roundtrip(lock_file)

        env = parsed.default_environment()
        assert env is not None

        # Check conda packages
        platforms = env.platforms()
        conda_packages = env.packages(platforms[0])
        assert conda_packages is not None
        assert len(conda_packages) == 4  # 2 conda + 2 pypi (all shown in packages)

        # Check pypi packages specifically
        pypi_packages = env.pypi_packages()
        assert "linux-64" in pypi_packages
        assert len(pypi_packages["linux-64"]) == 2

        pypi_names = {p.name for p in pypi_packages["linux-64"]}
        assert pypi_names == {"numpy", "pandas"}

    def test_roundtrip_with_multiple_environments(self) -> None:
        """Test round-tripping with multiple environments."""
        linux_platform = LockPlatform("linux-64")
        osx_platform = LockPlatform("osx-arm64")
        lock_file = LockFile([linux_platform, osx_platform])

        # Set up "default" environment
        lock_file.set_channels("default", [LockChannel("https://conda.anaconda.org/conda-forge/")])
        tzdata_record = _create_repo_data_record(
            TEST_DATA_DIR / "conda-meta" / "tzdata-2024a-h0c530f3_0.json",
            "noarch",
        )
        lock_file.add_conda_package("default", linux_platform, tzdata_record)

        # Set up "dev" environment
        lock_file.set_channels("dev", [LockChannel("https://conda.anaconda.org/conda-forge/")])
        lock_file.add_conda_package("dev", osx_platform, tzdata_record)
        lock_file.add_pypi_package(
            "dev",
            osx_platform,
            "pytest",
            "7.4.0",
            "https://files.pythonhosted.org/packages/pytest-7.4.0-py3-none-any.whl",
        )

        # Round-trip
        parsed = self._roundtrip(lock_file)

        # Check environments
        envs = dict(parsed.environments())
        assert "default" in envs
        assert "dev" in envs

        # Check default environment
        default_env = envs["default"]
        default_platforms = default_env.platforms()
        assert len(default_platforms) == 1
        default_packages = default_env.packages(default_platforms[0])
        assert default_packages is not None
        assert len(default_packages) == 1
        assert default_packages[0].name == "tzdata"

        # Check dev environment
        dev_env = envs["dev"]
        dev_platforms = dev_env.platforms()
        assert len(dev_platforms) == 1

        dev_pypi = dev_env.pypi_packages()
        assert "osx-arm64" in dev_pypi
        assert len(dev_pypi["osx-arm64"]) == 1
        assert dev_pypi["osx-arm64"][0].name == "pytest"

    def test_roundtrip_with_multiple_platforms_and_packages(self) -> None:
        """Test round-tripping with packages on multiple platforms."""
        linux_platform = LockPlatform("linux-64", virtual_packages=["__glibc=2.17"])
        osx_platform = LockPlatform("osx-arm64", virtual_packages=["__osx=11.0"])
        win_platform = LockPlatform("win-64")

        lock_file = LockFile([linux_platform, osx_platform, win_platform])

        lock_file.set_channels("default", [LockChannel("https://conda.anaconda.org/conda-forge/")])

        # Add packages for each platform
        tzdata_record = _create_repo_data_record(
            TEST_DATA_DIR / "conda-meta" / "tzdata-2024a-h0c530f3_0.json",
            "noarch",
        )

        # Same package on all platforms
        lock_file.add_conda_package("default", linux_platform, tzdata_record)
        lock_file.add_conda_package("default", osx_platform, tzdata_record)
        lock_file.add_conda_package("default", win_platform, tzdata_record)

        # Different pypi packages per platform
        lock_file.add_pypi_package(
            "default",
            linux_platform,
            "torch",
            "2.0.0",
            "https://files.pythonhosted.org/packages/torch-2.0.0-cp311-linux_x86_64.whl",
        )
        lock_file.add_pypi_package(
            "default",
            osx_platform,
            "torch",
            "2.0.0",
            "https://files.pythonhosted.org/packages/torch-2.0.0-cp311-macosx_arm64.whl",
        )
        lock_file.add_pypi_package(
            "default",
            win_platform,
            "torch",
            "2.0.0",
            "https://files.pythonhosted.org/packages/torch-2.0.0-cp311-win_amd64.whl",
        )

        # Round-trip
        parsed = self._roundtrip(lock_file)

        env = parsed.default_environment()
        assert env is not None

        # Check all platforms are present
        platforms = env.platforms()
        assert len(platforms) == 3
        platform_names = {p.name for p in platforms}
        assert platform_names == {"linux-64", "osx-arm64", "win-64"}

        # Check virtual packages preserved
        platform_dict = {p.name: p for p in platforms}
        assert platform_dict["linux-64"].virtual_packages == ["__glibc=2.17"]
        assert platform_dict["osx-arm64"].virtual_packages == ["__osx=11.0"]
        assert platform_dict["win-64"].virtual_packages == []

        # Check pypi packages on each platform
        pypi_packages = env.pypi_packages()
        for platform_name in ["linux-64", "osx-arm64", "win-64"]:
            assert platform_name in pypi_packages
            assert len(pypi_packages[platform_name]) == 1
            assert pypi_packages[platform_name][0].name == "torch"

    def test_roundtrip_with_channels(self) -> None:
        """Test that channels are preserved through round-tripping."""
        platform = LockPlatform("linux-64")
        lock_file = LockFile([platform])

        # Set multiple channels
        channels = [
            LockChannel("https://conda.anaconda.org/conda-forge/"),
            LockChannel("https://conda.anaconda.org/pytorch/"),
        ]
        lock_file.set_channels("default", channels)

        # Add a package
        tzdata_record = _create_repo_data_record(
            TEST_DATA_DIR / "conda-meta" / "tzdata-2024a-h0c530f3_0.json",
            "noarch",
        )
        lock_file.add_conda_package("default", platform, tzdata_record)

        # Round-trip
        parsed = self._roundtrip(lock_file)

        env = parsed.default_environment()
        assert env is not None

        # Check channels
        parsed_channels = env.channels()
        assert len(parsed_channels) == 2
        channel_urls = [str(c) for c in parsed_channels]
        assert "https://conda.anaconda.org/conda-forge/" in channel_urls
        assert "https://conda.anaconda.org/pytorch/" in channel_urls
