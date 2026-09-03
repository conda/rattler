use std::{collections::HashMap, env, path::Path, time::Instant};

use indexmap::IndexMap;
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{
    Channel, ChannelConfig, PackageName, PackageRecord, Platform, package::IndexJson,
};
use rattler_repodata_gateway::{
    Gateway, SourceConfig,
    who_needs::{DependencyKind, Dependent, WhoNeedsTarget},
};
use url::Url;

/// Show packages that depend on the given package (reverse dependencies).
#[derive(Debug, clap::Parser)]
#[clap(after_help = r#"Examples:
  rattler whoneeds numpy                      # packages that depend on numpy
  rattler whoneeds __cuda                     # packages that depend on a virtual package
  rattler whoneeds ./python-3.13.1-h123_0.conda   # packages that can use this exact package
  rattler whoneeds https://conda.anaconda.org/conda-forge/noarch/polars-1.44.1-pyh8da0edf_0.conda"#)]
pub struct Opt {
    /// The package to find reverse dependencies for.
    ///
    /// Either a package name (numpy, __cuda), matching every dependency
    /// that names the package; or a path or URL to a .conda/.tar.bz2
    /// package, matching only dependents whose match spec matches the
    /// package.
    #[clap(required = true)]
    package: String,

    /// Channels to search in
    #[clap(short, long, default_value = "conda-forge")]
    channels: Vec<String>,

    /// Platform to search for
    #[clap(short, long, default_value_t = Platform::current())]
    platform: Platform,

    /// Maximum number of packages to display
    #[clap(long, default_value = "100")]
    limit: usize,

    /// Show all packages (no limit)
    #[clap(long)]
    all: bool,

    /// Output in JSON format
    #[clap(long, conflicts_with_all = ["limit", "all"])]
    json: bool,
}

/// Interprets the package argument as a package archive URL or path, or as
/// a package name, and builds the corresponding target. Returns the target
/// together with a human readable form of it.
async fn resolve_target(
    package: &str,
    client: &reqwest_middleware::ClientWithMiddleware,
) -> miette::Result<(WhoNeedsTarget, String)> {
    let is_archive = package.ends_with(".conda") || package.ends_with(".tar.bz2");
    let index_json: Option<IndexJson> = if is_archive && package.contains("://") {
        let url = Url::parse(package)
            .into_diagnostic()
            .context("failed to parse the package URL")?;
        Some(
            rattler_package_streaming::reqwest::fetch::fetch_package_file_from_remote_url(
                client.clone(),
                url,
            )
            .await
            .into_diagnostic()
            .context("failed to read index.json from the package URL")?,
        )
    } else if is_archive {
        Some(
            rattler_package_streaming::seek::read_package_file(Path::new(package))
                .into_diagnostic()
                .context("failed to read index.json from the package file")?,
        )
    } else {
        None
    };

    let Some(index_json) = index_json else {
        let name: PackageName = package
            .parse()
            .into_diagnostic()
            .context("failed to parse the package name")?;
        let display = name.as_source().to_string();
        return Ok((name.into(), display));
    };

    let record = PackageRecord::from_index_json(index_json, None, None, None)
        .into_diagnostic()
        .context("failed to convert the package's index.json into a record")?;
    let display = format!(
        "{} {} {}",
        record.name.as_source(),
        record.version,
        record.build
    );
    Ok((record.into(), display))
}

pub async fn whoneeds(opt: Opt, offline: bool) -> miette::Result<()> {
    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);

    // Create HTTP client
    let download_client = super::client::create_client_with_middleware(offline)?;

    let (target, target_display) = resolve_target(&opt.package, &download_client).await?;

    eprintln!(
        "Searching for packages that depend on '{}' on {}",
        target_display, opt.platform
    );

    // Determine the channels
    let channels = opt
        .channels
        .into_iter()
        .map(|channel_str| Channel::from_str(channel_str, &channel_config))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    eprintln!(
        "Channels: {}",
        channels.iter().map(Channel::canonical_name).join(", ")
    );

    // Create gateway. Sharded repodata is disabled because a reverse
    // dependency lookup needs the records of every package in the channel,
    // which is one request per package with shards but a single request
    // with a full repodata.json.
    let gateway = Gateway::builder()
        .with_client(download_client)
        .with_channel_config(rattler_repodata_gateway::ChannelConfig {
            default: SourceConfig {
                sharded_enabled: false,
                cache_action: super::client::repodata_cache_action(offline),
                ..SourceConfig::default()
            },
            per_channel: HashMap::new(),
        })
        .finish();

    // Show progress while loading repodata
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb.set_style(ProgressStyle::with_template("{spinner:.green} {msg}").unwrap());
    pb.set_message("Loading repodata...");

    let start = Instant::now();
    let output = gateway
        .who_needs(channels, [opt.platform, Platform::NoArch], target)
        .execute()
        .await
        .into_diagnostic()
        .context("failed to compute reverse dependencies")?;
    let dependents = output.dependents;

    pb.finish_and_clear();

    if opt.json {
        let json_records: Vec<_> = dependents
            .iter()
            .map(|dependent| {
                serde_json::json!({
                    "name": dependent.record.package_record.name.as_normalized(),
                    "version": dependent.record.package_record.version.to_string(),
                    "build": dependent.record.package_record.build,
                    "subdir": dependent.record.package_record.subdir,
                    "channel": dependent.record.channel,
                    "dependency": &dependent.dependency,
                    "kind": dependent.kind.to_string(),
                })
            })
            .collect();
        let json_str = serde_json::to_string_pretty(&json_records).into_diagnostic()?;
        println!("{json_str}");
        return Ok(());
    }

    if dependents.is_empty() {
        println!(
            "No packages found that depend on '{target_display}' in {:?}",
            start.elapsed()
        );
        return Ok(());
    }

    // Group by package name, keeping the record with the highest version
    // per package as the representative shown in the output.
    let mut grouped: IndexMap<&str, (&Dependent, usize)> = IndexMap::new();
    for dependent in &dependents {
        let key = dependent.record.package_record.name.as_normalized();
        grouped
            .entry(key)
            .and_modify(|(best, count)| {
                *count += 1;
                if dependent.record.package_record.version > best.record.package_record.version {
                    *best = dependent;
                }
            })
            .or_insert((dependent, 1));
    }
    grouped.sort_keys();

    let total_packages = grouped.len();
    println!(
        "Found {} package{} ({} record{}) that depend{} on '{}' in {:?}\n",
        total_packages,
        if total_packages == 1 { "" } else { "s" },
        dependents.len(),
        if dependents.len() == 1 { "" } else { "s" },
        if total_packages == 1 { "s" } else { "" },
        target_display,
        start.elapsed()
    );

    let limit = if opt.all { usize::MAX } else { opt.limit };
    for (&name, &(dependent, record_count)) in grouped.iter().take(limit) {
        let record = &dependent.record.package_record;
        let kind = match &dependent.kind {
            DependencyKind::Depends => "via".to_string(),
            DependencyKind::Constrains => "via constraint".to_string(),
            DependencyKind::ExtraDepends(extra) => format!("via extra '{extra}'"),
            DependencyKind::RunExport(_) => "via run export".to_string(),
        };
        println!(
            "  {} {} {} ({} {}){}",
            console::style(name).bold().green(),
            console::style(&record.version).cyan(),
            record.build,
            kind,
            console::style(&dependent.dependency).dim(),
            if record_count > 1 {
                format!(" and {} more record{}", record_count - 1, {
                    if record_count == 2 { "" } else { "s" }
                })
            } else {
                String::new()
            }
        );
    }

    if total_packages > limit {
        println!(
            "\n... and {} more package{} (use --all to show all)",
            total_packages - limit,
            if total_packages - limit == 1 { "" } else { "s" }
        );
    }

    Ok(())
}
