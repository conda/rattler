//! Deciding which plugin speaks for a virtual package.
//!
//! A virtual package name means one thing per solve. A `MatchSpec` is matched
//! by name and carries nothing about the channel of the record that declared
//! it, so `depends: ["__rocm >=6.0"]` cannot ask for a particular channel's
//! `__rocm`: there is one candidate for the name, and every record in the solve
//! is matched against it. At most one plugin can therefore answer for a name,
//! and what is left is deciding which.
//!
//! **Between channels, priority decides.** Registrations arrive in the CEP-42
//! resolved channel order -- the order the channels' own relations and the
//! user's channel list put them in -- and the first claimant of a name wins.
//! The others are *shadowed* for that name: their verdict on it is discarded,
//! and a plugin shadowed for everything it claimed is not run at all.
//!
//! That is the right answer when two channels meant the same capability, and
//! the wrong one when they did not, which is a question no client can settle.
//! It is why a channel introducing a virtual package should build its own name
//! into it: `__acme_rocm` is a name nobody else contests.
//!
//! **Within one channel, nothing decides.** Two plugins in one channel claiming
//! one name is a channel contradicting itself, with no order to break the tie,
//! so it is an error rather than a guess.
//!
//! Built-ins are the weakest source of all: a plugin claiming a name the client
//! also detects overrides it. CEP 30 requires such a name to be *present* and
//! does not dictate that the client's own detection is what fills it.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

use indexmap::IndexMap;
use rattler_conda_types::{Channel, ChannelUrl, PackageName, Platform};
use rattler_repodata_gateway::{Gateway, GatewayError, SubdirVirtualPackagePlugins};
use regex::Regex;

/// What one channel registered, once its subdirs have been folded together.
type ChannelPlugins = IndexMap<PackageName, BTreeSet<PackageName>>;

/// What a channel registered, and how much of it survived the contest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPlugin {
    /// The channel that registered it.
    pub channel: ChannelUrl,

    /// The package providing it.
    pub plugin: PackageName,

    /// Everything its channel registered it for, across every subdir.
    ///
    /// The plugin is still held to all of it: the contract is between the
    /// plugin and its channel, and losing a name to a higher-priority channel
    /// does not excuse the plugin from giving a verdict on it.
    pub declared: BTreeSet<PackageName>,

    /// The subset of [`declared`](Self::declared) whose verdicts are used.
    pub provides: BTreeSet<PackageName>,

    /// For each name this plugin lost, the channel that speaks for it instead.
    /// Empty when the plugin won everything it claimed.
    pub shadowed_by: BTreeMap<PackageName, ChannelUrl>,
}

/// Which plugins a set of channels resolves to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedPlugins {
    /// The plugins to run, in channel-priority order and, within a channel, in
    /// the order the channel listed them. Each provides at least one virtual
    /// package.
    pub plugins: Vec<ResolvedPlugin>,

    /// Registrations that are not run at all, because a higher-priority channel
    /// already speaks for every name they claimed. Their `provides` is empty.
    ///
    /// Returned rather than dropped so a caller can say a registration was
    /// skipped and which channel took each of its names, instead of leaving a
    /// user to wonder where their plugin went.
    pub shadowed: Vec<ResolvedPlugin>,
}

/// A channel registered two different plugins for one virtual package.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "the channel '{channel}' registers both '{}' and '{}' for '{}', and nothing says which of \
     them speaks for it",
    first.as_source(),
    second.as_source(),
    virtual_package.as_source()
)]
pub struct ConflictingClaim {
    /// The channel that contradicts itself.
    pub channel: ChannelUrl,
    /// The virtual package claimed twice.
    pub virtual_package: PackageName,
    /// The plugin that claimed it first, in the channel's own order.
    pub first: PackageName,
    /// The plugin that claimed it again.
    pub second: PackageName,
}

/// Works out which plugins to run.
///
/// `registrations` is every registration the channels of a solve declared, **in
/// CEP-42 resolved channel order**: that order is what decides a name two
/// channels claim, so passing them in any other order silently changes which
/// plugin answers. A [`Gateway::query`](rattler_repodata_gateway::Gateway)
/// reports them that way, and [`channel_registrations`] produces them for a
/// caller that has no query to make.
///
/// Subdirs of one channel are folded together, since a channel repeats its
/// registration in every subdir and the same plugin appearing several times is
/// one plugin registered for the union of what those subdirs said.
pub fn resolve_registrations(
    registrations: impl IntoIterator<Item = SubdirVirtualPackagePlugins>,
) -> Result<ResolvedPlugins, Box<ConflictingClaim>> {
    let mut claimed: BTreeMap<PackageName, ChannelUrl> = BTreeMap::new();
    let mut resolved = ResolvedPlugins::default();

    for (channel, plugins) in fold_subdirs(registrations) {
        check_for_self_conflict(&channel, &plugins)?;

        for (plugin, declared) in plugins {
            let shadowed_by: BTreeMap<_, _> = declared
                .iter()
                .filter_map(|name| Some((name.clone(), claimed.get(name)?.clone())))
                .collect();
            let provides: BTreeSet<_> = declared
                .iter()
                .filter(|name| !shadowed_by.contains_key(*name))
                .cloned()
                .collect();

            for name in &provides {
                claimed.insert(name.clone(), channel.clone());
            }

            let plugin = ResolvedPlugin {
                channel: channel.clone(),
                plugin,
                declared,
                provides,
                shadowed_by,
            };
            if plugin.provides.is_empty() {
                resolved.shadowed.push(plugin);
            } else {
                resolved.plugins.push(plugin);
            }
        }
    }

    Ok(resolved)
}

/// What `channels` and the channels their CEP-42 relations reach register, in
/// the priority order those relations and the given order put them in.
///
/// For a caller that has no [`Gateway::query`] to take the registrations from,
/// because it has no specs to query for: listing what a channel registers needs
/// no packages. Relation problems are reported rather than fatal, as they are
/// for a query.
pub async fn channel_registrations(
    gateway: &Gateway,
    channels: impl IntoIterator<Item = Channel>,
    platforms: &[Platform],
) -> Result<Vec<SubdirVirtualPackagePlugins>, GatewayError> {
    let resolved = gateway
        .resolved_channels(channels, platforms.iter().copied())
        .await?;
    for warning in &resolved.warnings {
        tracing::warn!("{warning}");
    }

    let mut registrations = Vec::new();
    for channel in &resolved.channels {
        for platform in platforms {
            let plugins = gateway.virtual_package_plugins(channel, *platform).await?;
            if !plugins.is_empty() {
                registrations.push(SubdirVirtualPackagePlugins {
                    channel: channel.base_url.clone(),
                    platform: *platform,
                    plugins,
                });
            }
        }
    }

    Ok(registrations)
}

/// Rejects a channel that registered two different plugins for one virtual
/// package, before any of its plugins is run.
fn check_for_self_conflict(
    channel: &ChannelUrl,
    plugins: &ChannelPlugins,
) -> Result<(), Box<ConflictingClaim>> {
    let mut claimed_here: BTreeMap<&PackageName, &PackageName> = BTreeMap::new();
    for (plugin, declared) in plugins {
        for virtual_package in declared {
            if let Some(first) = claimed_here.insert(virtual_package, plugin) {
                return Err(Box::new(ConflictingClaim {
                    channel: channel.clone(),
                    virtual_package: virtual_package.clone(),
                    first: first.clone(),
                    second: plugin.clone(),
                }));
            }
        }
    }
    Ok(())
}

/// Whether `name` is a virtual package name CEP 26 allows.
///
/// The pattern is the CEP's own, quoted rather than reimplemented: getting a
/// character class subtly wrong here would either reject legitimate names or
/// admit ones the rest of the ecosystem will refuse.
///
/// Registrations reach this from channel metadata, so they are parsed leniently
/// -- one malformed entry must not make a whole `repodata.json` unusable -- but
/// parsing leniently is not the same as acting on the result. A name the CEP
/// forbids is dropped here rather than carried into a solve, where it would
/// fail later as an unusable package spec.
fn is_valid_virtual_package_name(name: &PackageName) -> bool {
    /// CEP 26: "Virtual package names MUST follow this regex."
    static VALID: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^__[a-z0-9][._-]?([a-z0-9]+(\.|-|_|$))*$")
            .expect("the pattern is a literal from CEP 26")
    });
    /// CEP 26: "the maximum length of a package name MUST NOT exceed 64
    /// characters."
    const MAX_LENGTH: usize = 64;

    let name = name.as_normalized();
    name.len() <= MAX_LENGTH && VALID.is_match(name)
}

/// The most virtual packages one plugin may register for.
///
/// The registration is what bounds how much a plugin gets to say: the contract
/// makes it answer for every name it registered, and the output a run may
/// produce is budgeted per name. A cap is what keeps that from being the
/// channel's to choose.
///
/// Sixteen is far above what a real detector needs -- the largest use this was
/// designed around, a vendor stack reporting a runtime and its capabilities,
/// wants a handful -- and far below the point where one registration could
/// drown a client.
pub const MAX_VIRTUAL_PACKAGES_PER_PLUGIN: usize = 16;

/// One entry per channel, in the order the channels first appear, each mapping
/// a plugin to everything that channel's subdirs registered it for.
///
/// A registration for more names than [`MAX_VIRTUAL_PACKAGES_PER_PLUGIN`]
/// invalidates the channel's whole registration section: the channel contradicted
/// the protocol, so there is no established set to act on.
fn fold_subdirs(
    registrations: impl IntoIterator<Item = SubdirVirtualPackagePlugins>,
) -> IndexMap<ChannelUrl, ChannelPlugins> {
    let mut channels: IndexMap<ChannelUrl, ChannelPlugins> = IndexMap::new();

    for subdir in registrations {
        let channel = subdir.channel.clone();
        let plugins = channels.entry(subdir.channel).or_default();
        for (plugin, declared) in subdir.plugins {
            let (valid, rejected): (Vec<_>, Vec<_>) = declared
                .into_iter()
                .partition(is_valid_virtual_package_name);
            for name in rejected {
                tracing::warn!(
                    "ignoring '{}', which '{}' registers '{}' for: CEP 26 does not allow it as a \
                     virtual package name",
                    name.as_source(),
                    channel,
                    plugin.as_source()
                );
            }
            plugins.entry(plugin).or_default().extend(valid);
        }
    }

    // Once every subdir has had its say, since the registrations of one
    // channel's subdirs are one registration for their union.
    channels.retain(|channel, plugins| {
        let Some((plugin, declared)) = plugins
            .iter()
            .find(|(_, declared)| declared.len() > MAX_VIRTUAL_PACKAGES_PER_PLUGIN)
        else {
            return true;
        };

        tracing::warn!(
            "ignoring every virtual package plugin registration by '{channel}': '{}' names {} \
             virtual packages, and no plugin may register for more than \
             {MAX_VIRTUAL_PACKAGES_PER_PLUGIN}",
            plugin.as_source(),
            declared.len(),
        );
        false
    });

    channels
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::Platform;

    use super::*;

    fn channel(name: &str) -> ChannelUrl {
        url::Url::parse(&format!("https://prefix.dev/{name}/"))
            .expect("a valid channel url")
            .into()
    }

    fn name(name: &str) -> PackageName {
        PackageName::new_unchecked(name)
    }

    /// One subdir's registration: `(plugin, [virtual packages])` pairs.
    fn subdir(
        channel_name: &str,
        platform: Platform,
        plugins: &[(&str, &[&str])],
    ) -> SubdirVirtualPackagePlugins {
        SubdirVirtualPackagePlugins {
            channel: channel(channel_name),
            platform,
            plugins: plugins
                .iter()
                .map(|(plugin, provides)| {
                    (name(plugin), provides.iter().map(|p| name(p)).collect())
                })
                .collect(),
        }
    }

    fn provides(resolved: &ResolvedPlugin) -> Vec<&str> {
        resolved
            .provides
            .iter()
            .map(PackageName::as_source)
            .collect()
    }

    #[test]
    fn a_single_uncontested_plugin_is_resolved() {
        let resolved = resolve_registrations([subdir(
            "org",
            Platform::Linux64,
            &[("rocm-detect", &["__rocm"])],
        )])
        .unwrap();

        assert_eq!(resolved.plugins.len(), 1);
        assert_eq!(provides(&resolved.plugins[0]), ["__rocm"]);
        assert!(resolved.plugins[0].shadowed_by.is_empty());
        assert!(resolved.shadowed.is_empty());
    }

    #[test]
    fn the_higher_priority_channel_speaks_for_a_contested_name() {
        let resolved = resolve_registrations([
            subdir("first", Platform::Linux64, &[("a-detect", &["__rocm"])]),
            subdir("second", Platform::Linux64, &[("b-detect", &["__rocm"])]),
        ])
        .unwrap();

        assert_eq!(resolved.plugins.len(), 1, "the loser must not run");
        assert_eq!(resolved.plugins[0].channel, channel("first"));

        assert_eq!(resolved.shadowed.len(), 1);
        assert_eq!(resolved.shadowed[0].channel, channel("second"));
        assert_eq!(
            resolved.shadowed[0].shadowed_by.get(&name("__rocm")),
            Some(&channel("first")),
            "a shadowed registration has to say who took it"
        );
    }

    #[test]
    fn channels_claiming_different_names_all_answer() {
        let resolved = resolve_registrations([
            subdir("first", Platform::Linux64, &[("a-detect", &["__rocm"])]),
            subdir("second", Platform::Linux64, &[("b-detect", &["__oneapi"])]),
        ])
        .unwrap();

        assert_eq!(resolved.plugins.len(), 2);
        assert!(resolved.shadowed.is_empty(), "there is no contest to lose");
    }

    #[test]
    fn a_partially_shadowed_plugin_still_runs_for_what_it_wins() {
        let resolved = resolve_registrations([
            subdir("first", Platform::Linux64, &[("a-detect", &["__rocm"])]),
            subdir(
                "second",
                Platform::Linux64,
                &[("b-detect", &["__rocm", "__oneapi"])],
            ),
        ])
        .unwrap();

        assert_eq!(resolved.plugins.len(), 2);
        assert!(resolved.shadowed.is_empty(), "it still runs");

        let partial = &resolved.plugins[1];
        assert_eq!(provides(partial), ["__oneapi"]);
        assert_eq!(
            partial.shadowed_by.keys().collect::<Vec<_>>(),
            [&name("__rocm")]
        );
        assert!(
            partial.declared.contains(&name("__rocm")),
            "the contract still covers the name it lost"
        );
    }

    #[test]
    fn subdirs_of_one_channel_are_folded_rather_than_compared() {
        let resolved = resolve_registrations([
            subdir("org", Platform::Linux64, &[("d-detect", &["__rocm"])]),
            subdir("org", Platform::NoArch, &[("d-detect", &["__oneapi"])]),
        ])
        .unwrap();

        assert_eq!(resolved.plugins.len(), 1, "one plugin, not two");
        assert_eq!(
            provides(&resolved.plugins[0]),
            ["__oneapi", "__rocm"],
            "what the subdirs registered is unioned"
        );
    }

    #[test]
    fn two_plugins_in_one_channel_claiming_one_name_is_an_error() {
        let error = resolve_registrations([subdir(
            "org",
            Platform::Linux64,
            &[("a-detect", &["__rocm"]), ("b-detect", &["__rocm"])],
        )])
        .unwrap_err();

        assert_eq!(error.virtual_package, name("__rocm"));
        assert_eq!(error.first, name("a-detect"));
        assert_eq!(error.second, name("b-detect"));
    }

    #[test]
    fn a_name_cep_26_forbids_is_not_resolved() {
        let resolved = resolve_registrations([subdir(
            "org",
            Platform::Linux64,
            &[("d-detect", &["__rocm", "no-underscores", "__UPPER", "__"])],
        )])
        .unwrap();

        assert_eq!(
            provides(&resolved.plugins[0]),
            ["__rocm"],
            "only the legal name survives"
        );
    }

    #[test]
    fn one_bad_name_does_not_spoil_a_channel() {
        let resolved = resolve_registrations([subdir(
            "org",
            Platform::Linux64,
            &[("bad-detect", &["nonsense"]), ("good-detect", &["__rocm"])],
        )])
        .unwrap();

        assert_eq!(resolved.plugins.len(), 1);
        assert_eq!(resolved.plugins[0].plugin.as_source(), "good-detect");
    }

    #[test]
    fn an_over_limit_registration_drops_the_channel_section() {
        let names: Vec<String> = (0..=MAX_VIRTUAL_PACKAGES_PER_PLUGIN)
            .map(|index| format!("__too_many{index}"))
            .collect();
        let too_many: Vec<&str> = names.iter().map(String::as_str).collect();

        let resolved = resolve_registrations([subdir(
            "org",
            Platform::Linux64,
            &[
                ("greedy-detect", &too_many),
                (
                    "modest-detect",
                    &too_many[..MAX_VIRTUAL_PACKAGES_PER_PLUGIN],
                ),
            ],
        )])
        .unwrap();

        assert!(
            resolved.plugins.is_empty(),
            "one erroneous registration invalidates the channel's whole section"
        );
        assert!(
            resolved.shadowed.is_empty(),
            "a dropped section is not a shadowed one"
        );
    }

    #[test]
    fn the_cap_is_reached_across_subdirs() {
        let names: Vec<String> = (0..=MAX_VIRTUAL_PACKAGES_PER_PLUGIN)
            .map(|index| format!("__too_many{index}"))
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let (first, rest) = names.split_at(MAX_VIRTUAL_PACKAGES_PER_PLUGIN);

        let resolved = resolve_registrations([
            subdir("org", Platform::Linux64, &[("greedy-detect", first)]),
            subdir("org", Platform::NoArch, &[("greedy-detect", rest)]),
        ])
        .unwrap();

        assert!(
            resolved.plugins.is_empty(),
            "neither subdir passes the cap on its own, but together they do"
        );
    }

    #[test]
    fn an_overlong_name_is_rejected() {
        let long = format!("__{}", "a".repeat(63));
        assert_eq!(long.len(), 65);
        assert!(!is_valid_virtual_package_name(&name(&long)));
        assert!(is_valid_virtual_package_name(&name(&long[..64])));
    }

    #[test]
    fn registering_nothing_resolves_to_nothing() {
        assert_eq!(
            resolve_registrations([]).unwrap(),
            ResolvedPlugins::default()
        );

        let resolved = resolve_registrations([subdir("org", Platform::Linux64, &[])]).unwrap();
        assert!(resolved.plugins.is_empty());
        assert!(resolved.shadowed.is_empty());
    }
}
