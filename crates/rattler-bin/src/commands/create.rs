use std::{
    borrow::Cow,
    collections::HashMap,
    env,
    future::IntoFuture,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use clap::ValueEnum;
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use rattler::{
    default_cache_dir,
    install::{IndicatifReporter, Installer, Transaction, TransactionOperation},
    package_cache::PackageCache,
};
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, Matches, PackageName,
    ParseStrictness, Platform, PrefixRecord, RepoDataRecord, Version,
};
use rattler_networking::{AuthenticationMiddleware, AuthenticationStorage};
use rattler_repodata_gateway::{Gateway, RepoData, SourceConfig};
use rattler_solve::{
    libsolv_c::{self},
    resolvo, SolverImpl, SolverTask,
};
use reqwest::Client;

use crate::global_multi_progress;

#[derive(Debug, clap::Parser)]
pub struct Opt {
    #[clap(short)]
    channels: Option<Vec<String>>,

    #[clap(required = true)]
    specs: Vec<String>,

    #[clap(long)]
    dry_run: bool,

    #[clap(long)]
    platform: Option<String>,

    #[clap(long)]
    virtual_package: Option<Vec<String>>,

    #[clap(long)]
    solver: Option<Solver>,

    #[clap(long)]
    timeout: Option<u64>,

    #[clap(long)]
    target_prefix: Option<PathBuf>,

    #[clap(long)]
    strategy: Option<SolveStrategy>,

    #[clap(long, group = "deps_mode")]
    only_deps: bool,

    #[clap(long, group = "deps_mode")]
    no_deps: bool,
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

pub async fn create(opt: Opt) -> anyhow::Result<()> {
    let channel_config = ChannelConfig::default_with_root_dir(env::current_dir()?);
    let current_dir = env::current_dir()?;
    let target_prefix = opt
        .target_prefix
        .unwrap_or_else(|| current_dir.join(".prefix"));

    // Make the target prefix absolute
    let target_prefix = std::path::absolute(target_prefix)?;
    println!("Target prefix: {}", target_prefix.display());

    // Determine the platform we're going to install for
    let install_platform = if let Some(platform) = opt.platform {
        Platform::from_str(&platform)?
    } else {
        Platform::current()
    };

    println!("Installing for platform: {install_platform:?}");

    // Parse the specs from the command line. We do this explicitly instead of allow
    // clap to deal with this because we need to parse the `channel_config` when
    // parsing matchspecs.
    let specs = opt
        .specs
        .iter()
        .map(|spec| MatchSpec::from_str(spec, ParseStrictness::Strict))
        .collect::<Result<Vec<_>, _>>()?;

    // Find the default cache directory. Create it if it doesnt exist yet.
    let cache_dir = default_cache_dir()?;
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| anyhow::anyhow!("could not create cache directory: {}", e))?;

    // Determine the channels to use from the command line or select the default.
    // Like matchspecs this also requires the use of the `channel_config` so we
    // have to do this manually.
    let channels = opt
        .channels
        .unwrap_or_else(|| vec![String::from("conda-forge")])
        .into_iter()
        .map(|channel_str| Channel::from_str(channel_str, &channel_config))
        .collect::<Result<Vec<_>, _>>()?;

    // Determine the packages that are currently installed in the environment.
    let installed_packages = PrefixRecord::collect_from_prefix::<PrefixRecord>(&target_prefix)?;

    // For each channel/subdirectory combination, download and cache the
    // `repodata.json` that should be available from the corresponding Url. The
    // code below also displays a nice CLI progress-bar to give users some more
    // information about what is going on.
    let download_client = Client::builder()
        .no_gzip()
        .build()
        .expect("failed to create client");

    let download_client = reqwest_middleware::ClientBuilder::new(download_client)
        .with_arc(Arc::new(AuthenticationMiddleware::from_env_and_defaults()?))
        .with(rattler_networking::OciMiddleware)
        .with(rattler_networking::S3Middleware::new(
            HashMap::new(),
            AuthenticationStorage::from_env_and_defaults()?,
        ))
        .with(rattler_networking::GCSMiddleware)
        .build();

    // Get the package names from the matchspecs so we can only load the package
    // records that we need.
    let gateway = Gateway::builder()
        .with_cache_dir(cache_dir.join(rattler_cache::REPODATA_CACHE_DIR))
        .with_package_cache(PackageCache::new(
            cache_dir.join(rattler_cache::PACKAGE_CACHE_DIR),
        ))
        .with_client(download_client.clone())
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
            .query(
                channels,
                [install_platform, Platform::NoArch],
                specs.clone(),
            )
            .recursive(true),
    )
    .await
    .context("failed to load repodata")?;

    // Determine the number of records
    let total_records: usize = repo_data.iter().map(RepoData::len).sum();
    println!(
        "Loaded {} records in {:?}",
        total_records,
        start_load_repo_data.elapsed()
    );

    // Determine virtual packages of the system. These packages define the
    // capabilities of the system. Some packages depend on these virtual
    // packages to indicate compatibility with the hardware of the system.
    let virtual_packages = wrap_in_progress("determining virtual packages", move || {
        if let Some(virtual_packages) = opt.virtual_package {
            Ok(virtual_packages
                .iter()
                .map(|virt_pkg| {
                    let elems = virt_pkg.split('=').collect::<Vec<&str>>();
                    Ok(GenericVirtualPackage {
                        name: elems[0].try_into()?,
                        version: elems
                            .get(1)
                            .map_or(Version::from_str("0"), |s| Version::from_str(s))
                            .expect("Could not parse virtual package version"),
                        build_string: (*elems.get(2).unwrap_or(&"")).to_string(),
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?)
        } else {
            rattler_virtual_packages::VirtualPackage::detect(
                &rattler_virtual_packages::VirtualPackageOverrides::default(),
            )
            .map(|vpkgs| {
                vpkgs
                    .iter()
                    .map(|vpkg| GenericVirtualPackage::from(vpkg.clone()))
                    .collect::<Vec<_>>()
            })
            .map_err(anyhow::Error::from)
        }
    })?;

    println!(
        "Virtual packages:\n{}\n",
        virtual_packages
            .iter()
            .format_with("\n", |i, f| f(&format_args!("  - {i}",)))
    );

    // Now that we parsed and downloaded all information, construct the packaging
    // problem that we need to solve. We do this by constructing a
    // `SolverProblem`. This encapsulates all the information required to be
    // able to solve the problem.
    let locked_packages = installed_packages
        .iter()
        .map(|record| record.repodata_record.clone())
        .collect();

    let solver_task = SolverTask {
        locked_packages,
        virtual_packages,
        specs: specs.clone(),
        timeout: opt.timeout.map(Duration::from_millis),
        strategy: opt.strategy.map_or_else(Default::default, Into::into),
        ..SolverTask::from_iter(&repo_data)
    };

    // Next, use a solver to solve this specific problem. This provides us with all
    // the operations we need to apply to our environment to bring it up to
    // date.
    let solver_result =
        wrap_in_progress("solving", move || match opt.solver.unwrap_or_default() {
            Solver::Resolvo => resolvo::Solver.solve(solver_task),
            Solver::LibSolv => libsolv_c::Solver.solve(solver_task),
        })?;

    let mut required_packages: Vec<RepoDataRecord> = solver_result.records;

    if opt.no_deps {
        required_packages.retain(|r| specs.iter().any(|s| s.matches(&r.package_record)));
    } else if opt.only_deps {
        required_packages.retain(|r| !specs.iter().any(|s| s.matches(&r.package_record)));
    };

    if opt.dry_run {
        // Construct a transaction to
        let transaction = Transaction::from_current_and_desired(
            installed_packages,
            required_packages,
            None,
            None, // ignored packages
            install_platform,
        )?;

        if transaction.operations.is_empty() {
            println!("No operations necessary");
        } else {
            print_transaction(&transaction, solver_result.extras);
        }

        return Ok(());
    }

    let install_start = Instant::now();
    let result = Installer::new()
        .with_download_client(download_client)
        .with_target_platform(install_platform)
        .with_installed_packages(installed_packages)
        .with_execute_link_scripts(true)
        .with_requested_specs(specs)
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(global_multi_progress())
                .finish(),
        )
        .install(&target_prefix, required_packages)
        .await?;

    if result.transaction.operations.is_empty() {
        println!(
            "{} Already up to date",
            console::style(console::Emoji("✔", "")).green(),
        );
    } else {
        println!(
            "{} Successfully updated the environment in {:?}",
            console::style(console::Emoji("✔", "")).green(),
            install_start.elapsed()
        );
        print_transaction(&result.transaction, solver_result.extras);
    }

    Ok(())
}

/// Prints the operations of the transaction to the console.
fn print_transaction(
    transaction: &Transaction<PrefixRecord, RepoDataRecord>,
    features: HashMap<PackageName, Vec<String>>,
) {
    let format_record = |r: &RepoDataRecord| {
        let direct_url_print = if let Some(channel) = &r.channel {
            channel.clone()
        } else {
            String::new()
        };

        if let Some(features) = features.get(&r.package_record.name) {
            format!(
                "{}[{}] {} {} {}",
                r.package_record.name.as_normalized(),
                features.join(", "),
                r.package_record.version,
                r.package_record.build,
                direct_url_print,
            )
        } else {
            format!(
                "{} {} {} {}",
                r.package_record.name.as_normalized(),
                r.package_record.version,
                r.package_record.build,
                direct_url_print,
            )
        }
    };

    for operation in &transaction.operations {
        match operation {
            TransactionOperation::Install(r) => {
                println!("{} {}", console::style("+").green(), format_record(r));
            }
            TransactionOperation::Change { old, new } => {
                println!(
                    "{} {} -> {}",
                    console::style("~").yellow(),
                    format_record(&old.repodata_record),
                    format_record(new)
                );
            }
            TransactionOperation::Reinstall { old, .. } => {
                println!(
                    "{} {}",
                    console::style("~").yellow(),
                    format_record(&old.repodata_record)
                );
            }
            TransactionOperation::Remove(r) => {
                println!(
                    "{} {}",
                    console::style("-").red(),
                    format_record(&r.repodata_record)
                );
            }
        }
    }
}

/// Displays a spinner with the given message while running the specified
/// function to completion.
fn wrap_in_progress<T, F: FnOnce() -> T>(msg: impl Into<Cow<'static, str>>, func: F) -> T {
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_message(msg);
    let result = func();
    pb.finish_and_clear();
    result
}

/// Displays a spinner with the given message while running the specified
/// function to completion.
async fn wrap_in_async_progress<T, F: IntoFuture<Output = T>>(
    msg: impl Into<Cow<'static, str>>,
    fut: F,
) -> T {
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_style(long_running_progress_style());
    pb.set_message(msg);
    let result = fut.into_future().await;
    pb.finish_and_clear();
    result
}

/// Returns the style to use for a progressbar that is indeterminate and simply
/// shows a spinner.
fn long_running_progress_style() -> indicatif::ProgressStyle {
    ProgressStyle::with_template("{spinner:.green} {msg}").unwrap()
}
