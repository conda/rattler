from pathlib import Path

import pytest

from rattler import RepoDataRecord
from rattler.exceptions import ExtractError


@pytest.mark.asyncio
async def test_from_package_archive_conda(test_data_dir: str) -> None:
    path = Path(test_data_dir) / "clobber" / "clobber-fd-1-0.1.0-h4616a5c_0.conda"

    record = await RepoDataRecord.from_package_archive(path)

    assert isinstance(record, RepoDataRecord)
    assert record.name.normalized == "clobber-fd-1"
    assert record.channel is None
    assert record.url == path.resolve().as_uri()
    assert record.sha256 is not None
    assert record.md5 is None
    assert record.size is not None


@pytest.mark.asyncio
async def test_from_package_archive_tar_bz2(test_data_dir: str) -> None:
    path = Path(test_data_dir) / "clobber" / "clobber-1-0.1.0-h4616a5c_0.tar.bz2"

    record = await RepoDataRecord.from_package_archive(path)

    assert isinstance(record, RepoDataRecord)
    assert record.name.normalized == "clobber-1"
    assert record.channel is None
    assert record.url == path.resolve().as_uri()


@pytest.mark.asyncio
async def test_from_package_archive_renamed_filename(test_data_dir: str, tmp_path: Path) -> None:
    """A local package file doesn't need to follow the `name-version-build.ext`
    filename convention; the identifier should still be derived from the
    archive's own `index.json` metadata."""
    original_path = Path(test_data_dir) / "clobber" / "clobber-fd-1-0.1.0-h4616a5c_0.conda"
    renamed_path = tmp_path / "my-renamed-package.conda"
    renamed_path.write_bytes(original_path.read_bytes())

    record = await RepoDataRecord.from_package_archive(renamed_path)

    assert record.name.normalized == "clobber-fd-1"
    assert record.channel is None


@pytest.mark.asyncio
async def test_from_package_archive_invalid_path(test_data_dir: str) -> None:
    with pytest.raises(ExtractError):
        await RepoDataRecord.from_package_archive(Path(test_data_dir) / "clobber" / "does-not-exist.conda")
