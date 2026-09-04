use std::{collections::HashMap, env, path::PathBuf, time::Instant};

use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler::{
    default_cache_dir,
    install::{IndicatifReporter, Installer, Transaction, TransactionOperation},
    package_cache::PackageCache,
};
use rattler_conda_types::{ChannelConfig, PackageName, Platform, PrefixRecord, RepoDataRecord};
use rattler_repodata_gateway::{Gateway, RepoData, SourceConfig};
use rattler_solve::SolverTask;

use crate::{
    commands::progress::{wrap_in_async_progress, wrap_in_progress},
    global_multi_progress,
    solver_args::SolverArgs,
};

/// Create a conda environment from package listing
///
/// Resolves and installs the specified packages into a target prefix,
/// pulling from the configured channels.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Package specs to install
    #[clap(required = true)]
    specs: Vec<String>,

    #[clap(flatten)]
    solver: SolverArgs,

    /// Simulate command without installation
    #[clap(long)]
    dry_run: bool,

    /// Target prefix (environment path) for package installation
    #[clap(
        short = 'p',
        long = "prefix",
        visible_alias = "target-prefix",
        default_value = ".prefix"
    )]
    target_prefix: PathBuf,
}

pub async fn create(opt: Opt, offline: bool) -> miette::Result<()> {
    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);
    // Make the target prefix absolute
    let target_prefix = std::path::absolute(opt.target_prefix).into_diagnostic()?;

    let install_platform = opt.solver.platform;

    println!("Installing for platform: {install_platform}");

    // Parse the specs from the command line. We do this explicitly instead of allow
    // clap to deal with this because we need to parse the `channel_config` when
    // parsing matchspecs.
    let specs = SolverArgs::parse_specs(&opt.specs)?;
    let constraints = opt.solver.constraints()?;

    // Find the default cache directory. Create it if it doesn't exist yet.
    let cache_dir = default_cache_dir()
        .map_err(|e| miette::miette!("could not determine default cache directory: {}", e))?;
    rattler_cache::ensure_cache_dir(&cache_dir)
        .map_err(|e| miette::miette!("could not create cache directory: {}", e))?;

    // Determine the channels to use from the command line or select the default.
    // Like matchspecs this also requires the use of the `channel_config` so we
    // have to do this manually.
    let channels = opt.solver.channels(&channel_config)?;

    // Determine the packages that are currently installed in the environment.
    let installed_packages =
        PrefixRecord::collect_from_prefix::<PrefixRecord>(&target_prefix).into_diagnostic()?;

    // For each channel/subdirectory combination, download and cache the
    // `repodata.json` that should be available from the corresponding Url. The
    // code below also displays a nice CLI progress-bar to give users some more
    // information about what is going on.
    let download_client = super::client::create_client_with_middleware(offline)?;

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
            .query(
                channels,
                [install_platform, Platform::NoArch],
                specs.clone(),
            )
            .recursive(true),
    )
    .await
    .into_diagnostic()
    .context("failed to load repodata")?;

    // Surface any non-fatal CEP-42 channel-relation problems.
    for warning in &repo_data.warnings {
        eprintln!("warning: {warning}");
    }

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
    let virtual_packages = wrap_in_progress("determining virtual packages", || {
        opt.solver.virtual_packages()
    })?;

    println!(
        "Virtual packages:\n{}\n",
        virtual_packages
            .iter()
            .format_with("\n", |i, f| f(&format_args!("  - {i}",)))
    );

    if !constraints.is_empty() {
        println!(
            "Constraints:\n{}\n",
            constraints
                .iter()
                .format_with("\n", |i, f| f(&format_args!("  - {i}",)))
        );
    }

    // Now that we parsed and downloaded all information, construct the packaging
    // problem that we need to solve. We do this by constructing a
    // `SolverProblem`. This encapsulates all the information required to be
    // able to solve the problem.
    let locked_packages: Vec<&RepoDataRecord> = installed_packages
        .iter()
        .map(|record| &record.repodata_record)
        .collect();

    let solver_task = SolverTask {
        locked_packages,
        virtual_packages,
        specs: specs.clone(),
        constraints,
        timeout: opt.solver.timeout(),
        strategy: opt.solver.strategy(),
        channel_priority: opt.solver.channel_priority(),
        exclude_newer: opt.solver.exclude_newer(),
        ..SolverTask::from_iter(&repo_data)
    };

    // Next, use a solver to solve this specific problem. This provides us with all
    // the operations we need to apply to our environment to bring it up to
    // date.
    let solver_result =
        wrap_in_progress("solving", || opt.solver.solve(solver_task)).into_diagnostic()?;

    let mut required_packages: Vec<RepoDataRecord> = solver_result.records;
    opt.solver.filter_deps_mode(&mut required_packages, &specs);

    if opt.dry_run {
        // Construct a transaction to
        let transaction = Transaction::from_current_and_desired(
            installed_packages,
            required_packages,
            None,
            None, // ignored packages
            install_platform,
        )
        .into_diagnostic()?;

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
        .await
        .into_diagnostic()?;

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
        // Since operations are nonempty we can safely unwrap.
        let transaction = result
            .transaction
            .into_prefix_record(target_prefix)
            .unwrap();
        print_transaction(&transaction, solver_result.extras);
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
