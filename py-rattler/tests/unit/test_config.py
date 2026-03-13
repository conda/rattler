import os
from pathlib import Path

import pytest

from rattler import (
    Config,
    ConcurrencyConfig,
    ProxyConfig,
    RepodataConfig,
    RepodataChannelConfig,
    S3Options,
)


@pytest.fixture
def test_data_dir() -> str:
    return os.path.normpath(os.path.join(os.path.dirname(__file__), "../../../crates/rattler_config/test-data"))


@pytest.fixture
def config_toml(test_data_dir: str) -> Path:
    return Path(test_data_dir) / "config.toml"


def test_config_default() -> None:
    config = Config()
    assert config.default_channels is None
    assert config.authentication_override_file is None
    assert config.tls_no_verify is False
    assert config.mirrors == {}
    assert config.s3_options == {}
    assert config.loaded_from == []


def test_config_repr() -> None:
    config = Config()
    r = repr(config)
    assert "Config(" in r
    assert "tls_no_verify" in r


def test_config_load_from_file(config_toml: Path) -> None:
    config = Config.load_from_files(config_toml)

    assert config.default_channels == ["conda-forge"]
    assert config.authentication_override_file == "/path/to/your/override.json"
    assert config.tls_no_verify is False


def test_config_concurrency(config_toml: Path) -> None:
    config = Config.load_from_files(config_toml)
    concurrency = config.concurrency

    assert isinstance(concurrency, ConcurrencyConfig)
    assert concurrency.solves == 2
    assert concurrency.downloads == 5
    assert "ConcurrencyConfig(" in repr(concurrency)


def test_config_concurrency_default() -> None:
    config = Config()
    concurrency = config.concurrency

    assert concurrency.solves > 0
    assert concurrency.downloads == 50


def test_config_repodata(config_toml: Path) -> None:
    config = Config.load_from_files(config_toml)
    repodata = config.repodata_config

    assert isinstance(repodata, RepodataConfig)

    default = repodata.default_config
    assert isinstance(default, RepodataChannelConfig)
    assert default.disable_bzip2 is True
    assert default.disable_zstd is True
    assert default.disable_sharded is True

    per_channel = repodata.per_channel
    assert len(per_channel) == 1
    assert "https://prefix.dev/" in per_channel or "https://prefix.dev" in per_channel

    # Find the per-channel config regardless of trailing slash
    prefix_key = next(k for k in per_channel if "prefix.dev" in k)
    prefix_config = per_channel[prefix_key]
    assert prefix_config.disable_sharded is False


def test_config_s3_options(config_toml: Path) -> None:
    config = Config.load_from_files(config_toml)
    s3 = config.s3_options

    assert "my-bucket" in s3
    bucket = s3["my-bucket"]
    assert isinstance(bucket, S3Options)
    assert bucket.endpoint_url == "https://my-s3-compatible-host.com/"
    assert bucket.region == "us-east-1"
    assert bucket.force_path_style is True
    assert "S3Options(" in repr(bucket)


def test_config_mirrors(config_toml: Path) -> None:
    config = Config.load_from_files(config_toml)
    mirrors = config.mirrors

    assert len(mirrors) == 2
    # Check that conda-forge mirror is present
    forge_key = next(k for k in mirrors if "conda-forge" in k)
    forge_mirrors = mirrors[forge_key]
    assert len(forge_mirrors) == 1
    assert any("prefix.dev" in m for m in forge_mirrors)

    # Check that bioconda mirror is present
    bioconda_key = next(k for k in mirrors if "bioconda" in k)
    bioconda_mirrors = mirrors[bioconda_key]
    assert len(bioconda_mirrors) == 4


def test_config_proxy_default() -> None:
    config = Config()
    proxy = config.proxy_config

    assert isinstance(proxy, ProxyConfig)
    assert proxy.non_proxy_hosts == [] or isinstance(proxy.non_proxy_hosts, list)
    assert "ProxyConfig(" in repr(proxy)


def test_config_load_nonexistent_file() -> None:
    with pytest.raises(Exception):
        Config.load_from_files("/nonexistent/path/config.toml")


def test_config_load_multiple_files(config_toml: Path, tmp_path: Path) -> None:
    """Test that loading multiple config files merges them correctly."""
    override_config = tmp_path / "override.toml"
    override_config.write_text('tls-no-verify = true\ndefault-channels = ["bioconda"]\n')

    config = Config.load_from_files(config_toml, override_config)

    # Override should take priority
    assert config.tls_no_verify is True
    assert config.default_channels == ["bioconda"]

    # Values from the first file should still be present
    assert config.authentication_override_file == "/path/to/your/override.json"
