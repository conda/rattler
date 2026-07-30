use miette::IntoDiagnostic;
use rattler_conda_types::GenericVirtualPackage;
#[cfg(feature = "experimental-virtual-package-plugins")]
use rattler_conda_types::Platform;
use rattler_virtual_packages::VirtualPackageOverrides;

/// Print detected virtual packages.
#[derive(Debug, clap::Parser)]
#[cfg_attr(
    feature = "experimental-virtual-package-plugins",
    clap(after_help = r#"Examples:
  rattler virtual-packages
  rattler virtual-packages -c ./test-data/channels/virtual-package-plugins"#)
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
}

pub async fn virtual_packages(opt: Opt, offline: bool) -> miette::Result<()> {
    let cache_dir = rattler::default_cache_dir().ok();
    tracing::debug!(
        cache_dir = %cache_dir
            .as_ref()
            .map_or_else(|| "<disabled>".to_string(), |path| path.display().to_string()),
        "detecting virtual packages"
    );

    let virtual_packages = rattler_virtual_packages::VirtualPackage::detect(
        &VirtualPackageOverrides::from_env(),
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

    #[cfg(feature = "experimental-virtual-package-plugins")]
    print_plugins(&opt.channels, &opt.platforms, offline).await?;

    #[cfg(not(feature = "experimental-virtual-package-plugins"))]
    let _ = (opt, offline);

    Ok(())
}

/// Prints the plugin registrations declared by each `(channel, platform)`
/// subdirectory, in the order the channels were given.
#[cfg(feature = "experimental-virtual-package-plugins")]
async fn print_plugins(
    channels: &[String],
    platforms: &[Platform],
    offline: bool,
) -> miette::Result<()> {
    use std::{collections::HashMap, env};

    use itertools::Itertools;
    use rattler_conda_types::{Channel, ChannelConfig, PackageName};
    use rattler_repodata_gateway::{Gateway, SourceConfig};

    if channels.is_empty() {
        return Ok(());
    }

    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);
    let channels = channels
        .iter()
        .map(|channel| Channel::from_str(channel, &channel_config))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    let platforms = if platforms.is_empty() {
        vec![Platform::current(), Platform::NoArch]
    } else {
        platforms.to_vec()
    };

    let gateway = Gateway::builder()
        .with_client(super::client::create_client_with_middleware(offline)?)
        .with_channel_config(rattler_repodata_gateway::ChannelConfig {
            default: SourceConfig {
                cache_action: super::client::repodata_cache_action(offline),
                ..SourceConfig::default()
            },
            per_channel: HashMap::new(),
        })
        .finish();

    for channel in &channels {
        for platform in &platforms {
            let plugins = gateway
                .virtual_package_plugins(channel, *platform)
                .await
                .into_diagnostic()?;
            if plugins.is_empty() {
                continue;
            }
            println!(
                "\nvirtual package plugins in {} [{platform}]:",
                channel.canonical_name()
            );
            for (plugin, provided) in &plugins {
                println!(
                    "  {} provides {}",
                    plugin.as_source(),
                    provided.iter().map(PackageName::as_source).join(", ")
                );
            }
        }
    }

    Ok(())
}
