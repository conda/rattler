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
    ParseMatchSpecOptions, Subdir,
};
use rattler_config::{ConfigBase, NoExtension};
use rattler_lock::{DEFAULT_ENVIRONMENT_NAME, LockFile, PlatformData, PlatformName};
use rattler_networking::LazyClient;
use rattler_repodata_gateway::RepoData;
use rattler_shell::shell::ShellEnum;
use rattler_solve::{SolverImpl, SolverTask, resolvo::Solver};
use rattler_vfs::{
    Mode, MountConfig, MountHandle, Transport, build_and_mount_with_client, force_unmount,
};
use rattler_virtual_packages::{VirtualPackage, VirtualPackageOverrides};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    env,
    path::{Path, PathBuf},
    str::FromStr,
};

use crate::{
    commands::{
        client::create_client_with_middleware,
        gateway::{build_gateway, load_config, resolve_channels},
        hyperlink,
        progress::{wrap_in_async_progress, wrap_in_progress},
        table::{Cell, Table},
    },
    global_multi_progress,
};

/// Run a command and install it in a temporary environment.
#[derive(Debug, Parser)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
#[clap(after_help = r#"Examples:
  rattler exec ruff check .                            # the package is guessed from the command name
  rattler exec --spec python=3.12 python --version     # pick the package(s) explicitly
  rattler exec --with rich python -c "import rich"     # guess python and add another package"#)]
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

    /// The platform to create the environment for. Defaults to the platform
    /// of the current host.
    #[clap(long, short)]
    pub platform: Option<Subdir>,

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

    /// Serve the environment through a virtual filesystem mount instead of
    /// installing it to disk.
    ///
    /// The packages are served lazily from the shared package cache, so
    /// nothing is extracted. This is faster for fresh environments and for
    /// moderately sized environments that are only run a handful of times;
    /// for environments that are reused often, the default on-disk install
    /// is faster.
    #[clap(long)]
    pub dematerialized: bool,
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

    let config = load_config()?;
    let channels = resolve_channels(opt.channels.as_deref(), &config, &channel_config)?;

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

    let cache_dir = default_cache_dir()
        .map_err(|e| miette::miette!("could not determine cache directory: {}", e))?;
    rattler_cache::ensure_cache_dir(&cache_dir)
        .map_err(|e| miette::miette!("could not create cache directory: {}", e))?;

    let dir_prefix = exec_dir_prefix(&install_specs, Some(command), should_guess);

    // Solve the environment and either install it to disk (default) or, with
    // `--dematerialized`, mount it as a virtual conda prefix served lazily from
    // the shared package cache. A `mount` must stay alive until the
    // child process exits; dropping it unmounts the prefix.
    let (prefix, mount) = create_exec_prefix(CreateExecPrefixOptions {
        specs: &install_specs,
        channels: &channels,
        config: &config,
        platform: opt.platform.map_or_else(crate::host_platform, Ok)?,
        dir_prefix,
        force_reinstall: opt.force_reinstall,
        list: opt.list.as_deref(),
        cache_dir: &cache_dir,
        offline,
        dematerialized: opt.dematerialized,
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

    // `std::process::exit` skips destructors, so the mount would leak (leaving a
    // stale mount at the prefix) unless we tear it down explicitly here.
    if let Some(mount) = mount {
        mount.unmount().await?;
    }

    std::process::exit(status.code().unwrap_or(1));
}

struct CreateExecPrefixOptions<'a> {
    specs: &'a [MatchSpec],
    channels: &'a [Channel],
    config: &'a ConfigBase<NoExtension>,
    platform: Subdir,
    dir_prefix: Option<String>,
    force_reinstall: bool,
    list: Option<&'a str>,
    cache_dir: &'a Path,
    offline: bool,
    dematerialized: bool,
}

/// Solves (or reuses) an environment for `rattler exec`.
///
/// By default the environment is installed to an on-disk prefix (like
/// `pixi exec`), and later runs reuse it as-is. With `dematerialized`, nothing
/// is extracted: the packages are fetched into the shared package cache and
/// served lazily through a [`rattler_vfs`] mount. In that case the returned
/// [`ExecMount`] owns the live mount — keep it alive for as long as the
/// prefix is in use and unmount it (or drop it) afterwards.
async fn create_exec_prefix(
    options: CreateExecPrefixOptions<'_>,
) -> miette::Result<(PathBuf, Option<ExecMount>)> {
    let CreateExecPrefixOptions {
        specs,
        channels,
        config,
        platform,
        dir_prefix,
        force_reinstall,
        list,
        cache_dir,
        offline,
        dematerialized,
    } = options;
    let channel_urls: Vec<String> = channels.iter().map(|c| c.base_url.to_string()).collect();
    let env_hash = compute_env_hash(specs, &channel_urls, platform);

    let mut dir_name = match dir_prefix {
        Some(ref p) => format!("{}-{}", p, &env_hash[..8]),
        None => env_hash[..16].to_string(),
    };
    // Keep mounted and installed environments apart, so a mount never shadows
    // an installed prefix (and vice versa).
    if dematerialized {
        dir_name.push_str("-vfs");
    }

    let prefix = cache_dir.join(EXEC_ENVS_DIR).join(&dir_name);
    let package_cache = PackageCache::new(cache_dir.join(rattler_cache::PACKAGE_CACHE_DIR));

    if dematerialized {
        // The VFS mount has no persistent on-disk prefix, so we cache the
        // solved lock file instead. On a warm run we skip repodata + solve
        // entirely and mount straight from the cached lock file. The state
        // lives next to the mount point, not inside it: while a mount is live
        // the mount point shows the virtual environment, so a concurrent run
        // could not see files stored in the directory underneath.
        let state_dir = prefix;
        let lockfile_path = state_dir.join("lock.yml");
        // Earlier versions mounted the environment at what is now the state
        // directory; a run of one of those that was killed leaves a dead mount
        // here, which would make every file operation below fail.
        clear_stale_mount(&state_dir);

        let (lockfile, client): (LockFile, LazyClient) =
            if lockfile_path.exists() && !force_reinstall {
                tracing::info!(
                    "reusing solved environment from {}",
                    lockfile_path.display()
                );
                let lockfile = LockFile::from_path(&lockfile_path)
                    .into_diagnostic()
                    .context("failed to read cached lock file")?;
                // Only needed when a package is missing from the cache, and
                // creating it loads the system certificates, so defer it.
                let client = LazyClient::new(move || {
                    create_client_with_middleware(offline).unwrap_or_else(|e| {
                        tracing::warn!("failed to create download client: {e:?}");
                        reqwest_middleware::ClientWithMiddleware::default()
                    })
                });
                (lockfile, client)
            } else {
                let (records, download_client) =
                    solve_environment(specs, channels, config, platform, list, offline).await?;

                // Turn the solved records into an in-memory lock file that
                // `rattler_vfs` consumes. A single `default`
                // environment/platform is enough here.
                let lockfile = lockfile_from_records(&records, platform)
                    .context("failed to build lock file from solved records")?;

                // Persist the lock file so subsequent runs can reuse the solve.
                // Written to a temporary file and renamed, so a concurrent run
                // never reads a partial lock file.
                std::fs::create_dir_all(&state_dir)
                    .into_diagnostic()
                    .context("failed to create environment directory")?;
                let tmp_path = state_dir.join(format!("lock.yml.{}.tmp", std::process::id()));
                lockfile
                    .to_path(&tmp_path)
                    .into_diagnostic()
                    .context("failed to write cached lock file")?;
                if let Err(e) = std::fs::rename(&tmp_path, &lockfile_path) {
                    // On Windows the rename fails while a concurrent run has
                    // the cached lock file open. That run wrote an equivalent
                    // solve, so carry on with ours in memory.
                    let _ = std::fs::remove_file(&tmp_path);
                    if !lockfile_path.is_file() {
                        return Err(e)
                            .into_diagnostic()
                            .context("failed to write cached lock file");
                    }
                    tracing::debug!("not replacing cached lock file: {e}");
                }
                (lockfile, download_client.into())
            };

        let mount = mount_prefix(MountPrefixOptions {
            lockfile: &lockfile,
            platform,
            package_cache: &package_cache,
            client,
            state_dir: &state_dir,
            env_hash: &env_hash,
        })
        .await?;
        return Ok((mount.mount_point.clone(), Some(mount)));
    }

    let sentinel = prefix.join(".exec-ready");

    // If the environment already exists, and we are not forcing a
    // reinstallation, we can return early.
    if sentinel.exists() && !force_reinstall {
        tracing::info!("reusing existing environment in {}", prefix.display());
        return Ok((prefix, None));
    }

    let (records, download_client) =
        solve_environment(specs, channels, config, platform, list, offline).await?;

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
        .with_package_cache(package_cache)
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(global_multi_progress())
                .clear_when_done(true)
                .finish(),
        )
        .install(&prefix, records)
        .await
        .into_diagnostic()
        .context("failed to install environment")?;

    // Mark the environment as ready so future runs can skip solve+install.
    std::fs::write(&sentinel, b"")
        .into_diagnostic()
        .context("failed to write sentinel file")?;

    Ok((prefix, None))
}

/// Fetches repodata and solves `specs` for `platform`, optionally printing the
/// solved environment. Returns the solved records and the download client so
/// the caller can reuse it to fetch packages.
async fn solve_environment(
    specs: &[MatchSpec],
    channels: &[Channel],
    config: &ConfigBase<NoExtension>,
    platform: Subdir,
    list: Option<&str>,
    offline: bool,
) -> miette::Result<(
    Vec<rattler_conda_types::RepoDataRecord>,
    reqwest_middleware::ClientWithMiddleware,
)> {
    let download_client = create_client_with_middleware(offline)?;

    let gateway = build_gateway(download_client.clone(), config, offline, true)?;

    let repo_data = wrap_in_async_progress(
        "fetching repodata",
        gateway
            .query(
                channels.to_vec(),
                [platform, Subdir::NoArch],
                specs.to_vec(),
            )
            .recursive(true),
    )
    .await
    .into_diagnostic()
    .context("failed to fetch repodata")?;

    // Surface any non-fatal CEP-42 channel-relation problems.
    for warning in &repo_data.warnings {
        eprintln!("warning: {warning}");
    }

    let total_records: usize = repo_data.iter().map(RepoData::len).sum();
    tracing::debug!("loaded {} records from repodata", total_records);

    // Determine virtual packages of the current platform
    let virtual_packages: Vec<GenericVirtualPackage> = VirtualPackage::detect(
        &VirtualPackageOverrides::from_env(),
        rattler::default_cache_dir().ok().as_deref(),
    )
    .into_diagnostic()
    .context("failed to determine virtual packages")?
    .into_iter()
    .map(GenericVirtualPackage::from)
    .collect();

    let solver_task = SolverTask {
        specs: specs.to_vec(),
        virtual_packages,
        ..SolverTask::from_iter(&repo_data)
    };

    let solved = wrap_in_progress("solving environment", || Solver.solve(solver_task))
        .into_diagnostic()
        .context("failed to solve environment")?;

    if let Some(regex) = list {
        list_environment(specs, &solved.records, regex)?;
    }

    Ok((solved.records, download_client))
}

/// Builds an in-memory [`LockFile`] with a single `default` environment holding
/// the solved conda records for `platform`.
fn lockfile_from_records(
    records: &[rattler_conda_types::RepoDataRecord],
    platform: Subdir,
) -> miette::Result<LockFile> {
    let mut builder = LockFile::builder()
        .with_platforms(vec![PlatformData {
            name: PlatformName::from(&platform),
            subdir: platform,
            virtual_packages: Vec::new(),
        }])
        .into_diagnostic()?;
    for record in records {
        builder
            .add_conda_package(
                DEFAULT_ENVIRONMENT_NAME,
                platform.as_str(),
                record.clone().into(),
            )
            .into_diagnostic()?;
    }
    Ok(builder.finish())
}

/// Whether we should attempt to force-unmount `path` before mounting.
///
/// Returns `true` when `path` is a mount point (it lives on a different
/// filesystem than its parent) or when its state can't be determined (a stat
/// error can indicate a stale mount whose userspace server has died). Returns
/// `false` for an ordinary or missing directory, so the common "nothing
/// mounted" case skips the `umount` subprocess entirely.
#[cfg(unix)]
fn should_force_unmount(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        // Nothing there yet (first run): nothing to clean up.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        // Ambiguous (possibly a stale/dead mount, which fails with ENOTCONN):
        // fall back to cleanup.
        Err(_) => return true,
    };
    match path.parent().and_then(|p| std::fs::metadata(p).ok()) {
        Some(parent_meta) => meta.dev() != parent_meta.dev(),
        None => false,
    }
}

/// On non-Unix targets there is no cheap portable mount-point probe, so keep
/// the previous always-attempt-cleanup behaviour.
#[cfg(not(unix))]
fn should_force_unmount(_path: &Path) -> bool {
    true
}

/// A live `--dematerialized` environment. Keep it alive for as long as the
/// prefix is in use, then call [`ExecMount::unmount`].
struct ExecMount {
    /// `None` once unmounted.
    handle: Option<MountHandle>,
    mount_point: PathBuf,
    /// Exclusive lock on the mount point, held while it is mounted. A run
    /// that finds the lock taken knows the mount is live (not stale) and
    /// mounts elsewhere instead of tearing it down. Released before the lock
    /// file is removed (Windows can't delete an open file).
    lock: Option<std::fs::File>,
    /// Whether the mount point is private to this run and should be removed
    /// after unmounting.
    ephemeral: bool,
}

impl ExecMount {
    async fn unmount(mut self) -> miette::Result<()> {
        if let Some(handle) = self.handle.take() {
            handle
                .unmount()
                .await
                .map_err(|e| miette::miette!("failed to unmount environment: {e}"))?;
        }
        self.remove_ephemeral_mount_point();
        Ok(())
    }

    fn remove_ephemeral_mount_point(&mut self) {
        if self.ephemeral {
            self.ephemeral = false;
            drop(self.lock.take());
            remove_mount_point(&self.mount_point);
        }
    }
}

impl Drop for ExecMount {
    /// Error paths drop the mount without calling [`ExecMount::unmount`]:
    /// dropping the handle unmounts (best effort), then clean up as usual.
    fn drop(&mut self) {
        drop(self.handle.take());
        self.remove_ephemeral_mount_point();
    }
}

/// Remove an (unmounted) private mount point and its lock file.
fn remove_mount_point(mount_point: &Path) {
    #[cfg(windows)]
    let removed = rattler_vfs::remove_projfs_root(mount_point);
    #[cfg(not(windows))]
    let removed = std::fs::remove_dir(mount_point);
    if let Err(e) = removed {
        tracing::debug!("failed to remove {}: {e}", mount_point.display());
    }
    let _ = std::fs::remove_file(mount_point.with_extension("lock"));
}

/// Detach a mount left at `path` by a run that was killed, if there is one.
fn clear_stale_mount(path: &Path) {
    if should_force_unmount(path) {
        let _ = force_unmount(path, Transport::best());
    }
}

/// Clean up the private `mnt-<pid>` mount points of runs that are gone: a
/// private mount point whose lock nobody holds belonged to a run that was
/// killed before it could remove it, and may still be a dead mount.
fn sweep_stale_private_mount_points(state_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(state_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_private_lock = path.extension().is_some_and(|ext| ext == "lock")
            && path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.strip_prefix("mnt-"))
                .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()));
        if !is_private_lock {
            continue;
        }
        let Ok(lock) = std::fs::OpenOptions::new().write(true).open(&path) else {
            continue;
        };
        if lock.try_lock().is_err() {
            continue; // in use by a live run
        }
        let mount_point = path.with_extension("");
        clear_stale_mount(&mount_point);
        drop(lock);
        remove_mount_point(&mount_point);
    }
}

/// Picks and locks the directory to mount the environment at.
///
/// Every run of the same environment uses `<state_dir>/mnt` when it is free.
/// When another run currently holds it, this run mounts at a private
/// `<state_dir>/mnt-<pid>` instead, so concurrent runs never unmount each
/// other.
fn claim_mount_point(state_dir: &Path) -> miette::Result<(PathBuf, std::fs::File, bool)> {
    let try_claim = |name: &str| -> miette::Result<Option<(PathBuf, std::fs::File)>> {
        let mount_point = state_dir.join(name);
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(mount_point.with_extension("lock"))
            .into_diagnostic()
            .context("failed to create mount lock")?;
        match lock.try_lock() {
            Ok(()) => Ok(Some((mount_point, lock))),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e)
                .into_diagnostic()
                .context("failed to lock mount point"),
        }
    };

    std::fs::create_dir_all(state_dir)
        .into_diagnostic()
        .context("failed to create environment directory")?;
    sweep_stale_private_mount_points(state_dir);
    if let Some((mount_point, lock)) = try_claim("mnt")? {
        return Ok((mount_point, lock, false));
    }
    let name = format!("mnt-{}", std::process::id());
    match try_claim(&name)? {
        Some((mount_point, lock)) => Ok((mount_point, lock, true)),
        None => Err(miette::miette!(
            "mount point {name} is locked by another process"
        )),
    }
}

struct MountPrefixOptions<'a> {
    lockfile: &'a LockFile,
    platform: Subdir,
    package_cache: &'a PackageCache,
    /// Used to download packages missing from the cache.
    client: LazyClient,
    /// Directory holding the environment's state and mount point(s).
    state_dir: &'a Path,
    env_hash: &'a str,
}

/// Mounts `lockfile` below `state_dir` via `rattler_vfs`, returning the live
/// mount. Clears any stale mount left behind by a previous crashed run first.
async fn mount_prefix(options: MountPrefixOptions<'_>) -> miette::Result<ExecMount> {
    let MountPrefixOptions {
        lockfile,
        platform,
        package_cache,
        client,
        state_dir,
        env_hash,
    } = options;
    let (mount_point, lock, ephemeral) = claim_mount_point(state_dir)?;

    // A previous run that was killed (SIGKILL, power loss) can leave a stale
    // mount at this path; clear it so the fresh mount can take over. We hold
    // the mount point's lock, so a mount found here is never a live one. Only
    // do this when the path actually looks mounted (or its state is
    // ambiguous) — otherwise every invocation would spawn a pointless `umount`
    // subprocess (adding latency + run-to-run jitter) and print a spurious
    // "not currently mounted" error.
    //
    // This has to happen before creating the directory: a dead FUSE mount
    // can't even be `stat`ed (ENOTCONN), so creating it would fail.
    clear_stale_mount(&mount_point);
    std::fs::create_dir_all(&mount_point)
        .into_diagnostic()
        .context("failed to create mount point")?;

    // ProjFS on Windows has no read-only mode, so ask for read-only where it is
    // supported and let it fall through to writable on Windows.
    let config = MountConfig::new(mount_point.clone(), Transport::best(), env_hash.to_string())
        .with_mode(Mode::ReadOnlyIfSupported);

    tracing::info!("mounting environment at {}", mount_point.display());
    let handle = build_and_mount_with_client(
        lockfile,
        DEFAULT_ENVIRONMENT_NAME,
        platform,
        package_cache,
        client,
        &config,
    )
    .await
    .map_err(|e| miette::miette!("failed to mount environment: {e}"))?;

    Ok(ExecMount {
        handle: Some(handle),
        mount_point,
        lock: Some(lock),
        ephemeral,
    })
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

/// Produces a deterministic hex hash over (sorted specs, sorted channels, platform).
///
/// Two invocations with the same logical environment always produce the same
/// hash, regardless of argument order.
fn compute_env_hash(specs: &[MatchSpec], channels: &[String], platform: Subdir) -> String {
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

    let mut table = Table::new().with_indent(2);
    for r in &packages {
        let is_explicit = specs.iter().any(|s| s.matches(&r.package_record));
        let bullet = if is_explicit {
            Cell::styled(console::style("*").green().bold(), "*")
        } else {
            Cell::plain(" ")
        };
        let name = r.package_record.name.as_normalized();
        table.add_row([
            bullet,
            Cell::styled(
                hyperlink::maybe_link(hyperlink::package_page(r.channel.as_deref(), name), name),
                name,
            ),
            Cell::plain(r.package_record.version.to_string()),
        ]);
    }
    table.print();

    Ok(())
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::{MatchSpec, ParseStrictness};

    use super::{compute_env_hash, exec_dir_prefix};
    use rattler_conda_types::Subdir;

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
        let h1 = compute_env_hash(&specs, &channels, Subdir::Linux64);
        let h2 = compute_env_hash(&specs, &channels, Subdir::Linux64);
        assert_eq!(h1, h2);
    }

    #[test]
    fn env_hash_is_order_independent() {
        let channels = vec!["https://conda.anaconda.org/conda-forge/".to_string()];
        let h1 = compute_env_hash(
            &[spec("numpy"), spec("python=3.12")],
            &channels,
            Subdir::Linux64,
        );
        let h2 = compute_env_hash(
            &[spec("python=3.12"), spec("numpy")],
            &channels,
            Subdir::Linux64,
        );
        assert_eq!(h1, h2);
    }

    #[test]
    fn env_hash_differs_by_platform() {
        let specs = vec![spec("python=3.12")];
        let channels = vec!["https://conda.anaconda.org/conda-forge/".to_string()];
        let h_linux = compute_env_hash(&specs, &channels, Subdir::Linux64);
        let h_osx = compute_env_hash(&specs, &channels, Subdir::OsxArm64);
        assert_ne!(h_linux, h_osx);
    }
}
