//! Detecting virtual packages with registered detectors, end to end.
//!
//! For every accepted registration, [`detect`] reads the override variables of
//! its names, decides whether the detector has to run at all, resolves it,
//! asks the consent policy, installs and runs it, checks the report, caches the
//! result and collects what the solve should see. A detector's failure never
//! aborts detection: all of that detector's results are discarded and the
//! failure is reported alongside the results of the others.

use std::{
    collections::{BTreeSet, HashSet},
    path::Path,
    sync::Arc,
    time::Duration,
};

use futures::{StreamExt, stream::FuturesOrdered};
use indexmap::IndexMap;
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::{
    ChannelUrl, GenericVirtualPackage, PackageName, Subdir,
    virtual_package_detector::override_variable,
};
use rattler_digest::Sha256Hash;
use rattler_networking::LazyClient;
use rattler_repodata_gateway::{AcceptedDetectorRegistration, Gateway};
use thiserror::Error;

use crate::{
    activation::{ActivationError, activated_environment},
    cache::{CacheClock, CacheError, CacheKey, ResultCache},
    consent::{Consent, ConsentRequest, DetectorConsent},
    environment::{EnvironmentError, EnvironmentOptions, ensure_environment, resolve_detector},
    limits::clamp_timeout,
    overrides::{OverrideError, OverrideValue, read_override},
    report::{DetectedVersion, ReportError, parse_report},
    runner::{RunError, RunLimits, run_detector},
};

/// Which of the registered names a detection has to provide.
#[derive(Clone, Debug)]
pub enum WantedNames {
    /// Every registered name; run every detector.
    All,
    /// Only these names; a detector none of whose names is listed is skipped.
    Only(BTreeSet<PackageName>),
}

impl WantedNames {
    fn wants(&self, name: &PackageName) -> bool {
        match self {
            Self::All => true,
            Self::Only(names) => names.contains(name),
        }
    }
}

/// What a detection needs from the client.
pub struct DetectOptions<'a> {
    /// The gateway to load repodata with.
    pub gateway: &'a Gateway,
    /// The package cache to install detectors from.
    pub package_cache: &'a PackageCache,
    /// The client to download packages with.
    pub download_client: LazyClient,
    /// The directory that holds the detector environments and the result
    /// cache.
    pub root: &'a Path,
    /// The platform of the machine running the client.
    pub host_platform: Subdir,
    /// The platform being solved for. Detectors only run when it equals
    /// `host_platform`.
    pub target_platform: Subdir,
    /// The client's own virtual packages for the host, with overrides
    /// applied. They resolve the detector environments and never include
    /// detector results.
    pub client_virtual_packages: Vec<GenericVirtualPackage>,
    /// How long a detector may run, and separately how long its activation
    /// may take. Clamped to the protocol's maximum.
    pub timeout: Duration,
    /// Decides whether a detector may run.
    pub consent: &'a dyn DetectorConsent,
    /// Which names the solve can reference.
    pub wanted: WantedNames,
    /// How many detectors may run at the same time.
    pub concurrency: usize,
    /// The clock the result cache is validated against.
    pub clock: CacheClock,
}

/// Where a result came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetectionSource {
    /// A detector reported it.
    Detector {
        /// The registration's origin.
        origin: ChannelUrl,
        /// The detector.
        detector: PackageName,
        /// The environment digest of the detector that ran.
        digest: Sha256Hash,
        /// Whether the result was served from the cache.
        from_cache: bool,
    },
    /// An override variable set it.
    Override {
        /// The variable.
        variable: String,
    },
}

/// A result for one virtual package name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetectedValue {
    /// The virtual package is absent.
    Absent,
    /// The virtual package is present with this version.
    Present(DetectedVersion),
}

/// One virtual package name and what was decided about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectorResult {
    /// The virtual package name.
    pub name: PackageName,
    /// Whether it is present, and with which version.
    pub value: DetectedValue,
    /// Where the value came from.
    pub source: DetectionSource,
}

impl DetectorResult {
    /// The virtual package record, if present.
    pub fn virtual_package(&self) -> Option<GenericVirtualPackage> {
        match &self.value {
            DetectedValue::Absent => None,
            DetectedValue::Present(version) => {
                Some(version.clone().into_virtual_package(self.name.clone()))
            }
        }
    }
}

/// Why a detector was not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The target platform is not the host platform.
    TargetIsNotHost {
        /// The override variables that could still supply the names.
        override_variables: Vec<String>,
    },
    /// No wanted name remained after overrides.
    NoWantedName,
    /// The consent policy declined.
    ConsentDenied,
}

/// A registration whose detector was not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedRegistration {
    /// The registration's origin.
    pub origin: ChannelUrl,
    /// The detector.
    pub detector: PackageName,
    /// Why it was skipped.
    pub reason: SkipReason,
}

/// Why a detector failed. Every variant discards all of the detector's
/// results.
#[derive(Debug, Error)]
pub enum DetectError {
    /// The detector environment could not be prepared.
    #[error(transparent)]
    Environment(#[from] EnvironmentError),

    /// Activation failed or timed out.
    #[error(transparent)]
    Activation(#[from] ActivationError),

    /// The detector could not be run, exceeded a bound or exited unsuccessfully.
    #[error(transparent)]
    Run(#[from] RunError),

    /// The report is malformed or violates the registration.
    #[error("{source}")]
    Report {
        /// Why the report was rejected.
        #[source]
        source: ReportError,
        /// What the detector wrote to standard error.
        stderr: String,
    },

    /// The result could not be cached.
    #[error(transparent)]
    Cache(#[from] CacheError),
}

/// A failed detector and its diagnostics.
#[derive(Debug)]
pub struct DetectorFailure {
    /// The registration's origin.
    pub origin: ChannelUrl,
    /// The detector.
    pub detector: PackageName,
    /// What went wrong.
    pub error: DetectError,
    /// What the detector wrote to standard error, where it ran at all.
    pub stderr: Option<String>,
}

/// Everything a detection produced.
#[derive(Debug, Default)]
pub struct DetectionOutcome {
    /// The results, in registration order, with overrides applied.
    pub results: Vec<DetectorResult>,
    /// The detectors that failed. Their names are absent unless overridden.
    pub failures: Vec<DetectorFailure>,
    /// The detectors that did not run.
    pub skipped: Vec<SkippedRegistration>,
}

/// Runs the detectors of `registrations` and collects their results.
///
/// The only error is an invalid override variable, which the protocol
/// requires to be an error rather than a fallback to detection.
pub async fn detect(
    registrations: &[AcceptedDetectorRegistration],
    options: DetectOptions<'_>,
) -> Result<DetectionOutcome, OverrideError> {
    let mut outcome = DetectionOutcome::default();
    let mut to_run = Vec::new();
    for registration in registrations {
        let origin = registration.origin().clone();
        let detector = registration.registration.detector.clone();
        let mut overridden = HashSet::new();
        for name in &registration.registration.virtual_packages {
            if let Some(value) = read_override(name)? {
                overridden.insert(name.clone());
                outcome.results.push(DetectorResult {
                    name: name.clone(),
                    value: match value {
                        OverrideValue::Absent => DetectedValue::Absent,
                        OverrideValue::Present(version) => DetectedValue::Present(version),
                    },
                    source: DetectionSource::Override {
                        variable: override_variable(name),
                    },
                });
            }
        }

        let applicable: Vec<&PackageName> = registration
            .registration
            .virtual_packages
            .iter()
            .filter(|name| !overridden.contains(*name))
            .collect();
        if applicable.is_empty() {
            outcome.skipped.push(SkippedRegistration {
                origin,
                detector,
                reason: SkipReason::NoWantedName,
            });
            continue;
        }
        if options.target_platform != options.host_platform {
            outcome.skipped.push(SkippedRegistration {
                origin,
                detector,
                reason: SkipReason::TargetIsNotHost {
                    override_variables: applicable
                        .iter()
                        .map(|name| override_variable(name))
                        .collect(),
                },
            });
            continue;
        }
        if !applicable.iter().any(|name| options.wanted.wants(name)) {
            outcome.skipped.push(SkippedRegistration {
                origin,
                detector,
                reason: SkipReason::NoWantedName,
            });
            continue;
        }
        if options
            .consent
            .decide_before_resolving(&registration.channel, &registration.registration)
            == Some(Consent::Deny)
        {
            outcome.skipped.push(SkippedRegistration {
                origin,
                detector,
                reason: SkipReason::ConsentDenied,
            });
            continue;
        }
        to_run.push((registration, overridden));
    }

    let timeout = clamp_timeout(options.timeout);
    let environment_options = EnvironmentOptions {
        gateway: options.gateway,
        package_cache: options.package_cache,
        download_client: options.download_client.clone(),
        root: &options.root.join("envs"),
        host_platform: options.host_platform,
        virtual_packages: options.client_virtual_packages.clone(),
    };
    let cache = ResultCache::new(options.root.join("results"));
    let semaphore = Arc::new(tokio::sync::Semaphore::new(options.concurrency.max(1)));

    let mut runs: FuturesOrdered<_> = to_run
        .into_iter()
        .map(|(registration, overridden)| {
            let semaphore = semaphore.clone();
            let environment_options = environment_options.clone();
            let cache = cache.clone();
            let clock = options.clock.clone();
            let consent = options.consent;
            async move {
                let _permit = semaphore
                    .acquire()
                    .await
                    .expect("semaphore is never closed");
                run_one(
                    registration,
                    &overridden,
                    &environment_options,
                    &cache,
                    &clock,
                    consent,
                    timeout,
                )
                .await
            }
        })
        .collect();

    while let Some(result) = runs.next().await {
        match result {
            RunOutcome::Results(results) => outcome.results.extend(results),
            RunOutcome::Skipped(skipped) => outcome.skipped.push(skipped),
            RunOutcome::Failed(failure) => outcome.failures.push(failure),
        }
    }
    Ok(outcome)
}

enum RunOutcome {
    Results(Vec<DetectorResult>),
    Skipped(SkippedRegistration),
    Failed(DetectorFailure),
}

async fn run_one(
    registration: &AcceptedDetectorRegistration,
    overridden: &HashSet<PackageName>,
    environment_options: &EnvironmentOptions<'_>,
    cache: &ResultCache,
    clock: &CacheClock,
    consent: &dyn DetectorConsent,
    timeout: Duration,
) -> RunOutcome {
    let origin = registration.origin().clone();
    let detector = registration.registration.detector.clone();
    match run_detector_pipeline(
        registration,
        environment_options,
        cache,
        clock,
        consent,
        timeout,
    )
    .await
    {
        Ok(Some((virtual_packages, source))) => RunOutcome::Results(
            virtual_packages
                .into_iter()
                .filter(|(name, _)| !overridden.contains(name))
                .map(|(name, version)| DetectorResult {
                    name,
                    value: match version {
                        None => DetectedValue::Absent,
                        Some(version) => DetectedValue::Present(version),
                    },
                    source: source.clone(),
                })
                .collect(),
        ),
        Ok(None) => RunOutcome::Skipped(SkippedRegistration {
            origin,
            detector,
            reason: SkipReason::ConsentDenied,
        }),
        Err(error) => {
            let stderr = match &error {
                DetectError::Run(run) => run.stderr().map(ToString::to_string),
                DetectError::Activation(activation) => activation.stderr(),
                DetectError::Report { stderr, .. } => Some(stderr.clone()),
                _ => None,
            };
            RunOutcome::Failed(DetectorFailure {
                origin,
                detector,
                error,
                stderr,
            })
        }
    }
}

type PipelineResult = Result<
    Option<(
        IndexMap<PackageName, Option<DetectedVersion>>,
        DetectionSource,
    )>,
    DetectError,
>;

/// Resolves, asks consent, installs, runs and caches. `Ok(None)` means
/// consent was denied.
async fn run_detector_pipeline(
    registration: &AcceptedDetectorRegistration,
    environment_options: &EnvironmentOptions<'_>,
    cache: &ResultCache,
    clock: &CacheClock,
    consent: &dyn DetectorConsent,
    timeout: Duration,
) -> PipelineResult {
    let origin = registration.origin().clone();
    let detector = registration.registration.detector.clone();
    let resolved = resolve_detector(registration, environment_options).await?;

    // A decision that needed nothing but the registration was taken before
    // resolving; only the policies that want to see the packages get here.
    let request = ConsentRequest {
        channel: &registration.channel,
        registration: &registration.registration,
        resolution_channels: &registration.resolution_channels,
        records: &resolved.records,
        digest: resolved.digest,
    };
    let decided =
        match consent.decide_before_resolving(&registration.channel, &registration.registration) {
            Some(consent) => consent,
            None => consent.decide(&request).await,
        };
    if decided == Consent::Deny {
        return Ok(None);
    }

    let key = CacheKey::new(
        origin.clone(),
        detector.clone(),
        registration.registration.virtual_packages.iter().cloned(),
        &resolved.digest,
    );
    let source = |from_cache| DetectionSource::Detector {
        origin: origin.clone(),
        detector: detector.clone(),
        digest: resolved.digest,
        from_cache,
    };
    if let Some(cached) = cache.read(&key, clock).await {
        tracing::debug!(%origin, detector = detector.as_source(), "using cached detector result");
        return Ok(Some((cached.virtual_packages, source(true))));
    }

    let environment = ensure_environment(resolved.clone(), environment_options).await?;
    let env = activated_environment(
        &environment.prefix,
        environment_options.host_platform,
        timeout,
    )
    .await?;
    let run = run_detector(
        &environment.prefix,
        &detector,
        environment_options.host_platform,
        &env,
        RunLimits {
            timeout,
            ..RunLimits::default()
        },
    )
    .await?;
    if !run.stderr.is_empty() {
        tracing::debug!(%origin, detector = detector.as_source(), stderr = %run.stderr, "detector diagnostics");
    }
    let report = parse_report(&run.stdout, &registration.registration).map_err(|source| {
        DetectError::Report {
            source,
            stderr: run.stderr,
        }
    })?;
    // Caching is a convenience; a report that could not be stored is still a
    // valid report.
    if let Err(error) = cache.write(&key, &report, clock).await {
        tracing::warn!(%origin, detector = detector.as_source(), "could not cache the detector result: {error}");
    }
    Ok(Some((report.virtual_packages, source(false))))
}

/// Applies detector results to the client's own virtual packages: a present
/// result replaces the record of the same name, an absent result removes it,
/// and names nobody decided about stay as they were.
pub fn merge_results(
    client_virtual_packages: impl IntoIterator<Item = GenericVirtualPackage>,
    results: &[DetectorResult],
) -> Vec<GenericVirtualPackage> {
    let decided: IndexMap<&str, &DetectorResult> = results
        .iter()
        .map(|result| (result.name.as_normalized(), result))
        .collect();
    let mut merged: Vec<GenericVirtualPackage> = client_virtual_packages
        .into_iter()
        .filter(|package| !decided.contains_key(package.name.as_normalized()))
        .collect();
    merged.extend(results.iter().filter_map(DetectorResult::virtual_package));
    merged
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rattler_conda_types::Version;

    use super::*;

    fn package(name: &str, version: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: PackageName::try_from(name).unwrap(),
            version: Version::from_str(version).unwrap(),
            build_string: "0".to_string(),
        }
    }

    fn result(name: &str, version: Option<&str>) -> DetectorResult {
        DetectorResult {
            name: PackageName::try_from(name).unwrap(),
            value: match version {
                None => DetectedValue::Absent,
                Some(version) => DetectedValue::Present(DetectedVersion {
                    version: Version::from_str(version).unwrap(),
                    build_string: "0".to_string(),
                }),
            },
            source: DetectionSource::Override {
                variable: "X".to_string(),
            },
        }
    }

    #[test]
    fn merge_replaces_removes_and_keeps() {
        let merged = merge_results(
            [package("__cuda", "12.0"), package("__glibc", "2.28")],
            &[
                result("__cuda", Some("12.8")),
                result("__glibc", None),
                result("__mpi", Some("5")),
            ],
        );
        assert_eq!(merged, [package("__cuda", "12.8"), package("__mpi", "5")]);
    }
}
