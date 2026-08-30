use std::{collections::HashMap, env, time::Instant};

use indexmap::IndexMap;
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, PackageName, ParseMatchSpecOptions, Platform, Version,
};
use rattler_repodata_gateway::{
    Gateway, SourceConfig,
    repoquery::{DependencyKind, Dependent, RunExportKind, WhoNeedsTarget},
};

/// Show packages that depend on the given package (reverse dependencies).
#[derive(Debug, clap::Parser)]
#[clap(after_help = r#"Examples:
  rattler whoneeds numpy                  # packages that depend on numpy
  rattler whoneeds python 3.13.1          # packages whose constraint admits python 3.13.1
  rattler whoneeds python 3.13.1 h123_0   # ... with this exact build string"#)]
pub struct Opt {
    /// The name of the package to find reverse dependencies for.
    #[clap(required = true)]
    package: String,

    /// A concrete version of the package. When given, only dependents whose
    /// version constraint admits this version are shown.
    version: Option<Version>,

    /// A concrete build string of the package. When given, only dependents
    /// whose build string constraint admits this build are shown.
    build: Option<String>,

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

pub async fn whoneeds(opt: Opt, offline: bool) -> miette::Result<()> {
    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);

    let package_name: PackageName = opt
        .package
        .parse()
        .into_diagnostic()
        .context("failed to parse the package name")?;
    let mut target = WhoNeedsTarget::new(package_name);
    if let Some(version) = opt.version.clone() {
        target = target.with_version(version);
    }
    if let Some(build) = opt.build.clone() {
        target = target.with_build(build);
    }

    // Human readable form of the queried package, e.g. `python 3.13.1`.
    let target_display = std::iter::once(opt.package.clone())
        .chain(opt.version.as_ref().map(ToString::to_string))
        .chain(opt.build.clone())
        .join(" ");

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

    // Create HTTP client
    let download_client = super::client::create_client_with_middleware(offline)?;

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

    // Reverse dependencies can hide anywhere, so fetch all records of the
    // queried subdirs with a wildcard spec.
    let wildcard = MatchSpec::from_str(
        "*",
        ParseMatchSpecOptions::strict().with_exact_names_only(false),
    )
    .into_diagnostic()?;

    let start = Instant::now();
    let repo_data = gateway
        .query(channels, [opt.platform, Platform::NoArch], vec![wildcard])
        .recursive(false)
        .await
        .into_diagnostic()
        .context("failed to query repodata")?;

    pb.set_message("Computing reverse dependencies...");

    let dependents = repo_data.who_needs(&target);

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
                    "dependency": dependent.dependency,
                    "kind": kind_str(dependent.kind),
                })
            })
            .collect();
        let json_str = serde_json::to_string_pretty(&json_records).into_diagnostic()?;
        println!("{json_str}");
        return Ok(());
    }

    if dependents.is_empty() {
        println!("No packages found that depend on '{target_display}'");
        return Ok(());
    }

    // Group by package name, keeping the record with the highest version
    // per package as the representative shown in the output.
    let mut grouped: IndexMap<&str, (&Dependent<'_>, usize)> = IndexMap::new();
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
        let kind = match dependent.kind {
            DependencyKind::Depends => "via",
            DependencyKind::Constrains => "via constraint",
            DependencyKind::RunExport(_) => "via run export",
        };
        println!(
            "  {} {} {} ({} {}){}",
            console::style(name).bold().green(),
            console::style(&record.version).cyan(),
            record.build,
            kind,
            console::style(dependent.dependency).dim(),
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

/// Stable string form of a dependency kind for the JSON output.
fn kind_str(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::Depends => "depends",
        DependencyKind::Constrains => "constrains",
        DependencyKind::RunExport(RunExportKind::Weak) => "run_export/weak",
        DependencyKind::RunExport(RunExportKind::Strong) => "run_export/strong",
        DependencyKind::RunExport(RunExportKind::Noarch) => "run_export/noarch",
        DependencyKind::RunExport(RunExportKind::WeakConstrains) => "run_export/weak_constrains",
        DependencyKind::RunExport(RunExportKind::StrongConstrains) => {
            "run_export/strong_constrains"
        }
    }
}
