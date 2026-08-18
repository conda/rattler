//! Every virtual package a solve over a set of channels should see.
//!
//! The pieces to do this have existed separately -- resolution, factories,
//! overrides -- and every caller that wanted the whole answer had to assemble
//! them in the right order: work out who speaks for what, and only then detect.
//! Getting that order wrong is silent rather than loud, so it is done once here.
//!
//! What comes back is the built-ins together with the plugin verdicts of every
//! channel in scope, which is what a solver wants. Each name appears at most
//! once: channel priority has already decided which channel's plugin speaks for
//! a contested one by the time the values get here.

use std::{collections::BTreeSet, path::Path};

use rattler_conda_types::{Channel, PackageName, Platform, SourcedVirtualPackage};
use rattler_repodata_gateway::{Gateway, SubdirVirtualPackagePlugins};

use crate::{
    factory::{
        BuiltinVirtualPackages, FactoryError, PluginContext, PluginVirtualPackages, resolve_needed,
    },
    overrides::PluginOverrides,
    resolve::{ConflictingClaim, resolve_registrations},
    runner::RunTimeout,
};

/// What assembling the virtual packages for a solve needs.
pub struct AssembleOptions<'a> {
    /// Where to read channel repodata from.
    pub gateway: &'a Gateway,

    /// What the channels of the solve register, **in CEP-42 resolved channel
    /// order**, as a [`Gateway::query`] reports them or
    /// [`channel_registrations`](crate::resolve::channel_registrations)
    /// produces them. The order decides which channel speaks for a name two of
    /// them claim.
    pub registrations: &'a [SubdirVirtualPackagePlugins],

    /// The platform being solved for. Detection is host-only, so this is also
    /// the platform plugins are solved for.
    pub platform: Platform,

    /// The package cache a plugin's install draws from.
    pub package_cache: &'a rattler_cache::package_cache::PackageCache,

    /// Where detection results are kept between runs.
    pub detection_cache: &'a rattler_cache::virtual_package_plugin_cache::VirtualPackagePluginCache,

    /// Directory the per-plugin prefixes live under.
    pub environment_root: &'a Path,

    /// How long a plugin may run before it is killed.
    pub timeout: RunTimeout,

    /// The current time in seconds since the Unix epoch, for cache expiry.
    pub now: i64,

    /// What the environment says a virtual package is, standing in for detecting
    /// it.
    pub overrides: &'a PluginOverrides,

    /// Where [`rattler_virtual_packages`] may keep what this machine's own
    /// virtual packages cost to detect. `None` detects them afresh.
    pub cache_dir: Option<&'a Path>,

    /// The virtual package names the solve could ask for, from
    /// [`virtual_packages_mentioned`](crate::demand::virtual_packages_mentioned).
    /// A plugin speaking only for names outside this set is never run.
    pub needed: &'a BTreeSet<PackageName>,
}

/// Assembling the virtual packages for a solve failed.
///
/// Neither variant is a plugin that went wrong: a failed plugin is reported and
/// its names left out, so the solver reports what that costs rather than the
/// solve being refused before it starts. These are the two things a client
/// cannot proceed past -- metadata that contradicts itself, and a user asking
/// for an override that cannot be read.
#[derive(Debug, thiserror::Error)]
pub enum AssembleError {
    /// One channel registers two plugins for one virtual package.
    #[error(transparent)]
    Conflict(#[from] Box<ConflictingClaim>),

    /// A `CONDA_OVERRIDE_*` variable was set to something unusable, or this
    /// system's own virtual packages could not be determined.
    #[error(transparent)]
    Factory(#[from] Box<FactoryError>),
}

/// The virtual packages a solve over the registering channels should be given.
///
/// The built-ins are always included, since CEP 30 obliges a client to offer
/// them. A channel's plugin is run only if the solve mentions one of the names
/// it won, and not at all if the environment already answers for all of them.
pub async fn virtual_packages_for_solve(
    options: AssembleOptions<'_>,
) -> Result<Vec<SourcedVirtualPackage>, AssembleError> {
    let resolved = resolve_registrations(options.registrations.iter().cloned())?;

    let context = PluginContext {
        gateway: options.gateway,
        package_cache: options.package_cache,
        detection_cache: options.detection_cache,
        environment_root: options.environment_root,
        host_platform: options.platform,
        timeout: options.timeout,
        now: options.now,
        overrides: options.overrides,
        cache_dir: options.cache_dir,
    };

    for skipped in &resolved.shadowed {
        tracing::info!(
            "not running the plugin '{}' of '{}': {}",
            skipped.plugin.as_source(),
            skipped.channel,
            describe_shadowing(skipped)
        );
    }

    // One factory per plugin that won something. A plugin that lost all its
    // names is not here at all: it has nothing to say that a higher-priority
    // channel is not already saying.
    let channels: Vec<Channel> = resolved
        .plugins
        .iter()
        .map(|resolved| Channel::from_url(resolved.channel.clone()))
        .collect();
    let factories: Vec<_> = resolved
        .plugins
        .iter()
        .zip(&channels)
        .map(|(resolved, channel)| PluginVirtualPackages::new(resolved, channel, context))
        .collect();

    resolve_needed(
        &BuiltinVirtualPackages::from_env(options.cache_dir),
        &factories,
        options.needed,
    )
    .await
    .map_err(Box::new)
    .map_err(AssembleError::Factory)
}

/// Which channel took each name a registration lost, so a skipped plugin is
/// reported rather than silently omitted.
fn describe_shadowing(shadowed: &crate::resolve::ResolvedPlugin) -> String {
    shadowed
        .shadowed_by
        .iter()
        .map(|(name, winner)| format!("{} is provided by {winner}", name.as_source()))
        .collect::<Vec<_>>()
        .join(", ")
}
