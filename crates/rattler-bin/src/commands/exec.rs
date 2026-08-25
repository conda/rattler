use clap::{Parser, ValueHint};
use miette::{Context, IntoDiagnostic};
use rattler::{
    default_cache_dir,
    install::{IndicatifReporter, Installer},
    package_cache::PackageCache,
};
use rattler_cache::EXEC_ENVS_DIR;
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, Matches, PackageName,
    ParseMatchSpecOptions, Platform,
};
use rattler_repodata_gateway::{
    Gateway, RepoData, RepoDataQueryResult, ShardQuerySnapshot, SourceConfig,
};
use rattler_shell::shell::ShellEnum;
use rattler_solve::{SolverImpl, SolverTask, resolvo::Solver};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    env, fs,
    path::{Path, PathBuf},
    str::FromStr,
};
use tokio;

use crate::{
    commands::{
        client::{create_client_with_middleware, repodata_cache_action},
        progress::{wrap_in_async_progress, wrap_in_progress},
    },
    global_multi_progress,
};

/// Run a command and install it in a temporary environment.
#[derive(Debug, Parser)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Opt {
    /// The executable to run, followed by any arguments.
    #[clap(num_args = 1.., value_hint = ValueHint::CommandWithArguments)]
    pub command: Vec<String>,

    /// Matchspecs of packages to install.
    /// When omitted, the package is guessed from the command name.
    #[clap(long = "spec", short = 's', value_name = "SPEC")]
    pub specs: Vec<String>,

    /// Matchspecs of package to install, while also guessing a package
    /// from the command.
    #[clap(long, short = 'w', conflicts_with = "specs")]
    pub with: Vec<String>,

    /// Channels to search for packages.
    #[clap(short, long = "channel")]
    pub channels: Option<Vec<String>>,

    /// The platform to create the environment for.
    #[clap(long, short, default_value_t = Platform::current())]
    pub platform: Platform,

    /// Always create a new environment, even if one already exists.
    #[clap(long)]
    pub force_reinstall: bool,

    /// Before executing the command, list packages in the environment
    /// Specify `--list=some_regex` to filter the shown packages    
    #[clap(long = "list", num_args = 0..=1, default_missing_value = "", require_equals = true)]
    pub list: Option<String>,

    /// Disable modification of PS1 to indicate the temporary environment.
    #[clap(long)]
    pub no_modify_ps1: bool,
}

/// The solver inputs and shard-index entries that produced an exec environment.
///
/// This is intentionally private to `rattler exec`: it is a fail-closed cache
/// validation format, not a lockfile format.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ShardStamp {
    input: ResolutionInput,
    query_snapshot: ShardQuerySnapshot,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ResolutionInput {
    rattler_version: String,
    specs: Vec<String>,
    channels: Vec<String>,
    platform: Platform,
    virtual_packages: Vec<String>,
}

/// CLI entry point for `rattler exec`.
pub async fn exec(opt: Opt, offline: bool) -> miette::Result<()> {
    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);

    let mut command_parts = opt.command.iter();
    let command = command_parts.next().ok_or_else(|| {
        miette::miette!(
            help = "e.g. `rattler exec --spec python=3.12 python`",
            "missing required command to execute"
        )
    })?;

    // Parse channels (default: conda-forge)
    let channels = opt
        .channels
        .unwrap_or_else(|| vec![String::from("conda-forge")])
        .into_iter()
        .map(|c| Channel::from_str(&c, &channel_config))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    // Determine the specs for installation and for the environment name.
    let explicit_specs = parse_specs(&opt.specs)?;
    let with_specs = parse_specs(&opt.with)?;

    // Guess a package from the command if no specs were provided at all OR if --with is used
    let should_guess = opt.specs.is_empty() || !opt.with.is_empty();

    let mut install_specs = explicit_specs.clone();
    install_specs.extend(with_specs.clone());
    if should_guess {
        install_specs.push(guess_package_spec(command));
    }

    // Locate / create the shared rattler cache
    let cache_dir = default_cache_dir()
        .map_err(|e| miette::miette!("could not determine cache directory: {}", e))?;
    rattler_cache::ensure_cache_dir(&cache_dir)
        .map_err(|e| miette::miette!("could not create cache directory: {}", e))?;

    let dir_prefix = exec_dir_prefix(&install_specs, Some(command), should_guess);

    // Solve + install (or reuse) the cached environment
    let prefix = create_exec_prefix(CreateExecPrefixOptions {
        specs: &install_specs,
        channels: &channels,
        platform: opt.platform,
        dir_prefix,
        force_reinstall: opt.force_reinstall,
        list: opt.list.as_deref(),
        cache_dir: &cache_dir,
        offline,
    })
    .await?;

    // Build extra environment variables
    let mut extra_env: HashMap<String, String> = HashMap::new();

    // Collect display names from the named specs (not the guessed one)
    let display_names: BTreeSet<String> = explicit_specs
        .iter()
        .chain(with_specs.iter())
        .filter_map(|s| s.name.as_exact().map(|n| n.as_normalized().to_string()))
        .collect();

    if !display_names.is_empty() {
        let env_name = format!(
            "temp:{}",
            display_names.iter().cloned().collect::<Vec<_>>().join(",")
        );
        extra_env.insert("PIXI_ENVIRONMENT_NAME".into(), env_name.clone());

        if !opt.no_modify_ps1 {
            // Mirror pixi exec's prompt formatting exactly
            let (var, val) = if cfg!(windows) {
                (
                    "_RATTLER_PROMPT".to_string(),
                    format!("(rattler:{env_name}) $P$G"),
                )
            } else {
                ("PS1".to_string(), format!(r"(rattler:{env_name}) [\w] \$"))
            };
            extra_env.insert(var, val);

            // Windows cmd.exe also needs PROMPT
            if cfg!(windows) {
                extra_env.insert("PROMPT".into(), "$P$G".into());
            }
        }
    }

    let full_command: Vec<String> = std::iter::once(command.clone())
        .chain(command_parts.cloned())
        .collect();

    // Ignore CTRL+C so that the child is solely responsible for its own signal handling.
    let _ctrl_c = tokio::spawn(async { while tokio::signal::ctrl_c().await.is_ok() {} });

    let shell = ShellEnum::from_env().unwrap_or_default();
    let status =
        rattler_shell::run_command_in_environment(&prefix, &full_command, shell, &extra_env, None)
            .await
            .map_err(|e| miette::miette!("failed to execute '{}': {}", command, e))?;

    std::process::exit(status.code().unwrap_or(1));
}

struct CreateExecPrefixOptions<'a> {
    specs: &'a [MatchSpec],
    channels: &'a [Channel],
    platform: Platform,
    dir_prefix: Option<String>,
    force_reinstall: bool,
    list: Option<&'a str>,
    cache_dir: &'a Path,
    offline: bool,
}

/// Creates a prefix for the `rattler exec` command.
async fn create_exec_prefix(options: CreateExecPrefixOptions<'_>) -> miette::Result<PathBuf> {
    let CreateExecPrefixOptions {
        specs,
        channels,
        platform,
        dir_prefix,
        force_reinstall,
        list,
        cache_dir,
        offline,
    } = options;
    let channel_urls: Vec<String> = channels.iter().map(|c| c.base_url.to_string()).collect();
    let env_hash = compute_env_hash(specs, &channel_urls, platform);

    let dir_name = match dir_prefix {
        Some(ref p) => format!("{}-{}", p, &env_hash[..8]),
        None => env_hash[..16].to_string(),
    };

    let prefix = cache_dir.join(EXEC_ENVS_DIR).join(&dir_name);

    let sentinel = prefix.join(".exec-ready");
    let download_client = create_client_with_middleware(offline)?;
    let gateway = Gateway::builder()
        .with_cache_dir(cache_dir.join(rattler_cache::REPODATA_CACHE_DIR))
        .with_package_cache(PackageCache::new(
            cache_dir.join(rattler_cache::PACKAGE_CACHE_DIR),
        ))
        .with_client(download_client.clone())
        .with_channel_config(rattler_repodata_gateway::ChannelConfig {
            default: SourceConfig {
                sharded_enabled: true,
                cache_action: repodata_cache_action(offline),
                ..SourceConfig::default()
            },
            per_channel: HashMap::new(),
        })
        .finish();
    let virtual_packages = detect_virtual_packages()?;
    let input = resolution_input(specs, channels, platform, &virtual_packages);

    // A ready prefix is reusable only if the exact solver inputs and the
    // query's complete CEP-16 lookup trace are still unchanged.
    let mut refreshed_repo_data = None;
    if sentinel.exists() && !force_reinstall {
        match read_shard_stamp(&sentinel) {
            Ok(stamp) if stamp.input == input => {
                match wrap_in_async_progress(
                    "checking environment freshness",
                    gateway
                        .query(
                            channels.to_vec(),
                            [platform, Platform::NoArch],
                            specs.to_vec(),
                        )
                        .recursive(true)
                        .execute_if_unchanged(&stamp.query_snapshot),
                )
                .await
                {
                    Ok(RepoDataQueryResult::NotModified) => {
                        tracing::info!("reusing up-to-date environment in {}", prefix.display());
                        return Ok(prefix);
                    }
                    Ok(RepoDataQueryResult::Updated(output)) => {
                        tracing::info!(
                            "environment in {} is stale or cannot use sharded repodata; resolving again",
                            prefix.display()
                        );
                        refreshed_repo_data = Some(output);
                    }
                    Err(error) => tracing::info!(
                        "could not validate cached environment in {} ({error}); resolving again",
                        prefix.display()
                    ),
                }
            }
            Ok(_) => tracing::info!(
                "cached environment in {} has different solver inputs; resolving again",
                prefix.display()
            ),
            Err(error) => tracing::info!(
                "cached environment in {} has no usable shard stamp ({error}); resolving again",
                prefix.display()
            ),
        }
    }

    let repo_data = match refreshed_repo_data {
        Some(output) => output,
        None => wrap_in_async_progress(
            "fetching repodata",
            gateway
                .query(
                    channels.to_vec(),
                    [platform, Platform::NoArch],
                    specs.to_vec(),
                )
                .recursive(true),
        )
        .await
        .into_diagnostic()
        .context("failed to fetch repodata")?,
    };

    // Surface any non-fatal CEP-42 channel-relation problems.
    for warning in &repo_data.warnings {
        eprintln!("warning: {warning}");
    }

    let total_records: usize = repo_data.iter().map(RepoData::len).sum();
    tracing::debug!("loaded {} records from repodata", total_records);

    let shard_stamp = repo_data
        .shard_query_snapshot()
        .cloned()
        .map(|query_snapshot| ShardStamp {
            input,
            query_snapshot,
        });

    let solver_task = SolverTask {
        specs: specs.to_vec(),
        virtual_packages,
        ..SolverTask::from_iter(&repo_data)
    };

    let solved = wrap_in_progress("solving environment", || Solver.solve(solver_task))
        .into_diagnostic()
        .context("failed to solve environment")?;

    // A stale prefix must be recreated instead of linked over. Do this only
    // after resolving successfully, so a solve failure leaves the old prefix intact.
    if prefix.exists() {
        fs::remove_dir_all(&prefix)
            .into_diagnostic()
            .context("failed to remove stale exec environment")?;
    }

    // Solve the environment
    tracing::info!(
        "installing environment in {}",
        dunce::canonicalize(&prefix)
            .as_deref()
            .unwrap_or(&prefix)
            .display()
    );

    Installer::new()
        .with_target_platform(platform)
        .with_download_client(download_client)
        .with_package_cache(PackageCache::new(
            cache_dir.join(rattler_cache::PACKAGE_CACHE_DIR),
        ))
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(global_multi_progress())
                .clear_when_done(true)
                .finish(),
        )
        .install(&prefix, solved.records.clone())
        .await
        .into_diagnostic()
        .context("failed to install environment")?;

    if let Some(shard_stamp) = shard_stamp {
        write_shard_stamp(&sentinel, &shard_stamp)?;
    } else {
        fs::write(&sentinel, b"")
            .into_diagnostic()
            .context("failed to write sentinel file")?;
    }

    if let Some(regex) = list {
        list_environment(specs, &solved.records, regex)?;
    }

    Ok(prefix)
}

fn parse_specs(raw: &[String]) -> miette::Result<Vec<MatchSpec>> {
    raw.iter()
        .map(|s| {
            MatchSpec::from_str(s, ParseMatchSpecOptions::default())
                .into_diagnostic()
                .with_context(|| format!("failed to parse matchspec '{s}'"))
        })
        .collect()
}

fn detect_virtual_packages() -> miette::Result<Vec<GenericVirtualPackage>> {
    VirtualPackage::detect(
        &VirtualPackageOverrides::from_env(),
        rattler::default_cache_dir().ok().as_deref(),
    )
    .into_diagnostic()
    .context("failed to determine virtual packages")
    .map(|packages| {
        packages
            .into_iter()
            .map(GenericVirtualPackage::from)
            .collect()
    })
}

fn resolution_input(
    specs: &[MatchSpec],
    channels: &[Channel],
    platform: Platform,
    virtual_packages: &[GenericVirtualPackage],
) -> ResolutionInput {
    let mut specs: Vec<_> = specs.iter().map(ToString::to_string).collect();
    specs.sort_unstable();
    let mut virtual_packages: Vec<_> = virtual_packages.iter().map(ToString::to_string).collect();
    virtual_packages.sort_unstable();
    ResolutionInput {
        rattler_version: env!("CARGO_PKG_VERSION").to_string(),
        specs,
        channels: channels
            .iter()
            .map(|channel| channel.base_url.to_string())
            .collect(),
        platform,
        virtual_packages,
    }
}

fn read_shard_stamp(path: &Path) -> miette::Result<ShardStamp> {
    let bytes = fs::read(path).into_diagnostic()?;
    serde_json::from_slice(&bytes).into_diagnostic()
}

fn write_shard_stamp(path: &Path, stamp: &ShardStamp) -> miette::Result<()> {
    let temporary_path = path.with_extension("tmp");
    fs::write(
        &temporary_path,
        serde_json::to_vec(stamp).into_diagnostic()?,
    )
    .into_diagnostic()?;
    fs::rename(&temporary_path, path).into_diagnostic()?;
    Ok(())
}

/// Produces a deterministic hex hash over (sorted specs, sorted channels, platform).
///
/// Two invocations with the same logical environment always produce the same
/// hash, regardless of argument order.
fn compute_env_hash(specs: &[MatchSpec], channels: &[String], platform: Platform) -> String {
    let mut sorted_specs: Vec<String> =
        specs.iter().map(std::string::ToString::to_string).collect();
    sorted_specs.sort_unstable();

    let mut sorted_channels = channels.to_vec();
    sorted_channels.sort_unstable();

    let mut hasher = Sha256::new();
    hasher.update(sorted_specs.join("|"));
    hasher.update("|");
    hasher.update(sorted_channels.join("|"));
    hasher.update("|");
    hasher.update(platform.to_string());

    hex::encode(hasher.finalize())
}

/// Returns the human-readable prefix used in the cached env directory name.
fn exec_dir_prefix(
    specs: &[MatchSpec],
    command: Option<&str>,
    has_guessed_package: bool,
) -> Option<String> {
    if let [single] = specs {
        return single
            .name
            .as_exact()
            .map(|n| n.as_normalized().to_string());
    }
    if has_guessed_package {
        return command.and_then(|c| {
            guess_package_spec(c)
                .name
                .as_exact()
                .map(|n| n.as_normalized().to_string())
        });
    }
    None
}

/// Converts a command name into a best-guess package `MatchSpec` by replacing
/// every character that is illegal in conda package names with a dash.
fn guess_package_spec(command: &str) -> MatchSpec {
    MatchSpec {
        name: PackageName::from_str(command)
            .expect("all illegal characters have been sanitized")
            .into(),
        ..Default::default()
    }
}

/// Prints a table of installed packages, with explicitly requested ones marked.
/// Optionally filtered to packages whose names match `regex`.
fn list_environment(
    specs: &[MatchSpec],
    records: &[rattler_conda_types::RepoDataRecord],
    regex: &str,
) -> miette::Result<()> {
    let regex_filter = if regex.is_empty() {
        None
    } else {
        Some(regex::Regex::new(regex).into_diagnostic()?)
    };

    let mut packages: Vec<_> = records
        .iter()
        .filter(|r| {
            regex_filter
                .as_ref()
                .is_none_or(|re| re.is_match(r.package_record.name.as_normalized()))
        })
        .collect();

    packages.sort_by(|a, b| a.package_record.name.cmp(&b.package_record.name));

    let count = packages.len();
    let header = match &regex_filter {
        Some(re) => format!(
            "The environment has {} packages filtered by `{}`:",
            console::style(count).bold(),
            re,
        ),
        None => format!(
            "The environment has {} packages:",
            console::style(count).bold(),
        ),
    };
    println!("{header}");

    for r in &packages {
        let is_explicit = specs.iter().any(|s| s.matches(&r.package_record));
        let bullet = if is_explicit {
            console::style("*").green().bold()
        } else {
            console::style(" ").dim()
        };
        println!(
            "  {} {:<40} {}",
            bullet,
            r.package_record.name.as_normalized(),
            r.package_record.version,
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::{MatchSpec, ParseStrictness};

    use super::{compute_env_hash, exec_dir_prefix};
    use rattler_conda_types::Platform;

    fn spec(s: &str) -> MatchSpec {
        MatchSpec::from_str(s, ParseStrictness::Lenient).unwrap()
    }

    #[test]
    fn single_explicit_spec_wins() {
        let prefix = exec_dir_prefix(&[spec("ripgrep")], Some("rg"), false);
        assert_eq!(prefix.as_deref(), Some("ripgrep"));
    }

    #[test]
    fn guessed_only_uses_command() {
        let prefix = exec_dir_prefix(&[spec("rg")], Some("rg"), true);
        assert_eq!(prefix.as_deref(), Some("rg"));
    }

    #[test]
    fn with_uses_command_not_extra_spec() {
        let prefix = exec_dir_prefix(&[spec("numpy"), spec("python")], Some("python"), true);
        assert_eq!(prefix.as_deref(), Some("python"));
    }

    #[test]
    fn multiple_explicit_specs_have_no_prefix() {
        let prefix = exec_dir_prefix(&[spec("foo"), spec("bar")], Some("cmd"), false);
        assert_eq!(prefix, None);
    }

    #[test]
    fn env_hash_is_deterministic() {
        let specs = vec![spec("python=3.12"), spec("numpy")];
        let channels = vec!["https://conda.anaconda.org/conda-forge/".to_string()];
        let h1 = compute_env_hash(&specs, &channels, Platform::Linux64);
        let h2 = compute_env_hash(&specs, &channels, Platform::Linux64);
        assert_eq!(h1, h2);
    }

    #[test]
    fn env_hash_is_order_independent() {
        let channels = vec!["https://conda.anaconda.org/conda-forge/".to_string()];
        let h1 = compute_env_hash(
            &[spec("numpy"), spec("python=3.12")],
            &channels,
            Platform::Linux64,
        );
        let h2 = compute_env_hash(
            &[spec("python=3.12"), spec("numpy")],
            &channels,
            Platform::Linux64,
        );
        assert_eq!(h1, h2);
    }

    #[test]
    fn env_hash_differs_by_platform() {
        let specs = vec![spec("python=3.12")];
        let channels = vec!["https://conda.anaconda.org/conda-forge/".to_string()];
        let h_linux = compute_env_hash(&specs, &channels, Platform::Linux64);
        let h_osx = compute_env_hash(&specs, &channels, Platform::OsxArm64);
        assert_ne!(h_linux, h_osx);
    }
}
