//! Run against a disposable conda prefix containing Python and `typing_extensions`:
//! `cargo run -p rattler --example wheel_cache_e2e -- <prefix> <empty-cache-dir>`
//! Leaves the installed fixture and cache available for manual inspection.

use std::{path::PathBuf, process::Command};

use anyhow::{Context, ensure};
use rattler::install::Installer;
use rattler_cache::package_cache::PackageCache;
use rattler_conda_types::{NoArchType, PackageName, PackageRecord, PrefixRecord, RepoDataRecord};
use rattler_networking::{LazyClient, retry_policies::DoNotRetryPolicy};
use url::Url;

const WHEEL: &[u8] =
    include_bytes!("../../../test-data/wheels/rattler_fixture-1.0.0-py3-none-any.whl");
const SHA256: &str = "1ea4e0472e18e5d5ab6200e1db11f3c1dc8d516c1a92a483eeccb4c6116d91d7";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let prefix = PathBuf::from(args.next().context("expected a disposable conda prefix")?);
    let cache_dir = PathBuf::from(args.next().context("expected an empty cache directory")?);
    ensure!(args.next().is_none(), "unexpected arguments");
    ensure!(!cache_dir.exists(), "use a fresh cache directory");

    let installed: Vec<PrefixRecord> = PrefixRecord::collect_from_prefix(&prefix)?;
    let mut desired: Vec<_> = installed.into_iter().map(|r| r.repodata_record).collect();
    for name in ["python", "typing_extensions"] {
        ensure!(
            desired
                .iter()
                .any(|r| r.package_record.name.as_normalized() == name),
            "prefix must contain {name}"
        );
    }
    ensure!(
        !desired
            .iter()
            .any(|r| r.package_record.name.as_normalized() == "rattler-fixture"),
        "fixture is already installed"
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url: Url = format!("http://{}/fixture.whl", listener.local_addr()?).parse()?;
    let app = axum::Router::new().route("/fixture.whl", axum::routing::get(|| async { WHEEL }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let wheel = RepoDataRecord {
        package_record: PackageRecord {
            subdir: "noarch".into(),
            noarch: NoArchType::python(),
            depends: vec!["python >=3.8".into(), "typing_extensions >=4".into()],
            sha256: rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(SHA256),
            ..PackageRecord::new(
                PackageName::new_unchecked("rattler-fixture"),
                "1.0.0".parse::<rattler_conda_types::Version>()?,
                "py3_none_any_0".parse()?,
            )
        },
        identifier: "rattler_fixture-1.0.0-py3_none_any_0.whl"
            .parse()
            .map_err(anyhow::Error::msg)?,
        url,
        channel: None,
    };
    let cache = PackageCache::new(&cache_dir);
    let cached = cache
        .get_or_fetch_from_url_with_retry(
            &wheel.package_record,
            wheel.url.clone(),
            wheel.identifier.archive_type,
            LazyClient::default(),
            DoNotRetryPolicy,
            None,
            None,
        )
        .await?;
    println!(
        "Downloaded and converted wheel: {}",
        cached.path().display()
    );
    let cached_record = cached
        .path()
        .join("site-packages/rattler_fixture-1.0.0.dist-info/RECORD");
    let before = std::fs::read(&cached_record)?;
    let revision = cached.revision();

    // Installation must use the cached conversion, not another download.
    server.abort();
    let _ = server.await;
    desired.push(wheel.clone());
    let result = Installer::new()
        .with_package_cache(cache.clone())
        .install(&prefix, desired.clone())
        .await?;
    ensure!(
        result.transaction.operations.len() == 1,
        "expected only the wheel to be installed"
    );
    println!("Standard Installer installed the wheel with the HTTP server stopped");

    let python = prefix.join(if cfg!(windows) {
        "python.exe"
    } else {
        "bin/python"
    });
    let output = Command::new(&python)
        .env_remove("PYTHONPATH")
        .env_remove("PYTHONHOME")
        .args(["-I", "-c", "import rattler_fixture; import importlib.metadata as m; assert rattler_fixture.__version__ == '1.0.0'; assert m.version('rattler-fixture') == '1.0.0'; print(rattler_fixture.__file__)"])
        .output()?;
    ensure!(
        output.status.success(),
        "Python import failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!(
        "Python import and distribution metadata OK: {}",
        String::from_utf8_lossy(&output.stdout).trim()
    );

    let entry_point = prefix.join(if cfg!(windows) {
        "Scripts/rattler-fixture.exe"
    } else {
        "bin/rattler-fixture"
    });
    let output = Command::new(&entry_point)
        .env_remove("PYTHONPATH")
        .env_remove("PYTHONHOME")
        .output()?;
    ensure!(
        output.status.success(),
        "entry point failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        String::from_utf8_lossy(&output.stdout).trim() == "moo",
        "unexpected entry-point output"
    );
    println!("Generated console entry point OK: moo");

    ensure!(
        std::fs::read(prefix.join("share/rattler_fixture/cow.txt"))? == b"(__)\n(oo)\n",
        "wheel data was not installed"
    );
    let records: Vec<PrefixRecord> = PrefixRecord::collect_from_prefix(&prefix)?;
    let record = records
        .iter()
        .find(|r| r.repodata_record.package_record.name.as_normalized() == "rattler-fixture")
        .context("wheel missing from conda-meta")?;
    ensure!(
        record.repodata_record.url == wheel.url,
        "artifact URL changed"
    );
    ensure!(
        record.repodata_record.identifier == wheel.identifier,
        "identifier changed"
    );
    ensure!(
        record.repodata_record.package_record.sha256 == wheel.package_record.sha256,
        "digest changed"
    );
    ensure!(!record.files.is_empty(), "installed files were not tracked");
    println!(
        "conda-meta preserves wheel identity and tracks {} files",
        record.files.len()
    );

    let again = cache
        .get_or_fetch_from_url_with_retry(
            &wheel.package_record,
            wheel.url.clone(),
            wheel.identifier.archive_type,
            LazyClient::default(),
            DoNotRetryPolicy,
            None,
            None,
        )
        .await?;
    ensure!(again.revision() == revision, "cache unexpectedly rebuilt");
    ensure!(
        std::fs::read(cached_record)? == before,
        "installation modified cached RECORD"
    );
    let result = Installer::new()
        .with_package_cache(cache)
        .install(&prefix, desired)
        .await?;
    ensure!(
        result.transaction.operations.is_empty(),
        "second install should be a no-op"
    );
    println!("Cache unchanged; second installation is a no-op");
    println!("Prefix: {}", prefix.display());
    Ok(())
}
