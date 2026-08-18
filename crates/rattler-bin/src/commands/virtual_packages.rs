use miette::IntoDiagnostic;
#[cfg(not(feature = "experimental-virtual-package-plugins"))]
use rattler_conda_types::GenericVirtualPackage;
#[cfg(feature = "experimental-virtual-package-plugins")]
use rattler_conda_types::{Channel, ChannelConfig, PackageName, Platform};
#[cfg(feature = "experimental-virtual-package-plugins")]
use rattler_repodata_gateway::Gateway;
/// The names every client detects itself. Owned by the plugins crate, which
/// needs the same list to say what its built-in factory speaks for.
#[cfg(feature = "experimental-virtual-package-plugins")]
use rattler_virtual_package_plugins::{
    ResolvedPlugin, STANDARDIZED_VIRTUAL_PACKAGES, channel_registrations, resolve_registrations,
};

/// Print detected virtual packages.
#[derive(Debug, clap::Parser)]
#[cfg_attr(
    feature = "experimental-virtual-package-plugins",
    clap(after_help = r#"Examples:
  rattler virtual-packages
  rattler virtual-packages -c ./test-data/channels/virtual-package-plugins
  rattler virtual-packages -c ./test-data/channels/virtual-package-plugins-derived
  rattler virtual-packages -c ./test-data/channels/virtual-package-plugins --detect"#)
)]
pub struct Opt {
    /// Channels to list registered virtual package plugins for
    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[clap(short, long)]
    channels: Vec<String>,

    /// Platforms to read registrations for [default: current and noarch]
    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[clap(short, long)]
    platforms: Vec<Platform>,

    /// Run each registered plugin and report the virtual packages it detects
    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[clap(long)]
    detect: bool,

    /// Seconds a plugin may run before it is killed [default: 5, maximum: 60]
    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[clap(long, requires = "detect")]
    plugin_timeout: Option<u64>,

    /// Report how long each stage of detection took
    #[cfg(feature = "experimental-virtual-package-plugins")]
    #[clap(long, requires = "detect")]
    timings: bool,
}

pub async fn virtual_packages(opt: Opt, offline: bool) -> miette::Result<()> {
    #[cfg(feature = "experimental-virtual-package-plugins")]
    {
        use rattler_virtual_package_plugins::{BuiltinVirtualPackages, VirtualPackageFactory};

        // Detected once and passed on: the same set is printed here and offered
        // to the solve, and detecting can mean a driver query -- which is what
        // the cache directory is for. The factory is the only thing that reads
        // `CONDA_OVERRIDE_*` for these.
        let cache_dir = rattler::default_cache_dir().ok();
        let built_in = BuiltinVirtualPackages::from_env(cache_dir.as_deref())
            .resolve()
            .await
            .map_err(|err| miette::miette!(err))?;
        for detected in &built_in {
            println!("{}", detected.package);
        }

        if opt.detect {
            let timeout = opt.plugin_timeout.map_or_else(
                rattler_virtual_package_plugins::RunTimeout::default,
                |seconds| {
                    rattler_virtual_package_plugins::RunTimeout::new(
                        std::time::Duration::from_secs(seconds),
                    )
                },
            );
            detect_plugins(
                &opt.channels,
                &opt.platforms,
                offline,
                timeout,
                opt.timings,
                &built_in,
            )
            .await?;
        } else {
            print_plugins(&opt.channels, &opt.platforms, offline, cache_dir.as_deref()).await?;
        }
    }

    #[cfg(not(feature = "experimental-virtual-package-plugins"))]
    {
        let _ = (opt, offline);

        let cache_dir = rattler::default_cache_dir().ok();
        tracing::debug!(
            cache_dir = %cache_dir
                .as_ref()
                .map_or_else(|| "<disabled>".to_string(), |path| path.display().to_string()),
            "detecting virtual packages"
        );

        let virtual_packages = rattler_virtual_packages::VirtualPackage::detect(
            &rattler_virtual_packages::VirtualPackageOverrides::from_env(),
            cache_dir.as_deref(),
        )
        .into_diagnostic()?;

        let generic_virtual_packages = virtual_packages
            .into_iter()
            .map(GenericVirtualPackage::from)
            .collect::<Vec<_>>();
        let package_strings = generic_virtual_packages
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        tracing::debug!(
            count = package_strings.len(),
            packages = ?package_strings,
            "detected virtual packages"
        );

        for package in generic_virtual_packages {
            println!("{package}");
        }
    }

    Ok(())
}

/// A gateway configured for reading plugin registrations: no sharded repodata
/// preference, and the caller's offline setting respected.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn plugin_gateway(offline: bool, cache_dir: Option<&std::path::Path>) -> miette::Result<Gateway> {
    use std::collections::HashMap;

    use rattler_repodata_gateway::SourceConfig;

    let mut builder = Gateway::builder()
        .with_client(super::client::create_client_with_middleware(offline)?)
        .with_channel_config(rattler_repodata_gateway::ChannelConfig {
            default: SourceConfig {
                cache_action: super::client::repodata_cache_action(offline),
                ..SourceConfig::default()
            },
            per_channel: HashMap::new(),
        });
    if let Some(cache_dir) = cache_dir {
        builder = builder.with_cache_dir(cache_dir.join(rattler_cache::REPODATA_CACHE_DIR));
    }
    Ok(builder.finish())
}

/// The channels as the user wrote them, resolved against the current directory.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn parse_channels(channels: &[String]) -> miette::Result<Vec<Channel>> {
    let channel_config =
        ChannelConfig::default_with_root_dir(std::env::current_dir().into_diagnostic()?);
    channels
        .iter()
        .map(|channel| Channel::from_str(channel, &channel_config))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()
}

/// The platforms to read registrations for. Detection inspects the running
/// machine, so the host platform and `noarch` are what a channel declares for.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn platforms_or_default(platforms: &[Platform]) -> Vec<Platform> {
    if platforms.is_empty() {
        vec![Platform::current(), Platform::NoArch]
    } else {
        platforms.to_vec()
    }
}

/// Prints which plugin speaks for which virtual package, once the channels'
/// priority order has settled every name two of them claim.
#[cfg(feature = "experimental-virtual-package-plugins")]
async fn print_plugins(
    channels: &[String],
    platforms: &[Platform],
    offline: bool,
    cache_dir: Option<&std::path::Path>,
) -> miette::Result<()> {
    if channels.is_empty() {
        return Ok(());
    }

    let gateway = plugin_gateway(offline, cache_dir)?;
    let registrations = channel_registrations(
        &gateway,
        parse_channels(channels)?,
        &platforms_or_default(platforms),
    )
    .await
    .into_diagnostic()?;
    let resolved = resolve_registrations(registrations).map_err(|err| miette::miette!(err))?;

    for plugin in resolved.plugins.iter().chain(&resolved.shadowed) {
        print_registration(plugin);
    }

    Ok(())
}

/// Prints one registration: the names it speaks for, and what it lost.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn print_registration(resolved: &ResolvedPlugin) {
    println!(
        "\n{}{} {}{}",
        console::Emoji("🔌 ", ""),
        console::style(resolved.plugin.as_source()).bold(),
        console::style(&resolved.channel).dim(),
        if resolved.provides.is_empty() {
            console::style(" (not run)").dim().to_string()
        } else {
            String::new()
        },
    );

    for virtual_package in &resolved.provides {
        println!(
            "  {} {}",
            console::style(console::Emoji("•", "-")).cyan(),
            console::style(virtual_package.as_source()).bold(),
        );
    }

    for warning in warnings_for(resolved) {
        println!(
            "  {} {}",
            console::style(console::Emoji("⚠", "!")).yellow(),
            console::style(warning).yellow(),
        );
    }
}

/// What is worth saying about a registration beyond the names it won: a name
/// this client detects itself, and a name a higher-priority channel took.
///
/// Both are reported rather than acted on. Overriding a built-in is what a
/// channel that knows better about a capability is for, and a shadowed name is
/// settled -- but a user whose plugin does not run should be told why.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn warnings_for(resolved: &ResolvedPlugin) -> Vec<String> {
    resolved
        .provides
        .iter()
        .filter(|name| STANDARDIZED_VIRTUAL_PACKAGES.contains(&name.as_source()))
        .map(|name| {
            format!(
                "{} is a standardized virtual package this client detects itself; the plugin's \
                 value replaces it",
                name.as_source()
            )
        })
        .chain(
            resolved
                .shadowed_by
                .iter()
                .map(|(name, winner)| format!("{} is provided by {winner}", name.as_source())),
        )
        .collect()
}

#[cfg(all(test, feature = "experimental-virtual-package-plugins"))]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    fn resolved(provides: &[&str], shadowed: &[&str]) -> ResolvedPlugin {
        let name = |name: &str| PackageName::new_unchecked(name);
        ResolvedPlugin {
            channel: url::Url::parse("https://prefix.dev/derived/")
                .expect("a valid channel url")
                .into(),
            plugin: name("vendor-detect"),
            declared: provides.iter().chain(shadowed).map(|n| name(n)).collect(),
            provides: provides.iter().map(|n| name(n)).collect::<BTreeSet<_>>(),
            shadowed_by: shadowed
                .iter()
                .map(|n| {
                    (
                        name(n),
                        url::Url::parse("https://prefix.dev/base/")
                            .expect("a valid channel url")
                            .into(),
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        }
    }

    /// A name nobody else speaks for and that no client detects needs no
    /// commentary at all.
    #[test]
    fn an_uncontested_name_warns_about_nothing() {
        assert!(warnings_for(&resolved(&["__rocm"], &[])).is_empty());
    }

    /// The two things a user cannot see from the list of names alone: that a
    /// plugin is taking over a name the client fills, and that another channel
    /// took one it claimed.
    #[test]
    fn replacing_a_built_in_and_losing_a_name_are_both_reported() {
        let warnings = warnings_for(&resolved(&["__glibc"], &["__cuda"]));
        assert_eq!(warnings.len(), 2, "got {warnings:#?}");
        assert!(warnings[0].contains("__glibc"), "{}", warnings[0]);
        assert!(
            warnings[1].contains("__cuda") && warnings[1].contains("base"),
            "{}",
            warnings[1]
        );
    }
}

/// Runs the plugins the given channels register and reports what each detected.
///
/// Where two channels claim one virtual package, the higher-priority channel
/// wins and the other plugin is reported as shadowed rather than run. Channels
/// are in the CEP-42 order their relations and the command line put them in.
///
/// A plugin that fails is reported and skipped rather than aborting the run: one
/// broken plugin should not hide what the others found, and a system without the
/// hardware is indistinguishable from a broken plugin at this level.
#[cfg(feature = "experimental-virtual-package-plugins")]
async fn detect_plugins(
    channels: &[String],
    platforms: &[Platform],
    offline: bool,
    timeout: rattler_virtual_package_plugins::RunTimeout,
    show_timings: bool,
    built_in: &[rattler_conda_types::SourcedVirtualPackage],
) -> miette::Result<()> {
    use rattler_cache::{
        default_cache_dir, package_cache::PackageCache,
        virtual_package_plugin_cache::VirtualPackagePluginCache,
    };
    use rattler_virtual_package_plugins::{
        DetectOptions, PluginContext, PluginOverrides, combine, detect_virtual_packages, overrides,
    };

    if channels.is_empty() {
        return Ok(());
    }

    // Detection inspects this machine, so only the host platform is meaningful.
    let platform = match platforms {
        [] => Platform::current(),
        [platform] => *platform,
        _ => miette::bail!("--detect works on one platform at a time"),
    };

    let cache_dir = default_cache_dir()
        .map_err(|e| miette::miette!("could not determine cache directory: {e}"))?;
    rattler_cache::ensure_cache_dir(&cache_dir)
        .map_err(|e| miette::miette!("could not create cache directory: {e}"))?;
    let gateway = plugin_gateway(offline, Some(&cache_dir))?;
    let package_cache = PackageCache::new(cache_dir.join(rattler_cache::PACKAGE_CACHE_DIR));
    let detection_cache = VirtualPackagePluginCache::new(
        cache_dir.join(rattler_cache::VIRTUAL_PACKAGE_PLUGINS_CACHE_DIR),
    );
    let environment_root = cache_dir.join(rattler_cache::EXEC_ENVS_DIR).join("plugins");
    // One timestamp for the whole run, so every plugin agrees on what now is.
    let now = jiff::Timestamp::now().as_second();

    let registrations = channel_registrations(
        &gateway,
        parse_channels(channels)?,
        &[platform, Platform::NoArch],
    )
    .await
    .into_diagnostic()?;
    let resolved = resolve_registrations(registrations).map_err(|err| miette::miette!(err))?;

    let overrides = PluginOverrides::from_env();
    let context = PluginContext {
        gateway: &gateway,
        package_cache: &package_cache,
        detection_cache: &detection_cache,
        environment_root: &environment_root,
        host_platform: platform,
        timeout,
        now,
        overrides: &overrides,
        cache_dir: Some(&cache_dir),
    };

    // Which built-ins survive depends on what the plugins produced, not on what
    // they claimed, so this is collected as they run.
    let mut produced: Vec<rattler_conda_types::SourcedVirtualPackage> = Vec::new();
    // The plugins come in channel order, so the header changes when the channel
    // does rather than being repeated on every line.
    let mut reported_channel = None;

    for resolved in resolved.plugins.iter().chain(&resolved.shadowed) {
        if reported_channel != Some(&resolved.channel) {
            reported_channel = Some(&resolved.channel);
            println!(
                "\n{}{} {}",
                console::Emoji("🔌 ", ""),
                console::style(&resolved.channel).bold(),
                console::style(format!("[{platform}]")).dim(),
            );
        }

        if resolved.provides.is_empty() {
            report_shadowed(resolved);
            continue;
        }

        let channel = Channel::from_url(resolved.channel.clone());

        // An override stands in for the plugin's verdict. When it covers every
        // name the plugin is on offer for, running it -- solving an
        // environment, installing it, starting a process -- cannot change the
        // answer.
        let overridden = context
            .overrides
            .for_names(&resolved.provides)
            .map_err(|err| miette::miette!(err))?;
        if overridden.len() == resolved.provides.len() {
            let stood_in_for = overrides::sourced(overridden, &resolved.channel, &resolved.plugin);
            report_overridden(resolved, &stood_in_for);
            produced.extend(stood_in_for);
            continue;
        }

        let detection = detect_virtual_packages(DetectOptions {
            gateway: context.gateway,
            package_cache: context.package_cache,
            detection_cache: context.detection_cache,
            channel: &channel,
            plugin: &resolved.plugin,
            declared: &resolved.declared,
            environment_root: context.environment_root,
            host_platform: context.host_platform,
            timeout: context.timeout,
            now: context.now,
            cache_dir: context.cache_dir,
        })
        .await;

        match detection {
            Ok(detection) => {
                produced.extend(
                    detection
                        .virtual_packages
                        .iter()
                        .filter(|detected| resolved.provides.contains(&detected.package.name))
                        .filter(|detected| !overridden.contains_key(&detected.package.name))
                        .cloned(),
                );
                report_detection(resolved, &detection, &overridden, show_timings);
                produced.extend(overrides::sourced(
                    overridden,
                    &resolved.channel,
                    &resolved.plugin,
                ));
            }
            Err(err) => {
                println!(
                    "  {} {} {}",
                    console::style(console::Emoji("✖", "x")).red(),
                    console::style(resolved.plugin.as_source()).bold(),
                    console::style("(skipped)").dim(),
                );
                for line in explain(&err) {
                    println!("      {}", console::style(line).red());
                }
                // The plugin's own account of what went wrong, which is usually
                // more specific than anything this side can say.
                for line in err.plugin_stderr().into_iter().flat_map(str::lines) {
                    println!("      {}", console::style(line).dim());
                }
            }
        }
    }

    // `combine` is what keeps a name CEP 30 mandates from vanishing when a
    // plugin claims it and comes back empty. Which built-ins survive depends on
    // what the plugins produced rather than on what they claimed, which is why
    // this comes after all of them have run.
    let surviving: Vec<_> = combine(built_in, produced)
        .into_iter()
        .filter(|detected| detected.source.is_built_in())
        .collect();
    if !surviving.is_empty() {
        println!(
            "\n{}{}",
            console::Emoji("🖥  ", ""),
            console::style("this client's own virtual packages").bold(),
        );
    }
    for detected in surviving {
        println!(
            "  {} {} {}",
            console::style(console::Emoji("•", "-")).dim(),
            console::style(&detected.package).dim(),
            console::style("(built in)").dim(),
        );
    }

    Ok(())
}

/// Reports what one plugin detected, and what of it a higher-priority channel
/// already spoke for.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn report_detection(
    resolved: &ResolvedPlugin,
    detection: &rattler_virtual_package_plugins::Detection,
    overridden: &std::collections::BTreeMap<
        PackageName,
        rattler_virtual_package_plugins::Overridden,
    >,
    show_timings: bool,
) {
    use itertools::Itertools;

    let source = if detection.from_cache {
        "from cache"
    } else {
        "ran the plugin"
    };
    println!(
        "  {} {} {}",
        console::style(console::Emoji("✔", "+")).green(),
        console::style(resolved.plugin.as_source()).bold(),
        console::style(format!("({source})")).dim(),
    );

    if show_timings {
        let timings = &detection.timings;
        println!(
            "      {}",
            console::style(format!(
                "repodata {:?}{}, solve {:?}, install {:?}, run {:?}",
                timings.environment.repodata,
                if timings.environment.refetched_for_dependencies {
                    " (two queries: the plugin has dependencies)"
                } else {
                    ""
                },
                timings.environment.solve,
                timings.environment.install,
                timings.run,
            ))
            .cyan(),
        );
    }

    let used: Vec<_> = detection
        .virtual_packages
        .iter()
        .filter(|detected| resolved.provides.contains(&detected.package.name))
        .filter(|detected| !overridden.contains_key(&detected.package.name))
        .collect();
    if used.is_empty() && overridden.is_empty() {
        println!(
            "      {}",
            console::style(format!(
                "none of {} are present on this system",
                resolved
                    .provides
                    .iter()
                    .map(PackageName::as_source)
                    .join(", ")
            ))
            .dim(),
        );
    }
    for detected in used {
        println!("      {}", console::style(&detected.package).green());
    }

    // Whatever the plugin said about an overridden name, the environment is what
    // counts. Saying so beats printing a value the solver will not see.
    for (name, overridden) in overridden {
        let line = match overridden {
            rattler_virtual_package_plugins::Overridden::Present(package) => {
                format!("{package} (overridden)")
            }
            rattler_virtual_package_plugins::Overridden::Absent => {
                format!("{} overridden to absent", name.as_source())
            }
        };
        println!("      {}", console::style(line).yellow());
    }

    // A verdict this plugin gave that another channel's plugin speaks for. It
    // still had to give one, and saying so beats a silently missing line.
    for (virtual_package, winner) in &resolved.shadowed_by {
        println!(
            "      {}",
            console::style(format!(
                "{} is provided by {winner}",
                virtual_package.as_source()
            ))
            .dim(),
        );
    }
}

/// Reports a registration that is not run because the environment already says
/// what it would have reported.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn report_overridden(
    resolved: &ResolvedPlugin,
    stood_in_for: &[rattler_conda_types::SourcedVirtualPackage],
) {
    println!(
        "  {} {} {}",
        console::style(console::Emoji("⇄", "=")).yellow(),
        console::style(resolved.plugin.as_source()).bold(),
        console::style("(overridden, not run)").dim(),
    );
    for detected in stood_in_for {
        println!("      {}", detected.package);
    }
    // A name overridden to absent is reported nowhere else, and silence would
    // read as the plugin having found nothing rather than as an instruction.
    for name in resolved
        .provides
        .iter()
        .filter(|name| !stood_in_for.iter().any(|d| &&d.package.name == name))
    {
        println!(
            "      {}",
            console::style(format!("{} overridden to absent", name.as_source())).dim(),
        );
    }
}

/// Reports a registration that is not run because another channel speaks for
/// everything it claimed.
#[cfg(feature = "experimental-virtual-package-plugins")]
fn report_shadowed(resolved: &ResolvedPlugin) {
    println!(
        "  {} {} {}",
        console::style(console::Emoji("○", "-")).dim(),
        console::style(resolved.plugin.as_source()).bold(),
        console::style("(not run)").dim(),
    );
    for (virtual_package, winner) in &resolved.shadowed_by {
        println!(
            "      {}",
            console::style(format!(
                "{} is provided by {winner}",
                virtual_package.as_source()
            ))
            .dim(),
        );
    }
}

/// The message of an error and of every cause beneath it.
///
/// A detection failure is usually reported by an outer layer -- "could not
/// prepare the environment" -- while the useful part is further down, so
/// printing only the top message throws away the answer to "why".
#[cfg(feature = "experimental-virtual-package-plugins")]
fn explain(err: &dyn std::error::Error) -> Vec<String> {
    let mut lines = vec![err.to_string()];
    let mut source = err.source();
    while let Some(cause) = source {
        let message = cause.to_string();
        // thiserror's `#[error(transparent)]` repeats the message it wraps.
        if lines.last() != Some(&message) {
            lines.push(message);
        }
        source = cause.source();
    }
    lines
}
