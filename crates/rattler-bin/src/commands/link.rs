use std::path::PathBuf;

use miette::IntoDiagnostic;
use rattler::install;

#[derive(Debug, clap::Parser)]
pub struct Opt {
    /// The package directory to link from
    #[clap(required = true)]
    package_dir: PathBuf,

    /// Destination directory where the package will be linked into
    #[clap(short, long)]
    destination: PathBuf,
}

pub async fn link(opt: Opt) -> miette::Result<()> {
    let driver = install::InstallDriver::default();
    let options = install::InstallOptions::default();
    let target_dir =
        rattler_conda_types::prefix::Prefix::create(opt.destination.clone()).into_diagnostic()?;

    install::link_package(&opt.package_dir, &target_dir, &driver, options)
        .await
        .into_diagnostic()?;
    Ok(())
}
