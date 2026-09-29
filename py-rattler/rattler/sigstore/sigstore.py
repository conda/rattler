from __future__ import annotations

import os
from dataclasses import dataclass
from enum import Enum
from urllib.parse import urlsplit

from rattler.networking import Client
from rattler.rattler import (
    PyCertificateClaims,
    PyTrustedRoot,
    PyVerificationOutcome,
    PyVerificationPolicy,
    PyVerifiedChecks,
    py_verify_attestation,
)
from rattler.repo_data import RepoDataRecord

DEFAULT_MAX_SIDECAR_SIZE = 4 * 1024 * 1024


class ChannelCheck(str, Enum):
    """How an attestation's target channel is compared with the package channel."""

    REQUIRE = "require"
    WARN = "warn"
    IGNORE = "ignore"


class VerificationMode(str, Enum):
    """How verification failures affect the operation."""

    DISABLED = "disabled"
    WARN = "warn"
    REQUIRE = "require"


@dataclass(frozen=True)
class Issuer:
    """An OIDC issuer accepted for a signing certificate."""

    url: str

    def __post_init__(self) -> None:
        parsed = urlsplit(self.url)
        if parsed.scheme not in {"http", "https"} or not parsed.netloc:
            raise ValueError("issuer must be an absolute HTTP(S) URL")

    @classmethod
    def github_actions(cls) -> Issuer:
        """Return the GitHub Actions OIDC issuer."""
        return cls("https://token.actions.githubusercontent.com")

    @classmethod
    def gitlab(cls) -> Issuer:
        """Return the GitLab CI OIDC issuer."""
        return cls("https://gitlab.com")


@dataclass(frozen=True)
class Publisher:
    """Signing certificate constraints applied to every verified package."""

    identity: str | None = None
    issuer: Issuer | None = None


class TrustedRoot:
    """The Sigstore trust anchors every attestation bundle is verified against.

    Verification normally loads the production trusted root over TUF, which
    needs network access. Supplying one of these instead makes verification use
    the given trust material, so it can run against a pinned
    ``trusted_root.json`` or against the snapshot embedded in py-rattler.
    """

    def __init__(self, inner: PyTrustedRoot) -> None:
        self._inner = inner

    @classmethod
    def from_json(cls, json: str) -> TrustedRoot:
        """Parse a trusted root from the contents of a ``trusted_root.json``.

        Raises:
            ValueError: If `json` is not a valid Sigstore trusted root.

        Examples
        --------
        ```python
        >>> root = TrustedRoot.from_json('{"mediaType": "application/vnd.dev.sigstore.trustedroot+json;version=0.1"}')
        >>> root
        TrustedRoot()
        >>>
        ```
        """
        return cls(PyTrustedRoot.from_json(json))

    @classmethod
    def from_path(cls, path: os.PathLike[str] | str) -> TrustedRoot:
        """Read a trusted root from a ``trusted_root.json`` file.

        Raises:
            ValueError: If `path` cannot be read or does not hold a valid
                Sigstore trusted root.
        """
        return cls(PyTrustedRoot.from_path(os.fspath(path)))

    @classmethod
    def embedded(cls) -> TrustedRoot:
        """Return the trust anchors of the public good instance that ship with py-rattler.

        This is the root to reach for when `tuf-repo-cdn.sigstore.dev` cannot be
        reached, not a general way to avoid the network. It is a snapshot taken
        when py-rattler's Sigstore dependencies were released, so unlike the
        root loaded over TUF it does not pick up key rotations or revocations
        and ages with the installed version of py-rattler. Prefer
        [`TrustedRoot.from_path`][rattler.sigstore.TrustedRoot.from_path] with a
        `trusted_root.json` you refresh yourself if you need a pinned root that
        can be updated independently.

        Examples
        --------
        ```python
        >>> root = TrustedRoot.embedded()
        >>> root
        TrustedRoot()
        >>>
        ```
        """
        return cls(PyTrustedRoot.embedded())

    def __repr__(self) -> str:
        """Return a string representation of this trusted root.

        Examples
        --------
        ```python
        >>> root = TrustedRoot.from_json('{"mediaType": "application/vnd.dev.sigstore.trustedroot+json;version=0.1"}')
        >>> repr(root)
        'TrustedRoot()'
        >>>
        ```
        """
        return "TrustedRoot()"


class VerificationPolicy:
    """Controls whether attestation failures warn or reject a package."""

    def __init__(
        self,
        inner: PyVerificationPolicy,
        mode: VerificationMode,
        publisher: Publisher | None,
        channel_check: ChannelCheck,
        max_sidecar_size: int,
    ) -> None:
        self._inner = inner
        self._mode = mode
        self._publisher = publisher
        self._channel_check = channel_check
        self._max_sidecar_size = max_sidecar_size

    @classmethod
    def disabled(cls) -> VerificationPolicy:
        """Create a policy that performs no attestation verification."""
        return cls(
            PyVerificationPolicy.disabled(),
            VerificationMode.DISABLED,
            None,
            ChannelCheck.REQUIRE,
            DEFAULT_MAX_SIDECAR_SIZE,
        )

    @classmethod
    def warn(
        cls,
        publisher: Publisher | None = None,
        *,
        channel_check: ChannelCheck = ChannelCheck.REQUIRE,
        max_sidecar_size: int = DEFAULT_MAX_SIDECAR_SIZE,
    ) -> VerificationPolicy:
        """Create a policy that reports verification problems as warnings."""
        publisher = publisher or Publisher()
        return cls._create(VerificationMode.WARN, publisher, channel_check, max_sidecar_size)

    @classmethod
    def require(
        cls,
        publisher: Publisher | None = None,
        *,
        channel_check: ChannelCheck = ChannelCheck.REQUIRE,
        max_sidecar_size: int = DEFAULT_MAX_SIDECAR_SIZE,
    ) -> VerificationPolicy:
        """Create a policy that rejects packages whose attestations do not verify."""
        publisher = publisher or Publisher()
        return cls._create(VerificationMode.REQUIRE, publisher, channel_check, max_sidecar_size)

    @classmethod
    def _create(
        cls,
        mode: VerificationMode,
        publisher: Publisher,
        channel_check: ChannelCheck,
        max_sidecar_size: int,
    ) -> VerificationPolicy:
        if max_sidecar_size <= 0:
            raise ValueError("max_sidecar_size must be greater than zero")
        factory = PyVerificationPolicy.warn if mode == VerificationMode.WARN else PyVerificationPolicy.require
        inner = factory(
            publisher.identity,
            publisher.issuer.url if publisher.issuer is not None else None,
            channel_check.value,
            max_sidecar_size,
        )
        return cls(inner, mode, publisher, channel_check, max_sidecar_size)

    @property
    def mode(self) -> VerificationMode:
        return self._mode

    @property
    def publisher(self) -> Publisher | None:
        return self._publisher

    @property
    def channel_check(self) -> ChannelCheck:
        return self._channel_check

    @property
    def max_sidecar_size(self) -> int:
        return self._max_sidecar_size

    @property
    def is_enabled(self) -> bool:
        return self._inner.is_enabled

    @property
    def is_required(self) -> bool:
        return self._inner.is_required


@dataclass(frozen=True)
class CertificateClaims:
    """The claims a Fulcio signing certificate makes about the CI workload that signed a package.

    Every claim is optional: Sigstore only records them for a certificate issued
    to a CI workload, and which of them are set depends on the identity
    provider. The names are provider-neutral, so GitHub Actions, GitLab CI and
    Buildkite all populate the same claims.
    """

    build_signer_uri: str | None
    build_signer_digest: str | None
    runner_environment: str | None
    source_repository_uri: str | None
    source_repository_digest: str | None
    source_repository_ref: str | None
    source_repository_identifier: str | None
    source_repository_owner_uri: str | None
    source_repository_owner_identifier: str | None
    build_config_uri: str | None
    build_config_digest: str | None
    build_trigger: str | None
    run_invocation_uri: str | None
    source_repository_visibility_at_signing: str | None
    deployment_environment: str | None
    token_subject: str | None

    @classmethod
    def _from_ffi(cls, claims: PyCertificateClaims) -> CertificateClaims:
        return cls(
            build_signer_uri=claims.build_signer_uri,
            build_signer_digest=claims.build_signer_digest,
            runner_environment=claims.runner_environment,
            source_repository_uri=claims.source_repository_uri,
            source_repository_digest=claims.source_repository_digest,
            source_repository_ref=claims.source_repository_ref,
            source_repository_identifier=claims.source_repository_identifier,
            source_repository_owner_uri=claims.source_repository_owner_uri,
            source_repository_owner_identifier=claims.source_repository_owner_identifier,
            build_config_uri=claims.build_config_uri,
            build_config_digest=claims.build_config_digest,
            build_trigger=claims.build_trigger,
            run_invocation_uri=claims.run_invocation_uri,
            source_repository_visibility_at_signing=claims.source_repository_visibility_at_signing,
            deployment_environment=claims.deployment_environment,
            token_subject=claims.token_subject,
        )


@dataclass(frozen=True)
class VerifiedChecks:
    """Which parts of the Sigstore verification of a bundle were performed."""

    certificate_chain: bool
    signed_certificate_timestamp: bool
    transparency_log: bool
    inclusion_proof: bool

    @classmethod
    def _from_ffi(cls, checks: PyVerifiedChecks) -> VerifiedChecks:
        return cls(
            certificate_chain=checks.certificate_chain,
            signed_certificate_timestamp=checks.signed_certificate_timestamp,
            transparency_log=checks.transparency_log,
            inclusion_proof=checks.inclusion_proof,
        )


@dataclass(frozen=True)
class VerifiedAttestation:
    """A Sigstore bundle that passed signature, [CEP 27], and publisher checks.

    [CEP 27]: https://conda.org/learn/ceps/cep-0027
    """

    index: int
    identity: str | None
    issuer: str | None
    integrated_time: str | None
    target_channel: str | None
    claims: CertificateClaims | None
    """The claims of the signing certificate, if it was issued to a CI workload."""
    signed_at: str | None
    """When the signing certificate was issued, which approximates the signing time."""
    log_index: int | None
    """The index that identifies the signature within its transparency log."""
    log_origin: str | None
    """The name the transparency log gives itself in its signed checkpoint."""
    checks: VerifiedChecks
    warnings: list[str]


@dataclass(frozen=True)
class VerificationOutcome:
    """The result of applying a verification policy to one package record."""

    attestation: VerifiedAttestation | None
    warnings: list[str]

    @property
    def is_verified(self) -> bool:
        return self.attestation is not None

    @classmethod
    def _from_ffi(cls, outcome: PyVerificationOutcome) -> VerificationOutcome:
        attestation = outcome.attestation
        verified = None
        if attestation is not None:
            claims = attestation.claims
            verified = VerifiedAttestation(
                index=attestation.index,
                identity=attestation.identity,
                issuer=attestation.issuer,
                integrated_time=attestation.integrated_time,
                target_channel=attestation.target_channel,
                claims=None if claims is None else CertificateClaims._from_ffi(claims),
                signed_at=attestation.signed_at,
                log_index=attestation.log_index,
                log_origin=attestation.log_origin,
                checks=VerifiedChecks._from_ffi(attestation.checks),
                warnings=attestation.warnings,
            )
        return cls(attestation=verified, warnings=outcome.warnings)


async def verify_attestation(
    record: RepoDataRecord,
    policy: VerificationPolicy,
    client: Client | None = None,
    trusted_root: TrustedRoot | None = None,
) -> VerificationOutcome:
    """Discover and verify the Sigstore attestations advertised by ``record``.

    Without a `trusted_root` the production trusted root is loaded over TUF on
    first use, which requires network access. Passing one verifies against that
    trust material instead, leaving the sidecar download as the only request.
    """
    if client is None:
        client = Client.default_client()
    outcome = await py_verify_attestation(
        record,
        policy._inner,
        client._client,
        trusted_root._inner if trusted_root is not None else None,
    )
    return VerificationOutcome._from_ffi(outcome)
