use std::{env, path::Path, time::Instant};

use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{Channel, ChannelConfig, Platform};
use rattler_lookup::{
    Kind, LookupStats, PathLookup, PathMatch, Query, Search, manifest::MANIFEST_FILE,
};
use url::Url;

use super::{QueryOutputFormat, print_url_lines};

/// Show the artifacts of a channel that contain the given file path.
///
/// Paths are matched exactly as they are recorded in the packages (the spelling
/// of `info/paths.json`), except for a leading `./` or `/`. A query may also be a
/// glob pattern: `*` and `?` match within a path component, `**` across
/// components. A pattern needs a literal start or has to begin with `**/`,
/// because everything else would mean reading the whole index.
///
/// This needs a channel that publishes a lookup index of the paths of its
/// packages; channels that don't are reported as not indexed.
#[derive(Debug, clap::Parser)]
#[clap(after_help = r#"Examples:
  rattler whoprovides bin/python                  # artifacts that contain bin/python
  rattler whoprovides lib/libssl.so.3 -p linux-64 # only for one platform
  rattler whoprovides 'site-packages/polars/**'   # every path below a directory
  rattler whoprovides '**/libz.so.1' --names      # the packages, wherever the file is
  rattler whoprovides bin/python --index ./my-index   # an index that is not advertised"#)]
pub struct Opt {
    /// The paths or patterns to look up, e.g. `bin/python` or `**/libz.so.1`.
    #[clap(required = true, value_name = "PATH")]
    queries: Vec<String>,

    /// Channels to search in
    #[clap(short = 'c', long = "channel", default_value = "conda-forge")]
    channels: Vec<String>,

    /// Platforms to search in. Defaults to the platform of the current host
    /// and `noarch`.
    #[clap(short = 'p', long = "platform")]
    platforms: Vec<Platform>,

    /// Query an index directly instead of the one the channel advertises.
    ///
    /// Either a directory or base url that contains
    /// `<platform>/lookup/manifest.json`, or one such `manifest.json`.
    #[clap(long, conflicts_with = "channels")]
    index: Option<String>,

    /// The maximum number of matching paths to report per query.
    #[clap(long, default_value = "1000")]
    limit: usize,

    /// Report the package names of the artifacts instead of their filenames.
    #[clap(long)]
    names: bool,

    /// Output format (defaults to human-readable output)
    #[clap(long)]
    format: Option<QueryOutputFormat>,
}

/// Interprets a `--index` argument as a url, taking anything that is not a
/// `http`, `https` or `file` url for a path on disk.
fn index_url(index: &str) -> miette::Result<Url> {
    if let Ok(url) = Url::parse(index)
        && matches!(url.scheme(), "http" | "https" | "file")
    {
        return Ok(url);
    }

    let path = dunce::canonicalize(Path::new(index))
        .into_diagnostic()
        .with_context(|| format!("failed to locate the index at '{index}'"))?;
    let url = if path.is_dir() {
        Url::from_directory_path(&path)
    } else {
        Url::from_file_path(&path)
    };
    url.map_err(|()| miette::miette!("'{}' is not a valid index location", path.display()))
}

/// Opens the indices to query: either the one given with `--index` or the ones
/// the channels advertise.
async fn open_indices(
    opt: &Opt,
    platforms: &[Platform],
    client: &reqwest_middleware::ClientWithMiddleware,
) -> miette::Result<Vec<PathLookup>> {
    if let Some(index) = &opt.index {
        let url = index_url(index)?;
        let lookup = if url.path().ends_with(MANIFEST_FILE) {
            PathLookup::for_manifest(&url, client.clone()).await
        } else {
            PathLookup::for_index_base(&url, platforms, client.clone()).await
        };
        return Ok(vec![lookup.into_diagnostic().with_context(|| {
            format!("failed to open the lookup index at '{url}'")
        })?]);
    }

    let channel_config =
        ChannelConfig::default_with_root_dir(env::current_dir().into_diagnostic()?);
    let channels = opt
        .channels
        .iter()
        .map(|channel| Channel::from_str(channel, &channel_config))
        .collect::<Result<Vec<_>, _>>()
        .into_diagnostic()?;

    let mut lookups = Vec::with_capacity(channels.len());
    for channel in channels {
        lookups.push(
            PathLookup::for_channel(channel.base_url.as_ref(), platforms, client.clone())
                .await
                .into_diagnostic()
                .with_context(|| {
                    format!("failed to open the lookup index of '{}'", channel.name())
                })?,
        );
    }
    Ok(lookups)
}

/// What one query found in all indices together.
struct Found {
    query: Query,
    search: Search,
}

/// Looks `query` up in every index and merges what they found.
async fn search(lookups: &[PathLookup], query: &str, limit: usize) -> miette::Result<Found> {
    let query = Query::parse(query).into_diagnostic()?;
    let mut search = Search::default();
    for lookup in lookups {
        // A `**/…` query needs a `reversed-paths` table, which an index does not
        // have to provide; saying "nothing found" would be a lie.
        if lookup.missing_kinds(query.kind()) {
            return Err(miette::miette!(
                help = format!(
                    "the index has no `{}` table, so it cannot answer `{}` patterns",
                    query.kind(),
                    if query.kind() == Kind::ReversedPaths {
                        "**/…"
                    } else {
                        "…"
                    }
                ),
                "cannot look up '{}'",
                query.text()
            ));
        }
        let found = lookup
            .search(&query, limit)
            .await
            .into_diagnostic()
            .with_context(|| format!("failed to look up '{}'", query.text()))?;
        search.truncated |= found.truncated;
        for (path, artifacts) in found.paths {
            search.paths.entry(path).or_default().extend(artifacts);
        }
    }
    if search.paths.len() > limit {
        search.truncated = true;
        search.paths = std::mem::take(&mut search.paths)
            .into_iter()
            .take(limit)
            .collect();
    }
    for artifacts in search.paths.values_mut() {
        artifacts.sort();
        artifacts.dedup();
    }
    Ok(Found { query, search })
}

/// How an artifact is reported: its filename, or its package name with `--names`.
fn label(found: &PathMatch, names: bool) -> String {
    if names {
        format!("{}/{}", found.subdir, found.package_name())
    } else {
        format!("{}/{}", found.subdir, found.file_name)
    }
}

pub async fn whoprovides(opt: Opt, offline: bool) -> miette::Result<()> {
    let platforms = if opt.platforms.is_empty() {
        vec![crate::host_platform()?, Platform::NoArch]
    } else {
        opt.platforms.iter().copied().unique().collect()
    };

    let client = super::client::create_client_with_middleware(offline)?;

    let start = Instant::now();
    let lookups = open_indices(&opt, &platforms, &client).await?;

    // A channel that publishes no index cannot answer the query at all, which
    // is something else than a path that no artifact contains.
    if lookups.iter().all(PathLookup::is_empty) {
        return Err(miette::miette!(
            help = "the channel has to publish a lookup index (info.lookup_url in its \
                    repodata); use --index to query an index directly",
            "no lookup index found for {}",
            platforms.iter().join(", ")
        ));
    }

    let mut found = Vec::with_capacity(opt.queries.len());
    for query in &opt.queries {
        found.push(search(&lookups, query, opt.limit).await?);
    }

    let stats = lookups
        .iter()
        .fold(LookupStats::default(), |mut all, lookup| {
            all += lookup.stats();
            all
        });
    tracing::debug!(
        "read {} bytes in {} requests in {:?}",
        stats.bytes,
        stats.requests,
        start.elapsed()
    );

    match opt.format {
        Some(QueryOutputFormat::Urls) => {
            let urls = found
                .iter()
                .flat_map(|found| found.search.artifacts())
                .map(|found| found.url())
                .collect::<Result<Vec<_>, _>>()
                .into_diagnostic()?;
            print_url_lines(urls)
        }
        Some(QueryOutputFormat::Json) => {
            let json = found
                .iter()
                .map(|found| {
                    serde_json::json!({
                        "query": found.query.text(),
                        "pattern": found.query.is_pattern(),
                        "truncated": found.search.truncated,
                        "paths": found.search.paths.iter().map(|(path, artifacts)| {
                            serde_json::json!({
                                "path": path,
                                "artifacts": artifacts.iter().map(|found| serde_json::json!({
                                    "subdir": found.subdir,
                                    "file_name": found.file_name,
                                    "name": found.package_name(),
                                    "channel": found.channel,
                                    "url": found.url().ok().map(String::from),
                                })).collect::<Vec<_>>(),
                            })
                        }).collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>();
            println!("{}", serde_json::to_string_pretty(&json).into_diagnostic()?);
            Ok(())
        }
        None => {
            for found in &found {
                println!("{}", console::style(found.query.text()).bold());
                if found.search.paths.is_empty() {
                    let message = if found.query.is_pattern() {
                        "no path matches this pattern"
                    } else {
                        "no artifact contains this path"
                    };
                    println!("  {}", console::style(message).dim());
                    continue;
                }

                // An exact path is one line per artifact; a pattern groups the
                // artifacts under every path it matched.
                if found.query.is_pattern() {
                    for (path, artifacts) in &found.search.paths {
                        println!("  {}", console::style(path).cyan());
                        for artifact in artifacts
                            .iter()
                            .map(|found| label(found, opt.names))
                            .unique()
                        {
                            println!("    {}", console::style(artifact).green());
                        }
                    }
                } else {
                    for artifact in found
                        .search
                        .artifacts()
                        .iter()
                        .map(|found| label(found, opt.names))
                        .unique()
                    {
                        println!("  {}", console::style(artifact).green());
                    }
                }

                if found.search.truncated {
                    println!(
                        "  {}",
                        console::style(format!(
                            "… stopped at {} paths, use --limit to see more",
                            opt.limit
                        ))
                        .dim()
                    );
                }
            }
            Ok(())
        }
    }
}
