from pathlib import Path

import pytest

from rattler import (
    ChannelCheck,
    Issuer,
    Publisher,
    RepoDataRecord,
    VerificationMode,
    VerificationPolicy,
    install,
    verify_attestation,
)
from rattler.exceptions import AttestationError, InstallerError


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
