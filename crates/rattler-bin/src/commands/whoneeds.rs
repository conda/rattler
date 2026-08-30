use std::{collections::HashMap, env, time::Instant};

use indexmap::IndexMap;
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, ParseMatchSpecOptions, Platform};
use rattler_repodata_gateway::{
    Gateway, SourceConfig,
    repoquery::{DependencyKind, Dependent, WhoNeedsOptions},
};

/// Show packages that depend on the given package (reverse dependencies).
#[derive(Debug, clap::Parser)]
#[clap(after_help = r#"Examples:
  rattler whoneeds numpy                     # packages that depend on numpy
  rattler whoneeds 'python >=3.13'           # packages that can use python >=3.13
  rattler whoneeds numpy --recursive         # also transitive dependents
  rattler whoneeds pandas --include-constrains"#)]
pub struct Opt {
    /// The package name (or matchspec) to find reverse dependencies for.
    ///
    /// A bare name (numpy) matches any dependency on that name. A spec with
    /// a version (python >=3.13) only matches dependents whose constraint
    /// admits such a version.
    #[clap(required = true)]
    matchspec: String,

    /// Channels to search in
    #[clap(short, long, default_value = "conda-forge")]
    channels: Vec<String>,

    /// Platform to search for
    #[clap(short, long, default_value_t = Platform::current())]
    platform: Platform,

    /// Also match packages that reference the package through `constrains`
    #[clap(long)]
    include_constrains: bool,

    /// Also show packages that transitively depend on the package
    #[clap(short, long)]
    recursive: bool,

    /// Maximum number of packages to display
    #[clap(long, default_value = "100")]
    limit: usize,

    /// Show all packages (no limit)
    #[clap(long)]
    all: bool,

    /// Enable sharded repodata. Disabled by default because a reverse
    /// dependency lookup needs the records of every package in the channel,
    /// which is one request per package with shards but a single request
    /// with a full repodata.json.
    #[clap(long, default_value = "false", action = clap::ArgAction::Set)]
    sharded: bool,

    /// Output in JSON format
    #[clap(long, conflicts_with_all = ["limit", "all"])]
    json: bool,
}

pub async fn whoneeds(opt: Opt, offline: bool) -> miette::Result<()> {
    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);

    // Parse the user input as a matchspec with an exact package name.
    let matchspec = MatchSpec::from_str(&opt.matchspec, ParseMatchSpecOptions::strict())
        .into_diagnostic()
        .context("failed to parse the package as a matchspec")?;

    eprintln!(
        "Searching for packages that depend on '{}' on {}",
        opt.matchspec, opt.platform
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

    let options = WhoNeedsOptions {
        include_constrains: opt.include_constrains,
        recursive: opt.recursive,
    };
    let dependents = repo_data
        .who_needs(&matchspec, &options)
        .into_diagnostic()?;

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
                    "kind": match dependent.kind {
                        DependencyKind::Depends => "depends",
                        DependencyKind::Constrains => "constrains",
                    },
                    "depth": dependent.depth,
                })
            })
            .collect();
        let json_str = serde_json::to_string_pretty(&json_records).into_diagnostic()?;
        println!("{json_str}");
        return Ok(());
    }

    if dependents.is_empty() {
        println!("No packages found that depend on '{}'", opt.matchspec);
        return Ok(());
    }

    // Group by (depth, package name), keeping the record with the highest
    // version per package as the representative shown in the output.
    let mut grouped: IndexMap<(usize, &str), (&Dependent<'_>, usize)> = IndexMap::new();
    for dependent in &dependents {
        let key = (
            dependent.depth,
            dependent.record.package_record.name.as_normalized(),
        );
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
        opt.matchspec,
        start.elapsed()
    );

    let limit = if opt.all { usize::MAX } else { opt.limit };
    let mut current_depth = 0;
    for (&(depth, name), &(dependent, record_count)) in grouped.iter().take(limit) {
        if opt.recursive && depth != current_depth {
            current_depth = depth;
            println!(
                "{}",
                console::style(format!("Depth {depth}:")).bold().yellow()
            );
        }
        let record = &dependent.record.package_record;
        let kind = match dependent.kind {
            DependencyKind::Depends => "via",
            DependencyKind::Constrains => "via constraint",
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
