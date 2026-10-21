//! Cache a wheel from a channel, optionally installing it into a disposable conda prefix.
//!
//! `cargo run -p rattler --example cache_channel_wheel -- cowsay`
//! Pass a fourth argument (the prefix) to install using only the populated cache.
//! Dependencies must already be installed; this example does not run a solver.
//! Non-sharded wheel discovery requires the gateway fix from PR #2952.

use std::{path::PathBuf, process::Command};

use anyhow::{Context, ensure};
use rattler::install::Installer;
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::{
    Channel, PackageName, PackageRecord, PrefixRecord, Subdir, package::DistArchiveType,
};
use rattler_networking::{LazyClient, OfflineMiddleware, retry_policies::DoNotRetryPolicy};
use rattler_repodata_gateway::Gateway;
use url::Url;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        println!(
            "Usage: cache_channel_wheel [package] [channel-url] [cache-dir] [prefix]\n\
             Defaults: cowsay https://conda.anaconda.org/conda-pypi/ ./wheel-test-cache\n\n\
             Selects the highest available wheel version and caches its conda layout.\n\
             With a prefix, installs using the standard installer with HTTP blocked.\n\
             Use a disposable conda prefix with Python and dependencies already installed.\n\
             Existing packages are retained, except the selected package is replaced.\n\
             This example does not run a dependency solver.\n\
             Non-sharded wheel discovery requires the gateway fix from PR #2952."
        );
        return Ok(());
    }
    ensure!(args.len() <= 4, "too many arguments; use --help");
    let prefix = args.get(3).map(PathBuf::from);
    if let Some(prefix) = &prefix {
        ensure!(
            prefix.join("conda-meta").is_dir(),
            "prefix must be an existing disposable conda environment"
        );
    }
    let package: PackageName = args.first().map_or("cowsay", String::as_str).parse()?;
    let url = Url::parse(
        args.get(1)
            .map_or("https://conda.anaconda.org/conda-pypi/", String::as_str),
    )?;
    ensure!(
        matches!(url.scheme(), "https" | "http" | "file"),
        "expected an HTTP(S) or file channel URL"
    );
    let root = PathBuf::from(args.get(2).map_or("./wheel-test-cache", String::as_str));
    // The channel rejects requests without a User-Agent header.
    let client: LazyClient = reqwest::Client::builder()
        .user_agent(concat!(
            "rattler-wheel-cache-example/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()?
        .into();
    let gateway = Gateway::builder()
        .with_client(client.clone())
        .with_cache_dir(root.join("repodata"))
        .finish();

    println!("Querying {} for {} (noarch)", url, package.as_normalized());
    let repodata = gateway
        .query([Channel::from_url(url)], [Subdir::NoArch], [package])
        .recursive(false)
        .await?;
    let record = repodata
        .iter()
        .flat_map(|repo| repo.iter())
        .filter(|record| matches!(record.identifier.archive_type, DistArchiveType::Wheel(_)))
        .max_by(|a, b| {
            a.package_record.version.cmp(&b.package_record.version)
                .then_with(|| a.package_record.build_number.cmp(&b.package_record.build_number))
                .then_with(|| a.identifier.cmp(&b.identifier))
        })
        .context("no wheel records found; verify that the package exists and the gateway wheel-discovery fix (PR #2952) is applied for non-sharded channels")?;

    println!("Selected: {}", record.identifier.to_file_name());
    println!("Artifact URL: {}", record.url);
    if let Some(hash) = record.package_record.sha256 {
        println!("Expected SHA-256: {}", hex::encode(hash));
    }
    let cache = PackageCache::new(root.join("packages"));
    let cached = cache
        .get_or_fetch_from_url_with_retry(
            &record.package_record,
            record.url.clone(),
            record.identifier.archive_type,
            client,
            DoNotRetryPolicy,
            None,
            None,
        )
        .await?;
    println!("Cached at: {}", cached.path().display());
    println!("Cache revision: {}", cached.revision());
    let Some(prefix) = prefix else {
        println!("Nothing was installed. Supply a fourth argument (prefix) to install from cache.");
        return Ok(());
    };
    let prefix = prefix.canonicalize()?;
    let installed: Vec<PrefixRecord> = PrefixRecord::collect_from_prefix(&prefix)?;
    ensure!(
        installed
            .iter()
            .any(|r| r.repodata_record.package_record.name.as_normalized() == "python"),
        "the prefix must already contain Python"
    );
    let mut desired: Vec<_> = installed
        .into_iter()
        .map(|r| r.repodata_record)
        .filter(|r| r.package_record.name != record.package_record.name)
        .collect();
    desired.push(record.clone());
    PackageRecord::validate(desired.clone()).context(
        "the selected wheel does not form a valid environment with the installed packages; install its dependencies first (this example does not solve dependencies)",
    )?;

    let offline_client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
        .with(OfflineMiddleware)
        .build();
    println!(
        "Installing into {} with HTTP requests blocked",
        prefix.display()
    );
    let result = Installer::new()
        .with_package_cache(cache.clone())
        .with_download_client(offline_client.clone())
        .install(&prefix, desired)
        .await?;
    println!(
        "Installer completed {} transaction operation(s)",
        result.transaction.operations.len()
    );

    let records: Vec<PrefixRecord> = PrefixRecord::collect_from_prefix(&prefix)?;
    let installed = records
        .iter()
        .find(|r| r.repodata_record.package_record.name == record.package_record.name)
        .context("installed package missing from conda-meta")?;
    ensure!(
        installed.repodata_record.identifier == record.identifier,
        "installed identifier differs from selected wheel"
    );
    ensure!(
        installed.repodata_record.url == record.url,
        "installed artifact URL changed"
    );
    ensure!(
        installed.repodata_record.package_record.sha256 == record.package_record.sha256,
        "installed archive digest changed"
    );
    ensure!(
        !installed.files.is_empty(),
        "installed files are not tracked"
    );
    println!("conda-meta tracks {} files", installed.files.len());

    let python = prefix.join(if cfg!(windows) {
        "python.exe"
    } else {
        "bin/python"
    });
    let output = Command::new(python)
        .env_remove("PYTHONPATH")
        .env_remove("PYTHONHOME")
        .args(["-I", "-c", "import importlib.metadata as m, sys; d = m.distribution(sys.argv[1]); print(d.metadata['Name'], d.version); assert d.version == sys.argv[2]"])
        .arg(record.package_record.name.as_normalized())
        .arg(record.package_record.version.to_string())
        .output()?;
    ensure!(
        output.status.success(),
        "installed Python distribution check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!(
        "Python distribution verified: {}",
        String::from_utf8_lossy(&output.stdout).trim()
    );

    let reused = cache
        .get_or_fetch_from_url_with_retry(
            &record.package_record,
            record.url.clone(),
            record.identifier.archive_type,
            offline_client.into(),
            DoNotRetryPolicy,
            None,
            None,
        )
        .await?;
    ensure!(
        reused.path() == cached.path() && reused.revision() == cached.revision(),
        "installation unexpectedly replaced the cached conversion"
    );
    println!(
        "Reused cache revision {}. No installation-time HTTP requests were allowed.",
        reused.revision()
    );
    Ok(())
}
