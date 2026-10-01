from pathlib import Path

import pytest

from rattler import (
    ChannelCheck,
    Issuer,
    Publisher,
    RepoDataRecord,
    TrustedRoot,
    VerificationMode,
    VerificationPolicy,
    install,
    verify_attestation,
)
from rattler.exceptions import AttestationError, InstallerError

TRUSTED_ROOT_JSON = '{"mediaType": "application/vnd.dev.sigstore.trustedroot+json;version=0.1"}'


def package_path() -> Path:
    return Path(__file__).parents[3] / "test-data/packages/empty-0.1.0-h4616a5c_0.conda"


def test_verification_policy() -> None:
    publisher = Publisher(
        identity="https://github.com/conda/rattler/*",
        issuer=Issuer.github_actions(),
    )
    policy = VerificationPolicy.require(
        publisher,
        channel_check=ChannelCheck.WARN,
        max_sidecar_size=1024,
    )

    assert policy.is_enabled
    assert policy.is_required
    assert policy.mode == VerificationMode.REQUIRE
    assert policy.publisher == publisher
    assert policy.channel_check == ChannelCheck.WARN
    assert policy.max_sidecar_size == 1024
    assert Issuer.gitlab().url == "https://gitlab.com"


def test_trusted_root_from_json() -> None:
    assert repr(TrustedRoot.from_json(TRUSTED_ROOT_JSON)) == "TrustedRoot()"

    with pytest.raises(ValueError, match="invalid trusted root"):
        TrustedRoot.from_json("not a trusted root")


def test_trusted_root_from_path(tmp_path: Path) -> None:
    path = tmp_path / "trusted_root.json"
    path.write_text(TRUSTED_ROOT_JSON)

    assert repr(TrustedRoot.from_path(path)) == "TrustedRoot()"

    with pytest.raises(ValueError, match="could not read trusted root"):
        TrustedRoot.from_path(tmp_path / "missing.json")


def test_trusted_root_embedded() -> None:
    assert repr(TrustedRoot.embedded()) == "TrustedRoot()"


@pytest.mark.asyncio
async def test_verify_attestation_accepts_embedded_trusted_root() -> None:
    record = await RepoDataRecord.from_package_archive(package_path())
    outcome = await verify_attestation(
        record,
        VerificationPolicy.warn(),
        trusted_root=TrustedRoot.embedded(),
    )

    assert not outcome.is_verified


@pytest.mark.asyncio
async def test_verify_attestation_accepts_trusted_root() -> None:
    record = await RepoDataRecord.from_package_archive(package_path())
    outcome = await verify_attestation(
        record,
        VerificationPolicy.warn(),
        trusted_root=TrustedRoot.from_json(TRUSTED_ROOT_JSON),
    )

    assert not outcome.is_verified


@pytest.mark.asyncio
async def test_verify_attestation_warns_for_unadvertised_attestation() -> None:
    record = await RepoDataRecord.from_package_archive(package_path())
    outcome = await verify_attestation(record, VerificationPolicy.warn())

    assert not outcome.is_verified
    assert len(outcome.warnings) == 1
    assert "no attestations are advertised" in outcome.warnings[0]


@pytest.mark.asyncio
async def test_verify_attestation_require_rejects_unadvertised_attestation() -> None:
    record = await RepoDataRecord.from_package_archive(package_path())

    with pytest.raises(AttestationError, match="no attestations are advertised"):
        await verify_attestation(record, VerificationPolicy.require())


@pytest.mark.asyncio
async def test_install_accepts_attestation_policy(tmp_path: Path) -> None:
    record = await RepoDataRecord.from_package_archive(package_path())
    prefix = tmp_path / "prefix"

    with pytest.raises(InstallerError, match="attestation verification failed"):
        await install(
            [record],
            prefix,
            attestation_policy=VerificationPolicy.require(),
            show_progress=False,
        )

    assert not list((prefix / "conda-meta").glob("*.json"))
