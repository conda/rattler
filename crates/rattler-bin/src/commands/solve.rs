use std::{
    collections::HashMap,
    env,
    str::FromStr,
    time::{Duration, Instant},
};

use clap::ValueEnum;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler::{default_cache_dir, package_cache::PackageCache};
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, Matches, PackageName,
    ParseMatchSpecOptions, Platform, RepoDataRecord, Version,
};
use rattler_repodata_gateway::{Gateway, RepoData, SourceConfig};
use rattler_solve::{
    SolverImpl, SolverTask,
    libsolv_c::{self},
    resolvo,
};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use url::Url;

use crate::{
    commands::progress::{wrap_in_async_progress, wrap_in_progress},
    exclude_newer::ExcludeNewer,
};

/// Solve a conda environment without installing it.
///
/// Resolves the specified package specs for a target platform and prints the
/// resulting package set.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Channel to search for packages.
    ///
    /// Example: -c conda-forge -c main
    #[clap(short, long = "channel")]
    channels: Option<Vec<String>>,

    /// Package specs to solve.
    #[clap(required = true)]
    specs: Vec<String>,

    /// Additional constraint that the solution must satisfy.
    ///
    /// A constrained package is not necessarily part of the solution, but if
    /// it is, it must match the constraint.
    ///
    /// Example: --constraint "numpy<2" --constraint "openssl=3.*"
    #[clap(long = "constraint", value_name = "SPEC")]
    constraints: Vec<String>,

    /// The platform to solve the environment for.
    #[clap(long, default_value_t = Platform::current())]
    platform: Platform,

    /// Virtual packages to use for solving, e.g. __glibc=2.28.
    #[clap(long)]
    virtual_package: Option<Vec<String>>,

    /// SAT Solver backend to use.
    #[clap(long)]
    solver: Option<Solver>,

    /// Request solver timeout in milliseconds.
    #[clap(long)]
    timeout: Option<u64>,

    /// Solver strategy to use.
    #[clap(long)]
    strategy: Option<SolveStrategy>,

    /// Only include dependencies of package specs in the output.
    #[clap(long, group = "deps_mode")]
    only_deps: bool,

    /// Only include package specifications without dependencies in the output.
    #[clap(long, group = "deps_mode")]
    no_deps: bool,

    /// Exclude packages that have been published after the specified timestamp.
    /// Can be specified as a timestamp (e.g., "2006-12-02T02:07:43Z") or as a date (e.g., "2006-12-02").
    /// When using a date, packages from the entire day are included.
    #[clap(long)]
    exclude_newer: Option<ExcludeNewer>,

    /// Output in JSON format
    #[clap(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SolveStrategy {
    /// Resolve the highest compatible version for every package.
    Highest,

    /// Resolve the lowest compatible version for every package.
    Lowest,

    /// Resolve the lowest compatible version for direct dependencies but the
    /// highest compatible for transitive dependencies.
    LowestDirect,
}

#[derive(Default, Debug, Clone, Copy, ValueEnum)]
pub enum Solver {
    #[default]
    Resolvo,
    #[value(name = "libsolv")]
    LibSolv,
}

impl From<SolveStrategy> for rattler_solve::SolveStrategy {
    fn from(value: SolveStrategy) -> Self {
        match value {
            SolveStrategy::Highest => rattler_solve::SolveStrategy::Highest,
            SolveStrategy::Lowest => rattler_solve::SolveStrategy::LowestVersion,
            SolveStrategy::LowestDirect => rattler_solve::SolveStrategy::LowestVersionDirect,
        }
    }
}

pub async fn solve(opt: Opt, offline: bool) -> miette::Result<()> {
    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);

    // All progress information goes to stderr so that stdout only contains the
    // solved package set.
    eprintln!("Solving for platform: {}", opt.platform);

    let match_spec_options = ParseMatchSpecOptions::strict()
        .with_extras(true)
        .with_conditionals(true)
        .with_flags(true);

    let specs = opt
        .specs
        .iter()
        .map(|spec| MatchSpec::from_str(spec, match_spec_options))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    let constraints = opt
        .constraints
        .iter()
        .map(|spec| MatchSpec::from_str(spec, match_spec_options))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    let cache_dir = default_cache_dir()
        .map_err(|e| miette::miette!("could not determine default cache directory: {}", e))?;
    rattler_cache::ensure_cache_dir(&cache_dir)
        .map_err(|e| miette::miette!("could not create cache directory: {}", e))?;

    let channels = opt
        .channels
        .unwrap_or_else(|| vec![String::from("conda-forge")])
        .into_iter()
        .map(|channel_str| Channel::from_str(channel_str, &channel_config))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    let download_client = super::client::create_client_with_middleware(offline)?;

    let gateway = Gateway::builder()
        .with_cache_dir(cache_dir.join(rattler_cache::REPODATA_CACHE_DIR))
        .with_package_cache(PackageCache::new(
            cache_dir.join(rattler_cache::PACKAGE_CACHE_DIR),
        ))
        .with_client(download_client)
        .with_channel_config(rattler_repodata_gateway::ChannelConfig {
            default: SourceConfig {
                sharded_enabled: true,
                cache_action: super::client::repodata_cache_action(offline),
                ..SourceConfig::default()
            },
            per_channel: HashMap::new(),
        })
        .finish();

    let start_load_repo_data = Instant::now();
    let repo_data = wrap_in_async_progress(
        "loading repodata",
        gateway
            .query(channels, [opt.platform, Platform::NoArch], specs.clone())
            .recursive(true),
    )
    .await
    .into_diagnostic()
    .context("failed to load repodata")?;

    let total_records: usize = repo_data.iter().map(RepoData::len).sum();
    eprintln!(
        "Loaded {} records in {}",
        total_records,
        format_elapsed(start_load_repo_data.elapsed())
    );

    let virtual_packages = wrap_in_progress("determining virtual packages", || {
        if let Some(virtual_packages) = &opt.virtual_package {
            parse_virtual_packages(virtual_packages)
        } else {
            VirtualPackages::detect_for_platform(
                opt.platform,
                &VirtualPackageOverrides::from_env(),
                rattler::default_cache_dir().ok().as_deref(),
            )
            .map(|vpkgs| vpkgs.into_generic_virtual_packages().collect::<Vec<_>>())
            .into_diagnostic()
        }
    })?;

    eprintln!(
        "Virtual packages:\n{}\n",
        virtual_packages
            .iter()
            .format_with("\n", |i, f| f(&format_args!("  - {i}",)))
    );

    let solver_task = SolverTask {
        virtual_packages,
        specs: specs.clone(),
        constraints,
        timeout: opt.timeout.map(Duration::from_millis),
        strategy: opt.strategy.map_or_else(Default::default, Into::into),
        exclude_newer: opt.exclude_newer.map(Into::into),
        ..SolverTask::from_iter(&repo_data)
    };

    let start_solve = Instant::now();
    let solver_result = wrap_in_progress("solving", || match opt.solver.unwrap_or_default() {
        Solver::Resolvo => resolvo::Solver.solve(solver_task),
        Solver::LibSolv => libsolv_c::Solver.solve(solver_task),
    })
    .into_diagnostic()?;
    let solve_duration = start_solve.elapsed();

    let mut solved_packages: Vec<RepoDataRecord> = solver_result.records;

    if opt.no_deps {
        solved_packages.retain(|r| specs.iter().any(|s| s.matches(&r.package_record)));
    } else if opt.only_deps {
        solved_packages.retain(|r| !specs.iter().any(|s| s.matches(&r.package_record)));
    }

    // The solver returns records in the order it decided on them, which is an
    // implementation detail and differs between backends. Sort by name so the
    // output is stable and diffable between runs.
    solved_packages.sort_by(|a, b| {
        a.package_record
            .name
            .as_normalized()
            .cmp(b.package_record.name.as_normalized())
    });

    if solved_packages.is_empty() {
        eprintln!("No packages solved");
        if opt.json {
            println!("[]");
        }
        return Ok(());
    }

    if opt.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&solved_packages).into_diagnostic()?
        );
    } else {
        eprintln!(
            "Solved {} package{} in {}:",
            solved_packages.len(),
            if solved_packages.len() == 1 { "" } else { "s" },
            format_elapsed(solve_duration)
        );
        print_records(
            &solved_packages,
            &solver_result.extras,
            &specs,
            &channel_config,
        );
    }

    Ok(())
}

/// Formats a duration in a compact, human readable way.
fn format_elapsed(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1000 {
        format!("{millis}ms")
    } else {
        format!("{:.2}s", duration.as_secs_f64())
    }
}

fn parse_virtual_packages(
    virtual_packages: &[String],
) -> miette::Result<Vec<GenericVirtualPackage>> {
    virtual_packages
        .iter()
        .map(|virt_pkg| {
            let elems = virt_pkg.split('=').collect::<Vec<&str>>();
            Ok(GenericVirtualPackage {
                name: elems[0].try_into().into_diagnostic()?,
                version: elems
                    .get(1)
                    .map_or(Version::from_str("0"), |s| Version::from_str(s))
                    .into_diagnostic()?,
                build_string: (*elems.get(2).unwrap_or(&"")).to_string(),
            })
        })
        .collect::<miette::Result<Vec<_>>>()
}

/// Prints the solved records as a table with aligned columns.
///
/// Packages that are explicitly requested through one of the input specs are
/// highlighted to distinguish them from the transitive dependencies that were
/// pulled in by the solver.
fn print_records(
    records: &[RepoDataRecord],
    extras: &HashMap<PackageName, Vec<String>>,
    specs: &[MatchSpec],
    channel_config: &ChannelConfig,
) {
    let header = [
        "Package".to_string(),
        "Version".to_string(),
        "Build".to_string(),
        "Channel".to_string(),
    ];

    // These initial widths match the header column lengths.
    let mut widths: [usize; 4] = header.clone().map(|field| field.len());
    let mut rows = Vec::with_capacity(records.len());
    for record in records {
        let mut name = record.package_record.name.as_normalized().to_string();
        if let Some(extras) = extras.get(&record.package_record.name) {
            name.push('[');
            name.push_str(&extras.join(","));
            name.push(']');
        }

        let fields = [
            name,
            record.package_record.version.to_string(),
            record.package_record.build.clone(),
            format_channel(record, channel_config),
        ];
        for (width, field) in widths.iter_mut().zip(&fields) {
            *width = (*width).max(field.chars().count());
        }

        let explicit = specs
            .iter()
            .any(|spec| spec.matches(&record.package_record));
        rows.push((fields, explicit));
    }

    // Separates the table from the status messages on stderr.
    eprintln!();
    let styled_header = header
        .clone()
        .map(|field| console::style(field).bold().to_string());
    print_row(&styled_header, &widths, &header);
    for (fields, explicit) in &rows {
        let styled = [
            if *explicit {
                console::style(&fields[0]).green().bold().to_string()
            } else {
                fields[0].clone()
            },
            fields[1].clone(),
            console::style(&fields[2]).dim().to_string(),
            console::style(&fields[3]).dim().to_string(),
        ];
        print_row(&styled, &widths, fields);
    }
}

/// Prints a single table row, padding each column to `widths`.
///
/// `styled` holds the fields as they should be displayed, `plain` the same
/// fields without any styling. Padding is computed from `plain` because ANSI
/// escape codes in `styled` do not occupy any terminal columns but would
/// otherwise be counted by the formatter.
fn print_row(styled: &[String; 4], widths: &[usize; 4], plain: &[String; 4]) {
    let mut line = String::new();
    for (i, field) in styled.iter().enumerate() {
        line.push_str(field);
        // Don't pad the last column, that would only add trailing whitespace.
        if i + 1 < styled.len() {
            let padding = widths[i].saturating_sub(plain[i].chars().count());
            // Two spaces as inter-column padding.
            line.push_str(&" ".repeat(padding + 2));
        }
    }
    println!("{}", line.trim_end());
}

/// Formats the channel of a record as `<channel name>/<subdir>`.
///
/// Records that come from a channel under the configured channel alias are
/// shortened to just their name (e.g. `conda-forge/noarch`), anything else
/// keeps its full URL so it stays unambiguous.
fn format_channel(record: &RepoDataRecord, channel_config: &ChannelConfig) -> String {
    let subdir = &record.package_record.subdir;
    let Some(channel) = &record.channel else {
        return subdir.clone();
    };

    let name = Url::parse(channel)
        .ok()
        .and_then(|url| channel_config.strip_channel_alias(&url))
        .unwrap_or_else(|| channel.trim_end_matches('/').to_string());

    format!("{name}/{subdir}")
}
