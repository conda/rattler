//! Cache a local wheel without installing it.
//!
//! Usage: `cargo run -p rattler --example cache_wheel -- <wheel.whl> <package-record.json> <cache-dir>`
//! The JSON must contain a conda `PackageRecord`, not a full repodata document.

use std::path::PathBuf;

use anyhow::{Context, ensure};
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::PackageRecord;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    ensure!(
        args.len() == 3,
        "usage: cache_wheel <wheel.whl> <package-record.json> <cache-dir>"
    );
    let wheel = PathBuf::from(&args[0]);
    ensure!(
        wheel.extension().is_some_and(|ext| ext == "whl"),
        "expected a .whl file"
    );
    let metadata = PathBuf::from(&args[1]);
    let record: PackageRecord = serde_json::from_reader(
        std::fs::File::open(&metadata).context("failed to open package metadata")?,
    )
    .context("expected conda PackageRecord JSON")?;
    let cache = PackageCache::new(PathBuf::from(&args[2]));
    let cached = cache
        .get_or_fetch_from_path(&wheel, Some(&record), None)
        .await?;
    println!("{}", cached.path().display());
    Ok(())
}
