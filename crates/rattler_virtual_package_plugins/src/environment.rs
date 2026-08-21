//! Installing a detection plugin into an environment of its own.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rattler::install::Installer;
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::{
    Channel, MatchSpec, PackageName, ParseMatchSpecOptions, Platform, RepoDataRecord,
};
use rattler_digest::{Sha256, Sha256Hash, compute_bytes_digest};
use rattler_repodata_gateway::{ChannelRelationsMode, Gateway};
use rattler_solve::{SolverImpl, SolverTask, resolvo::Solver};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};

/// Marks a prefix as fully installed. Without it a prefix left behind by an
/// interrupted install would be reused as if it were complete.
const READY_SENTINEL: &str = ".plugin-ready";

/// An environment a plugin can be run from.
#[derive(Debug, Clone)]
pub struct PluginEnvironment {
    /// The prefix the plugin is installed in.
    pub prefix: PathBuf,

    /// Identifies this environment by its contents: a hash over every package in
    /// it, so a dependency update yields a different environment rather than a
    /// stale one.
    pub sha256: Sha256Hash,

    /// How long preparing it took, stage by stage.
    pub timings: EnvironmentTimings,
}

/// How long each stage of preparing a plugin environment took.
///
/// Getting a plugin ready costs a network round trip, a solve and an install,
/// and which of those dominates decides what is worth optimising. Measuring
/// beats guessing, so the numbers travel with the result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnvironmentTimings {
    /// Reading the channel's repodata, which is where a cold cache is paid for.
    pub repodata: Duration,

    /// Resolving the plugin and its dependencies.
    pub solve: Duration,

    /// Installing the environment. Zero when an existing prefix was reused,
    /// which is the ordinary case after the first run.
    pub install: Duration,

    /// Whether the repodata had to be re-read because the plugin turned out to
    /// have dependencies. Two round trips rather than one.
    pub refetched_for_dependencies: bool,
}

/// A plugin environment could not be prepared.
#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    /// The plugin name could not be turned into a spec to solve for.
    #[error("'{plugin}' is not a usable package name")]
    InvalidPluginName {
        /// The name that came out of the channel's registration.
        plugin: String,
        /// Why it could not be used.
        #[source]
        source: rattler_conda_types::ParseMatchSpecError,
    },

    /// The channel's repodata could not be fetched.
    #[error("failed to fetch repodata for the plugin")]
    Fetch(#[from] rattler_repodata_gateway::GatewayError),

    /// The channel registers the plugin but ships no package providing it.
    ///
    /// Reported separately from a solve failure because it is a different
    /// problem with a different fix: the channel's metadata and its packages
    /// disagree, and no amount of dependency resolution will help.
    #[error("the channel registers the plugin '{plugin}' but provides no such package")]
    PluginPackageMissing {
        /// The plugin the channel registered.
        plugin: String,
        /// The channel that registered it, for a caller that reports errors
        /// away from where the channel is already named.
        channel: String,
    },

    /// The plugin and its dependencies could not be resolved.
    ///
    /// The solve deliberately sees only built-in virtual packages, so a plugin
    /// depending on a plugin-provided one fails here rather than recursing.
    #[error("failed to resolve the plugin and its dependencies")]
    Solve(#[from] rattler_solve::SolveError),

    /// The system's built-in virtual packages could not be determined.
    #[error("failed to determine the virtual packages of this system")]
    Detect(#[from] rattler_virtual_packages::DetectVirtualPackageError),

    /// The environment could not be installed.
    #[error("failed to install the plugin environment")]
    Install(#[from] rattler::install::InstallerError),

    /// The prefix could not be marked as ready.
    #[error("failed to write the plugin environment sentinel")]
    Sentinel(#[from] std::io::Error),
}

/// Everything needed to prepare one plugin's environment.
pub struct PluginEnvironmentOptions<'a> {
    /// Where to read the channel's repodata from.
    pub gateway: &'a Gateway,

    /// The package cache the install draws from.
    pub package_cache: &'a PackageCache,

    /// The channel that registered the plugin. The plugin and its dependencies
    /// are resolved from it and from the channels its CEP-42 relations reach,
    /// and from nowhere else.
    pub channel: &'a Channel,

    /// The package providing the plugin.
    pub plugin: &'a PackageName,

    /// Directory the per-plugin prefixes live under.
    pub root: &'a Path,

    /// The platform to solve for. Detection inspects the running machine, so
    /// this is the host platform and never a cross-compilation target.
    pub host_platform: Platform,

    /// Where [`rattler_virtual_packages`] may keep what it costs to detect this
    /// machine's own virtual packages, or `None` to detect them afresh. The
    /// solve below needs them, and asking a GPU driver for them is the slowest
    /// thing about preparing an environment that otherwise reuses everything.
    pub cache_dir: Option<&'a Path>,
}

/// Installs `plugin` into an environment of its own and returns it, reusing an
/// existing one when its contents would be identical.
///
/// The plugin is solved against **built-in virtual packages only**. Resolving a
/// plugin's dependencies is itself a solve against a channel whose plugin data is
/// not available yet, and restricting it to built-ins is what stops that
/// recursion.
///
/// The identity of the environment is not known until the solve has happened,
/// since it covers every resolved package. A hit therefore skips the install, not
/// the solve -- which against cached repodata is the cheap half.
///
/// The channel's repodata is read **without** the dependency closure first. A
/// detection plugin should be self-contained, and the common one is: a script
/// with no dependencies at all. Fetching the closure to discover that costs a
/// round trip the answer never needed, so it is only fetched when the plugin's
/// own record turns out to name dependencies.
pub async fn ensure_plugin_environment(
    options: PluginEnvironmentOptions<'_>,
) -> Result<PluginEnvironment, EnvironmentError> {
    let PluginEnvironmentOptions {
        gateway,
        package_cache,
        channel,
        plugin,
        root,
        host_platform,
        cache_dir,
    } = options;
    let mut timings = EnvironmentTimings::default();

    let spec = MatchSpec::from_str(plugin.as_source(), ParseMatchSpecOptions::default()).map_err(
        |source| EnvironmentError::InvalidPluginName {
            plugin: plugin.as_source().to_string(),
            source,
        },
    )?;

    let started = Instant::now();
    let mut repo_data = query_plugin(gateway, channel, host_platform, &spec, false).await?;

    let has_dependencies = {
        let mut candidates = plugin_records(&repo_data, plugin).peekable();

        // A registration naming a package the channel does not have is a mistake
        // in the channel, and saying so beats letting the solver report an
        // unsatisfiable dependency on a name the user never asked for.
        if candidates.peek().is_none() {
            return Err(EnvironmentError::PluginPackageMissing {
                plugin: plugin.as_source().to_string(),
                channel: channel.canonical_name(),
            });
        }

        candidates.any(|record| !record.package_record.depends.is_empty())
    };

    // Any candidate with dependencies means the solve needs records this query
    // did not ask for. Which candidate the solver picks is not known yet, so the
    // closure is fetched when *any* of them could need it.
    if has_dependencies {
        timings.refetched_for_dependencies = true;
        repo_data = query_plugin(gateway, channel, host_platform, &spec, true).await?;
    }
    timings.repodata = started.elapsed();

    let virtual_packages = VirtualPackage::detect(&VirtualPackageOverrides::from_env(), cache_dir)?
        .into_iter()
        .map(Into::into)
        .collect();

    let started = Instant::now();
    let solved = Solver.solve(SolverTask {
        specs: vec![spec],
        virtual_packages,
        ..SolverTask::from_iter(&repo_data)
    })?;
    timings.solve = started.elapsed();

    let sha256 = environment_sha256(&solved.records);
    let prefix = root.join(hex::encode(sha256));
    let sentinel = prefix.join(READY_SENTINEL);
    if sentinel.is_file() {
        tracing::debug!("reusing plugin environment at {}", prefix.display());
        return Ok(PluginEnvironment {
            prefix,
            sha256,
            timings,
        });
    }

    let started = Instant::now();
    Installer::new()
        .with_target_platform(host_platform)
        .with_package_cache(package_cache.clone())
        .install(&prefix, solved.records)
        .await?;
    fs_err::write(&sentinel, [])?;
    timings.install = started.elapsed();

    Ok(PluginEnvironment {
        prefix,
        sha256,
        timings,
    })
}

/// Reads what a channel has for one plugin, with or without the records its
/// dependencies would need.
///
/// The registering channel is the only one asked for, so nothing a user happens
/// to have configured can supply a plugin's dependency. The channels that
/// channel's own CEP-42 relations reach are asked too, which is how a channel
/// deriving from another registers a plugin built against it: the closure stays
/// bounded by the registering channel's own declarations either way.
async fn query_plugin(
    gateway: &Gateway,
    channel: &Channel,
    host_platform: Platform,
    spec: &MatchSpec,
    recursive: bool,
) -> Result<rattler_repodata_gateway::RepoDataQueryOutput, rattler_repodata_gateway::GatewayError> {
    gateway
        .query(
            vec![channel.clone()],
            [host_platform, Platform::NoArch],
            vec![spec.clone()],
        )
        .channel_relations(ChannelRelationsMode::Warn)
        .recursive(recursive)
        .execute()
        .await
}

/// Every record a query returned for the plugin package itself.
fn plugin_records<'a>(
    repo_data: &'a rattler_repodata_gateway::RepoDataQueryOutput,
    plugin: &'a PackageName,
) -> impl Iterator<Item = &'a RepoDataRecord> {
    repo_data
        .iter()
        .flat_map(rattler_repodata_gateway::RepoData::iter)
        .filter(move |record| record.package_record.name == *plugin)
}

/// Identifies a set of resolved packages by their contents.
///
/// The plugin archive's own hash is not enough: what a plugin reports depends on
/// the packages it runs with, so a dependency update has to change the identity.
/// The input is sorted, so the same environment hashes the same however the
/// solver happened to order it.
pub fn environment_sha256(records: &[RepoDataRecord]) -> Sha256Hash {
    let mut lines: Vec<String> = records
        .iter()
        .map(|record| {
            let package = &record.package_record;
            // The archive hash pins the exact build; the name and version keep
            // the line readable and distinct when a hash is missing.
            let archive = package
                .sha256
                .map(hex::encode)
                .or_else(|| package.md5.map(hex::encode))
                .unwrap_or_else(|| record.url.to_string());
            format!(
                "{}\t{}\t{}\t{archive}",
                package.name.as_normalized(),
                package.version,
                package.build
            )
        })
        .collect();
    lines.sort_unstable();
    compute_bytes_digest::<Sha256>(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::{
        PackageRecord,
        package::{ArchiveIdentifier, CondaArchiveType, DistArchiveIdentifier},
    };

    use super::*;

    fn record(name: &str, version: &str, sha: Option<u8>) -> RepoDataRecord {
        let parsed: rattler_conda_types::VersionWithSource = version.parse().unwrap();
        let mut package_record = PackageRecord::new(
            PackageName::new_unchecked(name),
            parsed,
            "h0000000_0".to_string(),
        );
        package_record.sha256 = sha.map(|byte| compute_bytes_digest::<Sha256>([byte]));
        RepoDataRecord {
            url: format!("https://example.com/{name}-{version}.conda")
                .parse()
                .unwrap(),
            channel: None,
            identifier: DistArchiveIdentifier::new(
                ArchiveIdentifier {
                    name: name.to_string(),
                    version: version.to_string(),
                    build_string: "h0000000_0".to_string(),
                },
                CondaArchiveType::Conda,
            ),
            package_record,
        }
    }

    #[test]
    fn the_same_packages_hash_the_same_in_any_order() {
        let forwards = [
            record("foobar-detect", "1.0.0", Some(1)),
            record("libc", "2.0", Some(2)),
        ];
        let backwards = [forwards[1].clone(), forwards[0].clone()];
        assert_eq!(
            environment_sha256(&forwards),
            environment_sha256(&backwards),
            "solver order must not change the identity"
        );
    }

    #[test]
    fn a_changed_dependency_changes_the_identity() {
        let plugin = record("foobar-detect", "1.0.0", Some(1));
        let before = [plugin.clone(), record("libfoo", "1.0", Some(2))];
        let upgraded = [plugin.clone(), record("libfoo", "1.1", Some(3))];
        let dropped = [plugin];

        assert_ne!(environment_sha256(&before), environment_sha256(&upgraded));
        assert_ne!(environment_sha256(&before), environment_sha256(&dropped));
    }

    #[test]
    fn a_rebuilt_package_changes_the_identity() {
        let one = [record("foobar-detect", "1.0.0", Some(1))];
        let other = [record("foobar-detect", "1.0.0", Some(2))];
        assert_ne!(environment_sha256(&one), environment_sha256(&other));
    }

    #[test]
    fn an_empty_environment_has_a_stable_identity() {
        assert_eq!(environment_sha256(&[]), environment_sha256(&[]));
    }
}
