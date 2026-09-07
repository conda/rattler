use std::io::Write;

use miette::{Context, IntoDiagnostic};

use super::package_source::{PackageSource, client_for};

/// Read a file from inside a local or remote conda package.
#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// Path or URL of the conda package (.conda or .tar.bz2 archive)
    #[clap(required = true)]
    package: String,

    /// Path of the file inside the package (e.g. "info/index.json" or "lib/libfoo.so")
    #[clap(required = true)]
    path: String,
}

pub async fn fetch_file(opt: Opt, offline: bool) -> miette::Result<()> {
    let Opt { package, path } = opt;

    let source = PackageSource::parse(&package);
    let client = client_for([&source], offline)?;
    let archive = source.open(client.as_ref()).await?;

    let bytes = archive
        .read_file(&path)
        .await
        .into_diagnostic()
        .with_context(|| format!("failed to read '{path}' from package {source}"))?
        .ok_or_else(|| miette::miette!("file '{path}' not found in package"))?;

    std::io::stdout()
        .write_all(&bytes)
        .into_diagnostic()
        .context("failed to write to stdout")?;
    Ok(())
}
