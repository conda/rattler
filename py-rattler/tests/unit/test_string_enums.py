import json

from rattler.config import Config, RunPostLinkScripts, TlsRootCerts
from rattler.index import RepodataRevisionSelection, RepodataRevisionWithMessage
from rattler.index.index import _repodata_revisions_to_dicts
from rattler.networking import CacheAction, FetchRepoDataOptions, Variant
from rattler.package import FileMode, FileModeName, NoArchKind, NoArchType, PathType, PathTypeName
from rattler.platform import Arch, ArchName, Platform, PlatformLiteral, PlatformName
from rattler.prefix import PrefixPathType, PrefixPathTypeName


def test_enum_string_compatibility() -> None:
    value = CacheAction.NO_CACHE
    assert str(value) == "no-cache"
    assert f"{value:>10}" == "  no-cache"
    assert json.dumps(value) == '"no-cache"'
    mapping: dict[str, int] = {value: 1}
    assert mapping["no-cache"] == 1
    assert CacheAction("no-cache") is value
    assert PlatformLiteral is PlatformName


def test_enum_inputs_cross_native_boundary() -> None:
    assert str(Platform(PlatformName.LINUX_64)) == "linux-64"
    assert str(Arch(ArchName.X86_64)) == "x86_64"
    assert PathType(PathTypeName.HARDLINK).hardlink
    assert PrefixPathType(PrefixPathTypeName.PYC_FILE).pyc_file
    assert NoArchType(NoArchKind.PYTHON).python
    assert NoArchType(True).generic
    assert NoArchType(None).none
    assert FileMode(FileModeName.TEXT).mode is FileModeName.TEXT
    assert FileMode(FileModeName.BINARY).mode is FileModeName.BINARY
    FetchRepoDataOptions(cache_action=CacheAction.NO_CACHE, variant=Variant.CURRENT)._into_py()


def test_config_returns_enum_members() -> None:
    config = Config.from_toml('tls-root-certs = "webpki"\nrun-post-link-scripts = "false"')
    assert config.tls_root_certs is TlsRootCerts.WEBPKI
    assert config.run_post_link_scripts is RunPostLinkScripts.FALSE
    assert Config().tls_root_certs is None
    assert Config().run_post_link_scripts is None


def test_revision_enum_wire_conversion() -> None:
    revision: RepodataRevisionWithMessage = {"revision": RepodataRevisionSelection.V3, "message": "test"}
    assert _repodata_revisions_to_dicts([RepodataRevisionSelection.V3, revision]) == [
        {"revision": 3},
        {"revision": 3, "message": "test"},
    ]
