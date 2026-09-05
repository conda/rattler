use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use console::style;
use indicatif::HumanBytes;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{
    NoArchType,
    package::{IndexJson, PathsEntry, PathsJson, RunExportsJson},
};
use rattler_package_streaming::{
    ExtractError,
    archive::{ArchiveEntryKind, PackageArchive, Section},
};
use sha2::{Digest, Sha256};

use super::package_source::{PackageSource, client_for};

/// Compare two conda packages and report the differences.
///
/// Reports changed metadata, changed dependencies/constraints/run exports,
/// changed/added/removed files in the package payload (based on
/// `info/paths.json`) and changed/added/removed files in the info section.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Path or URL of the first conda package (.tar.bz2 or .conda)
    #[clap(value_name = "LEFT")]
    left: String,

    /// Path or URL of the second conda package (.tar.bz2 or .conda)
    #[clap(value_name = "RIGHT")]
    right: String,
}

pub async fn compare_packages(opt: Opt, offline: bool) -> miette::Result<()> {
    let left_source = PackageSource::parse(&opt.left);
    let right_source = PackageSource::parse(&opt.right);
    let client = client_for([&left_source, &right_source], offline)?;

    let (left, right) = tokio::try_join!(
        left_source.open(client.as_ref()),
        right_source.open(client.as_ref()),
    )?;

    println!("comparing");
    println!("  left:  {}", opt.left);
    println!("  right: {}", opt.right);

    let (left_index, right_index, left_run_exports, right_run_exports, left_paths, right_paths) = tokio::try_join!(
        read_package_file::<IndexJson>(&left, "left"),
        read_package_file::<IndexJson>(&right, "right"),
        try_read_package_file::<RunExportsJson>(&left, "left"),
        try_read_package_file::<RunExportsJson>(&right, "right"),
        try_read_package_file::<PathsJson>(&left, "left"),
        try_read_package_file::<PathsJson>(&right, "right"),
    )?;

    let mut any_changes = false;
    any_changes |= compare_metadata(&left_index, &right_index);
    any_changes |= compare_dependencies(
        &left_index,
        &right_index,
        left_run_exports.as_ref(),
        right_run_exports.as_ref(),
    );
    any_changes |= compare_package_files(left_paths.as_ref(), right_paths.as_ref());
    any_changes |= compare_info_files(&left, &right).await?;

    println!();
    if any_changes {
        println!("{}", style("the packages differ").yellow().bold());
    } else {
        println!("{}", style("no differences found").green().bold());
    }

    Ok(())
}

async fn read_package_file<P: rattler_conda_types::package::PackageFile>(
    archive: &PackageArchive,
    side: &str,
) -> miette::Result<P> {
    archive
        .read_package_file()
        .await
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to read {} from the {side} package",
                P::package_path().display()
            )
        })
}

async fn try_read_package_file<P: rattler_conda_types::package::PackageFile>(
    archive: &PackageArchive,
    side: &str,
) -> miette::Result<Option<P>> {
    archive
        .try_read_package_file()
        .await
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to read {} from the {side} package",
                P::package_path().display()
            )
        })
}

fn fmt_opt(value: Option<&str>) -> String {
    value.map_or_else(|| "<not set>".to_string(), ToString::to_string)
}

fn fmt_noarch(noarch: &NoArchType) -> String {
    if noarch.is_python() {
        "python".to_string()
    } else if noarch.is_generic() {
        "generic".to_string()
    } else {
        "<not set>".to_string()
    }
}

/// Compares the scalar `index.json` metadata (everything except the
/// dependency related fields) and prints the changed fields.
fn compare_metadata(left: &IndexJson, right: &IndexJson) -> bool {
    println!();
    println!("{}", style("metadata").bold());

    let mut changes = Vec::new();
    let mut compare = |field: &str, left: String, right: String| {
        if left != right {
            changes.push((field.to_string(), left, right));
        }
    };

    compare(
        "name",
        left.name.as_normalized().to_string(),
        right.name.as_normalized().to_string(),
    );
    compare(
        "version",
        left.version.to_string(),
        right.version.to_string(),
    );
    compare("build", left.build.clone(), right.build.clone());
    compare(
        "build_number",
        left.build_number.to_string(),
        right.build_number.to_string(),
    );
    compare(
        "subdir",
        fmt_opt(left.subdir.as_deref()),
        fmt_opt(right.subdir.as_deref()),
    );
    compare(
        "arch",
        fmt_opt(left.arch.as_deref()),
        fmt_opt(right.arch.as_deref()),
    );
    compare(
        "platform",
        fmt_opt(left.platform.as_deref()),
        fmt_opt(right.platform.as_deref()),
    );
    compare(
        "noarch",
        fmt_noarch(&left.noarch),
        fmt_noarch(&right.noarch),
    );
    compare(
        "license",
        fmt_opt(left.license.as_deref()),
        fmt_opt(right.license.as_deref()),
    );
    compare(
        "license_family",
        fmt_opt(left.license_family.as_deref()),
        fmt_opt(right.license_family.as_deref()),
    );
    compare(
        "features",
        fmt_opt(left.features.as_deref()),
        fmt_opt(right.features.as_deref()),
    );
    compare(
        "track_features",
        fmt_opt(
            (!left.track_features.is_empty())
                .then(|| left.track_features.join(", "))
                .as_deref(),
        ),
        fmt_opt(
            (!right.track_features.is_empty())
                .then(|| right.track_features.join(", "))
                .as_deref(),
        ),
    );
    compare(
        "timestamp",
        fmt_opt(
            left.timestamp
                .map(|t| t.jiff_timestamp().to_string())
                .as_deref(),
        ),
        fmt_opt(
            right
                .timestamp
                .map(|t| t.jiff_timestamp().to_string())
                .as_deref(),
        ),
    );
    compare(
        "python_site_packages_path",
        fmt_opt(left.python_site_packages_path.as_deref()),
        fmt_opt(right.python_site_packages_path.as_deref()),
    );

    if changes.is_empty() {
        println!("  no changes");
        return false;
    }
    for (field, left, right) in changes {
        println!("  {} {field}: {left} -> {right}", style("~").yellow());
    }
    true
}

/// Returns the package name part of a match spec, used to pair a removed spec
/// with an added spec for the same package.
fn spec_name(spec: &str) -> &str {
    spec.split([' ', '=', '<', '>', '!', '~', '['])
        .next()
        .unwrap_or(spec)
}

/// Prints the diff between two lists of match specs. Specs for the same
/// package name are paired and reported as changed. Returns whether any
/// difference was printed.
fn print_spec_diff(label: &str, left: &[String], right: &[String]) -> bool {
    let mut removed: Vec<String> = left
        .iter()
        .filter(|s| !right.contains(s))
        .cloned()
        .collect();
    let mut added: Vec<String> = right
        .iter()
        .filter(|s| !left.contains(s))
        .cloned()
        .collect();
    if removed.is_empty() && added.is_empty() {
        return false;
    }

    let mut changed = Vec::new();
    removed.retain(|left_spec| {
        if let Some(pos) = added
            .iter()
            .position(|right_spec| spec_name(right_spec) == spec_name(left_spec))
        {
            changed.push((left_spec.clone(), added.remove(pos)));
            false
        } else {
            true
        }
    });

    println!("  {label}:");
    for (left_spec, right_spec) in changed {
        println!("    {} {left_spec} -> {right_spec}", style("~").yellow());
    }
    for spec in added {
        println!("    {} {spec}", style("+").green());
    }
    for spec in removed {
        println!("    {} {spec}", style("-").red());
    }
    true
}

/// Compares dependencies, constraints, extra dependency groups and run
/// exports.
fn compare_dependencies(
    left: &IndexJson,
    right: &IndexJson,
    left_run_exports: Option<&RunExportsJson>,
    right_run_exports: Option<&RunExportsJson>,
) -> bool {
    println!();
    println!("{}", style("dependencies").bold());

    let mut any_changes = false;
    any_changes |= print_spec_diff("depends", &left.depends, &right.depends);
    any_changes |= print_spec_diff("constrains", &left.constrains, &right.constrains);

    let extra_groups: BTreeSet<&String> = left
        .extra_depends
        .keys()
        .chain(right.extra_depends.keys())
        .collect();
    for group in extra_groups {
        any_changes |= print_spec_diff(
            &format!("extra_depends[{group}]"),
            left.extra_depends.get(group).map_or(&[], Vec::as_slice),
            right.extra_depends.get(group).map_or(&[], Vec::as_slice),
        );
    }

    let empty_run_exports = RunExportsJson::default();
    let left_run_exports = left_run_exports.unwrap_or(&empty_run_exports);
    let right_run_exports = right_run_exports.unwrap_or(&empty_run_exports);
    for (category, left_specs, right_specs) in [
        ("weak", &left_run_exports.weak, &right_run_exports.weak),
        (
            "strong",
            &left_run_exports.strong,
            &right_run_exports.strong,
        ),
        (
            "noarch",
            &left_run_exports.noarch,
            &right_run_exports.noarch,
        ),
        (
            "weak_constrains",
            &left_run_exports.weak_constrains,
            &right_run_exports.weak_constrains,
        ),
        (
            "strong_constrains",
            &left_run_exports.strong_constrains,
            &right_run_exports.strong_constrains,
        ),
    ] {
        any_changes |= print_spec_diff(
            &format!("run_exports ({category})"),
            left_specs,
            right_specs,
        );
    }

    if !any_changes {
        println!("  no changes");
    }
    any_changes
}

fn fmt_size(size: Option<u64>) -> String {
    size.map_or_else(|| "unknown size".to_string(), |s| HumanBytes(s).to_string())
}

/// Returns whether the file contents behind two paths.json entries differ,
/// preferring the sha256 hashes and falling back to the file sizes.
fn content_differs(left: &PathsEntry, right: &PathsEntry) -> bool {
    match (left.sha256, right.sha256) {
        (Some(left), Some(right)) => left != right,
        _ => left.size_in_bytes != right.size_in_bytes,
    }
}

/// Compares the payload files of both packages based on `info/paths.json`.
fn compare_package_files(left: Option<&PathsJson>, right: Option<&PathsJson>) -> bool {
    println!();
    println!("{}", style("package files (paths.json)").bold());

    let (left, right) = match (left, right) {
        (Some(left), Some(right)) => (left, right),
        (left, right) => {
            for (paths, side) in [(left, "left"), (right, "right")] {
                if paths.is_none() {
                    println!("  info/paths.json is missing in the {side} package");
                }
            }
            println!("  skipping package file comparison");
            return true;
        }
    };

    let left: BTreeMap<&Path, &PathsEntry> = left
        .paths
        .iter()
        .map(|entry| (entry.relative_path.as_path(), entry))
        .collect();
    let right: BTreeMap<&Path, &PathsEntry> = right
        .paths
        .iter()
        .map(|entry| (entry.relative_path.as_path(), entry))
        .collect();

    let (mut changed, mut added, mut removed, mut unchanged) = (0usize, 0usize, 0usize, 0usize);
    for path in left.keys().chain(right.keys()).collect::<BTreeSet<_>>() {
        match (left.get(path), right.get(path)) {
            (Some(left), Some(right)) if left == right => unchanged += 1,
            (Some(left), Some(right)) => {
                changed += 1;
                let note = if content_differs(left, right) {
                    format!(
                        "{} -> {}",
                        fmt_size(left.size_in_bytes),
                        fmt_size(right.size_in_bytes)
                    )
                } else {
                    "attributes changed".to_string()
                };
                println!("  {} {} ({note})", style("~").yellow(), path.display());
            }
            (None, Some(entry)) => {
                added += 1;
                println!(
                    "  {} {} ({})",
                    style("+").green(),
                    path.display(),
                    fmt_size(entry.size_in_bytes)
                );
            }
            (Some(entry), None) => {
                removed += 1;
                println!(
                    "  {} {} ({})",
                    style("-").red(),
                    path.display(),
                    fmt_size(entry.size_in_bytes)
                );
            }
            (None, None) => unreachable!("path comes from one of the two maps"),
        }
    }

    println!("  {changed} changed, {added} added, {removed} removed, {unchanged} unchanged");
    changed + added + removed > 0
}

/// The identity of an info file used to detect changes.
#[derive(PartialEq, Eq)]
enum InfoContent {
    File { sha256: [u8; 32] },
    Link { target: String },
}

struct InfoEntry {
    size: u64,
    content: InfoContent,
}

/// Streams the info section of a package and records the size and content
/// hash (or link target) of every entry.
async fn collect_info_entries(
    archive: &PackageArchive,
) -> Result<BTreeMap<PathBuf, InfoEntry>, ExtractError> {
    let mut stream = archive.stream(Section::Info).await?;
    let mut entries = BTreeMap::new();
    while let Some(mut entry) = stream.next_entry().await? {
        let path = entry.path().to_owned();
        match entry.kind() {
            ArchiveEntryKind::File => {
                let bytes = entry.read().await?;
                entries.insert(
                    path,
                    InfoEntry {
                        size: bytes.len() as u64,
                        content: InfoContent::File {
                            sha256: Sha256::digest(&bytes).into(),
                        },
                    },
                );
            }
            ArchiveEntryKind::Symlink | ArchiveEntryKind::Hardlink => {
                let target = entry
                    .link_target()?
                    .map(|target| target.display().to_string())
                    .unwrap_or_default();
                entries.insert(
                    path,
                    InfoEntry {
                        size: entry.size()?,
                        content: InfoContent::Link { target },
                    },
                );
            }
            _ => {}
        }
    }
    Ok(entries)
}

/// Compares the info sections of both packages by streaming them.
async fn compare_info_files(left: &PackageArchive, right: &PackageArchive) -> miette::Result<bool> {
    let (left, right) = tokio::try_join!(collect_info_entries(left), collect_info_entries(right))
        .into_diagnostic()
        .context("failed to stream the info sections")?;

    println!();
    println!("{}", style("info files").bold());

    let (mut changed, mut added, mut removed, mut unchanged) = (0usize, 0usize, 0usize, 0usize);
    for path in left.keys().chain(right.keys()).collect::<BTreeSet<_>>() {
        match (left.get(path), right.get(path)) {
            (Some(left), Some(right)) if left.content == right.content => unchanged += 1,
            (Some(left), Some(right)) => {
                changed += 1;
                println!(
                    "  {} {} ({} -> {})",
                    style("~").yellow(),
                    path.display(),
                    HumanBytes(left.size),
                    HumanBytes(right.size)
                );
            }
            (None, Some(entry)) => {
                added += 1;
                println!(
                    "  {} {} ({})",
                    style("+").green(),
                    path.display(),
                    HumanBytes(entry.size)
                );
            }
            (Some(entry), None) => {
                removed += 1;
                println!(
                    "  {} {} ({})",
                    style("-").red(),
                    path.display(),
                    HumanBytes(entry.size)
                );
            }
            (None, None) => unreachable!("path comes from one of the two maps"),
        }
    }

    println!("  {changed} changed, {added} added, {removed} removed, {unchanged} unchanged");
    Ok(changed + added + removed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_name() {
        assert_eq!(spec_name("python >=3.10"), "python");
        assert_eq!(spec_name("python>=3.10"), "python");
        assert_eq!(spec_name("libzlib"), "libzlib");
        assert_eq!(spec_name("foo[extras=[bar]]"), "foo");
        assert_eq!(spec_name("numpy 1.24.*"), "numpy");
    }
}
