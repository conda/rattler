from pathlib import Path

import pytest

from rattler import (
    ArchiveIdentifier,
    CondaArchiveIdentifier,
    CondaArchiveType,
)


def test_archive_identifier_construct_and_properties() -> None:
    ident = ArchiveIdentifier("python", "3.11.0", "h12345_0")
    assert ident.name == "python"
    assert ident.version == "3.11.0"
    assert ident.build_string == "h12345_0"
    assert str(ident) == "python-3.11.0-h12345_0"
    assert repr(ident) == "ArchiveIdentifier(name='python', version='3.11.0', build_string='h12345_0')"

    ident.name = "pypy"
    assert ident.name == "pypy"
    ident.version = "7.3.11"
    assert ident.version == "7.3.11"
    ident.build_string = "py39_0"
    assert ident.build_string == "py39_0"
    assert str(ident) == "pypy-7.3.11-py39_0"

    # From string
    parsed = ArchiveIdentifier.from_str("numpy-1.24.0-py310_0")
    assert parsed.name == "numpy"
    assert parsed.version == "1.24.0"
    assert parsed.build_string == "py310_0"
    assert parsed == ArchiveIdentifier("numpy", "1.24.0", "py310_0")
    assert hash(parsed) == hash(ArchiveIdentifier("numpy", "1.24.0", "py310_0"))

    # Also supports passing single string to __init__
    from_init = ArchiveIdentifier("numpy-1.24.0-py310_0")
    assert from_init == parsed

    with pytest.raises(ValueError):
        ArchiveIdentifier.from_str("invalid_identifier")

    with pytest.raises(ValueError):
        ArchiveIdentifier("only_name", "only_version")


def test_conda_archive_type() -> None:
    conda = CondaArchiveType.conda()
    tar_bz2 = CondaArchiveType.tar_bz2()

    assert conda.is_conda is True
    assert conda.is_tar_bz2 is False
    assert conda.extension == ".conda"
    assert str(conda) == ".conda"
    assert repr(conda) == "CondaArchiveType.Conda"

    assert tar_bz2.is_conda is False
    assert tar_bz2.is_tar_bz2 is True
    assert tar_bz2.extension == ".tar.bz2"
    assert str(tar_bz2) == ".tar.bz2"
    assert repr(tar_bz2) == "CondaArchiveType.TarBz2"

    assert conda == CondaArchiveType("conda")
    assert conda == CondaArchiveType(".conda")
    assert tar_bz2 == CondaArchiveType("tar.bz2")
    assert tar_bz2 == CondaArchiveType(".tar.bz2")
    assert conda != tar_bz2
    assert hash(conda) != hash(tar_bz2)

    with pytest.raises(ValueError):
        CondaArchiveType("invalid")

    # split_str
    res = CondaArchiveType.split_str("my-package-1.0.0-0.conda")
    assert res is not None
    stem, archive_type = res
    assert stem == "my-package-1.0.0-0"
    assert archive_type == conda

    res_bz2 = CondaArchiveType.split_str("my-package-1.0.0-0.tar.bz2")
    assert res_bz2 is not None
    stem_bz2, archive_type_bz2 = res_bz2
    assert stem_bz2 == "my-package-1.0.0-0"
    assert archive_type_bz2 == tar_bz2

    assert CondaArchiveType.split_str("my-package-1.0.0-0.whl") is None

    # try_from_path
    assert CondaArchiveType.try_from_path(Path("dir/my-package-1.0.0-0.conda")) == conda
    assert CondaArchiveType.try_from_path("dir/my-package-1.0.0-0.tar.bz2") == tar_bz2
    assert CondaArchiveType.try_from_path("dir/my-package-1.0.0-0.whl") is None

    # try_from_magic_bytes
    assert CondaArchiveType.try_from_magic_bytes(b"\x50\x4b\x03\x04") == conda
    assert CondaArchiveType.try_from_magic_bytes(b"\x42\x5a\x68\x39") == tar_bz2
    assert CondaArchiveType.try_from_magic_bytes(b"\x00\x00\x00\x00") is None


def test_conda_archive_identifier() -> None:
    ident = ArchiveIdentifier("clangdev", "9.0.1", "cling_v0.9_hd1e6b3a_3")
    cai = CondaArchiveIdentifier(ident, CondaArchiveType.conda())

    assert cai.name == "clangdev"
    assert cai.version == "9.0.1"
    assert cai.build_string == "cling_v0.9_hd1e6b3a_3"
    assert cai.identifier == ident
    assert cai.archive_type.is_conda is True
    assert cai.to_file_name() == "clangdev-9.0.1-cling_v0.9_hd1e6b3a_3.conda"
    assert str(cai) == "clangdev-9.0.1-cling_v0.9_hd1e6b3a_3.conda"
    assert repr(cai) == f"CondaArchiveIdentifier(identifier={ident!r}, archive_type={CondaArchiveType.conda()!r})"

    cai.identifier = ArchiveIdentifier("llvm", "10.0.0", "0")
    assert cai.name == "llvm"
    cai.archive_type = CondaArchiveType.tar_bz2()
    assert cai.archive_type.is_tar_bz2 is True
    assert cai.to_file_name() == "llvm-10.0.0-0.tar.bz2"

    # try_from_filename
    from_file = CondaArchiveIdentifier.try_from_filename("ros-noetic-rosbridge-suite-0.11.14-py39h6fdeb60_14.tar.bz2")
    assert from_file is not None
    assert from_file.name == "ros-noetic-rosbridge-suite"
    assert from_file.version == "0.11.14"
    assert from_file.build_string == "py39h6fdeb60_14"
    assert from_file.archive_type.is_tar_bz2 is True

    # Wheel packages should return None
    assert CondaArchiveIdentifier.try_from_filename("numpy-1.24.0-cp39-cp39-linux_x86_64.whl") is None

    # from_str
    parsed = CondaArchiveIdentifier.from_str("python-3.11.0-0.conda")
    assert parsed.name == "python"
    assert parsed.version == "3.11.0"
    assert parsed.build_string == "0"
    assert parsed.archive_type.is_conda is True

    with pytest.raises(ValueError):
        CondaArchiveIdentifier.from_str("invalid_filename")

    # try_from_path
    from_path = CondaArchiveIdentifier.try_from_path(Path("channels/conda-forge/linux-64/scipy-1.10.0-py310_0.conda"))
    assert from_path is not None
    assert from_path.name == "scipy"

    # try_from_url
    from_url = CondaArchiveIdentifier.try_from_url(
        "https://conda.anaconda.org/conda-forge/linux-64/scipy-1.10.0-py310_0.conda"
    )
    assert from_url is not None
    assert from_url.name == "scipy"
    assert from_url.archive_type.is_conda is True

    # Invalid URL returns None
    assert CondaArchiveIdentifier.try_from_url("not_a_valid_url") is None
