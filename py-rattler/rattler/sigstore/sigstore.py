from __future__ import annotations

import os
from dataclasses import dataclass
from enum import Enum
from urllib.parse import urlsplit

from rattler.networking import Client
from rattler.rattler import (
    PyTrustedRoot,
    PyVerificationOutcome,
    PyVerificationPolicy,
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
    ``trusted_root.json``.
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
class VerifiedAttestation:
    """A Sigstore bundle that passed signature, CEP 27, and publisher checks."""

    index: int
    identity: str | None
    issuer: str | None
    integrated_time: str | None
    target_channel: str | None
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
            verified = VerifiedAttestation(
                index=attestation.index,
                identity=attestation.identity,
                issuer=attestation.issuer,
                integrated_time=attestation.integrated_time,
                target_channel=attestation.target_channel,
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
