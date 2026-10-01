use std::collections::HashMap;
use std::path::{Path, PathBuf};

use indicatif::HumanBytes;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::package::{
    AboutJson, CondaArchiveType, IndexJson, PackageFile, PathsJson, RunExportsJson,
};
use rattler_conda_types::{
    ChannelConfig, MatchSpec, NoArchKind, ParseMatchSpecOptions, RepoDataRecord, Subdir,
};
use rattler_repodata_gateway::RepoData;
use serde::Serialize;
use url::Url;

use super::gateway::{build_gateway, load_config, resolve_channels};
use super::package_source::{PackageSource, client_for};

/// Inspect package metadata from a local or remote conda package, or a matchspec.
#[derive(Debug, clap::Parser)]
#[clap(after_help = r#"Examples:
  rattler inspect ./numpy-2.1.0-py312h1234_0.conda     # metadata and the first 10 files
  rattler inspect https://conda.anaconda.org/conda-forge/noarch/tzdata-2024a-h0c530f3_0.conda
  rattler inspect ./pkg.conda --json                    # machine-readable metadata
  rattler inspect ./pkg.conda --limit -1                # list all files
  rattler inspect numpy                                 # newest numpy on conda-forge
  rattler inspect 'python 3.12.*' -c conda-forge -p linux-64"#)]
pub struct Opt {
    /// Path or URL of the conda package to inspect (.conda or .tar.bz2
    /// archive), or a matchspec.
    ///
    /// Anything that is not a URL, an existing file or a path ending in
    /// `.conda` or `.tar.bz2` is treated as a matchspec: the channels are
    /// searched and the newest matching package is inspected. The matchspec
    /// must name exactly one package, globs and regexes are not supported.
    #[clap(required = true)]
    package: String,

    /// Channels to search in when inspecting a matchspec.
    #[clap(short, long)]
    channels: Option<Vec<String>>,

    /// Subdir to search in when inspecting a matchspec.
    #[clap(short, long)]
    platform: Option<Subdir>,

    /// Number of files to print (a negative value prints all files)
    #[clap(long, default_value_t = 10, allow_hyphen_values = true)]
    limit: i64,

    /// Print the package metadata as JSON
    #[clap(long)]
    json: bool,
}

/// All metadata read from the package; serialized as-is by `--json`.
#[derive(Serialize)]
struct Metadata {
    /// Size in bytes of the package archive itself.
    size: u64,
    index: IndexJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    about: Option<AboutJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_exports: Option<RunExportsJson>,
    paths: PathsJson,
}

pub async fn inspect(opt: Opt, offline: bool) -> miette::Result<()> {
    let source = if is_package_location(&opt.package) {
        if opt.channels.is_some() || opt.platform.is_some() {
            miette::bail!(
                "--channels and --platform can only be used when inspecting a matchspec, not a package path or URL"
            );
        }
        PackageSource::parse(&opt.package)
    } else {
        let record = find_newest_record(&opt, offline).await?;
        eprintln!("Inspecting {}", record.url);
        PackageSource::Url(record.url)
    };
    let client = client_for([&source], offline)?;
    let archive = source.open(client.as_ref()).await?;

    // All metadata lives in the info section; a single batched call reads it
    // in one pass (for sparse `.conda` archives usually straight from the
    // cached archive tail).
    let mut files = archive
        .read_files([
            IndexJson::package_path(),
            AboutJson::package_path(),
            RunExportsJson::package_path(),
            PathsJson::package_path(),
        ])
        .await
        .into_diagnostic()
        .context("failed to read package metadata")?;

    let index: IndexJson = parse_from_batch(&mut files)?
        .ok_or_else(|| miette::miette!("package does not contain an info/index.json"))?;
    let about: Option<AboutJson> = parse_from_batch(&mut files)?;
    let run_exports: Option<RunExportsJson> = parse_from_batch(&mut files)?;
    let paths: PathsJson = parse_from_batch(&mut files)?
        .ok_or_else(|| miette::miette!("package does not contain an info/paths.json"))?;

    let metadata = Metadata {
        size: archive.size(),
        index,
        about,
        run_exports: run_exports.filter(|run_exports| !run_exports.is_empty()),
        paths,
    };

    if opt.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&metadata).into_diagnostic()?
        );
    } else {
        print_human(&metadata, opt.limit);
    }
    Ok(())
}

/// Whether `package` refers to a package archive (a URL or a local file)
/// rather than a matchspec.
///
/// URLs must have a host: `conda-forge::numpy` parses as a URL with the
/// scheme `conda-forge` but is a matchspec with a channel.
fn is_package_location(package: &str) -> bool {
    matches!(PackageSource::parse(package), PackageSource::Url(url) if url.has_host())
        || CondaArchiveType::try_from(Path::new(package)).is_some()
        || Path::new(package).is_file()
}

/// Searches the channels for records matching the matchspec in `opt.package`
/// and returns the newest one.
async fn find_newest_record(opt: &Opt, offline: bool) -> miette::Result<RepoDataRecord> {
    // Extras and flags are rejected: they select optional dependencies or
    // variants when solving, which doesn't make much sense for a search-style
    // query that picks a single package.
    let matchspec = MatchSpec::from_str(&opt.package, ParseMatchSpecOptions::strict())
    .into_diagnostic()
    .with_context(|| {
        format!(
            "'{}' is not an existing file, a URL or a valid matchspec with an exact package name",
            opt.package
        )
    })?;

    let config = load_config()?;
    let channel_config =
        ChannelConfig::default_with_root_dir(std::env::current_dir().into_diagnostic()?);
    let channels = resolve_channels(opt.channels.as_deref(), &config, &channel_config)?;
    let platform = opt.platform.map_or_else(crate::host_platform, Ok)?;

    let client = super::client::create_client_with_middleware(offline)?;
    let gateway = build_gateway(client, &config, offline, true)?;
    let repo_data = gateway
        .query(channels, [platform, Subdir::NoArch], [matchspec])
        .recursive(false)
        .await
        .into_diagnostic()
        .context("failed to query repodata")?;

    repo_data
        .iter()
        .flat_map(RepoData::iter)
        .max()
        .cloned()
        .ok_or_else(|| {
            miette::miette!(
                "no packages found matching '{}' on {platform} or noarch",
                opt.package
            )
        })
}

/// Takes a file out of a batched `read_files` result and parses it, or `None`
/// when the package does not contain it.
fn parse_from_batch<P: PackageFile>(
    files: &mut HashMap<PathBuf, Option<Vec<u8>>>,
) -> miette::Result<Option<P>> {
    files
        .remove(P::package_path())
        .flatten()
        .map(|bytes| {
            P::from_slice(&bytes)
                .into_diagnostic()
                .with_context(|| format!("failed to parse {}", P::package_path().display()))
        })
        .transpose()
}

fn print_human(metadata: &Metadata, limit: i64) {
    print_index(&metadata.index, metadata.size);
    if let Some(about) = &metadata.about {
        print_about(about);
    }
    if let Some(run_exports) = &metadata.run_exports {
        print_run_exports(run_exports);
    }
    print_paths(&metadata.paths, limit);
}

fn print_index(index: &IndexJson, size: u64) {
    println!("name: {}", index.name.as_normalized());
    println!("version: {}", index.version);
    println!("build: {}", index.build);
    println!("build number: {}", index.build_number);
    if let Some(subdir) = &index.subdir {
        println!("subdir: {subdir}");
    }
    if let Some(noarch) = index.noarch.kind() {
        let noarch = match noarch {
            NoArchKind::Python => "python",
            NoArchKind::Generic => "generic",
        };
        println!("noarch: {noarch}");
    }
    if let Some(license) = &index.license {
        println!("license: {license}");
    }
    if let Some(timestamp) = &index.timestamp {
        println!("timestamp: {}", timestamp.jiff_timestamp());
    }
    println!("size: {}", HumanBytes(size));
    print_list("depends", &index.depends);
    print_list("constrains", &index.constrains);
    if !index.extra_depends.is_empty() {
        println!("extra depends:");
        for (extra, depends) in &index.extra_depends {
            println!("  {extra}:");
            for dep in depends {
                println!("    - {dep}");
            }
        }
    }
    print_list("track features", &index.track_features);
    if let Some(purls) = &index.purls {
        print_list("purls", purls);
    }
    if let Some(site_packages_path) = &index.python_site_packages_path {
        println!("python site-packages path: {site_packages_path}");
    }
}

fn print_about(about: &AboutJson) {
    let has_content = about.summary.is_some()
        || about.description.is_some()
        || !about.home.is_empty()
        || !about.doc_url.is_empty()
        || !about.dev_url.is_empty()
        || about.source_url.is_some();
    if !has_content {
        return;
    }

    println!();
    if let Some(summary) = &about.summary {
        print_text("summary", summary);
    }
    if let Some(description) = &about.description {
        print_text("description", description);
    }
    print_urls("homepage", &about.home);
    print_urls("documentation", &about.doc_url);
    print_urls("repository", &about.dev_url);
    if let Some(source_url) = &about.source_url {
        println!("source: {source_url}");
    }
}

fn print_run_exports(run_exports: &RunExportsJson) {
    println!();
    println!("run exports:");
    print_indented_list("weak", &run_exports.weak);
    print_indented_list("strong", &run_exports.strong);
    print_indented_list("noarch", &run_exports.noarch);
    print_indented_list("weak constrains", &run_exports.weak_constrains);
    print_indented_list("strong constrains", &run_exports.strong_constrains);
}

fn print_paths(paths: &PathsJson, limit: i64) {
    println!();
    let total = paths.paths.len();
    if paths
        .paths
        .iter()
        .any(|entry| entry.size_in_bytes.is_some())
    {
        let total_size: u64 = paths
            .paths
            .iter()
            .filter_map(|entry| entry.size_in_bytes)
            .sum();
        println!(
            "paths: ({total} total, {} installed)",
            HumanBytes(total_size)
        );
    } else {
        println!("paths: ({total} total)");
    }
    let limit = usize::try_from(limit).unwrap_or(total);
    for entry in paths.paths.iter().take(limit) {
        match entry.size_in_bytes {
            Some(size) => println!(
                "  - {} ({})",
                entry.relative_path.display(),
                HumanBytes(size)
            ),
            None => println!("  - {}", entry.relative_path.display()),
        }
    }
    if total > limit {
        println!("  ... and {} more", total - limit);
    }
}

/// Prints a `label:` line followed by one `  - item` line per item, or
/// nothing when there are no items.
fn print_list(label: &str, items: impl IntoIterator<Item = impl std::fmt::Display>) {
    let mut items = items.into_iter().peekable();
    if items.peek().is_none() {
        return;
    }
    println!("{label}:");
    for item in items {
        println!("  - {item}");
    }
}

/// Like [`print_list`] but indented one level, for the run exports section.
fn print_indented_list(label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    println!("  {label}:");
    for item in items {
        println!("    - {item}");
    }
}

/// Prints a single-line value inline and a multi-line value as an indented
/// block.
fn print_text(label: &str, text: &str) {
    let text = text.trim_end();
    if text.contains('\n') {
        println!("{label}:");
        for line in text.lines() {
            println!("  {line}");
        }
    } else {
        println!("{label}: {text}");
    }
}

/// Prints a single URL inline and multiple URLs as a list, or nothing when
/// there are none.
fn print_urls(label: &str, urls: &[Url]) {
    match urls {
        [] => {}
        [url] => println!("{label}: {url}"),
        urls => {
            println!("{label}:");
            for url in urls {
                println!("  - {url}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_package_location() {
        for package in [
            "https://conda.anaconda.org/conda-forge/noarch/tzdata-2024a-h0c530f3_0.conda",
            "./numpy-2.1.0-py312h1234_0.conda",
            "missing-1.0-0.tar.bz2",
        ] {
            assert!(
                is_package_location(package),
                "{package} should be a package"
            );
        }
        for matchspec in ["numpy", "python 3.12.*", "conda-forge::numpy >=2"] {
            assert!(
                !is_package_location(matchspec),
                "{matchspec} should be a matchspec"
            );
        }
    }
}
