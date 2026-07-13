use std::{collections::HashMap, env, time::Instant};

use indexmap::IndexMap;
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{
    Channel, ChannelConfig, MatchSpec, ParseMatchSpecOptions, Platform, RepoDataRecord,
};
use rattler_repodata_gateway::{Gateway, RepoData, SourceConfig};

/// Search for packages in conda channels using glob or regex patterns.
#[derive(Debug, clap::Parser)]
#[clap(after_help = r#"Examples:
  rattler search 'python*'            # glob pattern
  rattler search '^numpy-.*$'         # regex pattern
  rattler search openssl -c bioconda  # search in specific channel"#)]
pub struct Opt {
    /// The matchspec pattern to search for.
    ///
    /// Supports exact names (python), glob patterns (python*, *ssl*),
    /// and regex patterns (^numpy-.*$).
    #[clap(required = true)]
    matchspec: String,

    /// Channels to search in
    #[clap(short, long, default_value = "conda-forge")]
    channels: Vec<String>,

    /// Platform to search for
    #[clap(short, long, default_value_t = Platform::current())]
    platform: Platform,

    /// Maximum number of packages to display
    #[clap(long, default_value = "3")]
    limit_packages: usize,

    /// Maximum number of versions to display per package
    #[clap(long, default_value = "5")]
    limit: usize,

    /// Show all packages and versions (no limits)
    #[clap(long)]
    all: bool,

    /// Enable sharded repodata
    #[clap(long, default_value = "true", action = clap::ArgAction::Set)]
    sharded: bool,

    /// Output in JSON format
    #[clap(long, conflicts_with_all = ["limit", "limit_packages", "all"])]
    json: bool,
}

pub async fn search(opt: Opt, offline: bool) -> miette::Result<()> {
    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);

    eprintln!("Searching for '{}' on {}", opt.matchspec, opt.platform);

    // Parse the pattern as a matchspec with glob/regex support
    let matchspec = MatchSpec::from_str(
        &opt.matchspec,
        ParseMatchSpecOptions::strict()
            .with_exact_names_only(false)
            .with_extras(true)
            .with_flags(true),
    )
    .into_diagnostic()
    .context("failed to parse pattern as matchspec")?;

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

    // Create gateway
    let gateway = Gateway::builder()
        .with_client(download_client)
        .with_channel_config(rattler_repodata_gateway::ChannelConfig {
            default: SourceConfig {
                sharded_enabled: opt.sharded,
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
    let repo_data = gateway
        .query(
            channels,
            [opt.platform, Platform::NoArch],
            vec![matchspec.clone()],
        )
        .recursive(false) // Don't fetch dependencies for search
        .await
        .into_diagnostic()
        .context("failed to query repodata")?;

    pb.finish_and_clear();

    if opt.json {
        // Group records by platform (subdir), same format as `pixi search --json`
        let mut grouped: IndexMap<&str, Vec<&RepoDataRecord>> = IndexMap::new();
        for record in repo_data.iter().flat_map(RepoData::iter) {
            grouped
                .entry(record.package_record.subdir.as_str())
                .or_default()
                .push(record);
        }
        for records in grouped.values_mut() {
            records.sort_unstable_by(|a, b| b.cmp(a));
        }
        let json_str = serde_json::to_string_pretty(&grouped).into_diagnostic()?;
        println!("{json_str}");
        return Ok(());
    }

    // Collect all records
    let total_records: usize = repo_data.iter().map(RepoData::len).sum();
    println!(
        "Found {} matching records in {:?}\n",
        total_records,
        start.elapsed()
    );

    if total_records == 0 {
        println!("No packages found matching '{}'", opt.matchspec);
        return Ok(());
    }

    // Group records by package name
    let mut packages: HashMap<String, Vec<_>> = HashMap::new();
    for record in repo_data.iter().flat_map(RepoData::iter) {
        packages
            .entry(record.package_record.name.as_normalized().to_string())
            .or_default()
            .push(record);
    }

    // Sort package names alphabetically
    let mut package_names: Vec<_> = packages.keys().cloned().collect();
    package_names.sort();

    let limit_versions = if opt.all { usize::MAX } else { opt.limit };
    let limit_packages = if opt.all {
        usize::MAX
    } else {
        opt.limit_packages
    };

    let total_packages = package_names.len();
    let shown_packages = total_packages.min(limit_packages);

    // Print results
    for name in package_names.into_iter().take(limit_packages) {
        let mut records = packages.remove(&name).unwrap();
        // Sort by version descending
        records.sort_unstable();
        records.reverse();

        let total = records.len();
        let shown = records.len().min(limit_versions);

        println!(
            "{} ({} version{})",
            console::style(&name).bold().green(),
            total,
            if total == 1 { "" } else { "s" }
        );

        for record in records.iter().take(limit_versions) {
            let channel = record.channel.as_deref().unwrap_or("unknown");
            println!(
                "  {} {} [{}] {}",
                console::style(&record.package_record.version).cyan(),
                record.package_record.build,
                record.package_record.subdir,
                console::style(channel).dim()
            );
        }

        if shown < total {
            println!(
                "  {} ... and {} more version{}",
                console::style("").dim(),
                total - shown,
                if total - shown == 1 { "" } else { "s" }
            );
        }
        println!();
    }

    if shown_packages < total_packages {
        println!(
            "... and {} more package{}",
            total_packages - shown_packages,
            if total_packages - shown_packages == 1 {
                ""
            } else {
                "s"
            }
        );
    }

    Ok(())
}
