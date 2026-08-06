use miette::IntoDiagnostic;
use rattler_conda_types::GenericVirtualPackage;
use rattler_virtual_packages::VirtualPackageOverrides;

/// Print detected virtual packages.
#[derive(Debug, clap::Parser)]
pub struct Opt {}

pub fn virtual_packages(_opt: Opt) -> miette::Result<()> {
    let cache_dir = rattler::default_cache_dir().ok();
    tracing::debug!(
        cache_dir = %cache_dir
            .as_ref()
            .map_or_else(|| "<disabled>".to_string(), |path| path.display().to_string()),
        "detecting virtual packages"
    );

    let virtual_packages = rattler_virtual_packages::VirtualPackage::detect(
        &VirtualPackageOverrides::from_env(),
        cache_dir.as_deref(),
    )
    .into_diagnostic()?;

    let generic_virtual_packages = virtual_packages
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect::<Vec<_>>();
    let package_strings = generic_virtual_packages
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    tracing::debug!(
        count = package_strings.len(),
        packages = ?package_strings,
        "detected virtual packages"
    );

    for package in generic_virtual_packages {
        println!("{package}");
    }
    Ok(())
}
