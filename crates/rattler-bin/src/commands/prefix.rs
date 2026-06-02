use std::path::{Path, PathBuf};

use miette::{Context, IntoDiagnostic};
use rattler::install::{InstallDriver, InstallOptions, Transaction, link_package, unlink_package};
use rattler_conda_types::{
    PackageName, PackageRecord, Platform, PrefixRecord, RepoDataRecord,
    package::{CondaArchiveType, DistArchiveIdentifier},
    prefix::Prefix,
};
use rattler_package_streaming::fs::extract;
use url::Url;

const PIXI_ENVIRONMENT_FINGERPRINT_FILE: &str = ".pixi-environment-fingerprint";

/// Add a single conda package archive to a prefix without solving.
#[derive(Debug, clap::Parser)]
pub struct InjectOpt {
    /// Local path to a conda package archive (.conda or .tar.bz2)
    #[clap(required = true)]
    package: PathBuf,

    /// Target prefix to inject the package into
    #[clap(short = 'p', long = "prefix", default_value = ".prefix")]
    target_prefix: PathBuf,

    /// Skip platform and dependency compatibility checks
    #[clap(long)]
    skip_compatibility_checks: bool,
}

/// Remove a single installed conda package from a prefix without solving.
#[derive(Debug, clap::Parser)]
pub struct RemoveFromPrefixOpt {
    /// Exact package name of the installed package to remove
    #[clap(required = true)]
    package: PackageName,

    /// Target prefix to remove the package from
    #[clap(short = 'p', long = "prefix", default_value = ".prefix")]
    target_prefix: PathBuf,

    /// Skip dependency compatibility checks after removal
    #[clap(long)]
    skip_compatibility_checks: bool,
}

pub async fn inject(opt: InjectOpt) -> miette::Result<()> {
    let target_prefix = std::path::absolute(opt.target_prefix).into_diagnostic()?;
    let package_path = std::path::absolute(opt.package).into_diagnostic()?;
    let package_record = package_record_from_archive(&package_path)?;

    if !opt.skip_compatibility_checks {
        validate_package_platform(&package_record)?;
    }

    let installed_packages =
        PrefixRecord::collect_from_prefix::<PrefixRecord>(&target_prefix).into_diagnostic()?;

    reject_already_installed(&installed_packages, &package_record.name)?;

    if !opt.skip_compatibility_checks {
        let records = installed_packages
            .iter()
            .map(|record| &record.repodata_record.package_record)
            .chain(std::iter::once(&package_record))
            .collect::<Vec<_>>();
        PackageRecord::validate(records)
            .into_diagnostic()
            .context("injecting this package would make the prefix incompatible")?;
    }

    let repodata_record = RepoDataRecord {
        package_record,
        identifier: DistArchiveIdentifier::try_from_path(&package_path).ok_or_else(|| {
            miette::miette!(
                "could not derive package identity from {}",
                package_path.display()
            )
        })?,
        url: Url::from_file_path(&package_path).map_err(|_err| {
            miette::miette!("could not convert {} to a file URL", package_path.display())
        })?,
        channel: None,
    };

    let mut desired_records = installed_packages
        .iter()
        .map(|record| record.repodata_record.clone())
        .collect::<Vec<_>>();
    desired_records.push(repodata_record.clone());

    let transaction = Transaction::from_current_and_desired(
        installed_packages.clone(),
        desired_records,
        None,
        None,
        Platform::current(),
    )
    .into_diagnostic()?;

    let driver = InstallDriver::builder()
        .execute_link_scripts(true)
        .with_prefix_records(&installed_packages)
        .finish();
    let prefix = Prefix::create(&target_prefix).into_diagnostic()?;
    let extract_dir = tempfile::tempdir().into_diagnostic()?;

    extract(&package_path, extract_dir.path())
        .into_diagnostic()
        .with_context(|| format!("failed to extract {}", package_path.display()))?;

    let paths = link_package(
        extract_dir.path(),
        &prefix,
        &driver,
        InstallOptions {
            platform: Some(Platform::current()),
            python_info: transaction.python_info.clone(),
            ..InstallOptions::default()
        },
    )
    .await
    .into_diagnostic()
    .with_context(|| format!("failed to link package into {}", target_prefix.display()))?;

    let prefix_record = PrefixRecord::from_repodata_record(repodata_record, paths);
    write_prefix_record(&target_prefix, &prefix_record)?;

    driver
        .post_process(&transaction, &prefix, None)
        .into_diagnostic()
        .context("failed to post-process prefix after injection")?;

    invalidate_pixi_environment_fingerprint(&target_prefix)?;

    println!(
        "{} Injected {} into {}",
        console::style(console::Emoji("✔", "")).green(),
        prefix_record.repodata_record.package_record,
        target_prefix.display()
    );

    Ok(())
}

pub async fn remove_from_prefix(opt: RemoveFromPrefixOpt) -> miette::Result<()> {
    let target_prefix = std::path::absolute(opt.target_prefix).into_diagnostic()?;
    let installed_packages =
        PrefixRecord::collect_from_prefix::<PrefixRecord>(&target_prefix).into_diagnostic()?;
    let remove_record = select_installed_record(&installed_packages, &opt.package)?;
    let remaining_packages = installed_packages
        .iter()
        .filter(|record| *record != remove_record)
        .cloned()
        .collect::<Vec<_>>();

    if !opt.skip_compatibility_checks {
        let records = remaining_packages
            .iter()
            .map(|record| &record.repodata_record.package_record)
            .collect::<Vec<_>>();
        PackageRecord::validate(records)
            .into_diagnostic()
            .context("removing this package would make the prefix incompatible")?;
    }

    let desired_records = remaining_packages
        .iter()
        .map(|record| record.repodata_record.clone())
        .collect::<Vec<_>>();
    let transaction = Transaction::from_current_and_desired(
        installed_packages.clone(),
        desired_records,
        None,
        None,
        Platform::current(),
    )
    .into_diagnostic()?;

    let driver = InstallDriver::builder()
        .execute_link_scripts(true)
        .with_prefix_records(&installed_packages)
        .finish();
    let prefix = Prefix::create(&target_prefix).into_diagnostic()?;

    driver
        .pre_process(&transaction, &target_prefix, None)
        .into_diagnostic()
        .context("failed to pre-process prefix before removal")?;

    driver.clobber_registry().unregister_paths(remove_record);
    unlink_package(&prefix, remove_record)
        .await
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to unlink {}",
                remove_record.repodata_record.package_record
            )
        })?;

    driver
        .remove_empty_directories(
            &transaction.operations,
            transaction.unchanged_packages(),
            &target_prefix,
        )
        .into_diagnostic()
        .context("failed to remove empty directories after removal")?;

    driver
        .post_process(&transaction, &prefix, None)
        .into_diagnostic()
        .context("failed to post-process prefix after removal")?;

    invalidate_pixi_environment_fingerprint(&target_prefix)?;

    println!(
        "{} Removed {} from {}",
        console::style(console::Emoji("✔", "")).green(),
        remove_record.repodata_record.package_record,
        target_prefix.display()
    );

    Ok(())
}

fn package_record_from_archive(path: &Path) -> miette::Result<PackageRecord> {
    let archive_type = CondaArchiveType::try_from(path)
        .ok_or_else(|| miette::miette!("unsupported package archive: {}", path.display()))?;

    match archive_type {
        CondaArchiveType::TarBz2 => rattler_index::package_record_from_tar_bz2(path),
        CondaArchiveType::Conda => rattler_index::package_record_from_conda(path),
    }
    .into_diagnostic()
    .with_context(|| format!("failed to read package metadata from {}", path.display()))
}

fn validate_package_platform(package_record: &PackageRecord) -> miette::Result<()> {
    let package_subdir = &package_record.subdir;
    if package_subdir != &Platform::NoArch.to_string()
        && package_subdir != &Platform::current().to_string()
    {
        return Err(miette::miette!(
            "package {} is for platform {}, but the current platform is {}",
            package_record,
            package_subdir,
            Platform::current()
        ));
    }

    Ok(())
}

fn reject_already_installed(
    installed_packages: &[PrefixRecord],
    package_name: &PackageName,
) -> miette::Result<()> {
    if installed_packages.iter().any(|record| {
        record.repodata_record.package_record.name.as_normalized() == package_name.as_normalized()
    }) {
        return Err(miette::miette!(
            "package {} is already installed",
            package_name.as_normalized()
        ));
    }

    Ok(())
}

fn write_prefix_record(target_prefix: &Path, prefix_record: &PrefixRecord) -> miette::Result<()> {
    let conda_meta_path = target_prefix.join("conda-meta");
    std::fs::create_dir_all(&conda_meta_path)
        .into_diagnostic()
        .with_context(|| format!("failed to create {}", conda_meta_path.display()))?;
    prefix_record
        .write_to_path(conda_meta_path.join(prefix_record.file_name()), true)
        .into_diagnostic()
        .context("failed to write prefix record")?;

    Ok(())
}

fn invalidate_pixi_environment_fingerprint(target_prefix: &Path) -> miette::Result<()> {
    let fingerprint_path = target_prefix
        .join("conda-meta")
        .join(PIXI_ENVIRONMENT_FINGERPRINT_FILE);
    if fingerprint_path
        .try_exists()
        .into_diagnostic()
        .with_context(|| format!("failed to check {}", fingerprint_path.display()))?
    {
        std::fs::remove_file(&fingerprint_path)
            .into_diagnostic()
            .with_context(|| format!("failed to remove {}", fingerprint_path.display()))?;
    }

    Ok(())
}

fn select_installed_record<'a>(
    installed_packages: &'a [PrefixRecord],
    package_name: &PackageName,
) -> miette::Result<&'a PrefixRecord> {
    let matches = installed_packages
        .iter()
        .filter(|record| record.repodata_record.package_record.name == *package_name)
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [] => Err(miette::miette!(
            "no installed package matched '{}'",
            package_name.as_normalized()
        )),
        [record] => Ok(*record),
        records => Err(miette::miette!(
            "package name '{}' matched multiple installed packages: {}",
            package_name.as_normalized(),
            records
                .iter()
                .map(|record| record.repodata_record.package_record.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    use rattler_conda_types::Version;

    #[tokio::test]
    async fn test_inject_and_remove_local_package() {
        let prefix = tempfile::tempdir().unwrap();
        let fingerprint_path = prefix
            .path()
            .join("conda-meta")
            .join(PIXI_ENVIRONMENT_FINGERPRINT_FILE);
        std::fs::create_dir_all(fingerprint_path.parent().unwrap()).unwrap();
        std::fs::write(&fingerprint_path, "stale").unwrap();
        let package = workspace_root()
            .join("test-data")
            .join("packages")
            .join("empty-0.1.0-h4616a5c_0.conda");

        inject(InjectOpt {
            package,
            target_prefix: prefix.path().to_path_buf(),
            skip_compatibility_checks: false,
        })
        .await
        .unwrap();

        let records = PrefixRecord::collect_from_prefix::<PrefixRecord>(prefix.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0]
                .repodata_record
                .package_record
                .name
                .as_normalized(),
            "empty"
        );
        assert!(!fingerprint_path.exists());

        std::fs::write(&fingerprint_path, "stale").unwrap();

        remove_from_prefix(RemoveFromPrefixOpt {
            package: PackageName::new_unchecked("empty"),
            target_prefix: prefix.path().to_path_buf(),
            skip_compatibility_checks: false,
        })
        .await
        .unwrap();

        let records = PrefixRecord::collect_from_prefix::<PrefixRecord>(prefix.path()).unwrap();
        assert!(records.is_empty());
        assert!(!fingerprint_path.exists());
    }

    #[tokio::test]
    async fn test_inject_same_name_twice_errors() {
        let prefix = tempfile::tempdir().unwrap();
        let package = workspace_root()
            .join("test-data")
            .join("packages")
            .join("empty-0.1.0-h4616a5c_0.conda");

        inject(InjectOpt {
            package: package.clone(),
            target_prefix: prefix.path().to_path_buf(),
            skip_compatibility_checks: false,
        })
        .await
        .unwrap();

        let result = inject(InjectOpt {
            package,
            target_prefix: prefix.path().to_path_buf(),
            skip_compatibility_checks: false,
        })
        .await;

        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("already installed")
        );
    }

    #[test]
    fn test_remove_selector_exact_name() {
        let record = PrefixRecord::from_repodata_record(
            RepoDataRecord {
                package_record: PackageRecord::new(
                    PackageName::new_unchecked("my-package"),
                    Version::from_str("0.0.1").unwrap(),
                    "h123456".to_string(),
                ),
                identifier: "my-package-0.0.1-h123456.conda".parse().unwrap(),
                url: Url::parse("https://example.com/my-package-0.0.1-h123456.conda").unwrap(),
                channel: None,
            },
            Vec::new(),
        );
        let installed = vec![record];

        assert!(
            select_installed_record(&installed, &PackageName::new_unchecked("my-package")).is_ok()
        );
        assert!(
            select_installed_record(&installed, &PackageName::new_unchecked("other-package"))
                .is_err()
        );
    }

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    }
}
