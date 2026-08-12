# type: ignore
import os
import json
import shutil
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterator

import boto3
import pytest

from rattler import Platform
from rattler.index import index_fs, index_s3
from rattler.index.index import S3Credentials


# ------------------------------------ FILESYSTEM ------------------------------------ #


@pytest.fixture
def package_directory(tmp_path, package_file_ruff: Path, package_file_pytweening: Path) -> Path:
    win_subdir = tmp_path / "win-64"
    noarch_subdir = tmp_path / "noarch"
    win_subdir.mkdir()
    noarch_subdir.mkdir()
    shutil.copy(package_file_ruff, win_subdir)
    shutil.copy(package_file_pytweening, noarch_subdir)
    return tmp_path


@pytest.mark.asyncio
async def test_index(package_directory):
    await index_fs(package_directory)

    # `.tmp` is the atomic-write staging dir created in the channel root; ignore it.
    assert {name for name in os.listdir(package_directory) if name != ".tmp"} == {"noarch", "win-64"}
    assert "repodata.json" in os.listdir(package_directory / "win-64")
    with open(package_directory / "win-64/repodata.json") as f:
        assert "ruff-0.0.171-py310h298983d_0" in f.read()
    assert "repodata.json" in os.listdir(package_directory / "noarch")
    with open(package_directory / "noarch/repodata.json") as f:
        assert "pytweening-1.0.4-pyhd8ed1ab_0" in f.read()


@pytest.mark.asyncio
async def test_index_specific_subdir_non_noarch(package_directory):
    await index_fs(package_directory, Platform("win-64"))

    assert "repodata.json" in os.listdir(package_directory / "win-64")
    with open(package_directory / "win-64/repodata.json") as f:
        assert "ruff-0.0.171-py310h298983d_0" in f.read()


@pytest.mark.asyncio
async def test_index_specific_subdir_noarch(package_directory):
    await index_fs(package_directory, Platform("noarch"))

    win_files = os.listdir(package_directory / "win-64")
    assert "repodata.json" not in win_files
    assert "ruff-0.0.171-py310h298983d_0.conda" in win_files
    assert "repodata.json" in os.listdir(package_directory / "noarch")
    with open(package_directory / "noarch/repodata.json") as f:
        assert "pytweening-1.0.4-pyhd8ed1ab_0" in f.read()


@pytest.mark.asyncio
async def test_index_repodata_revisions(package_directory):
    await index_fs(
        package_directory,
        Platform("noarch"),
        repodata_revisions=[{"revision": "v3", "message": "v3 packages"}],
        package_revision_assignment="latest",
        force=True,
    )

    with open(package_directory / "noarch/repodata.json") as f:
        repodata = json.load(f)

    assert "pytweening-1.0.4-pyhd8ed1ab_0" in repodata["v3"]["tar.bz2"]
    assert repodata["info"]["repodata_revisions"]["v3"]["n_packages"] == 1
    assert repodata["info"]["repodata_revisions"]["v3"]["message"] == "v3 packages"


@pytest.mark.asyncio
async def test_index_repodata_revisions_reject_legacy_selection(package_directory):
    with pytest.raises(ValueError, match="expected 'v3'"):
        await index_fs(
            package_directory,
            Platform("noarch"),
            repodata_revisions=["legacy"],
        )


@pytest.mark.asyncio
@pytest.mark.parametrize("obsolete_field", ["n_packages", "oldest", "newest"])
async def test_index_repodata_revisions_reject_obsolete_statistics(package_directory, obsolete_field):
    with pytest.raises(TypeError, match="no longer accepted.*derives package statistics"):
        await index_fs(
            package_directory,
            Platform("noarch"),
            repodata_revisions=[{"revision": "v3", obsolete_field: 1}],
        )


@pytest.mark.asyncio
async def test_index_repodata_revisions_reject_legacy_mapping(package_directory):
    with pytest.raises(TypeError, match="no longer accepts a vN-keyed metadata mapping"):
        await index_fs(
            package_directory,
            Platform("noarch"),
            repodata_revisions={"v3": {"n_packages": 1}},  # type: ignore[arg-type]
        )


# ---------------------------------------- S3 ---------------------------------------- #


@dataclass
class S3Config:
    access_key_id: str
    secret_access_key: str
    region: str = "auto"
    endpoint_url: str = "https://e1a7cde76f1780ec06bac859036dbaf7.r2.cloudflarestorage.com"
    bucket_name: str = "rattler-build-upload-test"
    channel_name: str = field(default_factory=lambda: f"channel{uuid.uuid4()}")


@pytest.fixture()
def s3_config() -> S3Config:
    access_key_id = os.environ.get("RATTLER_TEST_R2_READWRITE_ACCESS_KEY_ID")
    if not access_key_id:
        pytest.skip("RATTLER_TEST_R2_READWRITE_ACCESS_KEY_ID environment variable is not set")
    secret_access_key = os.environ.get("RATTLER_TEST_R2_READWRITE_SECRET_ACCESS_KEY")
    if not secret_access_key:
        pytest.skip("RATTLER_TEST_R2_READWRITE_SECRET_ACCESS_KEY environment variable is not set")
    return S3Config(
        access_key_id=access_key_id,
        secret_access_key=secret_access_key,
    )


@pytest.fixture()
def s3_client(s3_config: S3Config):
    return boto3.client(
        service_name="s3",
        endpoint_url=s3_config.endpoint_url,
        aws_access_key_id=s3_config.access_key_id,
        aws_secret_access_key=s3_config.secret_access_key,
        region_name=s3_config.region,
    )


@pytest.fixture()
def s3_channel(s3_config: S3Config, s3_client) -> Iterator[str]:
    channel_url = f"s3://{s3_config.bucket_name}/{s3_config.channel_name}"

    yield channel_url

    # Clean up the channel after the test
    objects_to_delete = s3_client.list_objects_v2(Bucket=s3_config.bucket_name, Prefix=f"{s3_config.channel_name}/")
    delete_keys = [{"Key": obj["Key"]} for obj in objects_to_delete.get("Contents", [])]
    if delete_keys:
        result = s3_client.delete_objects(Bucket=s3_config.bucket_name, Delete={"Objects": delete_keys})
        assert result["ResponseMetadata"]["HTTPStatusCode"] == 200


@pytest.mark.asyncio
async def test_index_s3(
    package_directory,
    s3_config: S3Config,
    s3_channel: str,
    s3_client,
):
    # Upload package to channel
    filepath = package_directory / "noarch" / "pytweening-1.0.4-pyhd8ed1ab_0.tar.bz2"
    s3_client.upload_file(
        Filename=str(filepath),
        Bucket=s3_config.bucket_name,
        Key=f"{s3_config.channel_name}/noarch/pytweening-1.0.4-pyhd8ed1ab_0.tar.bz2",
    )

    # Run index command
    await index_s3(
        channel_url=s3_channel,
        credentials=S3Credentials(
            region=s3_config.region,
            endpoint_url=s3_config.endpoint_url,
            access_key_id=s3_config.access_key_id,
            secret_access_key=s3_config.secret_access_key,
            session_token=s3_config.session_token,
            addressing_style="path",
        ),
        repodata_patch=None,
        force=True,
    )

    # Check if repodata.json was created
    repodata_json = f"{s3_config.channel_name}/noarch/repodata.json"
    result = s3_client.head_object(
        Bucket=s3_config.bucket_name,
        Key=repodata_json,
    )
    assert result["ResponseMetadata"]["HTTPStatusCode"] == 200
