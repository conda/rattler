use std::path::PathBuf;

use miette::IntoDiagnostic;
use rattler::install;

/// Link an extracted package into a prefix.
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
    let link_context = install::TransactionLinkContext::default();
    let options = install::InstallOptions::default();
    let target_dir =
        rattler_conda_types::prefix::Prefix::create(opt.destination.clone()).into_diagnostic()?;

    install::link_package(&opt.package_dir, &target_dir, &link_context, options)
        .await
        .into_diagnostic()?;
    Ok(())
}
