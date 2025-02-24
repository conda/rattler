use anyhow::{Context, Result};
use clap::Parser;
use rattler_conda_types::{menuinst::MenuMode, PackageName, Platform, PrefixRecord};
use std::{fs, path::PathBuf};

#[derive(Debug, Parser)]
pub struct InstallOpt {
    /// Target prefix to look for the package (defaults to `.prefix`)
    #[clap(long, short, default_value = ".prefix")]
    target_prefix: PathBuf,

    /// Name of the package for which to install menu items
    package_name: PackageName,
}

pub async fn install_menu(opts: InstallOpt) -> Result<()> {
    // Find the prefix record in the target_prefix and call `install_menu` on it
    let records: Vec<PrefixRecord> = PrefixRecord::collect_from_prefix(&opts.target_prefix)?;

    let record = records
        .iter()
        .find(|r| r.repodata_record.package_record.name == opts.package_name)
        .with_context(|| {
            format!(
                "Package {} not found in prefix {:?}",
                opts.package_name.as_normalized(),
                opts.target_prefix
            )
        })?;
    let prefix = fs::canonicalize(&opts.target_prefix)?;
    rattler_menuinst::install_menuitems_for_record(
        &prefix,
        record,
        Platform::current(),
        MenuMode::User,
    )?;

    Ok(())
}

pub async fn remove_menu(opts: InstallOpt) -> Result<()> {
    // Find the prefix record in the target_prefix and call `remove_menu` on it
    let records: Vec<PrefixRecord> = PrefixRecord::collect_from_prefix(&opts.target_prefix)?;

    let record = records
        .iter()
        .find(|r| r.repodata_record.package_record.name == opts.package_name)
        .with_context(|| {
            format!(
                "Package {} not found in prefix {:?}",
                opts.package_name.as_normalized(),
                opts.target_prefix
            )
        })?;

    rattler_menuinst::remove_menu_items(&record.installed_system_menus)?;

    Ok(())
}
