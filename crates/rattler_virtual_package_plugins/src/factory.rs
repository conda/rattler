//! Producing a set of virtual packages, from whatever source provides them.
//!
//! A caller assembling the virtual packages for a solve has two kinds of source
//! to deal with: the ones this client detects itself, which CEP 30 obliges it to
//! offer, and the ones a channel's plugin reports. They behave differently --
//! one is a synchronous read of the running system, the other installs an
//! environment and starts a process -- but a caller should not have to care
//! which it is holding.
//!
//! [`VirtualPackageFactory`] is that common shape. It separates the cheap
//! question from the expensive one:
//!
//! - [`provides`](VirtualPackageFactory::provides) is the set of names this
//!   source speaks for. It costs nothing: no detection, no plugin run.
//! - [`resolve`](VirtualPackageFactory::resolve) is what is actually on this
//!   system, and may be slow.
//!
//! That split is the point of the abstraction. A caller can see what a factory
//! *would* answer for and skip resolving one whose names nothing needs, rather
//! than paying for every plugin a channel happens to register. In both
//! specializations `provides` is what the source claims and `resolve` is what
//! turned out to be there: names reported absent simply do not come back.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use rattler_cache::{
    package_cache::PackageCache, virtual_package_plugin_cache::VirtualPackagePluginCache,
};
use rattler_conda_types::{
    Channel, ChannelUrl, PackageName, Platform, SourcedVirtualPackage, VirtualPackageSource,
};
use rattler_repodata_gateway::Gateway;
use rattler_virtual_packages::{
    DetectVirtualPackageError, VirtualPackage, VirtualPackageOverrides,
};

use crate::{
    detect::{DetectError, DetectOptions, detect_virtual_packages},
    overrides::{self, Overridden, OverrideError, PluginOverrides},
    resolve::ResolvedPlugin,
    runner::RunTimeout,
};

/// A source of virtual packages.
#[async_trait]
pub trait VirtualPackageFactory {
    /// The virtual packages this source speaks for.
    ///
    /// Cheap by contract: a caller uses this to decide whether resolving is
    /// worth it, so an implementation must not detect anything here.
    fn provides(&self) -> &BTreeSet<PackageName>;

    /// What this source finds on the running system.
    ///
    /// Only names in [`provides`](Self::provides) can come back, and fewer of
    /// them: a name this source speaks for but does not find is absent rather
    /// than reported.
    async fn resolve(&self) -> Result<Vec<SourcedVirtualPackage>, FactoryError>;
}

/// A factory could not produce its virtual packages.
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    /// This system's own virtual packages could not be determined.
    #[error("failed to determine the virtual packages of this system")]
    BuiltIn(#[from] DetectVirtualPackageError),

    /// A channel's plugin could not be run, or did not honour its registration.
    ///
    /// Never fatal to a solve; see [`resolve_needed`]. Boxed because a detection
    /// failure carries the plugin's stderr and the chain of causes beneath it,
    /// which makes it far larger than the other variants.
    #[error(transparent)]
    Plugin(#[from] Box<PluginFailure>),

    /// A `CONDA_OVERRIDE_*` variable was set to something unusable.
    ///
    /// An error rather than a warning: the user asked for a specific value, and
    /// carrying on with the detected one would look like the override worked.
    #[error(transparent)]
    Override(#[from] Box<OverrideError>),
}

/// Which plugin failed, and why.
///
/// The plugin and its channel travel with the failure because by the time it is
/// caught the factory that produced it is gone, and a failure nobody can name is
/// one nobody can act on.
#[derive(Debug, thiserror::Error)]
#[error("the plugin '{}' of '{channel}' could not be run", plugin.as_source())]
pub struct PluginFailure {
    /// The channel that registered the plugin.
    pub channel: ChannelUrl,

    /// The package providing the plugin.
    pub plugin: PackageName,

    /// What went wrong.
    #[source]
    pub error: DetectError,
}

/// The virtual packages this client detects itself.
///
/// CEP 30 makes these an obligation of the client rather than of any channel, so
/// this factory answers whatever the channels are and its results carry no
/// channel. It is also the weakest source: a plugin claiming one of these names
/// overrides it, because the CEP requires the name to be *present* and does not
/// dictate that the client's own detection is what fills it.
pub struct BuiltinVirtualPackages {
    provides: BTreeSet<PackageName>,
    overrides: VirtualPackageOverrides,
    cache_dir: Option<PathBuf>,
}

impl BuiltinVirtualPackages {
    /// Detects with the `CONDA_OVERRIDE_*` variables this process was started
    /// with, which is what CEP 30 specifies for them.
    ///
    /// `cache_dir` is where [`rattler_virtual_packages`] may keep what these
    /// cost to detect -- asking a GPU driver about `__cuda` is measured in
    /// seconds on some machines, and this factory is resolved on the way into
    /// every solve. `None` detects them afresh each time.
    pub fn from_env(cache_dir: Option<&Path>) -> Self {
        Self::with_overrides(VirtualPackageOverrides::from_env(), cache_dir)
    }

    /// Detects with the given overrides.
    pub fn with_overrides(overrides: VirtualPackageOverrides, cache_dir: Option<&Path>) -> Self {
        Self {
            provides: STANDARDIZED_VIRTUAL_PACKAGES
                .iter()
                .map(|name| PackageName::new_unchecked(*name))
                .collect(),
            overrides,
            cache_dir: cache_dir.map(Path::to_path_buf),
        }
    }
}

/// Every virtual package this client knows how to look for.
///
/// Written out rather than derived because [`rattler_virtual_packages`] exposes
/// no enumeration: the names live in the `From<VirtualPackage> for
/// GenericVirtualPackage` impls. `standardized_names_stay_in_sync` guards the
/// drift, but only for names some platform detects by default -- ones that never
/// are (`__cuda`, `__cuda_arch`, and the non-glibc libc flavours) have to be
/// added here by hand.
///
/// A fixed list rather than the result of detecting is what keeps
/// [`provides`](VirtualPackageFactory::provides) honest about costing nothing.
/// It claims names this machine may turn out not to have, which is exactly what
/// `provides` means: `__cuda` is a name this client speaks for even where there
/// is no GPU to find.
pub const STANDARDIZED_VIRTUAL_PACKAGES: &[&str] = &[
    "__unix",
    "__linux",
    "__win",
    "__osx",
    "__ios",
    "__android",
    "__glibc",
    "__musl",
    "__eglibc",
    "__cuda",
    "__cuda_arch",
    "__archspec",
];

#[async_trait]
impl VirtualPackageFactory for BuiltinVirtualPackages {
    fn provides(&self) -> &BTreeSet<PackageName> {
        &self.provides
    }

    async fn resolve(&self) -> Result<Vec<SourcedVirtualPackage>, FactoryError> {
        Ok(
            VirtualPackage::detect(&self.overrides, self.cache_dir.as_deref())?
                .into_iter()
                .map(|package| SourcedVirtualPackage {
                    source: VirtualPackageSource::BuiltIn,
                    package: package.into(),
                })
                .collect(),
        )
    }
}

/// The virtual packages one channel's plugin detects.
///
/// One of these per plugin the channels resolved to, so the expensive work is
/// behind exactly the names that plugin won and a caller can skip it if nothing
/// needs them.
pub struct PluginVirtualPackages<'a> {
    resolved: &'a ResolvedPlugin,
    channel: &'a Channel,
    context: PluginContext<'a>,
}

/// What every plugin factory in one run shares: where to fetch from, where to
/// cache, and the bounds a plugin run is held to.
///
/// Separate from [`ResolvedPlugin`] because it is the same for every plugin in a
/// run, while the resolution differs per plugin.
#[derive(Clone, Copy)]
pub struct PluginContext<'a> {
    /// Where to read channel repodata from.
    pub gateway: &'a Gateway,

    /// The package cache a plugin's install draws from.
    pub package_cache: &'a PackageCache,

    /// Where detection results are kept between runs.
    pub detection_cache: &'a VirtualPackagePluginCache,

    /// Directory the per-plugin prefixes live under.
    pub environment_root: &'a Path,

    /// The platform to solve plugins for; detection is host-only.
    pub host_platform: Platform,

    /// How long a plugin may run before it is killed.
    pub timeout: RunTimeout,

    /// The current time in seconds since the Unix epoch, for cache expiry. One
    /// value for a whole run so every plugin agrees on what now is.
    pub now: i64,

    /// What the environment says a plugin's virtual packages are, standing in for
    /// running it. One snapshot for a whole run, for the same reason as `now`.
    pub overrides: &'a PluginOverrides,

    /// Where [`rattler_virtual_packages`] may keep what this machine's own
    /// virtual packages cost to detect, which preparing a plugin's environment
    /// needs. `None` detects them afresh.
    pub cache_dir: Option<&'a Path>,
}

impl<'a> PluginVirtualPackages<'a> {
    /// A factory for one plugin the channels resolved to.
    ///
    /// `channel` must be the [`Channel`] the resolution named; it is taken
    /// separately because resolution works in `ChannelUrl`s while fetching needs
    /// the full channel.
    pub fn new(
        resolved: &'a ResolvedPlugin,
        channel: &'a Channel,
        context: PluginContext<'a>,
    ) -> Self {
        debug_assert_eq!(
            channel.base_url, resolved.channel,
            "a plugin factory must be given the channel that registered it"
        );
        Self {
            resolved,
            channel,
            context,
        }
    }
}

#[async_trait]
impl VirtualPackageFactory for PluginVirtualPackages<'_> {
    fn provides(&self) -> &BTreeSet<PackageName> {
        // What the plugin *won*, not everything its channel registered it for.
        // A name a higher-priority channel already speaks for is not on offer
        // here, even though the plugin is still held to reporting it.
        &self.resolved.provides
    }

    async fn resolve(&self) -> Result<Vec<SourcedVirtualPackage>, FactoryError> {
        let overridden = self.overridden()?;

        // Every name this plugin is on offer for is spoken for by the
        // environment, so running it could not change the answer -- and running
        // it means solving an environment, installing it and starting a process.
        // Skipping that is most of the point of being able to override at all.
        if overridden.len() == self.resolved.provides.len() {
            tracing::debug!(
                "not running the plugin '{}': every virtual package it provides is overridden",
                self.resolved.plugin.as_source()
            );
            return Ok(self.present(overridden));
        }

        let detection = detect_virtual_packages(DetectOptions {
            gateway: self.context.gateway,
            package_cache: self.context.package_cache,
            detection_cache: self.context.detection_cache,
            channel: self.channel,
            plugin: &self.resolved.plugin,
            declared: &self.resolved.declared,
            environment_root: self.context.environment_root,
            host_platform: self.context.host_platform,
            timeout: self.context.timeout,
            now: self.context.now,
            cache_dir: self.context.cache_dir,
        })
        .await
        .map_err(|error| {
            Box::new(PluginFailure {
                channel: self.resolved.channel.clone(),
                plugin: self.resolved.plugin.clone(),
                error,
            })
        })?;

        // The plugin answers for everything its channel registered it for, but
        // only what it won is on offer. The rest is dropped here rather than
        // never asked for: the contract is between the plugin and its channel,
        // so it still had to give a verdict.
        //
        // An overridden name drops out too, whatever the plugin said about it:
        // the plugin ran because some *other* name needed it.
        let detected: Vec<_> = detection
            .virtual_packages
            .into_iter()
            .filter(|detected| self.resolved.provides.contains(&detected.package.name))
            .filter(|detected| !overridden.contains_key(&detected.package.name))
            .collect();

        Ok(detected
            .into_iter()
            .chain(self.present(overridden))
            .collect())
    }
}

impl PluginVirtualPackages<'_> {
    /// What the environment says about the names this plugin is on offer for.
    ///
    /// Absent from the map means the environment said nothing; present but
    /// [`Overridden::Absent`] means it said the name is not there, which is not
    /// the same thing and is why both are kept.
    fn overridden(&self) -> Result<BTreeMap<PackageName, Overridden>, FactoryError> {
        self.context
            .overrides
            .for_names(&self.resolved.provides)
            .map_err(Box::new)
            .map_err(FactoryError::Override)
    }

    /// The overrides that name a package, attributed to this plugin.
    fn present(&self, overridden: BTreeMap<PackageName, Overridden>) -> Vec<SourcedVirtualPackage> {
        overrides::sourced(overridden, &self.resolved.channel, &self.resolved.plugin)
    }
}

/// Resolves only the factories that could affect a solve, and combines what they
/// find with the built-ins.
///
/// `needed` is the set of virtual package names the solve could ask for, from
/// [`virtual_packages_mentioned`](crate::demand::virtual_packages_mentioned). A
/// factory whose [`provides`](VirtualPackageFactory::provides) does not
/// intersect it is never resolved: nothing in the solve can constrain on what it
/// speaks for, so what it would report cannot change the answer. That is the
/// whole point of `provides` being cheap.
///
/// The built-ins are resolved regardless. CEP 30 obliges the client to offer
/// them whether or not anything asks, they cost a synchronous read of this
/// machine rather than a plugin run, and skipping them would be the one saving
/// that changes what a solve is allowed to see.
///
/// Factories are resolved in the order given, which is CEP-42 channel priority
/// order.
///
/// **A plugin that fails does not fail this.** Its names are simply not in the
/// result, which leaves the solver to report a dependency it cannot satisfy in
/// its own words -- a better answer than aborting before the solve, since a
/// machine without the hardware and a broken plugin are indistinguishable from
/// here and neither is worth refusing to solve over. The failure is reported so
/// that "unsolvable" is not the only thing a user gets to see.
///
/// What does fail this is the caller's own doing: an unusable
/// `CONDA_OVERRIDE_*` is the user asking for something that cannot be given.
pub async fn resolve_needed(
    built_in: &BuiltinVirtualPackages,
    plugins: &[impl VirtualPackageFactory + Sync],
    needed: &BTreeSet<PackageName>,
) -> Result<Vec<SourcedVirtualPackage>, FactoryError> {
    let mut from_plugins = Vec::new();

    for factory in plugins {
        if factory.provides().is_disjoint(needed) {
            tracing::debug!(
                "not resolving a source for {:?}: nothing in this solve mentions any of them",
                factory
                    .provides()
                    .iter()
                    .map(PackageName::as_source)
                    .collect::<Vec<_>>()
            );
            continue;
        }
        match factory.resolve().await {
            Ok(resolved) => from_plugins.extend(resolved),
            Err(FactoryError::Plugin(failure)) => report(&failure),
            Err(error) => return Err(error),
        }
    }

    Ok(combine(&built_in.resolve().await?, from_plugins))
}

/// Says that a plugin failed, and everything known about why.
///
/// The chain of causes carries the useful half -- the outer message is "the
/// plugin could not be run" -- and a plugin that ran and complained has its own
/// account of it, which is usually more specific than anything this side can
/// say.
fn report(failure: &PluginFailure) {
    let mut causes = vec![failure.to_string()];
    let mut source = std::error::Error::source(failure);
    while let Some(cause) = source {
        let message = cause.to_string();
        // thiserror's `#[error(transparent)]` repeats the message it wraps.
        if causes.last() != Some(&message) {
            causes.push(message);
        }
        source = cause.source();
    }

    let stderr = failure.error.plugin_stderr().unwrap_or_default();
    tracing::error!(
        "{}: the virtual packages it speaks for are not available to this solve{}",
        causes.join(": "),
        if stderr.trim().is_empty() {
            String::new()
        } else {
            format!("\n{}", stderr.trim())
        }
    );
}

/// The virtual packages a solve is offered: everything the plugins found, plus
/// the built-ins none of them replaced.
///
/// **A plugin may change what a name means; it may not make the name go away.**
/// A built-in survives unless a plugin actually produced the same name, which is
/// not the same as a plugin having *claimed* it. A plugin registered for
/// `__archspec` that reports it absent has claimed the name and produced
/// nothing, and dropping the built-in there would leave the set without a name
/// CEP 30 says MUST always be present -- because a channel got its detection
/// wrong.
///
/// The rule holds for every built-in rather than only the always-present ones.
/// CEP 30 pins when each of its names must and must not appear (`__cuda` when
/// there are NVIDIA drivers, `__linux` on Linux, and so on), so a client that
/// detected one is already meeting the CEP; a plugin contradicting that is
/// asserting something the CEP does not let it assert.
pub fn combine(
    built_in: &[SourcedVirtualPackage],
    from_plugins: Vec<SourcedVirtualPackage>,
) -> Vec<SourcedVirtualPackage> {
    let produced: BTreeSet<_> = from_plugins
        .iter()
        .map(|detected| detected.package.name.clone())
        .collect();

    let (replaced, kept): (Vec<_>, Vec<_>) = built_in
        .iter()
        .cloned()
        .partition(|detected| produced.contains(&detected.package.name));

    if !replaced.is_empty() {
        tracing::debug!(
            "a plugin replaced the built-in {:?}",
            replaced
                .iter()
                .map(|detected| detected.package.name.as_source())
                .collect::<Vec<_>>()
        );
    }

    from_plugins.into_iter().chain(kept).collect()
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::Platform;
    use rattler_virtual_packages::VirtualPackages;

    use super::*;

    #[tokio::test]
    async fn the_built_ins_include_what_cep_30_always_requires() {
        let factory = BuiltinVirtualPackages::from_env(None);

        let archspec = PackageName::new_unchecked("__archspec");
        assert!(
            factory.provides().contains(&archspec),
            "CEP 30 requires __archspec to always be present, got {:?}",
            factory.provides()
        );

        let resolved = factory.resolve().await.unwrap();
        assert!(
            resolved
                .iter()
                .any(|detected| detected.package.name == archspec)
        );
    }

    #[tokio::test]
    async fn built_ins_are_not_attributed_to_a_channel() {
        let factory = BuiltinVirtualPackages::from_env(None);

        for detected in factory.resolve().await.unwrap() {
            assert!(
                detected.source.is_built_in(),
                "{:?} should be a built-in, got {:?}",
                detected.package.name,
                detected.source
            );
        }
    }

    #[tokio::test]
    async fn resolving_answers_only_for_what_it_promised() {
        let factory = BuiltinVirtualPackages::from_env(None);

        for detected in factory.resolve().await.unwrap() {
            assert!(
                factory.provides().contains(&detected.package.name),
                "{:?} was resolved but never promised",
                detected.package.name
            );
        }
    }

    #[test]
    fn provides_does_not_depend_on_the_machine() {
        let names: Vec<_> = BuiltinVirtualPackages::from_env(None)
            .provides()
            .iter()
            .map(|name| name.as_source().to_string())
            .collect();

        let mut expected: Vec<_> = STANDARDIZED_VIRTUAL_PACKAGES
            .iter()
            .map(ToString::to_string)
            .collect();
        expected.sort();
        assert_eq!(names, expected);
    }

    #[tokio::test]
    async fn a_plugin_cannot_delete_a_mandated_virtual_package() {
        let built_in = BuiltinVirtualPackages::from_env(None)
            .resolve()
            .await
            .unwrap();
        let archspec = PackageName::new_unchecked("__archspec");
        assert!(
            built_in
                .iter()
                .any(|detected| detected.package.name == archspec),
            "CEP 30 requires __archspec of every system"
        );

        // The plugin claimed __archspec and found nothing, so it produced
        // nothing: an empty result, not an entry saying absent.
        let combined = combine(&built_in, Vec::new());

        assert!(
            combined
                .iter()
                .any(|detected| detected.package.name == archspec),
            "__archspec disappeared because a plugin claimed it and found nothing"
        );
    }

    #[tokio::test]
    async fn a_plugin_may_replace_a_built_in_value() {
        let built_in = BuiltinVirtualPackages::from_env(None)
            .resolve()
            .await
            .unwrap();
        let archspec = PackageName::new_unchecked("__archspec");

        let from_plugin = SourcedVirtualPackage {
            source: VirtualPackageSource::BuiltIn,
            package: rattler_conda_types::GenericVirtualPackage {
                name: archspec.clone(),
                version: "1".parse().unwrap(),
                build_string: "from-a-plugin".to_string(),
            },
        };
        let combined = combine(&built_in, vec![from_plugin]);

        let found: Vec<_> = combined
            .iter()
            .filter(|detected| detected.package.name == archspec)
            .collect();
        assert_eq!(found.len(), 1, "the name must not be reported twice");
        assert_eq!(found[0].package.build_string, "from-a-plugin");
    }

    /// A factory that fails the test if anything resolves it. The saving is the
    /// whole point, so it has to be observable that the work did not happen --
    /// asserting on the output alone would pass even if the plugin had run.
    struct MustNotRun(BTreeSet<PackageName>);

    #[async_trait]
    impl VirtualPackageFactory for MustNotRun {
        fn provides(&self) -> &BTreeSet<PackageName> {
            &self.0
        }

        async fn resolve(&self) -> Result<Vec<SourcedVirtualPackage>, FactoryError> {
            panic!("resolved a source nothing in the solve mentions");
        }
    }

    fn speaking_for(names: &[&str]) -> MustNotRun {
        MustNotRun(
            names
                .iter()
                .map(|n| PackageName::new_unchecked(*n))
                .collect(),
        )
    }

    fn needing(names: &[&str]) -> BTreeSet<PackageName> {
        names
            .iter()
            .map(|n| PackageName::new_unchecked(*n))
            .collect()
    }

    #[tokio::test]
    async fn a_source_nothing_mentions_is_not_resolved() {
        let resolved = resolve_needed(
            &BuiltinVirtualPackages::from_env(None),
            &[speaking_for(&["__rocm"])],
            &needing(&["__cuda", "__glibc"]),
        )
        .await
        .expect("skipping a source cannot fail");

        assert!(
            resolved
                .iter()
                .all(|detected| detected.source.is_built_in()),
            "only the built-ins should be here"
        );
    }

    #[tokio::test]
    async fn one_mentioned_name_is_enough_to_resolve_a_source() {
        let factory = speaking_for(&["__rocm", "__oneapi"]);
        assert!(!factory.provides().is_disjoint(&needing(&["__oneapi"])));
    }

    #[tokio::test]
    async fn the_built_ins_are_resolved_even_when_unmentioned() {
        let resolved = resolve_needed(
            &BuiltinVirtualPackages::from_env(None),
            &[speaking_for(&["__rocm"])],
            &needing(&[]),
        )
        .await
        .unwrap();

        assert!(
            resolved
                .iter()
                .any(|d| d.package.name == PackageName::new_unchecked("__archspec")),
            "CEP 30 requires __archspec regardless of what the solve asks for"
        );
    }

    #[test]
    fn standardized_names_stay_in_sync() {
        let overrides = VirtualPackageOverrides::default();
        for platform in [
            Platform::Linux64,
            Platform::LinuxAarch64,
            Platform::Osx64,
            Platform::OsxArm64,
            Platform::Win64,
            Platform::EmscriptenWasm32,
        ] {
            let detected = VirtualPackages::detect_for_platform(platform, &overrides, None)
                .expect("detection for a known platform");
            for package in detected.into_generic_virtual_packages() {
                let name = package.name.as_source().to_string();
                assert!(
                    STANDARDIZED_VIRTUAL_PACKAGES.contains(&name.as_str()),
                    "{platform} detects {name}, which STANDARDIZED_VIRTUAL_PACKAGES omits"
                );
            }
        }
    }

    /// A factory that only fails, in one of the two ways that mean different
    /// things: a plugin that could not be run, and a user asking for an
    /// override that cannot be read.
    struct AlwaysFails {
        provides: BTreeSet<PackageName>,
        failure: Failure,
    }

    enum Failure {
        Plugin,
        Override,
    }

    #[async_trait]
    impl VirtualPackageFactory for AlwaysFails {
        fn provides(&self) -> &BTreeSet<PackageName> {
            &self.provides
        }

        async fn resolve(&self) -> Result<Vec<SourcedVirtualPackage>, FactoryError> {
            Err(match self.failure {
                Failure::Plugin => FactoryError::Plugin(Box::new(PluginFailure {
                    channel: url::Url::parse("https://prefix.dev/org/")
                        .expect("a valid url")
                        .into(),
                    plugin: PackageName::new_unchecked("rocm-detect"),
                    error: DetectError::PluginFailed {
                        exit_code: Some(1),
                        stderr: "no device".to_string(),
                    },
                })),
                Failure::Override => FactoryError::Override(Box::new(OverrideError {
                    variable: "CONDA_OVERRIDE_ROCM".to_string(),
                    source: "not a version"
                        .parse::<rattler_conda_types::Version>()
                        .expect_err("this is not a version"),
                })),
            })
        }
    }

    fn failing(failure: Failure) -> AlwaysFails {
        AlwaysFails {
            provides: needing(&["__rocm"]),
            failure,
        }
    }

    #[tokio::test]
    async fn a_plugin_that_fails_leaves_the_solve_to_the_solver() {
        let resolved = resolve_needed(
            &BuiltinVirtualPackages::from_env(None),
            &[failing(Failure::Plugin)],
            &needing(&["__rocm"]),
        )
        .await
        .expect("a broken plugin must not stop a solve");

        assert!(
            resolved
                .iter()
                .all(|detected| detected.source.is_built_in()),
            "nothing the failed plugin spoke for may be in the result"
        );
    }

    #[tokio::test]
    async fn an_unusable_override_still_stops_everything() {
        let error = resolve_needed(
            &BuiltinVirtualPackages::from_env(None),
            &[failing(Failure::Override)],
            &needing(&["__rocm"]),
        )
        .await
        .expect_err("an override that cannot be read must not be ignored");

        assert!(matches!(error, FactoryError::Override(_)), "got: {error:?}");
    }

    /// A plugin registered by a channel that does not exist, so reaching the
    /// gateway for it cannot succeed. Any test below that finishes therefore
    /// proves the plugin was never run.
    fn unreachable_plugin(provides: &[&str]) -> (ResolvedPlugin, Channel) {
        let channel = Channel::from_url(
            url::Url::parse("https://nothing.invalid/org/")
                .expect("a valid url")
                .clone(),
        );
        let resolved = ResolvedPlugin {
            channel: channel.base_url.clone(),
            plugin: PackageName::new_unchecked("foobar-detect"),
            declared: provides
                .iter()
                .map(|name| PackageName::new_unchecked(*name))
                .collect(),
            provides: provides
                .iter()
                .map(|name| PackageName::new_unchecked(*name))
                .collect(),
            shadowed_by: BTreeMap::default(),
        };
        (resolved, channel)
    }

    fn overrides(variables: &[(&str, &str)]) -> PluginOverrides {
        PluginOverrides::from_variables(
            variables
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
        )
    }

    /// Resolves `resolved` with `overrides`, through a context whose caches point
    /// into `scratch`. Running the plugin would need the channel to exist.
    async fn resolve_overridden(
        provides: &[&str],
        overrides: &PluginOverrides,
    ) -> Result<Vec<SourcedVirtualPackage>, FactoryError> {
        let scratch = tempfile::tempdir().expect("a temporary directory");
        let gateway = Gateway::new();
        let package_cache = PackageCache::new(scratch.path().join("packages"));
        let detection_cache = VirtualPackagePluginCache::new(scratch.path().join("detections"));
        let environment_root = scratch.path().join("envs");
        let (resolved, channel) = unreachable_plugin(provides);

        PluginVirtualPackages::new(
            &resolved,
            &channel,
            PluginContext {
                gateway: &gateway,
                package_cache: &package_cache,
                detection_cache: &detection_cache,
                environment_root: &environment_root,
                host_platform: Platform::current(),
                timeout: RunTimeout::default(),
                now: 1_000,
                overrides,
                cache_dir: None,
            },
        )
        .resolve()
        .await
    }

    #[tokio::test]
    async fn a_fully_overridden_plugin_is_not_run() {
        let resolved = resolve_overridden(
            &["__foobar", "__foobar_arch"],
            &overrides(&[
                ("CONDA_OVERRIDE_FOOBAR", "1.2.3"),
                ("CONDA_OVERRIDE_FOOBAR_ARCH", "0=gen4"),
            ]),
        )
        .await
        .expect("an overridden plugin resolves without being reachable");

        let reported: Vec<_> = resolved
            .iter()
            .map(|detected| {
                format!(
                    "{}={}={}",
                    detected.package.name.as_source(),
                    detected.package.version,
                    detected.package.build_string
                )
            })
            .collect();
        assert_eq!(reported, ["__foobar=1.2.3=0", "__foobar_arch=0=gen4"]);
    }

    #[tokio::test]
    async fn an_override_is_sourced_to_the_plugin_it_stands_in_for() {
        let resolved = resolve_overridden(
            &["__foobar"],
            &overrides(&[("CONDA_OVERRIDE_FOOBAR", "1.2.3")]),
        )
        .await
        .expect("an overridden plugin resolves");

        let source = &resolved.first().expect("one virtual package").source;
        assert!(
            matches!(
                source,
                VirtualPackageSource::Overridden { channel, plugin }
                    if channel.url().as_str() == "https://nothing.invalid/org/"
                        && plugin.as_source() == "foobar-detect"
            ),
            "expected an overridden source naming the plugin, got: {source:?}"
        );
        assert!(!source.is_built_in(), "an override is not a built-in");
    }

    #[tokio::test]
    async fn an_override_can_say_a_name_is_absent() {
        let resolved =
            resolve_overridden(&["__foobar"], &overrides(&[("CONDA_OVERRIDE_FOOBAR", "")]))
                .await
                .expect("an overridden plugin resolves");

        assert!(
            resolved.is_empty(),
            "an absent override reports nothing, got: {resolved:?}"
        );
    }

    #[tokio::test]
    async fn an_unusable_override_is_an_error() {
        let error = resolve_overridden(
            &["__foobar"],
            &overrides(&[("CONDA_OVERRIDE_FOOBAR", "not a version")]),
        )
        .await
        .expect_err("an unusable override must not be ignored");

        assert!(matches!(error, FactoryError::Override(_)), "got: {error:?}");
    }
}
