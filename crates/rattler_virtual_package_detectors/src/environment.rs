//! Resolving a detector and installing it into an environment of its own.
//!
//! Clients can implement [`DetectorEnvironmentProvider`] to use their own
//! environment flow. The standalone [`RattlerEnvironmentProvider`] resolves the
//! detector by name, qualified with its registering channel, against the
//! registration's resolution channels for the host platform. The
//! only virtual packages available to that solve are the client's own. The
//! resolved records are fingerprinted with the
//! [`environment_digest`], and the environment lives in a directory named
//! after that digest, so an environment is reused exactly when the resolved
//! records are unchanged and reinstalled otherwise.
//!
//! Installation never runs link scripts. A prefix guard makes concurrent
//! installations of the same digest wait for each other and marks an
//! environment ready only once installation completed, so a detector never
//! runs from an incomplete installation.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use rattler::install::Installer;
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, RepoDataRecord, Subdir};
use rattler_digest::Sha256Hash;
use rattler_networking::LazyClient;
use rattler_prefix_guard::AsyncPrefixGuard;
use rattler_repodata_gateway::{
    AcceptedDetectorRegistration, ChannelRelationsMode, Gateway, GatewayError,
};
use rattler_solve::{SolveError, SolverImpl, SolverTask};
use thiserror::Error;

use rattler_environment_digest::environment_digest;

/// What resolving and installing a detector needs from the client.
#[derive(Clone)]
pub struct EnvironmentOptions<'a> {
    /// The gateway to load repodata with.
    pub gateway: &'a Gateway,
    /// The package cache to install from.
    pub package_cache: &'a PackageCache,
    /// The client to download packages with.
    pub download_client: LazyClient,
    /// The directory that holds every detector environment.
    pub root: &'a Path,
    /// The platform of the machine running the client.
    pub host_platform: Subdir,
    /// The client's own virtual packages, with overrides applied. Detector
    /// results never take part in resolving a detector.
    pub virtual_packages: Vec<GenericVirtualPackage>,
}

/// Lets a client resolve and create isolated detector environments using its
/// own environment machinery.
///
/// Resolution must use only the client's builtin virtual packages, qualify the
/// detector with its registering channel, and use the registration's already
/// expanded resolution channels without expanding them again. Detector results
/// must never participate in this solve. Installation must disable link scripts,
/// guard concurrent creation and repair interrupted installations. The engine
/// calls `install` only after consent and only when no valid result is cached.
#[async_trait]
pub trait DetectorEnvironmentProvider: Send + Sync {
    /// Resolves the detector against current repodata without installing it.
    async fn resolve(
        &self,
        registration: &AcceptedDetectorRegistration,
    ) -> Result<ResolvedDetector, EnvironmentError>;

    /// Returns a complete environment for the resolved detector.
    async fn install(
        &self,
        resolved: ResolvedDetector,
    ) -> Result<DetectorEnvironment, EnvironmentError>;
}

/// The standalone Rattler solver and installer implementation.
pub struct RattlerEnvironmentProvider<'a> {
    options: EnvironmentOptions<'a>,
}

impl<'a> RattlerEnvironmentProvider<'a> {
    /// Creates a provider whose environment root and solve configuration are
    /// supplied by the client.
    pub fn new(options: EnvironmentOptions<'a>) -> Self {
        Self { options }
    }
}

#[async_trait]
impl DetectorEnvironmentProvider for RattlerEnvironmentProvider<'_> {
    async fn resolve(
        &self,
        registration: &AcceptedDetectorRegistration,
    ) -> Result<ResolvedDetector, EnvironmentError> {
        resolve_detector(registration, &self.options).await
    }

    async fn install(
        &self,
        resolved: ResolvedDetector,
    ) -> Result<DetectorEnvironment, EnvironmentError> {
        ensure_environment(resolved, &self.options).await
    }
}

/// A detector resolved against current repodata.
#[derive(Clone, Debug)]
pub struct ResolvedDetector {
    /// The detector and its resolved dependencies.
    pub records: Vec<RepoDataRecord>,
    /// The [`environment_digest`] of `records`, used for consent and result
    /// cache invalidation.
    pub digest: Sha256Hash,
}

/// An installed, ready detector environment.
#[derive(Clone, Debug)]
pub struct DetectorEnvironment {
    /// The prefix the detector is installed in.
    pub prefix: PathBuf,
    /// Whether this call installed the environment rather than reusing it.
    pub installed: bool,
}

/// Why a detector environment could not be prepared.
#[derive(Debug, Error)]
pub enum EnvironmentError {
    /// The repodata of the resolution channels could not be loaded.
    #[error("failed to load repodata for the detector")]
    Repodata(#[from] GatewayError),

    /// The detector could not be resolved.
    #[error("failed to resolve the detector")]
    Solve(#[from] SolveError),

    /// The solve task panicked or was cancelled.
    #[error("the solve was interrupted")]
    SolveInterrupted(#[source] tokio::task::JoinError),

    /// The detector could not be installed.
    #[error("failed to install the detector")]
    Install(#[from] rattler::install::InstallerError),

    /// The prefix guard could not be acquired or updated.
    #[error("failed to guard the detector environment at {}", prefix.display())]
    Guard {
        /// The prefix.
        prefix: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A client-provided environment implementation failed.
    #[error("failed to prepare the detector environment: {0}")]
    Provider(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// The prefix a detector environment with `digest` lives in under `root`.
pub fn prefix_for(root: &Path, digest: &Sha256Hash) -> PathBuf {
    root.join(hex::encode(digest))
}

/// The `MatchSpec` a detector is resolved with: its name qualified with the
/// registering channel, without version or build constraints.
pub fn detector_spec(registration: &AcceptedDetectorRegistration) -> MatchSpec {
    MatchSpec {
        name: registration.registration.detector.clone().into(),
        channel: Some(Arc::new(registration.channel.clone())),
        ..MatchSpec::default()
    }
}

/// Resolves `registration`'s detector against current repodata and computes
/// the environment digest.
pub async fn resolve_detector(
    registration: &AcceptedDetectorRegistration,
    options: &EnvironmentOptions<'_>,
) -> Result<ResolvedDetector, EnvironmentError> {
    let spec = detector_spec(registration);
    let repodata = options
        .gateway
        .query(
            registration.resolution_channels.iter().cloned(),
            [options.host_platform, Subdir::NoArch],
            [spec.clone()],
        )
        .recursive(true)
        // The resolution channels are already the fully resolved set.
        .channel_relations(ChannelRelationsMode::Disabled)
        .await?;

    let virtual_packages = options.virtual_packages.clone();
    let records = tokio::task::spawn_blocking(move || {
        let task = SolverTask {
            specs: vec![spec],
            virtual_packages,
            ..SolverTask::from_iter(&repodata.repodata)
        };
        rattler_solve::resolvo::Solver.solve(task)
    })
    .await
    .map_err(EnvironmentError::SolveInterrupted)??
    .records;

    let digest = environment_digest(&records);
    Ok(ResolvedDetector { records, digest })
}

/// Makes sure the environment for `resolved` exists and is complete,
/// installing it when it is missing or was never completed.
pub async fn ensure_environment(
    resolved: ResolvedDetector,
    options: &EnvironmentOptions<'_>,
) -> Result<DetectorEnvironment, EnvironmentError> {
    let prefix = prefix_for(options.root, &resolved.digest);
    let guard_error = |source| EnvironmentError::Guard {
        prefix: prefix.clone(),
        source,
    };
    let guard = AsyncPrefixGuard::new(&prefix).await.map_err(guard_error)?;
    let mut write_guard = guard.write().await.map_err(guard_error)?;
    if write_guard.is_ready() {
        tracing::debug!(prefix = %prefix.display(), "reusing detector environment");
        return Ok(DetectorEnvironment {
            prefix,
            installed: false,
        });
    }

    tracing::debug!(prefix = %prefix.display(), "installing detector environment");
    write_guard.begin().await.map_err(guard_error)?;
    Installer::new()
        .with_package_cache(options.package_cache.clone())
        .with_download_client(options.download_client.clone())
        .with_target_platform(options.host_platform)
        .with_execute_link_scripts(false)
        .install(&prefix, resolved.records)
        .await?;
    write_guard.finish().await.map_err(guard_error)?;

    Ok(DetectorEnvironment {
        prefix,
        installed: true,
    })
}
