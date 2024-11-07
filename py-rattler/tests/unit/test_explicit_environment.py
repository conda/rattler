import pytest
from pathlib import Path
from rattler.explicit_environment import ExplicitEnvironmentSpec
from rattler.platform import Platform

test_env = """# This file may be used to create an environment using:
# $ conda create --name <env> --file <this file>
# platform: linux-64
@EXPLICIT
https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2#d7c89558ba9fa0495403155b64376d81
https://conda.anaconda.org/conda-forge/linux-64/libstdcxx-ng-9.3.0-h2ae2ef3_17.tar.bz2#342f3c931d0a3a209ab09a522469d20c
https://conda.anaconda.org/conda-forge/linux-64/libgomp-9.3.0-h5dbcf3e_17.tar.bz2#8fd587013b9da8b52050268d50c12305
https://conda.anaconda.org/conda-forge/linux-64/_openmp_mutex-4.5-1_gnu.tar.bz2#561e277319a41d4f24f5c05a9ef63c04
https://conda.anaconda.org/conda-forge/linux-64/libgcc-ng-9.3.0-h5dbcf3e_17.tar.bz2#fc9f5adabc4d55cd4b491332adc413e0
https://conda.anaconda.org/conda-forge/linux-64/xtl-0.6.21-h0efe328_0.tar.bz2#9eee90b98fd394db7a049792e67e1659
https://conda.anaconda.org/conda-forge/linux-64/xtensor-0.21.8-hc9558a2_0.tar.bz2#1030174db5c183f3afb4181a0a02873d
"""


def test_parse_explicit_environment_from_str() -> None:
    spec = ExplicitEnvironmentSpec.from_str(test_env)

    assert spec.platform is not None
    assert spec.platform == Platform("linux-64")
    assert len(spec.packages) == 7

    assert (
        spec.packages[0].url
        == "https://conda.anaconda.org/conda-forge/linux-64/_libgcc_mutex-0.1-conda_forge.tar.bz2#d7c89558ba9fa0495403155b64376d81"
    )
    assert (
        spec.packages[1].url
        == "https://conda.anaconda.org/conda-forge/linux-64/libstdcxx-ng-9.3.0-h2ae2ef3_17.tar.bz2#342f3c931d0a3a209ab09a522469d20c"
    )


def test_parse_explicit_environment_no_platform() -> None:
    content = """@EXPLICIT\nhttp://repo.anaconda.com/pkgs/main/linux-64/python-3.9.0-h1234.tar.bz2"""
    spec = ExplicitEnvironmentSpec.from_str(content)

    assert spec.platform is None
    assert len(spec.packages) == 1
    assert spec.packages[0].url == "http://repo.anaconda.com/pkgs/main/linux-64/python-3.9.0-h1234.tar.bz2"


def test_parse_explicit_environment_from_file(tmp_path: Path) -> None:
    content = """# platform: win-64
@EXPLICIT
http://repo.anaconda.com/pkgs/main/win-64/python-3.9.0-h1234.tar.bz2"""

    env_file = tmp_path / "env.txt"
    env_file.write_text(content)

    spec = ExplicitEnvironmentSpec.from_path(env_file)
    assert spec.platform is not None
    assert spec.platform == Platform("win-64")
    assert len(spec.packages) == 1


def test_parse_invalid_explicit_environment() -> None:
    with pytest.raises(Exception):
        ExplicitEnvironmentSpec.from_str("invalid content # platform: invalid-platform")
