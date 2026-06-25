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

pub async fn solve(opt: Opt) -> miette::Result<()> {
    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);

    println!("Solving for platform: {}", opt.platform);

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

    let download_client = super::client::create_client_with_middleware()?;

    let gateway = Gateway::builder()
        .with_cache_dir(cache_dir.join(rattler_cache::REPODATA_CACHE_DIR))
        .with_package_cache(PackageCache::new(
            cache_dir.join(rattler_cache::PACKAGE_CACHE_DIR),
        ))
        .with_client(download_client)
        .with_channel_config(rattler_repodata_gateway::ChannelConfig {
            default: SourceConfig {
                sharded_enabled: true,
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
    println!(
        "Loaded {} records in {:?}",
        total_records,
        start_load_repo_data.elapsed()
    );

    let virtual_packages = wrap_in_progress("determining virtual packages", || {
        if let Some(virtual_packages) = &opt.virtual_package {
            parse_virtual_packages(virtual_packages)
        } else {
            VirtualPackages::detect_for_platform(opt.platform, &VirtualPackageOverrides::from_env())
                .map(|vpkgs| vpkgs.into_generic_virtual_packages().collect::<Vec<_>>())
                .into_diagnostic()
        }
    })?;

    println!(
        "Virtual packages:\n{}\n",
        virtual_packages
            .iter()
            .format_with("\n", |i, f| f(&format_args!("  - {i}",)))
    );

    let solver_task = SolverTask {
        virtual_packages,
        specs: specs.clone(),
        timeout: opt.timeout.map(Duration::from_millis),
        strategy: opt.strategy.map_or_else(Default::default, Into::into),
        exclude_newer: opt.exclude_newer.map(Into::into),
        ..SolverTask::from_iter(&repo_data)
    };

    let solver_result = wrap_in_progress("solving", || match opt.solver.unwrap_or_default() {
        Solver::Resolvo => resolvo::Solver.solve(solver_task),
        Solver::LibSolv => libsolv_c::Solver.solve(solver_task),
    })
    .into_diagnostic()?;

    let mut solved_packages: Vec<RepoDataRecord> = solver_result.records;

    if opt.no_deps {
        solved_packages.retain(|r| specs.iter().any(|s| s.matches(&r.package_record)));
    } else if opt.only_deps {
        solved_packages.retain(|r| !specs.iter().any(|s| s.matches(&r.package_record)));
    }

    if solved_packages.is_empty() {
        println!("No packages solved");
    } else {
        println!("Solved {} packages:", solved_packages.len());
        print_records(&solved_packages, solver_result.extras);
    }

    Ok(())
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

fn print_records(records: &[RepoDataRecord], features: HashMap<PackageName, Vec<String>>) {
    for record in records {
        let direct_url_print = record.channel.clone().unwrap_or_default();
        if let Some(features) = features.get(&record.package_record.name) {
            println!(
                "{}[{}] {} {} {} {}",
                record.package_record.name.as_normalized(),
                features.join(", "),
                record.package_record.version,
                record.package_record.build,
                record.package_record.subdir,
                direct_url_print,
            );
        } else {
            println!(
                "{} {} {} {} {}",
                record.package_record.name.as_normalized(),
                record.package_record.version,
                record.package_record.build,
                record.package_record.subdir,
                direct_url_print,
            );
        }
    }
}
