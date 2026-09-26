//! `rattler whoprovides`: which packages contain a file, answered with the
//! lookup index a channel publishes next to its repodata (`info.lookup_url`).

use std::{collections::BTreeMap, env, str::FromStr, time::Instant};

use futures_util::future::try_join_all;
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{
    Channel, ChannelConfig, Platform, Version, package::CondaArchiveIdentifier,
};
use rattler_lookup::{Kind, Location, Matches, Query, SubdirIndex, discovery};

use super::{QueryOutputFormat, print_url_lines};

/// Show packages that contain a file (or files matching a pattern).
#[derive(Debug, clap::Parser)]
#[clap(after_help = r#"Examples:
  rattler whoprovides include/zlib.h              # packages that ship this file
  rattler whoprovides '**/zlib.h'                 # ... wherever it is in the package
  rattler whoprovides '**/libssl.so*' -p linux-64 # file names with a literal start
  rattler whoprovides 'site-packages/polars/*'    # files directly below a directory
  rattler whoprovides bin/git --format urls       # print only the urls of the packages
  rattler whoprovides bin/git -c https://example.org/channel   # another channel (URL or directory)

Patterns use `*` and `?` within a path component and `**` for any number of
components. A pattern needs a literal start (`site-packages/polars/*`), or
its last component does if it starts with `**/` (`**/libssl.so*`); `**/*.h`
cannot be answered. Paths are looked up as they are stored in the packages,
e.g. `lib/python3.12/site-packages/numpy/__init__.py` for a Python package
and `site-packages/numpy/__init__.py` for a noarch one; `**/numpy/__init__.py`
finds both."#)]
pub struct Opt {
    /// The paths to look up, relative to the environment prefix (`bin/git`,
    /// `include/zlib.h`), or patterns (`**/zlib.h`, `**/libssl.so*`,
    /// `site-packages/polars/*`).
    #[clap(required = true, value_name = "PATH")]
    paths: Vec<String>,

    /// Channels to search in
    #[clap(short, long, default_value = "conda-forge")]
    channels: Vec<String>,

    /// Platform to search for. Defaults to the platform of the current host.
    /// `noarch` is always searched, too.
    #[clap(short, long)]
    platform: Option<Platform>,

    /// Maximum number of packages to display
    #[clap(long, default_value = "100")]
    limit: usize,

    /// Show all packages (no limit)
    #[clap(long)]
    all: bool,

    /// Maximum number of matching paths per pattern and subdir
    #[clap(long, default_value = "1000", value_name = "N")]
    max_paths: usize,

    /// Output format (defaults to human-readable output)
    #[clap(long, conflicts_with_all = ["limit", "all"])]
    format: Option<QueryOutputFormat>,
}

/// One artifact containing a matching path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    identifier: CondaArchiveIdentifier,
    /// A parsed version, for ordering.
    version: Option<Version>,
    subdir: String,
    filename: String,
    url: String,
    /// The path in the artifact that matched.
    path: String,
}

impl Found {
    /// Sort key: by name, then newest version first, then build string.
    fn sort_key(&self) -> (String, std::cmp::Reverse<Option<Version>>, String, String) {
        (
            self.identifier.identifier.name.clone(),
            std::cmp::Reverse(self.version.clone()),
            self.identifier.identifier.build_string.clone(),
            self.url.clone(),
        )
    }
}

/// The manifest locations of the subdirs to search.
async fn locate_indexes(
    opt: &Opt,
    subdirs: &[Platform],
    client: &reqwest_middleware::ClientWithMiddleware,
) -> miette::Result<Vec<Location>> {
    let mut locations = Vec::new();
    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);
    let channels = opt
        .channels
        .iter()
        .map(|channel| Channel::from_str(channel, &channel_config))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;
    eprintln!(
        "Channels: {}",
        channels.iter().map(Channel::canonical_name).join(", ")
    );
    for channel in &channels {
        let base = Location::from(url::Url::from(channel.base_url.clone()));
        for subdir in subdirs {
            match discovery::discover_manifest(&base, subdir.as_str(), client)
                .await
                .into_diagnostic()
                .with_context(|| {
                    format!(
                        "failed to look for the lookup index of {subdir} in {}",
                        channel.canonical_name()
                    )
                })? {
                Some(location) => locations.push(location),
                None => eprintln!(
                    "{} has no lookup index for {subdir}",
                    channel.canonical_name()
                ),
            }
        }
    }
    Ok(locations)
}

pub async fn whoprovides(opt: Opt, offline: bool) -> miette::Result<()> {
    let queries = opt
        .paths
        .iter()
        .map(|path| Query::parse(path))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;
    let kinds: Vec<Kind> = queries.iter().map(Query::kind).sorted().dedup().collect();

    let client = super::client::create_client_with_middleware(offline)?;
    let platform = opt.platform.map_or_else(crate::host_platform, Ok)?;
    let subdirs: Vec<Platform> = [platform, Platform::NoArch].into_iter().dedup().collect();

    let start = Instant::now();
    let pb = ProgressBar::new_spinner();
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb.set_style(ProgressStyle::with_template("{spinner:.green} {msg}").unwrap());
    pb.set_message("Locating lookup indexes...");

    let locations = locate_indexes(&opt, &subdirs, &client).await?;
    if locations.is_empty() {
        pb.finish_and_clear();
        return Err(miette::miette!(
            "none of the channels publishes a lookup index for {}",
            subdirs.iter().join(", ")
        ));
    }

    pb.set_message("Opening lookup indexes...");
    let mut indexes = try_join_all(
        locations
            .into_iter()
            .map(|location| SubdirIndex::open(location, &kinds, &client)),
    )
    .await
    .into_diagnostic()
    .context("failed to open the lookup index")?;
    let opened = start.elapsed();

    let mut all_found: Vec<(Query, Vec<Found>)> = Vec::new();
    for query in &queries {
        pb.set_message(format!("Looking up {query}..."));
        let results: Vec<Matches> = try_join_all(
            indexes
                .iter_mut()
                .map(|index| index.query(query, Some(opt.max_paths))),
        )
        .await
        .into_diagnostic()
        .with_context(|| format!("failed to look up `{query}`"))?;
        let mut found = Vec::new();
        for (index, matches) in indexes.iter().zip(results) {
            if matches.truncated {
                pb.suspend(|| {
                    eprintln!(
                        "`{query}`: stopped after {} matching paths in {}, use --max-paths to get more",
                        opt.max_paths,
                        index.subdir()
                    );
                });
            }
            let channel = index.channel().trim_end_matches('/');
            for (path, filenames) in matches.paths {
                for filename in filenames {
                    let Some(identifier) = CondaArchiveIdentifier::try_from_filename(&filename)
                    else {
                        tracing::warn!("skipping unrecognized artifact filename {filename}");
                        continue;
                    };
                    let version = Version::from_str(&identifier.identifier.version).ok();
                    found.push(Found {
                        version,
                        identifier,
                        subdir: index.subdir().to_string(),
                        url: format!("{channel}/{}/{filename}", index.subdir()),
                        filename,
                        path: path.clone(),
                    });
                }
            }
        }
        found.sort_by_cached_key(Found::sort_key);
        all_found.push((query.clone(), found));
    }
    pb.finish_and_clear();

    if opt.format == Some(QueryOutputFormat::Urls) {
        let urls = all_found
            .into_iter()
            .flat_map(|(_, found)| found)
            .map(|found| found.url)
            .unique();
        return print_url_lines(urls);
    }

    if opt.format == Some(QueryOutputFormat::Json) {
        let records: Vec<serde_json::Value> = all_found
            .iter()
            .flat_map(|(query, found)| {
                found.iter().map(move |found| {
                    serde_json::json!({
                        "query": query.as_str(),
                        "path": found.path,
                        "name": found.identifier.identifier.name,
                        "version": found.identifier.identifier.version,
                        "build": found.identifier.identifier.build_string,
                        "subdir": found.subdir,
                        "filename": found.filename,
                        "url": found.url,
                    })
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&records).into_diagnostic()?
        );
        return Ok(());
    }

    let (requests, bytes) = indexes
        .iter()
        .map(SubdirIndex::stats)
        .fold((0, 0), |(r, b), (r2, b2)| (r + r2, b + b2));
    tracing::debug!(
        "opened {} lookup index(es) in {:?}, {requests} range requests, {:.1} KiB read",
        indexes.len(),
        opened,
        bytes as f64 / 1024.0
    );

    let limit = if opt.all { usize::MAX } else { opt.limit };
    for (i, (query, found)) in all_found.iter().enumerate() {
        if i > 0 {
            println!();
        }
        if found.is_empty() {
            println!(
                "No packages found that provide '{query}' in {} in {:?}",
                subdirs.iter().join(", "),
                start.elapsed()
            );
            continue;
        }

        // One line per package name, with its newest artifact.
        let mut grouped: BTreeMap<&str, (&Found, usize)> = BTreeMap::new();
        for artifact in found {
            let name = artifact.identifier.identifier.name.as_str();
            grouped
                .entry(name)
                .and_modify(|(_, count)| *count += 1)
                .or_insert((artifact, 1));
        }
        let records = found.iter().map(|f| &f.url).unique().count();
        println!(
            "Found {} package{} ({} record{}) that provide{} '{}' in {} in {:?}\n",
            grouped.len(),
            if grouped.len() == 1 { "" } else { "s" },
            records,
            if records == 1 { "" } else { "s" },
            if grouped.len() == 1 { "s" } else { "" },
            query,
            subdirs.iter().join(", "),
            start.elapsed()
        );
        for (name, (artifact, count)) in grouped.iter().take(limit) {
            println!(
                "  {} {} {} ({}){}{}",
                console::style(name).bold().green(),
                console::style(&artifact.identifier.identifier.version).cyan(),
                artifact.identifier.identifier.build_string,
                artifact.subdir,
                match query {
                    Query::Path(_) => String::new(),
                    Query::Pattern(_) => format!(" {}", console::style(&artifact.path).dim()),
                },
                if *count > 1 {
                    format!(
                        " and {} more record{}",
                        count - 1,
                        if *count == 2 { "" } else { "s" }
                    )
                } else {
                    String::new()
                }
            );
        }
        if grouped.len() > limit {
            println!(
                "\n... and {} more package{} (use --all to show all)",
                grouped.len() - limit,
                if grouped.len() - limit == 1 { "" } else { "s" }
            );
        }
    }
    Ok(())
}
