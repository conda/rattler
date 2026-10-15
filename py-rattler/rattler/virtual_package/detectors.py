"""Channel-registered virtual package detectors.

A detector is a conda package with an executable that reports virtual packages
as JSON. Opt into registration discovery with `Gateway.query(detector_target=...)`
to collect registrations and candidate demand with repodata. Pass the accepted
registrations and the query's wanted names to `detect_virtual_packages` to
install and run the needed detectors. `Gateway.virtual_package_detectors`
also supports standalone registration discovery.
"""

from __future__ import annotations

import os
from collections.abc import Callable, Iterable
from typing import TYPE_CHECKING

from rattler.channel.channel import Channel
from rattler.package.package_name import PackageName
from rattler.platform.subdir import Subdir, SubdirLiteral
from rattler.rattler import (
    PyConsentRequest,
    PyDetectionOutcome,
    PyDetectorDiagnostics,
    PyDetectorFailure,
    PyDetectorRegistration,
    PyDetectorResult,
    PyRejectedDetectorRegistration,
    PySkippedRegistration,
    py_detect_virtual_packages,
)
from rattler.repo_data.record import RepoDataRecord
from rattler.virtual_package.generic import GenericVirtualPackage

if TYPE_CHECKING:
    from rattler.networking.client import Client
    from rattler.repo_data.gateway import Gateway


class DetectorRegistration:
    """A detector registration a gateway accepted: its detector speaks for its
    virtual package names in the solve."""

    _inner: PyDetectorRegistration

    @classmethod
    def _from_py(cls, inner: PyDetectorRegistration) -> DetectorRegistration:
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def channel(self) -> Channel:
        """The channel that registered the detector."""
        return Channel._from_py_channel(self._inner.channel)

    @property
    def origin(self) -> str:
        """The registration's origin: the registering channel's base URL."""
        return self._inner.origin

    @property
    def detector(self) -> PackageName:
        """The package providing the detector; also the executable's name."""
        return PackageName._from_py_package_name(self._inner.detector)

    @property
    def virtual_packages(self) -> list[PackageName]:
        """The virtual packages the detector reports."""
        return [PackageName._from_py_package_name(name) for name in self._inner.virtual_packages]

    @property
    def resolution_channels(self) -> list[Channel]:
        """The channels the detector and its dependencies resolve against, highest priority first."""
        return [Channel._from_py_channel(channel) for channel in self._inner.resolution_channels]

    def __repr__(self) -> str:
        names = ", ".join(str(name) for name in self.virtual_packages)
        return f"DetectorRegistration({self.detector!s} from {self.origin} reporting [{names}])"


class RejectedDetectorRegistration:
    """A registration rejected because a higher-priority channel already
    provides one of its names. Its detector must not run."""

    _inner: PyRejectedDetectorRegistration

    @classmethod
    def _from_py(cls, inner: PyRejectedDetectorRegistration) -> RejectedDetectorRegistration:
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def channel(self) -> Channel:
        """The channel that registered the detector."""
        return Channel._from_py_channel(self._inner.channel)

    @property
    def detector(self) -> PackageName:
        """The rejected detector."""
        return PackageName._from_py_package_name(self._inner.detector)

    @property
    def virtual_packages(self) -> list[PackageName]:
        """The virtual packages it would have reported."""
        return [PackageName._from_py_package_name(name) for name in self._inner.virtual_packages]

    @property
    def conflicting_name(self) -> PackageName:
        """The first name that conflicts with an accepted registration."""
        return PackageName._from_py_package_name(self._inner.conflicting_name)

    @property
    def conflict(self) -> str:
        """``"name"`` when the names are equal, otherwise the shared ``CONDA_OVERRIDE_*`` variable."""
        return self._inner.conflict

    @property
    def accepted_origin(self) -> str:
        """The origin of the accepted registration that reserved the name."""
        return self._inner.accepted_origin

    @property
    def accepted_detector(self) -> PackageName:
        """The detector of the accepted registration that reserved the name."""
        return PackageName._from_py_package_name(self._inner.accepted_detector)

    def __repr__(self) -> str:
        return (
            f"RejectedDetectorRegistration({self.detector!s} loses {self.conflicting_name!s} "
            f"to {self.accepted_detector!s} of {self.accepted_origin})"
        )


class DetectorRegistrations:
    """The result of `Gateway.virtual_package_detectors`."""

    accepted: list[DetectorRegistration]
    """The accepted registrations in CEP-42 channel order."""

    rejected: list[RejectedDetectorRegistration]
    """The registrations rejected for conflicting with an accepted one."""

    @classmethod
    def _from_py(
        cls,
        accepted: list[PyDetectorRegistration],
        rejected: list[PyRejectedDetectorRegistration],
    ) -> DetectorRegistrations:
        instance = cls.__new__(cls)
        instance.accepted = [DetectorRegistration._from_py(registration) for registration in accepted]
        instance.rejected = [RejectedDetectorRegistration._from_py(registration) for registration in rejected]
        return instance

    def __repr__(self) -> str:
        return f"DetectorRegistrations(accepted={self.accepted!r}, rejected={self.rejected!r})"


class ConsentRequest:
    """What a consent callback is asked about, after the detector was resolved
    and before anything is installed or run."""

    _inner: PyConsentRequest

    @classmethod
    def _from_py(cls, inner: PyConsentRequest) -> ConsentRequest:
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def channel(self) -> Channel:
        """The channel that registered the detector."""
        return Channel._from_py_channel(self._inner.channel)

    @property
    def detector(self) -> PackageName:
        """The detector."""
        return PackageName._from_py_package_name(self._inner.detector)

    @property
    def virtual_packages(self) -> list[PackageName]:
        """The virtual packages the detector reports."""
        return [PackageName._from_py_package_name(name) for name in self._inner.virtual_packages]

    @property
    def resolution_channels(self) -> list[Channel]:
        """The channels the detector was resolved against."""
        return [Channel._from_py_channel(channel) for channel in self._inner.resolution_channels]

    @property
    def records(self) -> list[RepoDataRecord]:
        """The packages that would be installed, the detector included."""
        return [RepoDataRecord._from_py_record(record) for record in self._inner.records]

    @property
    def digest(self) -> str:
        """The environment digest of the records, as lowercase hexadecimal."""
        return self._inner.digest


class DetectorResult:
    """One virtual package name and what was decided about it."""

    _inner: PyDetectorResult

    @classmethod
    def _from_py(cls, inner: PyDetectorResult) -> DetectorResult:
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def name(self) -> PackageName:
        """The virtual package name."""
        return PackageName._from_py_package_name(self._inner.name)

    @property
    def virtual_package(self) -> GenericVirtualPackage | None:
        """The virtual package, or ``None`` when the name is absent."""
        package = self._inner.virtual_package
        if package is None:
            return None
        return GenericVirtualPackage._from_py_generic_virtual_package(package)

    @property
    def absent(self) -> bool:
        """Whether the name was decided to be absent."""
        return self._inner.absent

    @property
    def source(self) -> str:
        """``"detector"`` or ``"override"``."""
        return self._inner.source

    @property
    def origin(self) -> str | None:
        """The registration's origin, for results a detector produced."""
        return self._inner.origin

    @property
    def detector(self) -> PackageName | None:
        """The detector, for results a detector produced."""
        detector = self._inner.detector
        return None if detector is None else PackageName._from_py_package_name(detector)

    @property
    def digest(self) -> str | None:
        """The environment digest of the detector that ran, as lowercase hexadecimal."""
        return self._inner.digest

    @property
    def from_cache(self) -> bool:
        """Whether the result was served from the cache."""
        return self._inner.from_cache

    @property
    def override_variable(self) -> str | None:
        """The ``CONDA_OVERRIDE_*`` variable, for results an override produced."""
        return self._inner.override_variable

    def __repr__(self) -> str:
        return f"DetectorResult({self.name!s}={self.virtual_package!r}, source={self.source!r})"


class DetectorDiagnostics:
    """Nonempty standard error from one successful detector invocation."""

    _inner: PyDetectorDiagnostics

    @classmethod
    def _from_py(cls, inner: PyDetectorDiagnostics) -> DetectorDiagnostics:
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def origin(self) -> str:
        """The registration's origin."""
        return self._inner.origin

    @property
    def detector(self) -> PackageName:
        """The detector."""
        return PackageName._from_py_package_name(self._inner.detector)

    @property
    def digest(self) -> str:
        """The detector environment digest, as lowercase hexadecimal."""
        return self._inner.digest

    @property
    def from_cache(self) -> bool:
        """Whether these diagnostics came from the result cache."""
        return self._inner.from_cache

    @property
    def stderr(self) -> str:
        """The detector's nonempty, lossily decoded standard error."""
        return self._inner.stderr

    def __repr__(self) -> str:
        return f"DetectorDiagnostics({self.detector!s} from {self.origin}, from_cache={self.from_cache})"


class DetectorFailure:
    """A detector that failed; all of its results were discarded."""

    _inner: PyDetectorFailure

    @classmethod
    def _from_py(cls, inner: PyDetectorFailure) -> DetectorFailure:
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def origin(self) -> str:
        """The registration's origin."""
        return self._inner.origin

    @property
    def detector(self) -> PackageName:
        """The detector."""
        return PackageName._from_py_package_name(self._inner.detector)

    @property
    def message(self) -> str:
        """What went wrong, including the causes."""
        return self._inner.message

    @property
    def stderr(self) -> str | None:
        """What the detector wrote to standard error, where it ran at all."""
        return self._inner.stderr

    def __repr__(self) -> str:
        return f"DetectorFailure({self.detector!s} from {self.origin}: {self.message})"


class SkippedRegistration:
    """A detector that did not run."""

    _inner: PySkippedRegistration

    @classmethod
    def _from_py(cls, inner: PySkippedRegistration) -> SkippedRegistration:
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def origin(self) -> str:
        """The registration's origin."""
        return self._inner.origin

    @property
    def detector(self) -> PackageName:
        """The detector."""
        return PackageName._from_py_package_name(self._inner.detector)

    @property
    def reason(self) -> str:
        """``"target-is-not-host"``, ``"no-wanted-name"`` or ``"consent-denied"``."""
        return self._inner.reason

    @property
    def override_variables(self) -> list[str]:
        """The ``CONDA_OVERRIDE_*`` variables that could still supply the names."""
        return self._inner.override_variables

    def __repr__(self) -> str:
        return f"SkippedRegistration({self.detector!s} from {self.origin}: {self.reason})"


class DetectionOutcome:
    """Everything `detect_virtual_packages` produced."""

    _inner: PyDetectionOutcome

    @classmethod
    def _from_py(cls, inner: PyDetectionOutcome) -> DetectionOutcome:
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    @property
    def results(self) -> list[DetectorResult]:
        """The results, in registration order, with overrides applied."""
        return [DetectorResult._from_py(result) for result in self._inner.results]

    @property
    def diagnostics(self) -> list[DetectorDiagnostics]:
        """Nonempty standard error, once per successful invocation, including cache hits."""
        return [DetectorDiagnostics._from_py(diagnostic) for diagnostic in self._inner.diagnostics]

    @property
    def failures(self) -> list[DetectorFailure]:
        """The detectors that failed. Their names are absent unless overridden."""
        return [DetectorFailure._from_py(failure) for failure in self._inner.failures]

    @property
    def skipped(self) -> list[SkippedRegistration]:
        """The detectors that did not run."""
        return [SkippedRegistration._from_py(skipped) for skipped in self._inner.skipped]

    def merge(self, virtual_packages: Iterable[GenericVirtualPackage]) -> list[GenericVirtualPackage]:
        """Applies the results to ``virtual_packages`` the way a solve should
        see them: present results replace records of the same name, absent
        results remove them, and names nobody decided about stay as they were."""
        merged = self._inner.merge([package._generic_virtual_package for package in virtual_packages])
        return [GenericVirtualPackage._from_py_generic_virtual_package(package) for package in merged]

    def __repr__(self) -> str:
        return f"DetectionOutcome(results={self.results!r}, failures={self.failures!r}, skipped={self.skipped!r})"


async def detect_virtual_packages(
    registrations: Iterable[DetectorRegistration],
    gateway: Gateway,
    host_platform: Subdir | SubdirLiteral,
    target_platform: Subdir | SubdirLiteral,
    client_virtual_packages: Iterable[GenericVirtualPackage],
    consent: bool | Callable[[ConsentRequest], bool],
    cache_dir: os.PathLike[str] | str | None = None,
    client: Client | None = None,
    timeout_seconds: int | None = None,
    wanted: Iterable[PackageName] | None = None,
    concurrency: int = 4,
) -> DetectionOutcome:
    """Runs the detectors of ``registrations`` and returns what they reported.

    Every detector is resolved against current repodata, installed into an
    environment of its own under ``cache_dir`` when that environment does not
    exist yet, activated, and run within the protocol's time and output
    bounds. Results are cached as the detector's report asks. A detector's
    failure never aborts detection: its results are discarded and reported in
    the outcome. ``CONDA_OVERRIDE_*`` variables replace results for their
    names; an invalid one raises `VirtualPackageOverrideError`. An exception raised by the
    ``consent`` callback denies that detector and is re-raised once detection is done.

    ``outcome.diagnostics`` retains nonempty standard error once per successful
    invocation, including cache hits. Each entry identifies the origin, detector,
    environment digest and cache provenance.

    Arguments:
        registrations: The accepted registrations, from `Gateway.virtual_package_detectors`.
        gateway: The gateway to resolve detectors with.
        host_platform: The platform of this machine.
        target_platform: The platform being solved for. Detectors only run when it
                         equals ``host_platform``.
        client_virtual_packages: The client's own virtual packages for the host, used
                                 to resolve the detector environments.
        consent: ``True`` to run every detector, ``False`` to run none, or a callable
                 that receives a `ConsentRequest` after the detector was resolved and
                 returns whether it may run.
        cache_dir: The rattler cache directory the environments and results live in.
                   Defaults to the standard rattler cache directory.
        client: The client to download packages with.
        timeout_seconds: How long a detector may run, and separately how long its
                         activation may take. Defaults to 30 and is capped at 300.
        wanted: Only run detectors that report one of these names. ``None`` runs all;
                an empty iterable runs none.
        concurrency: How many detectors may run at the same time.
    """

    if isinstance(consent, bool):
        py_consent: bool | Callable[[PyConsentRequest], bool] = consent
    elif callable(consent):
        callback = consent

        def wrap_consent(request: PyConsentRequest) -> bool:
            return bool(callback(ConsentRequest._from_py(request)))

        py_consent = wrap_consent
    else:
        raise TypeError("consent must be a bool or a callable taking a ConsentRequest")
    outcome = await py_detect_virtual_packages(
        [registration._inner for registration in registrations],
        gateway._gateway,
        host_platform._inner if isinstance(host_platform, Subdir) else Subdir(host_platform)._inner,
        target_platform._inner if isinstance(target_platform, Subdir) else Subdir(target_platform)._inner,
        [package._generic_virtual_package for package in client_virtual_packages],
        py_consent,
        None if cache_dir is None else os.fspath(cache_dir),
        None if client is None else client._client,
        timeout_seconds,
        None if wanted is None else [name._name for name in wanted],
        concurrency,
    )
    return DetectionOutcome._from_py(outcome)
