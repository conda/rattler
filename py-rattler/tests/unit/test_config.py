# type: ignore
from pathlib import Path

import pytest

from rattler import Client, Config, Gateway
from rattler.exceptions import AuthenticationStorageError, ConfigError


def test_default_config_is_empty():
    config = Config()

    assert config.default_channels is None
    assert config.tls_no_verify is None
    assert config.tls_root_certs is None
    assert config.mirrors == {}
    assert config.loaded_from == []
    assert config.concurrency_downloads == 50
    assert config.concurrency_solves >= 1


def test_config_constructs_networking_consumers():
    config = Config.from_toml("""
        tls-no-verify = true

        [concurrency]
        downloads = 8

        [repodata-config]
        disable-sharded = true
    """)

    assert isinstance(Client.from_config(config), Client)
    assert isinstance(Gateway.from_config(config), Gateway)


def test_configured_authentication_override_is_used(tmp_path):
    auth_file = tmp_path / "auth.json"
    auth_file.write_text("not json")
    config = Config()
    config.set("authentication-override-file", str(auth_file))

    with pytest.raises(AuthenticationStorageError):
        Client.from_config(config)


def test_from_toml_reads_common_keys():
    config = Config.from_toml("""
        default-channels = ["conda-forge", "https://repo.prefix.dev/my-channel"]
        authentication-override-file = "/tmp/auth.json"
        tls-no-verify = true
        tls-root-certs = "webpki"
        run-post-link-scripts = "insecure"
        allow-symbolic-links = false
        allow-hard-links = true
        allow-ref-links = false

        [concurrency]
        solves = 2
        downloads = 8

        [proxy-config]
        https = "https://proxy.example.com/"
        non-proxy-hosts = ["localhost"]
    """)

    assert config.default_channels == ["conda-forge", "https://repo.prefix.dev/my-channel"]
    assert config.authentication_override_file == Path("/tmp/auth.json")
    assert config.tls_no_verify is True
    assert config.tls_root_certs == "webpki"
    assert config.run_post_link_scripts == "insecure"
    assert config.allow_symbolic_links is False
    assert config.allow_hard_links is True
    assert config.allow_ref_links is False
    assert config.concurrency_solves == 2
    assert config.concurrency_downloads == 8
    assert config.proxy_https == "https://proxy.example.com/"
    assert config.proxy_http is None
    assert config.proxy_non_proxy_hosts == ["localhost"]


def test_snake_case_aliases_are_accepted():
    config = Config.from_toml("""
        default_channels = ["conda-forge"]
        tls_no_verify = true
    """)

    assert config.default_channels == ["conda-forge"]
    assert config.tls_no_verify is True


def test_legacy_tls_root_certs_spellings_map_to_system():
    for value in ("system", "native", "all"):
        assert Config.from_toml(f'tls-root-certs = "{value}"').tls_root_certs == "system"


def test_unknown_keys_are_reported():
    config, unused = Config.from_toml_with_unused_keys("""
        tls-no-verify = true
        not-a-real-key = 1
    """)

    assert config.tls_no_verify is True
    assert unused == ["not-a-real-key"]


def test_extension_keys_are_reported_as_unused():
    # `ConfigBase<NoExtension>` has no tool-specific keys, so a pixi-only key
    # like `detached-environments` cannot be understood here.
    _config, unused = Config.from_toml_with_unused_keys("detached-environments = true")

    assert unused == ["detached-environments"]


def test_shared_parsing_rejects_nothing_extra_for_common_keys():
    config, unused = Config.from_toml_with_unused_keys("tls-no-verify = true", shared=True)

    assert config.tls_no_verify is True
    assert unused == []


def test_invalid_toml_raises():
    with pytest.raises(ValueError):
        Config.from_toml("this is not toml")


def test_nested_sections_are_dicts():
    config = Config.from_toml("""
        [repodata-config]
        disable-zstd = true

        [repodata-config."https://conda.anaconda.org/conda-forge"]
        disable-sharded = true

        [s3-options.my-bucket]
        endpoint-url = "https://my-s3.example.com/"
        region = "eu-central-1"
        force-path-style = true
    """)

    assert config.repodata_config["disable-zstd"] is True
    assert config.repodata_config["https://conda.anaconda.org/conda-forge"]["disable-sharded"] is True
    assert config.s3_options["my-bucket"] == {
        "endpoint-url": "https://my-s3.example.com/",
        "region": "eu-central-1",
        "force-path-style": True,
    }


def test_index_config_resolves_longest_prefix():
    config = Config.from_toml("""
        [index-config]
        write-zst = true
        write-shards = true

        [index-config."s3://my-bucket"]
        base-url = "../packages/"

        [index-config."s3://my-bucket/staging"]
        write-shards = false
    """)

    staging = config.resolve_index_config("s3://my-bucket/staging")
    assert staging["write-zst"] is True
    assert staging["write-shards"] is False
    assert staging["base-url"] == "../packages/"

    other = config.resolve_index_config("s3://other-bucket")
    assert other["write-shards"] is True
    assert "base-url" not in other


def test_mirrors_round_trip():
    config = Config.from_toml("""
        [mirrors]
        "https://conda.anaconda.org/conda-forge" = [
            "https://repo.prefix.dev/conda-forge",
            "https://my-mirror.example.com/conda-forge",
        ]
    """)

    assert config.mirrors == {
        "https://conda.anaconda.org/conda-forge": [
            "https://repo.prefix.dev/conda-forge",
            "https://my-mirror.example.com/conda-forge",
        ]
    }


def test_build_package_format():
    assert (
        Config.from_toml("""
            [build]
            package-format = "conda:max"
        """).build_package_format
        == "conda:max"
    )
    assert Config().build_package_format is None


def test_channel_config_is_the_default():
    # `ChannelConfig` exposes its fields only through `repr`.
    assert 'channel_alias="https://conda.anaconda.org/"' in repr(Config().channel_config)


def test_merge_gives_other_precedence():
    low = Config.from_toml("""
        default-channels = ["conda-forge"]
        tls-no-verify = false

        [concurrency]
        downloads = 4
    """)
    high = Config.from_toml("""
        tls-no-verify = true

        [concurrency]
        downloads = 12
    """)

    merged = low.merge(high)

    assert merged.tls_no_verify is True
    assert merged.concurrency_downloads == 12
    # Keys absent from `high` are kept from `low`.
    assert merged.default_channels == ["conda-forge"]
    # The originals are untouched.
    assert low.tls_no_verify is False
    assert high.default_channels is None


def test_set_and_unset():
    config = Config()

    config.set("concurrency.solves", "3")
    assert config.concurrency_solves == 3

    config.set("default-channels", '["conda-forge", "bioconda"]')
    assert config.default_channels == ["conda-forge", "bioconda"]

    config.set("tls-no-verify", "true")
    assert config.tls_no_verify is True

    config.unset("tls-no-verify")
    assert config.tls_no_verify is None


def test_set_quoted_key_with_dots():
    config = Config()

    config.set('mirrors."https://conda.anaconda.org/conda-forge"', '["https://repo.prefix.dev/conda-forge"]')

    assert config.mirrors == {"https://conda.anaconda.org/conda-forge": ["https://repo.prefix.dev/conda-forge"]}


def test_set_unknown_key_raises():
    with pytest.raises(ValueError, match="Unknown configuration key"):
        Config().set("not-a-real-key", "1")


def test_unset_unknown_key_raises():
    with pytest.raises(ValueError, match="Unknown configuration key"):
        Config().unset("not-a-real-key")


def test_keys_include_nested_paths():
    keys = Config().keys()

    assert "tls-no-verify" in keys
    assert "concurrency.solves" in keys
    assert "concurrency.downloads" in keys
    assert "proxy-config.https" in keys
    assert "build.package-format" in keys


def test_validate_rejects_zero_concurrency():
    config = Config.from_toml("""
        [concurrency]
        solves = 0
    """)

    with pytest.raises(ValueError, match="must be greater than 0"):
        config.validate()


def test_load_from_files_merges_in_order(tmp_path):
    low = tmp_path / "low.toml"
    low.write_text("""
        default-channels = ["conda-forge"]
        tls-no-verify = false
    """)
    high = tmp_path / "high.toml"
    high.write_text("tls-no-verify = true")

    config = Config.load_from_files([low, high])

    assert config.tls_no_verify is True
    assert config.default_channels == ["conda-forge"]
    assert config.loaded_from == [low, high]


def test_load_from_files_missing_file_raises(tmp_path):
    with pytest.raises(ConfigError):
        Config.load_from_files([tmp_path / "does-not-exist.toml"])


def test_load_from_files_validates(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text("""
        [concurrency]
        downloads = 0
    """)

    with pytest.raises(ConfigError, match="must be greater than 0"):
        Config.load_from_files([path])


def test_load_from_locations_shared_layer_ignores_unshared_keys(tmp_path):
    shared = tmp_path / "shared.toml"
    shared.write_text("""
        tls-no-verify = true
        detached-environments = true
    """)

    config = Config.load_from_locations([(shared, True)])

    assert config.tls_no_verify is True
    assert config.loaded_from == [shared]


def test_config_search_paths_order_and_layers():
    locations = Config.config_search_paths("rattler-build")

    assert locations
    assert all(isinstance(path, Path) for path, _ in locations)
    # The shared layer contributes the lowest-precedence entry.
    assert locations[0][1] is True
    assert locations[0][0].parent.name == "rattler"
    # The tool's own files come last and take precedence.
    assert locations[-1][1] is False
    assert "rattler-build" in str(locations[-1][0])


def test_load_from_default_locations_tolerates_missing_files():
    # No files are guaranteed to exist, so this must not raise.
    config = Config.load_from_default_locations("a-tool-that-does-not-exist")

    assert config.loaded_from == []


def test_to_toml_omits_defaults():
    toml = Config.from_toml("tls-no-verify = true").to_toml()

    assert toml.strip() == "tls-no-verify = true"


def test_save_round_trips(tmp_path):
    config = Config.from_toml("""
        default-channels = ["conda-forge"]
        tls-no-verify = true

        [concurrency]
        downloads = 7
    """)
    path = tmp_path / "nested" / "config.toml"

    config.save(path)
    reloaded = Config.load_from_files([path])

    assert reloaded.default_channels == ["conda-forge"]
    assert reloaded.tls_no_verify is True
    assert reloaded.concurrency_downloads == 7


def test_repr_mentions_loaded_from(tmp_path):
    path = tmp_path / "config.toml"
    path.write_text("tls-no-verify = true")

    assert repr(Config()) == "Config(loaded_from=[])"
    assert str(path) in repr(Config.load_from_files([path]))
