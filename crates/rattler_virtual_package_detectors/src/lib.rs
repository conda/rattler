//! Run the virtual package detectors that conda channels register in their
//! repodata, following the virtual package detector protocol CEP.
//!
//! A detector is a conda package with an executable that reports virtual
//! packages as JSON. Detecting with it means resolving and installing the
//! detector into an environment of its own, activating that environment,
//! running the executable under the protocol's time and output bounds, and
//! checking its report against what the channel registered. The modules follow
//! that pipeline:
//!
//! - [`rattler_environment_digest`] fingerprints a resolved detector environment.
//! - [`environment`] lets clients provide detector environments, with a standalone
//!   solver and installer implementation keyed by the environment digest.
//! - [`activation`] computes the environment variables of the activated prefix.
//! - [`runner`] finds and runs the executable within the protocol's limits.
//! - [`report`] parses the report and holds it to the registration.
//! - [`cache`] stores results and expires them as the report asked.
//! - [`overrides`] reads the `CONDA_OVERRIDE_*` variables of detector names.
//! - [`consent`] lets the client decide whether a detector may run at all.
//! - [`detect`](mod@detect) composes all of that into [`detect()`], the entry point.
//!
//! Enable [`RepoDataQuery::virtual_package_detectors`](rattler_repodata_gateway::RepoDataQuery::virtual_package_detectors)
//! to collect registrations and candidate-wide demand with the package records.
//! Pass all accepted registrations to [`detect()`] and use the query's wanted names
//! as [`WantedNames::Only`]. Standalone discovery is also available through
//! [`Gateway::virtual_package_detectors`](rattler_repodata_gateway::Gateway::virtual_package_detectors).
//!
//! Pass a [`DetectorEnvironmentProvider`] in [`DetectOptions`] to create
//! detector environments through the client's own environment flow. The
//! standalone [`RattlerEnvironmentProvider`] takes [`EnvironmentOptions`],
//! including the environment root, host platform and builtin virtual packages.
//! Registrations denied before resolution require no environment work. Otherwise
//! resolution precedes post-solve consent and cache lookup; installation happens
//! only after consent and on a result cache miss.
//! Supply an [`EnvironmentSnapshot`] in [`DetectOptions`] for overrides,
//! activation inheritance and watched-variable cache validation. Capture the
//! host environment explicitly with [`EnvironmentSnapshot::from_system`] when
//! desired, or construct an isolated snapshot without modifying process globals.
//!
//! [`DetectionOutcome::diagnostics`] retains nonempty stderr once per successful
//! invocation, including cache hits. Each entry identifies the origin, detector,
//! environment digest and cache provenance for clients that reject results later.
//!
//! Failed report validation preserves captured diagnostics. Concurrent result
//! publications are atomic, and watched environment variable names follow the
//! host's case-sensitivity rules. Cancelling an active detection terminates its
//! detector or activation process group on Unix and its job on Windows.

#![deny(missing_docs)]

pub mod activation;
pub mod cache;
pub mod consent;
pub mod detect;
pub mod environment;
pub mod overrides;
pub mod report;
pub mod runner;

pub use activation::{ActivationError, activated_environment};
pub use cache::{CacheClock, CacheError, CacheKey, CachedResult, ResultCache};
pub use consent::{AllowAll, ConfiguredConsent, Consent, ConsentRequest, DenyAll, DetectorConsent};
pub use detect::{
    DetectError, DetectOptions, DetectedValue, DetectionOutcome, DetectionSource,
    DetectorDiagnostics, DetectorFailure, DetectorResult, SkipReason, SkippedRegistration,
    WantedNames, detect, merge_results,
};
pub use environment::{
    DetectorEnvironment, DetectorEnvironmentProvider, EnvironmentError, EnvironmentOptions,
    RattlerEnvironmentProvider, ResolvedDetector, ensure_environment, resolve_detector,
};
pub use overrides::{OverrideError, OverrideValue, read_override};
pub use rattler_shell::environment::EnvironmentSnapshot;
pub use report::{
    CacheHints, CacheLifetime, DetectedVersion, DetectorReport, PROTOCOL_VERSION, ReportError,
    parse_report,
};
pub use runner::{DetectorRun, RunError, RunLimits, run_detector};

/// The bounds the protocol places on a detector run.
pub mod limits {
    use std::time::Duration;

    pub use rattler_conda_types::virtual_package_detector::{DEFAULT_TIMEOUT, MAX_TIMEOUT};

    /// Combined standard output and standard error byte count at which a
    /// detector is terminated and its run fails.
    pub const OUTPUT_LIMIT: usize = 1_048_576;

    /// The most entries a `watch_paths` or `watch_env` list may have.
    pub const MAX_WATCH_ENTRIES: usize = 32;

    /// The most UTF-8 bytes one watch entry may have.
    pub const MAX_WATCH_ENTRY_BYTES: usize = 4096;

    /// The lifetime of a cached result whose report gave no `ttl_seconds`.
    pub const DEFAULT_CACHE_LIFETIME: Duration = Duration::from_secs(60 * 60);

    /// The longest a cached result may live, whatever the report asked for.
    pub const MAX_CACHE_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);

    /// The lifetime of a `"REBOOT"` result on a host whose boot session cannot
    /// be observed.
    pub const REBOOT_FALLBACK_LIFETIME: Duration = DEFAULT_CACHE_LIFETIME;

    /// Clamps a requested timeout to [`MAX_TIMEOUT`].
    pub fn clamp_timeout(timeout: Duration) -> Duration {
        timeout.min(MAX_TIMEOUT)
    }
}
