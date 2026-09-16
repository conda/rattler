use std::collections::HashMap;
use std::path::PathBuf;

use indicatif::HumanBytes;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::NoArchKind;
use rattler_conda_types::package::{AboutJson, IndexJson, PackageFile, PathsJson, RunExportsJson};
use serde::Serialize;
use url::Url;

use super::package_source::{PackageSource, client_for};

/// Inspect package metadata from a local or remote conda package.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Path or URL of the conda package to inspect (.conda or .tar.bz2 archive)
    #[clap(required = true)]
    package: String,

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
    let source = PackageSource::parse(&opt.package);
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
