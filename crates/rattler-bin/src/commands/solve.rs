use std::{
    collections::HashMap,
    env,
    time::{Duration, Instant},
};

use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler::{default_cache_dir, package_cache::PackageCache};
use rattler_conda_types::{
    ChannelConfig, MatchSpec, Matches, PackageName, Platform, RepoDataRecord,
};
use rattler_repodata_gateway::{Gateway, RepoData, SourceConfig};
use rattler_solve::SolverTask;
use url::Url;

use crate::{
    commands::progress::{wrap_in_async_progress, wrap_in_progress},
    solver_args::SolverArgs,
};

/// Solve a conda environment without installing it.
///
/// Resolves the specified package specs for a target platform and prints the
/// resulting package set.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Package specs to solve.
    #[clap(required = true)]
    specs: Vec<String>,

    #[clap(flatten)]
    solver: SolverArgs,

    /// Output in JSON format
    #[clap(long)]
    json: bool,
}

pub async fn solve(opt: Opt, offline: bool) -> miette::Result<()> {
    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);
    let platform = opt.solver.platform;

    // All progress information goes to stderr so that stdout only contains the
    // solved package set.
    eprintln!("Solving for platform: {platform}");

    let specs = SolverArgs::parse_specs(&opt.specs)?;
    let constraints = opt.solver.constraints()?;

    let cache_dir = default_cache_dir()
        .map_err(|e| miette::miette!("could not determine default cache directory: {}", e))?;
    rattler_cache::ensure_cache_dir(&cache_dir)
        .map_err(|e| miette::miette!("could not create cache directory: {}", e))?;

    let channels = opt.solver.channels(&channel_config)?;

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
            .query(channels, [platform, Platform::NoArch], specs.clone())
            .recursive(true),
    )
    .await
    .into_diagnostic()
    .context("failed to load repodata")?;

    // Surface any non-fatal CEP-42 channel-relation problems.
    for warning in &repo_data.warnings {
        eprintln!("warning: {warning}");
    }

    let total_records: usize = repo_data.iter().map(RepoData::len).sum();
    eprintln!(
        "Loaded {} records in {}",
        total_records,
        format_elapsed(start_load_repo_data.elapsed())
    );

    let virtual_packages = wrap_in_progress("determining virtual packages", || {
        opt.solver.virtual_packages()
    })?;

    eprintln!(
        "Virtual packages:\n{}\n",
        virtual_packages
            .iter()
            .format_with("\n", |i, f| f(&format_args!("  - {i}",)))
    );

    if !constraints.is_empty() {
        eprintln!(
            "Constraints:\n{}\n",
            constraints
                .iter()
                .format_with("\n", |i, f| f(&format_args!("  - {i}",)))
        );
    }

    let solver_task = SolverTask {
        virtual_packages,
        specs: specs.clone(),
        constraints: constraints.clone(),
        timeout: opt.solver.timeout(),
        strategy: opt.solver.strategy(),
        channel_priority: opt.solver.channel_priority(),
        exclude_newer: opt.solver.exclude_newer(),
        ..SolverTask::from_iter(&repo_data)
    };

    let start_solve = Instant::now();
    let solver_result =
        wrap_in_progress("solving", || opt.solver.solve(solver_task)).into_diagnostic()?;
    let solve_duration = start_solve.elapsed();

    let mut solved_packages: Vec<RepoDataRecord> = solver_result.records;
    opt.solver.filter_deps_mode(&mut solved_packages, &specs);

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
            &constraints,
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

/// Prints the solved records as a table with aligned columns.
///
/// Packages that are explicitly requested through one of the input specs are
/// highlighted to distinguish them from the transitive dependencies that were
/// pulled in by the solver. When constraints were given, an extra column shows
/// which constraints apply to each package.
fn print_records(
    records: &[RepoDataRecord],
    extras: &HashMap<PackageName, Vec<String>>,
    specs: &[MatchSpec],
    constraints: &[MatchSpec],
    channel_config: &ChannelConfig,
) {
    let mut header = vec![
        "Package".to_string(),
        "Version".to_string(),
        "Build".to_string(),
        "Channel".to_string(),
    ];
    if !constraints.is_empty() {
        header.push("Constraint".to_string());
    }

    // These initial widths match the header column lengths.
    let mut widths: Vec<usize> = header.iter().map(String::len).collect();
    let mut rows = Vec::with_capacity(records.len());
    for record in records {
        let mut name = record.package_record.name.as_normalized().to_string();
        if let Some(extras) = extras.get(&record.package_record.name) {
            name.push('[');
            name.push_str(&extras.join(","));
            name.push(']');
        }

        let mut fields = vec![
            name,
            record.package_record.version.to_string(),
            record.package_record.build.clone(),
            format_channel(record, channel_config),
        ];
        if !constraints.is_empty() {
            fields.push(
                constraints
                    .iter()
                    .filter(|constraint| constraint.matches(&record.package_record))
                    .join(", "),
            );
        }
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
    let styled_header: Vec<String> = header
        .iter()
        .map(|field| console::style(field).bold().to_string())
        .collect();
    print_row(&styled_header, &widths, &header);
    for (fields, explicit) in &rows {
        let styled: Vec<String> = fields
            .iter()
            .enumerate()
            .map(|(i, field)| match i {
                0 if *explicit => console::style(field).green().bold().to_string(),
                0 | 1 => field.clone(),
                _ => console::style(field).dim().to_string(),
            })
            .collect();
        print_row(&styled, &widths, fields);
    }
}

/// Prints a single table row, padding each column to `widths`.
///
/// `styled` holds the fields as they should be displayed, `plain` the same
/// fields without any styling. Padding is computed from `plain` because ANSI
/// escape codes in `styled` do not occupy any terminal columns but would
/// otherwise be counted by the formatter.
fn print_row(styled: &[String], widths: &[usize], plain: &[String]) {
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
